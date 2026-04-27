use crate::state::AppState;
use anyhow::Result;
use chrono::Utc;
use cm_core::config::PathFilter;
use cm_core::db;
use cm_core::schema::LiveEvent;
use cm_core::transcript::parse_assistant_usage;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// Spawns a blocking thread that watches `~/.claude/projects/**/*.jsonl`, tails new bytes
/// into the DB, and broadcasts each new turn on the live channel.
pub fn spawn(state: AppState, watch_root: PathBuf, filter: PathFilter) {
    std::thread::spawn(move || {
        if let Err(e) = run(state, watch_root, filter) {
            tracing::error!("tailer exited: {e:#}");
        }
    });
}

fn run(state: AppState, watch_root: PathBuf, filter: PathFilter) -> Result<()> {
    std::fs::create_dir_all(&watch_root).ok();

    // Drain every jsonl that already exists, so history shows up immediately.
    let existing = discover_jsonl(&watch_root)?;
    for p in &existing {
        if !filter.matches(p) {
            continue;
        }
        if let Err(e) = ingest_new_bytes(&state, p) {
            tracing::warn!("initial ingest {}: {e:#}", p.display());
        }
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    let tx_clone = tx.clone();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            if matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Any
            ) {
                for p in ev.paths {
                    if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                        let _ = tx_clone.send(p);
                    }
                }
            }
        }
    })?;
    watcher.watch(&watch_root, RecursiveMode::Recursive)?;

    // Drain the channel with mild debouncing via a small batch window.
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let mut pending: HashMap<PathBuf, ()> = HashMap::new();
        loop {
            tokio::select! {
                Some(p) = rx.recv() => {
                    if filter.matches(&p) {
                        pending.insert(p, ());
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {
                    if pending.is_empty() { continue; }
                    let paths: Vec<_> = pending.drain().map(|(p,_)| p).collect();
                    for p in paths {
                        if let Err(e) = ingest_new_bytes(&state, &p) {
                            tracing::warn!("ingest {}: {e:#}", p.display());
                        }
                    }
                }
            }
        }
    });
    Ok(())
}

fn discover_jsonl(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
    Ok(())
}

pub fn ingest_new_bytes(state: &AppState, path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    let mut offset = db::get_tail_offset(&state.db, &path_str)?;
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len < offset {
        // File was truncated or rotated — restart from the beginning.
        offset = 0;
    }
    if len == offset {
        return Ok(());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut consumed = offset;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        // If the last line lacks a newline the file is still being written — stop so we resume cleanly.
        if !line.ends_with('\n') {
            break;
        }
        consumed += n as u64;
        if let Some(turn) = parse_assistant_usage(line.trim_end()) {
            let project_dir = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .map(decode_project_dir);
            db::upsert_session(
                &state.db,
                &turn.session_id,
                project_dir.as_deref(),
                Some(&path_str),
                turn.model_id.as_deref(),
                turn.ts_ms.max(Utc::now().timestamp_millis()),
            )
            .ok();
            if db::insert_turn(&state.db, &turn).unwrap_or(false) {
                let _ = state.tx.send(LiveEvent::Turn(turn));
            }
        }
    }
    db::set_tail_offset(&state.db, &path_str, consumed)?;
    Ok(())
}

/// `~/.claude/projects/-Users--vpon-Documents-foo` → `/Users/_vpon/Documents/foo`
fn decode_project_dir(dirname: &str) -> String {
    let mut s = dirname.replace("--", "\0");
    s = s.replace('-', "/");
    s.replace('\0', "-")
}
