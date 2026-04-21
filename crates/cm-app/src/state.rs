use cm_core::db::Pool;
use cm_core::schema::LiveEvent;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub db: Pool,
    pub tx: broadcast::Sender<LiveEvent>,
    pub ui_dir: Arc<std::path::PathBuf>,
}

impl AppState {
    pub fn new(db: Pool, ui_dir: std::path::PathBuf) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            db,
            tx,
            ui_dir: Arc::new(ui_dir),
        }
    }
}
