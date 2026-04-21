use crate::schema::{StatuslineInput, TurnUsage};
use anyhow::Result;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use std::path::Path;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

pub fn open(path: &Path) -> Result<Pool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let manager = SqliteConnectionManager::file(path);
    let pool = r2d2::Pool::builder().max_size(8).build(manager)?;
    let conn = pool.get()?;
    conn.execute_batch(SCHEMA)?;
    Ok(pool)
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sessions (
    session_id      TEXT PRIMARY KEY,
    project_dir     TEXT,
    transcript_path TEXT,
    model_id        TEXT,
    started_at      INTEGER,
    last_seen_at    INTEGER
);

CREATE TABLE IF NOT EXISTS turns (
    id             INTEGER PRIMARY KEY,
    session_id     TEXT NOT NULL,
    turn_uuid      TEXT NOT NULL UNIQUE,
    ts             INTEGER NOT NULL,
    model_id       TEXT,
    input_tokens                INTEGER NOT NULL DEFAULT 0,
    output_tokens               INTEGER NOT NULL DEFAULT 0,
    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens     INTEGER NOT NULL DEFAULT 0,
    ephemeral_1h_tokens         INTEGER NOT NULL DEFAULT 0,
    ephemeral_5m_tokens         INTEGER NOT NULL DEFAULT 0,
    service_tier                TEXT
);
CREATE INDEX IF NOT EXISTS idx_turns_session_ts ON turns(session_id, ts);

CREATE TABLE IF NOT EXISTS snapshots (
    id             INTEGER PRIMARY KEY,
    session_id     TEXT NOT NULL,
    ts             INTEGER NOT NULL,
    total_cost_usd        REAL,
    total_duration_ms     INTEGER,
    total_api_duration_ms INTEGER,
    context_used_pct      REAL,
    context_current_input INTEGER,
    context_current_output INTEGER,
    context_current_cache_creation INTEGER,
    context_current_cache_read     INTEGER,
    five_hour_pct         REAL,
    five_hour_resets_at   INTEGER,
    seven_day_pct         REAL,
    seven_day_resets_at   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_snapshots_session_ts ON snapshots(session_id, ts);

CREATE TABLE IF NOT EXISTS tail_state (
    path    TEXT PRIMARY KEY,
    offset  INTEGER NOT NULL
);
"#;

pub fn upsert_session(
    pool: &Pool,
    session_id: &str,
    project_dir: Option<&str>,
    transcript_path: Option<&str>,
    model_id: Option<&str>,
    ts_ms: i64,
) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        r#"INSERT INTO sessions (session_id, project_dir, transcript_path, model_id, started_at, last_seen_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?5)
           ON CONFLICT(session_id) DO UPDATE SET
             project_dir     = COALESCE(excluded.project_dir, sessions.project_dir),
             transcript_path = COALESCE(excluded.transcript_path, sessions.transcript_path),
             model_id        = COALESCE(excluded.model_id, sessions.model_id),
             last_seen_at    = MAX(sessions.last_seen_at, excluded.last_seen_at)"#,
        params![session_id, project_dir, transcript_path, model_id, ts_ms],
    )?;
    Ok(())
}

pub fn insert_turn(pool: &Pool, t: &TurnUsage) -> Result<bool> {
    let conn = pool.get()?;
    let n = conn.execute(
        r#"INSERT OR IGNORE INTO turns (
             session_id, turn_uuid, ts, model_id,
             input_tokens, output_tokens,
             cache_creation_input_tokens, cache_read_input_tokens,
             ephemeral_1h_tokens, ephemeral_5m_tokens, service_tier
           ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"#,
        params![
            t.session_id,
            t.turn_uuid,
            t.ts_ms,
            t.model_id,
            t.input_tokens,
            t.output_tokens,
            t.cache_creation_input_tokens,
            t.cache_read_input_tokens,
            t.ephemeral_1h_tokens,
            t.ephemeral_5m_tokens,
            t.service_tier,
        ],
    )?;
    Ok(n > 0)
}

pub fn insert_snapshot(pool: &Pool, s: &StatuslineInput, ts_ms: i64) -> Result<()> {
    let conn = pool.get()?;
    let cost = s.cost.as_ref();
    let ctx = s.context_window.as_ref();
    let cur = ctx.and_then(|c| c.current_usage.as_ref());
    let five = s.rate_limits.as_ref().and_then(|r| r.five_hour.as_ref());
    let seven = s.rate_limits.as_ref().and_then(|r| r.seven_day.as_ref());
    conn.execute(
        r#"INSERT INTO snapshots (
             session_id, ts,
             total_cost_usd, total_duration_ms, total_api_duration_ms,
             context_used_pct, context_current_input, context_current_output,
             context_current_cache_creation, context_current_cache_read,
             five_hour_pct, five_hour_resets_at,
             seven_day_pct, seven_day_resets_at
           ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)"#,
        params![
            s.session_id,
            ts_ms,
            cost.and_then(|c| c.total_cost_usd),
            cost.and_then(|c| c.total_duration_ms),
            cost.and_then(|c| c.total_api_duration_ms),
            ctx.and_then(|c| c.used_percentage),
            cur.and_then(|c| c.input_tokens),
            cur.and_then(|c| c.output_tokens),
            cur.and_then(|c| c.cache_creation_input_tokens),
            cur.and_then(|c| c.cache_read_input_tokens),
            five.map(|f| f.used_percentage),
            five.map(|f| f.resets_at),
            seven.map(|s| s.used_percentage),
            seven.map(|s| s.resets_at),
        ],
    )?;
    Ok(())
}

pub fn get_tail_offset(pool: &Pool, path: &str) -> Result<u64> {
    let conn = pool.get()?;
    let off: Option<i64> = conn
        .query_row(
            "SELECT offset FROM tail_state WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )
        .optional()?;
    Ok(off.unwrap_or(0) as u64)
}

pub fn set_tail_offset(pool: &Pool, path: &str, offset: u64) -> Result<()> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO tail_state (path, offset) VALUES (?1, ?2)
         ON CONFLICT(path) DO UPDATE SET offset = excluded.offset",
        params![path, offset as i64],
    )?;
    Ok(())
}
