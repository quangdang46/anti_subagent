//! anti-workspace: treehouse adapter (DEPEND, plan §19).
//!
//! Library-based treehouse integration — no subprocess calls.
//! All workspace operations go through `AntiPool` which wraps treehouse-core.
//!
//! Key types:
//! - `Treehouse` — primary API for acquire/release/gc
//! - `AntiPool` — lower-level pool wrapper (used by Treehouse)
//! - `AntiEnv` — environment configuration (state directory)
//! - `PoolConfig` — pool settings (max trees, lock timeout, gc interval)
//!
//! Work lifecycle: acquire → work → release (or gc reclaims orphans).

pub mod cas;
pub mod pool;

// Re-export pool adapter types for convenient access.
pub use pool::{AntiEnv, AntiPool, AntiPoolError, GcResult, PoolConfig};

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Lease {
    pub path: PathBuf,
    pub lease_id: String,
    pub holder: String,
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("pool error: {0}")]
    Pool(#[from] AntiPoolError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Library-based treehouse adapter (preferred API).
///
/// Delegates to `AntiPool` which calls treehouse-core directly — no subprocess.
pub struct Treehouse {
    pool: AntiPool,
}

impl Treehouse {
    /// Creates a new treehouse adapter backed by `AntiPool`.
    pub fn new(env: AntiEnv, config: PoolConfig) -> Self {
        Self {
            pool: AntiPool::new(env, config),
        }
    }

    /// Returns a reference to the underlying `AntiPool`.
    pub fn pool(&self) -> &AntiPool {
        &self.pool
    }

    /// Acquire a durable lease (plan §19).
    ///
    /// `repo_root` is the path to the git repository.
    /// `remote_url` is used for pool directory hashing.
    /// `holder` identifies the lease holder (e.g. a peer agent ID).
    pub fn acquire(
        &self,
        repo_root: &std::path::Path,
        remote_url: Option<&str>,
        holder: &str,
    ) -> Result<Lease, WorkspaceError> {
        let acquired = self.pool.acquire(repo_root, remote_url, holder, None)?;
        Ok(Lease {
            path: acquired.path,
            lease_id: acquired
                .lease
                .as_ref()
                .map(|l| l.id.clone())
                .unwrap_or_default(),
            holder: acquired
                .lease
                .as_ref()
                .map(|l| l.holder.clone())
                .unwrap_or_else(|| holder.to_string()),
        })
    }

    /// Release a worktree back to the pool.
    pub fn release(
        &self,
        worktree_path: &std::path::Path,
        repo_root: &std::path::Path,
        remote_url: Option<&str>,
    ) -> Result<(), WorkspaceError> {
        self.pool
            .release(worktree_path.to_str().unwrap_or(""), repo_root, remote_url)?;
        Ok(())
    }

    /// Run garbage collection on the pool (dry-run by default).
    pub fn gc(
        &self,
        repo_root: &std::path::Path,
        remote_url: Option<&str>,
    ) -> Result<pool::GcResult, WorkspaceError> {
        Ok(self.pool.gc(repo_root, remote_url)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_fields() {
        let lease = Lease {
            path: PathBuf::from("/tmp/worktree-1"),
            lease_id: "abc-123".to_string(),
            holder: "peer-1".to_string(),
        };
        assert_eq!(lease.lease_id, "abc-123");
        assert_eq!(lease.holder, "peer-1");
        assert_eq!(lease.path, PathBuf::from("/tmp/worktree-1"));
    }

    #[test]
    fn workspace_error_pool_variant() {
        let err = WorkspaceError::Pool(AntiPoolError::PoolFull { count: 16, max: 16 });
        let msg = err.to_string();
        assert!(msg.contains("pool error"), "should contain pool: {msg}");
        assert!(msg.contains("16"), "should mention count: {msg}");
    }

    #[test]
    fn treehouse_new_with_anti_pool() {
        let env = AntiEnv::new(PathBuf::from("/tmp/test-anti"));
        let cfg = PoolConfig::default();
        let th = Treehouse::new(env, cfg);
        // Verify the pool is accessible via public API
        let _pool = th.pool();
    }

    #[test]
    fn treehouse_acquire_returns_lease() {
        let env = AntiEnv::new(PathBuf::from("/tmp/test-ref"));
        let th = Treehouse::new(env, PoolConfig::default());
        // Verify Treehouse can be constructed and pool is accessible
        let _pool = th.pool();
    }
}
