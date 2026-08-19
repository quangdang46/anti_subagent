//! Library-based treehouse adapter (DEPEND, plan §19).
//!
//! Wraps `treehouse_core::pool::Pool` to replace the subprocess-based
//! `Treehouse` struct in `lib.rs`. Pool lives inside `~/.anti_subagent/worktrees/`.
//!
//! Usage:
//! ```ignore
//! let env = AntiEnv::new(PathBuf::from("~/.anti_subagent"));
//! let pool = AntiPool::new(env, PoolConfig::default());
//! let lease = pool.acquire(repo_root, "https://github.com/x/y.git", "peer-1", None)?;
//! pool.release(&lease.path, "https://github.com/x/y.git")?;
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use treehouse_core::config::TreehouseConfig;
use treehouse_core::env::TreehouseEnv;
use treehouse_core::gc::GcOptions;
pub use treehouse_core::gc::GcResult;
use treehouse_core::pool::{Acquired, OpenOptions, Pool, PoolError, WorktreeStatus};

// ── Environment ──────────────────────────────────────────────────────

/// Environment configuration for the anti-subagent worktree pool.
///
/// Holds the state directory and per-operation repo root. All filesystem
/// operations delegate to `std::fs`.
#[derive(Debug, Clone)]
pub struct AntiEnv {
    /// Root state directory, e.g. `~/.anti_subagent`.
    pub state_dir: PathBuf,
}

impl AntiEnv {
    /// Creates a new environment rooted at `state_dir`.
    pub fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }

    /// The directory where treehouse pools are stored.
    pub fn pool_root(&self) -> PathBuf {
        self.state_dir.join("worktrees")
    }
}

/// Implement TreehouseEnv so Pool::open_with_env uses our pool_root
/// instead of DefaultEnv's hardcoded $HOME/.treehouse.
impl TreehouseEnv for AntiEnv {
    fn pool_root(&self) -> Option<PathBuf> {
        Some(self.state_dir.join("worktrees"))
    }

    fn update_cache_path(&self) -> Option<PathBuf> {
        Some(self.pool_root().join("update-check.json"))
    }

    fn user_config_path(&self) -> Option<PathBuf> {
        None // no user config needed
    }

    fn read_file(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read_bytes(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write_file(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, data)
    }

    fn ensure_dir(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn list_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)
            .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.path()).collect())
    }

    fn file_meta(&self, path: &Path) -> std::io::Result<treehouse_core::env::FileMeta> {
        let meta = std::fs::metadata(path)?;
        Ok(treehouse_core::env::FileMeta {
            size: meta.len(),
            modified: meta.modified().ok(),
        })
    }

    fn env_var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn env_var_os(&self, name: &str) -> Option<PathBuf> {
        std::env::var_os(name).map(PathBuf::from)
    }

    fn cwd(&self) -> Option<PathBuf> {
        std::env::current_dir().ok()
    }
}

// ── Pool configuration ───────────────────────────────────────────────

/// Configuration for the worktree pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of worktrees per pool (default: 16).
    pub max_trees: u32,
    /// Lock timeout in seconds (default: 10).
    pub lock_timeout_secs: u64,
    /// GC interval in seconds (default: 300). Advisory — the caller decides
    /// when to invoke `gc()`.
    pub gc_interval_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_trees: 16,
            lock_timeout_secs: 10,
            gc_interval_secs: 300,
        }
    }
}

// ── Errors ───────────────────────────────────────────────────────────

/// Errors from the anti-subagent pool adapter.
#[derive(Debug, Error)]
pub enum AntiPoolError {
    #[error("pool: {0}")]
    Pool(#[from] PoolError),
    #[error("pool full ({count} worktrees, max {max})")]
    PoolFull { count: u32, max: u32 },
    #[error("io error: {0}")]
    Io(String, #[source] std::io::Error),
}

// ── AntiPool ─────────────────────────────────────────────────────────

/// Wrapper around `treehouse_core::pool::Pool` for anti-subagent.
///
/// Provides a simplified API for worktree lifecycle: acquire, release, gc,
/// and status. The pool is opened on first use (lazy initialization).
pub struct AntiPool {
    env: AntiEnv,
    config: PoolConfig,
}

impl AntiPool {
    /// Creates a new pool wrapper.
    pub fn new(env: AntiEnv, config: PoolConfig) -> Self {
        Self { env, config }
    }

    /// Opens the underlying treehouse pool for a specific repo.
    /// Uses Pool::open_with_env() with AntiEnv so pool_root() is respected.
    fn open_pool(&self, repo_root: &Path, remote_url: Option<&str>) -> Result<Pool, AntiPoolError> {
        let treehouse_config = TreehouseConfig {
            max_trees: self.config.max_trees,
            root: Some(self.env.pool_root().to_string_lossy().into_owned()),
            ..TreehouseConfig::default_config()
        };
        let opts = OpenOptions {
            config: treehouse_config,
            lock_timeout: Duration::from_secs(self.config.lock_timeout_secs),
        };
        Pool::open_with_env(repo_root, remote_url, &opts, Arc::new(self.env.clone()))
            .map_err(AntiPoolError::Pool)
    }

    /// Acquires a worktree lease.
    ///
    /// `repo_root` is the path to the git repository.
    /// `remote_url` is used for pool directory hashing; `None` falls back to `repo_root`.
    /// `holder` identifies the lease holder (e.g. a peer agent ID).
    /// `ttl` is an optional time-to-live for the lease; `None` = permanent.
    pub fn acquire(
        &self,
        repo_root: &Path,
        remote_url: Option<&str>,
        holder: &str,
        ttl: Option<Duration>,
    ) -> Result<Acquired, AntiPoolError> {
        let pool = self.open_pool(repo_root, remote_url)?;
        let ttl_chrono = ttl.map(|d| {
            chrono::Duration::from_std(d).unwrap_or(chrono::Duration::seconds(d.as_secs() as i64))
        });
        let lease = pool
            .acquire_lease_with_ttl(holder, ttl_chrono)
            .map_err(AntiPoolError::Pool)?;
        Ok(Acquired {
            name: String::new(),
            path: PathBuf::from(&lease.path),
            branch: String::new(),
            lease: Some(treehouse_core::lease::Lease {
                id: lease.lease_id,
                holder: lease.lease_holder,
                acquired_at: lease.leased_at,
                expires_at: ttl_chrono.map(|d| lease.leased_at + d),
            }),
        })
    }

    /// Releases a worktree back to the pool.
    ///
    /// `worktree_path` is the full path to the worktree directory.
    pub fn release(
        &self,
        worktree_path: &str,
        repo_root: &Path,
        remote_url: Option<&str>,
    ) -> Result<(), AntiPoolError> {
        let pool = self.open_pool(repo_root, remote_url)?;
        pool.release(worktree_path).map_err(AntiPoolError::Pool)?;
        Ok(())
    }

    /// Runs garbage collection on the pool (dry-run by default).
    ///
    /// Reclaims stale, orphaned, and dead-owner worktrees.
    pub fn gc(
        &self,
        repo_root: &Path,
        remote_url: Option<&str>,
    ) -> Result<GcResult, AntiPoolError> {
        let pool = self.open_pool(repo_root, remote_url)?;
        let result = pool
            .gc(&GcOptions::default())
            .map_err(AntiPoolError::Pool)?;
        Ok(result)
    }

    /// Reports the status of all managed worktrees.
    pub fn status(
        &self,
        repo_root: &Path,
        remote_url: Option<&str>,
    ) -> Result<Vec<WorktreeStatus>, AntiPoolError> {
        let pool = self.open_pool(repo_root, remote_url)?;
        pool.status().map_err(AntiPoolError::Pool)
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anti_env_pool_root() {
        let env = AntiEnv::new(PathBuf::from("/tmp/test"));
        assert_eq!(env.pool_root(), PathBuf::from("/tmp/test/worktrees"));
    }

    #[test]
    fn anti_env_pool_root_nested() {
        let env = AntiEnv::new(PathBuf::from("/home/user/.anti_subagent"));
        assert_eq!(
            env.pool_root(),
            PathBuf::from("/home/user/.anti_subagent/worktrees")
        );
    }

    #[test]
    fn pool_config_defaults() {
        let cfg = PoolConfig::default();
        assert_eq!(cfg.max_trees, 16);
        assert_eq!(cfg.lock_timeout_secs, 10);
        assert_eq!(cfg.gc_interval_secs, 300);
    }

    #[test]
    fn pool_config_custom() {
        let cfg = PoolConfig {
            max_trees: 32,
            lock_timeout_secs: 30,
            gc_interval_secs: 600,
        };
        assert_eq!(cfg.max_trees, 32);
        assert_eq!(cfg.lock_timeout_secs, 30);
        assert_eq!(cfg.gc_interval_secs, 600);
    }

    #[test]
    fn anti_pool_new() {
        let env = AntiEnv::new(PathBuf::from("/tmp/test"));
        let cfg = PoolConfig::default();
        let pool = AntiPool::new(env, cfg);
        assert_eq!(pool.env.pool_root(), PathBuf::from("/tmp/test/worktrees"));
        assert_eq!(pool.config.max_trees, 16);
    }

    #[test]
    fn anti_pool_error_display() {
        let err = AntiPoolError::PoolFull { count: 16, max: 16 };
        let msg = err.to_string();
        assert!(msg.contains("16"), "should mention count: {msg}");
        assert!(msg.contains("max"), "should mention max: {msg}");
    }

    #[test]
    fn anti_env_clone() {
        let env = AntiEnv::new(PathBuf::from("/tmp/clone"));
        let cloned = env.clone();
        assert_eq!(env.pool_root(), cloned.pool_root());
    }

    #[test]
    fn pool_config_clone() {
        let cfg = PoolConfig {
            max_trees: 8,
            lock_timeout_secs: 5,
            gc_interval_secs: 120,
        };
        let cloned = cfg.clone();
        assert_eq!(cfg.max_trees, cloned.max_trees);
        assert_eq!(cfg.lock_timeout_secs, cloned.lock_timeout_secs);
        assert_eq!(cfg.gc_interval_secs, cloned.gc_interval_secs);
    }
}
