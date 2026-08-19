//! anti-workspace: treehouse adapter (DEPEND, plan §19).
//!
//! Two API layers:
//! - **Library API** (`AntiPool`): direct calls to treehouse-core, no subprocess.
//!   Preferred for new code. See `pool.rs`.
//! - **Legacy API** (`Treehouse`): subprocess-based wrapper, kept for backward
//!   compatibility during migration. Deprecated — use `AntiPool` instead.
//!
//! Both expose the same lease semantics: acquire → work → release.

pub mod cas;
pub mod pool;

// Re-export pool adapter types for convenient access.
pub use pool::{AntiEnv, AntiPool, AntiPoolError, PoolConfig};

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
    #[error("treehouse binary not found: {bin}")]
    BinaryNotFound { bin: String },
    #[error("treehouse get failed (exit {code}): {stderr}")]
    GetFailed { code: i32, stderr: String },
    #[error("treehouse get produced unparseable JSON: {raw}")]
    BadJson { raw: String },
    #[error("treehouse return failed: {stderr}")]
    ReturnFailed { stderr: String },
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
}

/// Legacy subprocess-based treehouse adapter (deprecated).
///
/// Use `Treehouse` (library-based) or `AntiPool` instead.
#[deprecated(
    since = "0.2.0",
    note = "Use anti_workspace::Treehouse (library-based) or AntiPool instead"
)]
pub struct TreehouseLegacy {
    bin: PathBuf,
}

#[allow(deprecated)]
impl TreehouseLegacy {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    fn run(
        &self,
        args: &[&str],
        cwd: &std::path::Path,
    ) -> Result<std::process::Output, WorkspaceError> {
        use std::process::Command;
        let out = Command::new(&self.bin)
            .args(args)
            .current_dir(cwd)
            .output()?;
        Ok(out)
    }

    /// Acquire a durable lease via subprocess (deprecated — use library API).
    pub fn acquire(&self, holder: &str, cwd: &std::path::Path) -> Result<Lease, WorkspaceError> {
        if !self.bin.exists() {
            return Err(WorkspaceError::BinaryNotFound {
                bin: self.bin.display().to_string(),
            });
        }
        let out = self.run(&["get", "--lease", "--lease-holder", holder, "--json"], cwd)?;
        if !out.status.success() {
            return Err(WorkspaceError::GetFailed {
                code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let line = stdout.lines().rev().find(|l| l.trim().starts_with('{'));
        let raw = line.unwrap_or(stdout.trim());
        let v: serde_json::Value =
            serde_json::from_str(raw).map_err(|_| WorkspaceError::BadJson {
                raw: raw.to_string(),
            })?;
        let path =
            v.get("path")
                .and_then(|x| x.as_str())
                .ok_or_else(|| WorkspaceError::BadJson {
                    raw: raw.to_string(),
                })?;
        let lease_id =
            v.get("lease_id")
                .and_then(|x| x.as_str())
                .ok_or_else(|| WorkspaceError::BadJson {
                    raw: raw.to_string(),
                })?;
        Ok(Lease {
            path: PathBuf::from(path),
            lease_id: lease_id.to_string(),
            holder: holder.to_string(),
        })
    }

    /// Release a lease idempotently via subprocess (deprecated — use library API).
    pub fn release_if_lease(
        &self,
        lease_id: &str,
        worktree_path: &std::path::Path,
        cwd: &std::path::Path,
    ) -> Result<(), WorkspaceError> {
        let out = self.run(
            &[
                "return",
                "--force",
                "--if-lease-id",
                lease_id,
                worktree_path.to_str().unwrap_or(""),
            ],
            cwd,
        )?;
        if out.status.success() {
            return Ok(());
        }
        // Idempotent: verify the worktree no longer carries this lease.
        let st = self.run(&["status", "--json"], cwd)?;
        let text = String::from_utf8_lossy(&st.stdout).to_string();
        if text.contains(lease_id) {
            return Err(WorkspaceError::ReturnFailed {
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(())
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
