//! Config hierarchy (slb pattern, plan §5): defaults < user < project < env < flags.
//! P0 keeps a minimal set — values are read from `~/.anti_subagent/config.toml`
//! over defaults; env and flags are layered by callers.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Config {
    pub state_dir: PathBuf,
    /// Window in which a send must produce a state change before the peer is
    /// declared stalled (herdr used 5s; plan §21 defaults to 60s because LLM
    /// agents legitimately go silent during long tool calls).
    pub stall_timeout: std::time::Duration,
    pub poll_interval: std::time::Duration,
    pub claude_bin: PathBuf,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let base = PathBuf::from(home).join(".anti_subagent");
        Self {
            state_dir: base,
            stall_timeout: std::time::Duration::from_secs(60),
            poll_interval: std::time::Duration::from_millis(100),
            claude_bin: PathBuf::from("claude"),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let mut cfg = Config::default();
        let file = cfg.state_dir.join("config.toml");
        if file.exists() {
            let raw = std::fs::read_to_string(&file).map_err(|source| ConfigError::Io {
                path: file.clone(),
                source,
            })?;
            if let Ok(t) = toml::from_str::<toml::Value>(&raw) {
                if let Some(v) = t.get("stall_timeout_secs").and_then(|v| v.as_integer()) {
                    cfg.stall_timeout = std::time::Duration::from_secs(v as u64);
                }
                if let Some(v) = t.get("poll_interval_ms").and_then(|v| v.as_integer()) {
                    cfg.poll_interval = std::time::Duration::from_millis(v as u64);
                }
                if let Some(v) = t.get("claude_bin").and_then(|v| v.as_str()) {
                    cfg.claude_bin = PathBuf::from(v);
                }
            }
        }
        Ok(cfg)
    }
}
