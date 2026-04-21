pub mod server;
pub mod state;
pub mod tailer;

use anyhow::Result;
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
    let db = db::open(&paths::db_path())?;
    tracing::info!("UI dir: {}", ui_dir.display());

    let state = AppState::new(db, ui_dir);

    drain_queue(&state).ok();
    tailer::spawn(state.clone(), paths::claude_projects_dir());

    let port = std::env::var("CM_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
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

fn drain_queue(state: &AppState) -> Result<()> {
    let p = paths::queue_file();
    if !p.exists() {
        return Ok(());
    }
    let body = std::fs::read_to_string(&p)?;
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
    std::fs::remove_file(&p).ok();
    Ok(())
}
