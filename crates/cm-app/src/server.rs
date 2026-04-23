use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use cm_core::db;
use cm_core::schema::{LiveEvent, StatuslineInput, SubagentSnapshotEvent, SubagentStatuslineInput};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

pub fn router(state: AppState) -> Router {
    let ui_dir = state.ui_dir.as_ref().clone();
    Router::new()
        .route("/v1/event", post(post_event))
        .route("/v1/subagent-event", post(post_subagent_event))
        .route("/v1/live", get(ws_live))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/quota-caps", get(quota_caps))
        .route("/v1/sessions/:id/turns", get(list_turns))
        .route("/v1/sessions/:id/snapshots", get(list_snapshots))
        .route("/v1/sessions/:id/subagents", get(list_subagents))
        .route("/v1/trends", get(list_trends))
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

#[derive(Deserialize)]
struct SubagentQuery {
    session_id: Option<String>,
}

async fn post_subagent_event(
    State(state): State<AppState>,
    Query(q): Query<SubagentQuery>,
    Json(raw): Json<Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let input: SubagentStatuslineInput = serde_json::from_value(raw)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad json: {e}")))?;
    let session_id = input
        .session_id
        .clone()
        .or(q.session_id)
        .ok_or((StatusCode::BAD_REQUEST, "missing session_id".to_string()))?;
    let ts = Utc::now().timestamp_millis();
    db::upsert_subagent_tasks(&state.db, &session_id, &input.tasks, ts).map_err(internal)?;
    let _ = state
        .tx
        .send(LiveEvent::SubagentSnapshot(Box::new(SubagentSnapshotEvent {
            session_id,
            ts_ms: ts,
            tasks: input.tasks,
        })));
    Ok(StatusCode::ACCEPTED)
}

#[derive(Serialize)]
struct SubagentRow {
    task_id: String,
    name: Option<String>,
    task_type: Option<String>,
    status: Option<String>,
    description: Option<String>,
    label: Option<String>,
    start_time: Option<f64>,
    token_count: Option<i64>,
    cwd: Option<String>,
    first_seen_at: i64,
    last_seen_at: i64,
}

async fn list_subagents(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SubagentRow>>, (StatusCode, String)> {
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    let mut stmt = conn
        .prepare(
            r#"SELECT task_id, name, task_type, status, description, label,
                      start_time, token_count, cwd, first_seen_at, last_seen_at
                 FROM subagent_tasks
                 WHERE session_id = ?1
                 ORDER BY first_seen_at ASC"#,
        )
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map([id], |r| {
            Ok(SubagentRow {
                task_id: r.get(0)?,
                name: r.get(1)?,
                task_type: r.get(2)?,
                status: r.get(3)?,
                description: r.get(4)?,
                label: r.get(5)?,
                start_time: r.get(6)?,
                token_count: r.get(7)?,
                cwd: r.get(8)?,
                first_seen_at: r.get(9)?,
                last_seen_at: r.get(10)?,
            })
        })
        .map_err(|e| internal(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    Ok(Json(rows))
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
    total_cache_write_5m: i64,
    total_cache_write_1h: i64,
    quota_tokens: i64,
    last_cost_usd: Option<f64>,
    estimated_cost_usd: Option<f64>,
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<Vec<SessionRow>>, (StatusCode, String)> {
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    let mut stmt = conn
        .prepare(
            r#"SELECT s.session_id, s.project_dir, s.model_id, s.started_at, s.last_seen_at,
                      COALESCE(t.n,0), COALESCE(t.i,0), COALESCE(t.o,0),
                      COALESCE(t.cr,0), COALESCE(t.cc,0),
                      COALESCE(t.cw5,0), COALESCE(t.cw1,0),
                      CAST(COALESCE(t.quota,0) AS INTEGER),
                      (SELECT total_cost_usd FROM snapshots
                         WHERE session_id=s.session_id
                         ORDER BY ts DESC LIMIT 1),
                      t.est
                 FROM sessions s
                 LEFT JOIN (
                   SELECT session_id,
                          COUNT(*) n,
                          SUM(input_tokens) i, SUM(output_tokens) o,
                          SUM(cache_read_input_tokens) cr,
                          SUM(cache_creation_input_tokens) cc,
                          SUM(ephemeral_5m_tokens + iif(cache_creation_input_tokens > ephemeral_5m_tokens + ephemeral_1h_tokens, cache_creation_input_tokens - ephemeral_5m_tokens - ephemeral_1h_tokens, 0)) cw5,
                          SUM(ephemeral_1h_tokens) cw1,
                          SUM(estimated_cost_usd) est,
                          SUM(
                            input_tokens * 1.0
                            + output_tokens * 5.0
                            + cache_read_input_tokens * 0.1
                            + ephemeral_5m_tokens * 1.25
                            + ephemeral_1h_tokens * 2.0
                            + iif(cache_creation_input_tokens > ephemeral_5m_tokens + ephemeral_1h_tokens, cache_creation_input_tokens - ephemeral_5m_tokens - ephemeral_1h_tokens, 0) * 1.25
                          ) quota
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
                total_cache_write_5m: r.get(10)?,
                total_cache_write_1h: r.get(11)?,
                quota_tokens: r.get(12)?,
                last_cost_usd: r.get(13)?,
                estimated_cost_usd: r.get(14)?,
            })
        })
        .map_err(|e| internal(anyhow::anyhow!(e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    Ok(Json(rows))
}

#[derive(Serialize)]
struct QuotaCaps {
    five_hour: Option<i64>,
    weekly: Option<i64>,
    derived_from_ts: Option<i64>,
}

async fn quota_caps(State(state): State<AppState>) -> Result<Json<QuotaCaps>, (StatusCode, String)> {
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    // Latest snapshot with at least one non-null pct > 0.
    let latest: Option<(i64, Option<f64>, Option<f64>)> = conn
        .query_row(
            r#"SELECT ts, five_hour_pct, seven_day_pct FROM snapshots
                 WHERE (five_hour_pct IS NOT NULL AND five_hour_pct > 0)
                    OR (seven_day_pct  IS NOT NULL AND seven_day_pct  > 0)
                 ORDER BY ts DESC LIMIT 1"#,
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((ts, p5_opt, p7_opt)) = latest else {
        return Ok(Json(QuotaCaps {
            five_hour: None,
            weekly: None,
            derived_from_ts: None,
        }));
    };
    const QUOTA_SQL: &str = r#"SELECT COALESCE(SUM(
        input_tokens * 1.0
        + output_tokens * 5.0
        + cache_read_input_tokens * 0.1
        + ephemeral_5m_tokens * 1.25
        + ephemeral_1h_tokens * 2.0
        + iif(cache_creation_input_tokens > ephemeral_5m_tokens + ephemeral_1h_tokens,
              cache_creation_input_tokens - ephemeral_5m_tokens - ephemeral_1h_tokens, 0) * 1.25
      ), 0) FROM turns WHERE ts > ?1 AND ts <= ?2"#;
    let derive = |window_ms: i64, pct: Option<f64>| -> Option<i64> {
        let p = pct.filter(|p| *p > 0.0)?;
        let sum: f64 = conn
            .query_row(QUOTA_SQL, [ts - window_ms, ts], |r| r.get(0))
            .ok()?;
        if sum <= 0.0 {
            return None;
        }
        Some((sum / (p / 100.0)) as i64)
    };
    Ok(Json(QuotaCaps {
        five_hour: derive(5 * 3600 * 1000, p5_opt),
        weekly: derive(7 * 86400 * 1000, p7_opt),
        derived_from_ts: Some(ts),
    }))
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
    estimated_cost_usd: Option<f64>,
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
                      ephemeral_1h_tokens, ephemeral_5m_tokens,
                      estimated_cost_usd
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
                estimated_cost_usd: r.get(9)?,
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

#[derive(Deserialize)]
struct TrendQuery {
    window: Option<String>,
}

#[derive(Serialize)]
struct TrendRow {
    ts: i64,
    total_tokens: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_read: i64,
    total_cache_creation: i64,
    total_cost_usd: Option<f64>,
    turns: i64,
}

async fn list_trends(
    State(state): State<AppState>,
    Query(params): Query<TrendQuery>,
) -> Result<Json<Vec<TrendRow>>, (StatusCode, String)> {
    let bucket_seconds: i64 = match params.window.as_deref().unwrap_or("day") {
        "day" => 86_400,
        "week" => 7 * 86_400,
        "hour" => 3_600,
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unsupported window '{other}' (expected hour|day|week)"),
            ));
        }
    };
    let bucket_ms = bucket_seconds * 1_000;
    let conn = state.db.get().map_err(|e| internal(anyhow::anyhow!(e)))?;
    let mut stmt = conn
        .prepare(
            r#"SELECT (ts / ?1) * ?2                  AS bucket_ts,
                      SUM(input_tokens)               AS i,
                      SUM(output_tokens)              AS o,
                      SUM(cache_read_input_tokens)    AS cr,
                      SUM(cache_creation_input_tokens) AS cc,
                      SUM(estimated_cost_usd)         AS cost,
                      COUNT(*)                        AS n
                 FROM turns
                 GROUP BY bucket_ts
                 ORDER BY bucket_ts ASC"#,
        )
        .map_err(|e| internal(anyhow::anyhow!(e)))?;
    let rows = stmt
        .query_map([bucket_ms, bucket_seconds], |r| {
            let i: i64 = r.get::<_, Option<i64>>(1)?.unwrap_or(0);
            let o: i64 = r.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let cr: i64 = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
            let cc: i64 = r.get::<_, Option<i64>>(4)?.unwrap_or(0);
            Ok(TrendRow {
                ts: r.get(0)?,
                total_tokens: i + o + cr + cc,
                total_input_tokens: i,
                total_output_tokens: o,
                total_cache_read: cr,
                total_cache_creation: cc,
                total_cost_usd: r.get(5)?,
                turns: r.get(6)?,
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
