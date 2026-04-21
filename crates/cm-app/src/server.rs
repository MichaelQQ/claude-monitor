use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use cm_core::db;
use cm_core::schema::{LiveEvent, StatuslineInput};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

pub fn router(state: AppState) -> Router {
    let ui_dir = state.ui_dir.as_ref().clone();
    Router::new()
        .route("/v1/event", post(post_event))
        .route("/v1/live", get(ws_live))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/:id/turns", get(list_turns))
        .route("/v1/sessions/:id/snapshots", get(list_snapshots))
        .route("/v1/health", get(|| async { "ok" }))
        .fallback_service(ServeDir::new(ui_dir).append_index_html_on_directories(true))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn post_event(
    State(state): State<AppState>,
    Json(raw): Json<Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let input: StatuslineInput = serde_json::from_value(raw)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad json: {e}")))?;
    let ts = Utc::now().timestamp_millis();
    let project_dir = input
        .workspace
        .as_ref()
        .and_then(|w| w.project_dir.as_deref());
    db::upsert_session(
        &state.db,
        &input.session_id,
        project_dir,
        input.transcript_path.as_deref(),
        Some(&input.model.id),
        ts,
    )
    .map_err(internal)?;
    db::insert_snapshot(&state.db, &input, ts).map_err(internal)?;
    let _ = state.tx.send(LiveEvent::Snapshot(Box::new(input)));
    Ok(StatusCode::ACCEPTED)
}

fn internal(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[derive(Serialize)]
struct SessionRow {
    session_id: String,
    project_dir: Option<String>,
    model_id: Option<String>,
    started_at: i64,
    last_seen_at: i64,
    total_turns: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_read: i64,
    total_cache_creation: i64,
    last_cost_usd: Option<f64>,
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<Vec<SessionRow>>, (StatusCode, String)> {
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    let mut stmt = conn
        .prepare(
            r#"SELECT s.session_id, s.project_dir, s.model_id, s.started_at, s.last_seen_at,
                      COALESCE(t.n,0), COALESCE(t.i,0), COALESCE(t.o,0),
                      COALESCE(t.cr,0), COALESCE(t.cc,0),
                      (SELECT total_cost_usd FROM snapshots
                         WHERE session_id=s.session_id
                         ORDER BY ts DESC LIMIT 1)
                 FROM sessions s
                 LEFT JOIN (
                   SELECT session_id,
                          COUNT(*) n,
                          SUM(input_tokens) i, SUM(output_tokens) o,
                          SUM(cache_read_input_tokens) cr,
                          SUM(cache_creation_input_tokens) cc
                   FROM turns GROUP BY session_id
                 ) t ON t.session_id = s.session_id
                 ORDER BY s.last_seen_at DESC"#,
        )
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SessionRow {
                session_id: r.get(0)?,
                project_dir: r.get(1)?,
                model_id: r.get(2)?,
                started_at: r.get(3)?,
                last_seen_at: r.get(4)?,
                total_turns: r.get(5)?,
                total_input_tokens: r.get(6)?,
                total_output_tokens: r.get(7)?,
                total_cache_read: r.get(8)?,
                total_cache_creation: r.get(9)?,
                last_cost_usd: r.get(10)?,
            })
        })
        .map_err(|e| internal(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    Ok(Json(rows))
}

#[derive(Serialize)]
struct TurnRow {
    turn_uuid: String,
    ts: i64,
    model_id: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_input_tokens: i64,
    cache_read_input_tokens: i64,
    ephemeral_1h_tokens: i64,
    ephemeral_5m_tokens: i64,
}

async fn list_turns(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<TurnRow>>, (StatusCode, String)> {
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    let mut stmt = conn
        .prepare(
            r#"SELECT turn_uuid, ts, model_id,
                      input_tokens, output_tokens,
                      cache_creation_input_tokens, cache_read_input_tokens,
                      ephemeral_1h_tokens, ephemeral_5m_tokens
                 FROM turns WHERE session_id = ?1 ORDER BY ts ASC"#,
        )
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map([id], |r| {
            Ok(TurnRow {
                turn_uuid: r.get(0)?,
                ts: r.get(1)?,
                model_id: r.get(2)?,
                input_tokens: r.get(3)?,
                output_tokens: r.get(4)?,
                cache_creation_input_tokens: r.get(5)?,
                cache_read_input_tokens: r.get(6)?,
                ephemeral_1h_tokens: r.get(7)?,
                ephemeral_5m_tokens: r.get(8)?,
            })
        })
        .map_err(|e| internal(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    Ok(Json(rows))
}

#[derive(Serialize)]
struct SnapshotRow {
    ts: i64,
    total_cost_usd: Option<f64>,
    context_used_pct: Option<f64>,
    five_hour_pct: Option<f64>,
    five_hour_resets_at: Option<i64>,
    seven_day_pct: Option<f64>,
    seven_day_resets_at: Option<i64>,
}

async fn list_snapshots(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SnapshotRow>>, (StatusCode, String)> {
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    let mut stmt = conn
        .prepare(
            r#"SELECT ts, total_cost_usd, context_used_pct,
                      five_hour_pct, five_hour_resets_at,
                      seven_day_pct, seven_day_resets_at
                 FROM snapshots WHERE session_id = ?1 ORDER BY ts ASC"#,
        )
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map([id], |r| {
            Ok(SnapshotRow {
                ts: r.get(0)?,
                total_cost_usd: r.get(1)?,
                context_used_pct: r.get(2)?,
                five_hour_pct: r.get(3)?,
                five_hour_resets_at: r.get(4)?,
                seven_day_pct: r.get(5)?,
                seven_day_resets_at: r.get(6)?,
            })
        })
        .map_err(|e| internal(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    Ok(Json(rows))
}

async fn ws_live(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |sock| live_socket(sock, state))
}

async fn live_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.tx.subscribe();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });
    let send_task = tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&ev) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });
    tokio::select! { _ = recv_task => {}, _ = send_task => {} }
}
