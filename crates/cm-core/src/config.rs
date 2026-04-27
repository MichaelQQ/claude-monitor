use crate::paths;
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Persistent configuration loaded from `~/.claude/claude-monitor/config.toml`.
/// All fields are optional; absent fields fall back to defaults defined below.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Pin the HTTP port. If unset, the daemon binds `0` and writes the
    /// chosen port to the `port` file. `CM_PORT` env var still wins over this.
    #[serde(default)]
    pub port: Option<u16>,
    /// Days to keep `turns` / `snapshots` / `subagent_tasks` rows. Absent or
    /// `0` disables retention.
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// If non-empty, only jsonl paths matching one of these globs are tailed.
    #[serde(default)]
    pub include_globs: Vec<String>,
    /// Paths matching any of these globs are skipped even if `include_globs`
    /// would have matched them.
    #[serde(default)]
    pub exclude_globs: Vec<String>,
}

pub fn config_file() -> PathBuf {
    paths::app_data_dir().join("config.toml")
}

/// Read and parse the config file. Returns `Config::default()` on a missing
/// file. Parse errors bubble up so the daemon can log + refuse to silently
/// ignore typos.
pub fn load() -> Result<Config> {
    load_from(&config_file())
}

pub fn load_from(path: &Path) -> Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(toml::from_str(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e.into()),
    }
}

/// Compiled include/exclude filter. `matches` returns `true` when a path
/// should be ingested.
#[derive(Debug, Clone)]
pub struct PathFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl PathFilter {
    pub fn from_config(cfg: &Config) -> Result<Self> {
        Ok(Self {
            include: build(&cfg.include_globs)?,
            exclude: build(&cfg.exclude_globs)?,
        })
    }

    pub fn matches(&self, path: &Path) -> bool {
        if let Some(ex) = &self.exclude {
            if ex.is_match(path) {
                return false;
            }
        }
        match &self.include {
            Some(inc) => inc.is_match(path),
            None => true,
        }
    }
}

fn build(globs: &[String]) -> Result<Option<GlobSet>> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for g in globs {
        b.add(Glob::new(g)?);
    }
    Ok(Some(b.build()?))
}
