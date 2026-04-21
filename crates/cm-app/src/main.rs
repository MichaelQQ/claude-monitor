use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    cm_app::init_tracing();
    let ui_dir = cm_app::locate_ui_dir();
    let daemon = cm_app::start(ui_dir).await?;
    let _ = tokio::signal::ctrl_c().await;
    let _ = daemon.shutdown_tx.send(());
    let _ = daemon.join.await;
    Ok(())
}
