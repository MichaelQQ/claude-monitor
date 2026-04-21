mod server;
mod state;
mod tailer;

use anyhow::Result;
use cm_core::{db, paths};
use state::AppState;
use std::net::{Ipv4Addr, SocketAddr};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "cm_app=info,tower_http=warn".into()))
        .init();

    let app_dir = paths::app_data_dir();
    std::fs::create_dir_all(&app_dir).ok();
    let db = db::open(&paths::db_path())?;

    // Default UI dir lives inside the binary's crate — look both at
    // <target>/../../crates/cm-app/ui (dev) and <exe dir>/ui (installed).
    let ui_dir = locate_ui_dir();
    tracing::info!("UI dir: {}", ui_dir.display());

    let state = AppState::new(db, ui_dir);

    // Drain any CLI-queued events from earlier offline periods.
    drain_queue(&state).ok();

    // Start the transcript tailer in a background thread.
    tailer::spawn(state.clone(), paths::claude_projects_dir());

    // Bind on any free port so two app instances can coexist during dev.
    let port = std::env::var("CM_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    std::fs::write(paths::port_file(), bound.port().to_string())?;
    tracing::info!("serving on http://{}", bound);

    let router = server::router(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown())
        .await?;
    // Clean up the port file on exit.
    std::fs::remove_file(paths::port_file()).ok();
    Ok(())
}

fn locate_ui_dir() -> std::path::PathBuf {
    // Walk up from the current binary looking for `crates/cm-app/ui`.
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
    // Fallback to CARGO_MANIFEST_DIR at compile time.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
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

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
