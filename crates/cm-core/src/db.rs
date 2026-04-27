use crate::pricing;
use crate::schema::{StatuslineInput, SubagentTask, TurnUsage};
use anyhow::Result;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension};
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
    migrate(&conn)?;
    Ok(pool)
}

/// Ordered list of incremental migrations. Each entry runs when its 1-based
/// index is greater than the DB's `user_version`. Append only — never reorder
/// or delete, since that changes version numbers on already-migrated DBs.
type MigrationFn = fn(&Connection) -> Result<()>;
const MIGRATIONS: &[(&str, MigrationFn)] = &[
    ("add_turns_estimated_cost_usd", migrate_v1_estimated_cost),
];

fn migrate(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, (_name, f)) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as i64;
        if version > current {
            f(conn)?;
            conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
        }
    }
    Ok(())
}

fn migrate_v1_estimated_cost(conn: &Connection) -> Result<()> {
    if !has_column(conn, "turns", "estimated_cost_usd")? {
        conn.execute("ALTER TABLE turns ADD COLUMN estimated_cost_usd REAL", [])?;
    }
    backfill_turn_costs(conn)?;
    Ok(())
}

fn backfill_turn_costs(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare(
        r#"SELECT id, session_id, turn_uuid, ts, model_id,
                  input_tokens, output_tokens,
                  cache_creation_input_tokens, cache_read_input_tokens,
                  ephemeral_1h_tokens, ephemeral_5m_tokens, service_tier
             FROM turns WHERE estimated_cost_usd IS NULL"#,
    )?;
    let rows: Vec<(i64, TurnUsage)> = select
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            Ok((
                id,
                TurnUsage {
                    session_id: r.get(1)?,
                    turn_uuid: r.get(2)?,
                    ts_ms: r.get(3)?,
                    model_id: r.get(4)?,
                    input_tokens: r.get(5)?,
                    output_tokens: r.get(6)?,
                    cache_creation_input_tokens: r.get(7)?,
                    cache_read_input_tokens: r.get(8)?,
                    ephemeral_1h_tokens: r.get(9)?,
                    ephemeral_5m_tokens: r.get(10)?,
                    service_tier: r.get(11)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(select);
    let mut update = conn.prepare("UPDATE turns SET estimated_cost_usd = ?1 WHERE id = ?2")?;
    for (id, t) in rows {
        if let Some(cost) = pricing::estimate_cost_usd(&t) {
            update.execute(params![cost, id])?;
        }
    }
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
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
    service_tier                TEXT,
    estimated_cost_usd          REAL
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

CREATE TABLE IF NOT EXISTS subagent_tasks (
    session_id     TEXT NOT NULL,
    task_id        TEXT NOT NULL,
    name           TEXT,
    task_type      TEXT,
    status         TEXT,
    description    TEXT,
    label          TEXT,
    start_time     REAL,
    token_count    INTEGER,
    cwd            TEXT,
    first_seen_at  INTEGER NOT NULL,
    last_seen_at   INTEGER NOT NULL,
    PRIMARY KEY (session_id, task_id)
);
CREATE INDEX IF NOT EXISTS idx_subagent_tasks_session ON subagent_tasks(session_id, last_seen_at);
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
    let estimated_cost_usd = pricing::estimate_cost_usd(t);
    let n = conn.execute(
        r#"INSERT OR IGNORE INTO turns (
             session_id, turn_uuid, ts, model_id,
             input_tokens, output_tokens,
             cache_creation_input_tokens, cache_read_input_tokens,
             ephemeral_1h_tokens, ephemeral_5m_tokens, service_tier,
             estimated_cost_usd
           ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)"#,
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
            estimated_cost_usd,
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

pub fn upsert_subagent_tasks(
    pool: &Pool,
    session_id: &str,
    tasks: &[SubagentTask],
    ts_ms: i64,
) -> Result<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            r#"INSERT INTO subagent_tasks (
                 session_id, task_id, name, task_type, status,
                 description, label, start_time, token_count, cwd,
                 first_seen_at, last_seen_at
               ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)
               ON CONFLICT(session_id, task_id) DO UPDATE SET
                 name          = COALESCE(excluded.name, subagent_tasks.name),
                 task_type     = COALESCE(excluded.task_type, subagent_tasks.task_type),
                 status        = COALESCE(excluded.status, subagent_tasks.status),
                 description   = COALESCE(excluded.description, subagent_tasks.description),
                 label         = COALESCE(excluded.label, subagent_tasks.label),
                 start_time    = COALESCE(excluded.start_time, subagent_tasks.start_time),
                 token_count   = COALESCE(excluded.token_count, subagent_tasks.token_count),
                 cwd           = COALESCE(excluded.cwd, subagent_tasks.cwd),
                 last_seen_at  = excluded.last_seen_at"#,
        )?;
        for t in tasks {
            stmt.execute(params![
                session_id,
                t.id,
                t.name,
                t.task_type,
                t.status,
                t.description,
                t.label,
                t.start_time,
                t.token_count,
                t.cwd,
                ts_ms,
            ])?;
        }
    }
    tx.commit()?;
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

/// Number of rows deleted, by table.
#[derive(Debug, Default, Clone, Copy)]
pub struct RetentionStats {
    pub turns: usize,
    pub snapshots: usize,
    pub subagent_tasks: usize,
    pub sessions: usize,
}

/// Delete rows older than `cutoff_ms`. Sessions are removed only when they
/// have no remaining turns, snapshots, or subagent tasks (i.e. only empty
/// shells get cleaned up).
pub fn delete_older_than(pool: &Pool, cutoff_ms: i64) -> Result<RetentionStats> {
    let conn = pool.get()?;
    let turns = conn.execute("DELETE FROM turns WHERE ts < ?1", params![cutoff_ms])?;
    let snapshots = conn.execute("DELETE FROM snapshots WHERE ts < ?1", params![cutoff_ms])?;
    let subagent_tasks = conn.execute(
        "DELETE FROM subagent_tasks WHERE last_seen_at < ?1",
        params![cutoff_ms],
    )?;
    let sessions = conn.execute(
        r#"DELETE FROM sessions
             WHERE last_seen_at < ?1
               AND NOT EXISTS (SELECT 1 FROM turns      WHERE session_id = sessions.session_id)
               AND NOT EXISTS (SELECT 1 FROM snapshots  WHERE session_id = sessions.session_id)
               AND NOT EXISTS (SELECT 1 FROM subagent_tasks WHERE session_id = sessions.session_id)"#,
        params![cutoff_ms],
    )?;
    Ok(RetentionStats {
        turns,
        snapshots,
        subagent_tasks,
        sessions,
    })
}
