pub mod server;
pub mod state;
pub mod tailer;

use anyhow::Result;
use cm_core::config::{self, PathFilter};
use cm_core::{db, paths};
use state::AppState;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use tokio::sync::oneshot;

pub struct Daemon {
    pub port: u16,
    pub shutdown_tx: oneshot::Sender<()>,
    pub join: tokio::task::JoinHandle<Result<()>>,
}

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cm_app=info,tower_http=warn".into()),
        )
        .try_init();
}

pub async fn start(ui_dir: PathBuf) -> Result<Daemon> {
    let app_dir = paths::app_data_dir();
    std::fs::create_dir_all(&app_dir).ok();
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("config.toml invalid, using defaults: {e:#}");
            config::Config::default()
        }
    };
    let db = db::open(&paths::db_path())?;
    tracing::info!("UI dir: {}", ui_dir.display());

    let state = AppState::new(db, ui_dir);

    drain_queue(&state, &paths::queue_file()).ok();
    let filter = PathFilter::from_config(&cfg)?;
    tailer::spawn(state.clone(), paths::claude_projects_dir(), filter);
    spawn_retention(state.clone(), cfg.retention_days);

    let port = std::env::var("CM_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .or(cfg.port)
        .unwrap_or(0);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let port = bound.port();
    std::fs::write(paths::port_file(), port.to_string())?;
    tracing::info!("serving on http://{}", bound);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let router = server::router(state);
    let join = tokio::spawn(async move {
        let res = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        std::fs::remove_file(paths::port_file()).ok();
        res.map_err(Into::into)
    });

    Ok(Daemon {
        port,
        shutdown_tx,
        join,
    })
}

/// Apply `retention_days` once now and then every 24h. `None` or `Some(0)`
/// disables retention entirely.
fn spawn_retention(state: AppState, retention_days: Option<u32>) {
    let Some(days) = retention_days.filter(|d| *d > 0) else {
        return;
    };
    tokio::spawn(async move {
        let period = std::time::Duration::from_secs(24 * 3600);
        loop {
            run_retention_once(&state, days);
            tokio::time::sleep(period).await;
        }
    });
}

fn run_retention_once(state: &AppState, days: u32) {
    let cutoff_ms = chrono::Utc::now().timestamp_millis() - (days as i64) * 86_400_000;
    match db::delete_older_than(&state.db, cutoff_ms) {
        Ok(s) if s.turns + s.snapshots + s.subagent_tasks + s.sessions > 0 => {
            tracing::info!(
                "retention: removed {} turns, {} snapshots, {} subagent rows, {} sessions (older than {}d)",
                s.turns, s.snapshots, s.subagent_tasks, s.sessions, days
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("retention sweep failed: {e:#}"),
    }
}

pub fn locate_ui_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.as_path();
        while let Some(parent) = dir.parent() {
            let candidate = parent.join("crates/cm-app/ui");
            if candidate.join("index.html").exists() {
                return candidate;
            }
            dir = parent;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

pub fn drain_queue(state: &AppState, path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let body = std::fs::read_to_string(path)?;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(input) = serde_json::from_str::<cm_core::schema::StatuslineInput>(line) else {
            continue;
        };
        let ts = chrono::Utc::now().timestamp_millis();
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
        .ok();
        db::insert_snapshot(&state.db, &input, ts).ok();
    }
    std::fs::remove_file(path).ok();
    Ok(())
}
