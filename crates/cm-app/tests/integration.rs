use axum::body::Body;
use axum::http::{Request, StatusCode};
use cm_app::state::AppState;
use cm_core::db;
use http_body_util::BodyExt;
use std::io::Write;
use std::path::PathBuf;
use tempfile::NamedTempFile;
use tower::ServiceExt;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn state_on_tempdb() -> (AppState, NamedTempFile) {
    let tmp = NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    (AppState::new(pool, fixtures_dir()), tmp)
}

const SNAPSHOT_JSON: &str = r#"{
    "session_id": "S1",
    "transcript_path": "/tmp/t.jsonl",
    "model": { "id": "claude-opus-4-7", "display_name": "Opus" },
    "workspace": { "project_dir": "/work" },
    "cost": { "total_cost_usd": 0.42, "total_duration_ms": 1000, "total_api_duration_ms": 200 },
    "context_window": { "used_percentage": 12.5 }
}"#;

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let (state, _db) = state_on_tempdb();
    let app = cm_app::server::router(state);
    let res = app
        .oneshot(Request::builder().uri("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn post_event_persists_snapshot_and_session() {
    let (state, _db) = state_on_tempdb();
    let app = cm_app::server::router(state.clone());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/event")
                .header("content-type", "application/json")
                .body(Body::from(SNAPSHOT_JSON))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let conn = state.db.get().unwrap();
    let (sid, pct, cost): (String, Option<f64>, Option<f64>) = conn
        .query_row(
            "SELECT session_id, context_used_pct, total_cost_usd FROM snapshots",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(sid, "S1");
    assert_eq!(pct, Some(12.5));
    assert_eq!(cost, Some(0.42));
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE session_id='S1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn post_event_rejects_garbage_json() {
    let (state, _db) = state_on_tempdb();
    let app = cm_app::server::router(state);
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/event")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"not":"a snapshot"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_subagent_event_requires_session_id_somewhere() {
    let (state, _db) = state_on_tempdb();
    let app = cm_app::server::router(state);
    let payload = r#"{"tasks":[{"id":"a","status":"running"}]}"#;
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/subagent-event")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_subagent_event_accepts_session_id_from_query() {
    let (state, _db) = state_on_tempdb();
    let app = cm_app::server::router(state.clone());
    let payload = r#"{"tasks":[{"id":"a","status":"running","tokenCount":42}]}"#;
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/subagent-event?session_id=SQ")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let conn = state.db.get().unwrap();
    let (tid, tok): (String, Option<i64>) = conn
        .query_row(
            "SELECT task_id, token_count FROM subagent_tasks WHERE session_id='SQ'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(tid, "a");
    assert_eq!(tok, Some(42));
}

#[tokio::test]
async fn trends_endpoint_rejects_unknown_window() {
    let (state, _db) = state_on_tempdb();
    let app = cm_app::server::router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/v1/trends?window=month")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sessions_list_reflects_inserted_data() {
    let (state, _db) = state_on_tempdb();
    // Seed directly so we can assert on the JSON without round-tripping.
    cm_core::db::upsert_session(&state.db, "S1", Some("/work"), None, Some("claude-opus-4-7"), 1_000).unwrap();
    cm_core::db::insert_turn(
        &state.db,
        &cm_core::schema::TurnUsage {
            session_id: "S1".into(),
            turn_uuid: "t1".into(),
            ts_ms: 1_000,
            model_id: Some("claude-opus-4-7".into()),
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            ephemeral_1h_tokens: 0,
            ephemeral_5m_tokens: 0,
            service_tier: None,
        },
    )
    .unwrap();
    let app = cm_app::server::router(state);
    let res = app
        .oneshot(Request::builder().uri("/v1/sessions").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = v.as_array().expect("sessions is an array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["session_id"], "S1");
    assert_eq!(arr[0]["total_turns"], 1);
    assert_eq!(arr[0]["total_input_tokens"], 10);
    assert_eq!(arr[0]["total_output_tokens"], 20);
}

#[test]
fn tailer_ingests_new_assistant_lines_and_advances_offset() {
    let (state, _db) = state_on_tempdb();
    let mut tf = NamedTempFile::new().unwrap();
    let line = r#"{"type":"assistant","sessionId":"SX","timestamp":"2026-04-01T00:00:00Z","uuid":"u-outer","message":{"id":"u-msg","model":"claude-opus-4-7","usage":{"input_tokens":5,"output_tokens":7,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    writeln!(tf, "{line}").unwrap();
    tf.flush().unwrap();

    cm_app::tailer::ingest_new_bytes(&state, tf.path()).unwrap();

    let conn = state.db.get().unwrap();
    let (sid, inp): (String, i64) = conn
        .query_row(
            "SELECT session_id, input_tokens FROM turns WHERE turn_uuid='u-msg'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(sid, "SX");
    assert_eq!(inp, 5);
    let off = cm_core::db::get_tail_offset(&state.db, &tf.path().to_string_lossy()).unwrap();
    assert!(off > 0, "tail offset should have advanced, got {off}");
}

#[test]
fn tailer_stops_at_partial_line_and_resumes_when_completed() {
    let (state, _db) = state_on_tempdb();
    let mut tf = NamedTempFile::new().unwrap();
    // Write a partial line (no trailing newline). Tailer must NOT advance past it.
    let partial = r#"{"type":"assistant","sessionId":"SY","timestamp":"2026-04-01T00:00:00Z","uuid":"u1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":2}"#;
    tf.write_all(partial.as_bytes()).unwrap();
    tf.flush().unwrap();
    cm_app::tailer::ingest_new_bytes(&state, tf.path()).unwrap();
    let conn = state.db.get().unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0, "partial line must not be ingested");
    // Now complete the line and run again.
    tf.write_all(b"}}\n").unwrap();
    tf.flush().unwrap();
    cm_app::tailer::ingest_new_bytes(&state, tf.path()).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "completed line should be ingested on the second pass");
}

#[test]
fn tailer_restarts_on_truncation() {
    let (state, _db) = state_on_tempdb();
    let mut tf = NamedTempFile::new().unwrap();
    // Write two lines first so the stored offset is larger than any post-rotation file.
    let line1 = r#"{"type":"assistant","sessionId":"ST","timestamp":"2026-04-01T00:00:00Z","uuid":"u1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":3,"output_tokens":4}}}"#;
    let line1b = r#"{"type":"assistant","sessionId":"ST","timestamp":"2026-04-01T00:00:01Z","uuid":"u1b","message":{"id":"m1b","model":"claude-opus-4-7","usage":{"input_tokens":3,"output_tokens":4}}}"#;
    writeln!(tf, "{line1}").unwrap();
    writeln!(tf, "{line1b}").unwrap();
    tf.flush().unwrap();
    cm_app::tailer::ingest_new_bytes(&state, tf.path()).unwrap();
    let initial_offset = cm_core::db::get_tail_offset(&state.db, &tf.path().to_string_lossy()).unwrap();
    assert!(initial_offset > 0);

    // Rotate: truncate the file to empty, then write a shorter fresh line.
    let f = std::fs::OpenOptions::new().write(true).truncate(true).open(tf.path()).unwrap();
    drop(f);
    let mut tf2 = std::fs::OpenOptions::new().append(true).open(tf.path()).unwrap();
    let line2 = r#"{"type":"assistant","sessionId":"ST","timestamp":"2026-04-01T00:00:02Z","uuid":"u2","message":{"id":"m2","model":"claude-opus-4-7","usage":{"input_tokens":9,"output_tokens":0}}}"#;
    writeln!(tf2, "{line2}").unwrap();
    tf2.flush().unwrap();
    assert!(
        std::fs::metadata(tf.path()).unwrap().len() < initial_offset,
        "post-rotation file must be shorter than stored offset to trigger restart"
    );

    cm_app::tailer::ingest_new_bytes(&state, tf.path()).unwrap();
    let conn = state.db.get().unwrap();
    let inp: Option<i64> = conn
        .query_row("SELECT input_tokens FROM turns WHERE turn_uuid='m2'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(inp, Some(9), "post-rotation line must be ingested");
}

#[test]
fn drain_queue_persists_queued_snapshots_and_removes_file() {
    let (state, _db) = state_on_tempdb();
    let mut tf = NamedTempFile::new().unwrap();
    writeln!(tf, "{}", SNAPSHOT_JSON.replace('\n', "")).unwrap();
    writeln!(tf, "").unwrap(); // blank line should be tolerated
    writeln!(tf, r#"{{"garbage":true}}"#).unwrap(); // bad line should be skipped
    tf.flush().unwrap();
    let path = tf.path().to_path_buf();

    cm_app::drain_queue(&state, &path).unwrap();

    assert!(!path.exists(), "drain should have removed the queue file");
    let conn = state.db.get().unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM snapshots WHERE session_id='S1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}
