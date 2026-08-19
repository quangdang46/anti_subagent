//! Config hierarchy (slb / Paseo pattern, plan §5, §11): defaults < user < project < env < flags.
//!
//! Layers in precedence order (each overrides the previous):
//! 1. Defaults (compiled-in).
//! 2. User config: `~/.anti_subagent/config.toml`.
//! 3. Project config: `.anti_subagent.toml` in repo root (or `.anti/config.toml`).
//! 4. Env vars: `ANTI_*` prefix (e.g. `ANTI_CLaude_BIN`, `ANTI_STALL_TIMEOUT_SECS`).
//! 5. CLI flags: passed explicitly by callers (layered by call site, not here).

use std::path::PathBuf;
use thiserror::Error;

/// IPC transport selection (plan §13, §33, yrd bead).
///
/// `auto` resolves to Unix socket on Linux/macOS, TCP loopback on Windows.
/// Override via `ipc_transport` in config.toml or `ANTI_IPC_TRANSPORT` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcTransport {
    Unix,
    Tcp,
    #[cfg(windows)]
    NamedPipe,
}

impl IpcTransport {
    /// Auto-select based on platform.
    pub fn auto() -> Self {
        #[cfg(unix)]
        {
            IpcTransport::Unix
        }
        #[cfg(windows)]
        {
            IpcTransport::Tcp
        }
    }

    /// Parse from config string.
    pub fn from_config(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "unix" | "socket" => Ok(IpcTransport::Unix),
            "tcp" | "loopback" => Ok(IpcTransport::Tcp),
            #[cfg(windows)]
            "named_pipe" | "pipe" => Ok(IpcTransport::NamedPipe),
            _ => Err(format!(
                "unknown IPC transport '{s}'. Valid: unix, tcp{}",
                if cfg!(windows) { ", named_pipe" } else { "" }
            )),
        }
    }

    /// Display name for diagnostics (doctor output).
    pub fn name(&self) -> &'static str {
        match self {
            IpcTransport::Unix => "unix_socket",
            IpcTransport::Tcp => "tcp_loopback",
            #[cfg(windows)]
            IpcTransport::NamedPipe => "named_pipe",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub state_dir: PathBuf,
    /// Window in which a send must produce a state change before the peer is
    /// declared stalled (herdr used 5s; plan §21 defaults to 60s because LLM
    /// agents legitimately go silent during long tool calls).
    pub stall_timeout: std::time::Duration,
    pub poll_interval: std::time::Duration,
    pub claude_bin: PathBuf,
    /// IPC transport: auto-selects based on platform unless overridden.
    pub ipc_transport: IpcTransport,
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
            ipc_transport: IpcTransport::auto(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_with_project(None)
    }

    /// Load from all sources, optionally merging a project-local config.
    pub fn load_with_project(project_root: Option<&std::path::Path>) -> Result<Self, ConfigError> {
        let mut cfg = Config::default();
        // User config
        let user_path = cfg.state_dir.join("config.toml");
        Self::apply_file(&mut cfg, &user_path)?;
        // Project config (if a repo root was supplied)
        if let Some(root) = project_root {
            Self::apply_file(&mut cfg, &root.join(".anti_subagent.toml"))?;
            Self::apply_file(&mut cfg, &root.join(".anti/config.toml"))?;
        } else if let Ok(cwd) = std::env::current_dir() {
            Self::apply_file(&mut cfg, &cwd.join(".anti_subagent.toml"))?;
        }
        // Env vars (override files + defaults)
        Self::apply_env(&mut cfg);
        Ok(cfg)
    }

    fn apply_file(cfg: &mut Config, path: &std::path::Path) -> Result<(), ConfigError> {
        if !path.exists() {
            return Ok(());
        }
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        // Parse error is not fatal — warn and keep defaults for that file.
        let t: toml::Value = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "anti: config parse failed for {}: {e} — using defaults for this file",
                    path.display()
                );
                return Ok(());
            }
        };
        Self::merge_table(cfg, &t);
        Ok(())
    }

    fn merge_table(cfg: &mut Config, t: &toml::Value) {
        // Read from top-level first; if a [daemon] section exists, also read from
        // it (last-writer wins within a key).
        for key in [
            "stall_timeout_secs",
            "poll_interval_ms",
            "claude_bin",
            "ipc_transport",
        ] {
            if let Some(v) = t.get(key) {
                apply_key(cfg, key, v);
            }
        }
        if let Some(daemon) = t.get("daemon").and_then(|d| d.as_table()) {
            for key in [
                "stall_timeout_secs",
                "poll_interval_ms",
                "claude_bin",
                "ipc_transport",
            ] {
                if let Some(v) = daemon.get(key) {
                    apply_key(cfg, key, v);
                }
            }
        }
    }

    fn apply_env(cfg: &mut Config) {
        // Direct anti vars (highest-priority env layer after flags)
        if let Ok(v) = std::env::var("ANTI_STALL_TIMEOUT_SECS") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.stall_timeout = std::time::Duration::from_secs(n);
            }
        }
        if let Ok(v) = std::env::var("ANTI_POLL_INTERVAL_MS") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.poll_interval = std::time::Duration::from_millis(n);
            }
        }
        if let Ok(v) = std::env::var("ANTI_CLaude_BIN") {
            cfg.claude_bin = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("ANTI_IPC_TRANSPORT") {
            if let Ok(t) = IpcTransport::from_config(&v) {
                cfg.ipc_transport = t;
            }
        }
        // Legacy / alias: ANTI_CLAUDE_BIN
        if let Ok(v) = std::env::var("ANTI_CLAUDE_BIN") {
            cfg.claude_bin = PathBuf::from(v);
        }
        // Paseo-compat: PASEO_LISTEN maps to ipc_transport hint only when
        // anti var not set (Paseo sets PASEO_LISTEN; we respect it as fallback).
        if std::env::var("ANTI_IPC_TRANSPORT").is_err() {
            if let Ok(v) = std::env::var("PASEO_LISTEN") {
                if let Ok(t) = IpcTransport::from_config(&v) {
                    cfg.ipc_transport = t;
                }
            }
        }
    }
}

fn apply_key(cfg: &mut Config, key: &str, v: &toml::Value) {
    match key {
        "stall_timeout_secs" => {
            if let Some(n) = v.as_integer() {
                cfg.stall_timeout = std::time::Duration::from_secs(n as u64);
            }
        }
        "poll_interval_ms" => {
            if let Some(n) = v.as_integer() {
                cfg.poll_interval = std::time::Duration::from_millis(n as u64);
            }
        }
        "claude_bin" => {
            if let Some(s) = v.as_str() {
                cfg.claude_bin = PathBuf::from(s);
            }
        }
        "ipc_transport" => {
            if let Some(s) = v.as_str() {
                if let Ok(t) = IpcTransport::from_config(s) {
                    cfg.ipc_transport = t;
                }
            }
        }
        _ => {}
    }
}
