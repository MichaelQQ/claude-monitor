use std::path::PathBuf;

pub fn home() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.home_dir().to_path_buf().into())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn claude_projects_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

pub fn app_data_dir() -> PathBuf {
    home().join(".claude").join("claude-monitor")
}

pub fn db_path() -> PathBuf {
    app_data_dir().join("monitor.db")
}

pub fn port_file() -> PathBuf {
    app_data_dir().join("port")
}

pub fn queue_file() -> PathBuf {
    app_data_dir().join("queue.jsonl")
}
