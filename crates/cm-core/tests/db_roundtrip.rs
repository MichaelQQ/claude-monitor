use cm_core::db;
use cm_core::schema::{
    ContextWindow, Cost, CurrentUsage, Model, RateLimitWindow, RateLimits, StatuslineInput,
    SubagentTask, TurnUsage,
};

fn sample_turn(uuid: &str) -> TurnUsage {
    TurnUsage {
        session_id: "s1".into(),
        turn_uuid: uuid.into(),
        ts_ms: 1_700_000_000_000,
        model_id: Some("claude-opus-4-7".into()),
        input_tokens: 10,
        output_tokens: 20,
        cache_creation_input_tokens: 30,
        cache_read_input_tokens: 40,
        ephemeral_1h_tokens: 5,
        ephemeral_5m_tokens: 1,
        service_tier: Some("standard".into()),
    }
}

fn sample_snapshot() -> StatuslineInput {
    StatuslineInput {
        session_id: "s1".into(),
        transcript_path: Some("/tmp/x.jsonl".into()),
        model: Model {
            id: "claude-opus-4-7".into(),
            display_name: "Opus".into(),
        },
        workspace: None,
        cost: Some(Cost {
            total_cost_usd: Some(0.12),
            total_duration_ms: Some(45_000),
            total_api_duration_ms: Some(2_300),
        }),
        context_window: Some(ContextWindow {
            total_input_tokens: None,
            total_output_tokens: None,
            context_window_size: Some(200_000),
            used_percentage: Some(8.0),
            remaining_percentage: Some(92.0),
            current_usage: Some(CurrentUsage {
                input_tokens: Some(8_500),
                output_tokens: Some(1_200),
                cache_creation_input_tokens: Some(5_000),
                cache_read_input_tokens: Some(2_000),
            }),
        }),
        rate_limits: Some(RateLimits {
            five_hour: Some(RateLimitWindow {
                used_percentage: 23.5,
                resets_at: 1_738_425_600,
            }),
            seven_day: None,
        }),
    }
}

#[test]
fn turn_insert_is_idempotent_on_uuid() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    let t = sample_turn("u1");
    assert!(db::insert_turn(&pool, &t).unwrap(), "first insert");
    assert!(!db::insert_turn(&pool, &t).unwrap(), "second is no-op");
    let conn = pool.get().unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn turn_insert_writes_estimated_cost() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    db::insert_turn(&pool, &sample_turn("u1")).unwrap();
    let conn = pool.get().unwrap();
    let cost: Option<f64> = conn
        .query_row(
            "SELECT estimated_cost_usd FROM turns WHERE turn_uuid='u1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let c = cost.expect("opus turn should have an estimate");
    assert!(c > 0.0, "got {c}");
}

#[test]
fn session_upsert_merges_fields_without_wiping() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    db::upsert_session(&pool, "s1", Some("/a"), None, Some("m1"), 100).unwrap();
    db::upsert_session(&pool, "s1", None, Some("/t.jsonl"), None, 200).unwrap();
    let conn = pool.get().unwrap();
    let row: (Option<String>, Option<String>, Option<String>, i64, i64) = conn
        .query_row(
            "SELECT project_dir, transcript_path, model_id, started_at, last_seen_at FROM sessions WHERE session_id='s1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("/a"));
    assert_eq!(row.1.as_deref(), Some("/t.jsonl"));
    assert_eq!(row.2.as_deref(), Some("m1"));
    assert_eq!(row.3, 100);
    assert_eq!(row.4, 200);
}

#[test]
fn snapshot_insert_handles_absent_rate_limit_window() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    let s = sample_snapshot();
    db::insert_snapshot(&pool, &s, 1_700_000_000_000).unwrap();
    let conn = pool.get().unwrap();
    let row: (Option<f64>, Option<i64>, Option<f64>, Option<i64>) = conn
        .query_row(
            "SELECT five_hour_pct, five_hour_resets_at, seven_day_pct, seven_day_resets_at FROM snapshots",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row.0, Some(23.5));
    assert_eq!(row.1, Some(1_738_425_600));
    assert_eq!(row.2, None);
    assert_eq!(row.3, None);
}

#[test]
fn subagent_tasks_upsert_merges_fields_and_holds_first_seen() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    let t = SubagentTask {
        id: "a1".into(),
        name: Some("planner".into()),
        task_type: Some("plan".into()),
        status: Some("running".into()),
        token_count: Some(1_234),
        cwd: Some("/work".into()),
        start_time: Some(1_700_000_000.0),
        ..Default::default()
    };
    db::upsert_subagent_tasks(&pool, "s1", &[t.clone()], 1_000).unwrap();
    let mut later = t.clone();
    later.status = Some("done".into());
    later.token_count = Some(5_678);
    later.name = None; // absent fields must not wipe existing columns
    db::upsert_subagent_tasks(&pool, "s1", &[later], 2_000).unwrap();
    let conn = pool.get().unwrap();
    let row: (String, Option<String>, Option<i64>, i64, i64) = conn
        .query_row(
            "SELECT status, name, token_count, first_seen_at, last_seen_at FROM subagent_tasks WHERE session_id='s1' AND task_id='a1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(row.0, "done");
    assert_eq!(row.1.as_deref(), Some("planner"));
    assert_eq!(row.2, Some(5_678));
    assert_eq!(row.3, 1_000);
    assert_eq!(row.4, 2_000);
}

#[test]
fn tail_offset_persists_across_queries() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pool = db::open(tmp.path()).unwrap();
    assert_eq!(db::get_tail_offset(&pool, "/a").unwrap(), 0);
    db::set_tail_offset(&pool, "/a", 12345).unwrap();
    assert_eq!(db::get_tail_offset(&pool, "/a").unwrap(), 12345);
    db::set_tail_offset(&pool, "/a", 99999).unwrap();
    assert_eq!(db::get_tail_offset(&pool, "/a").unwrap(), 99999);
}
