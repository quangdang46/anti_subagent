//! anti-workspace: treehouse adapter (DEPEND, plan §19).
//!
//! treehouse is a plain CLI subprocess — we never vendor it. Protocol:
//!   acquire:  treehouse get --lease --lease-holder <id> --json
//!   release:  treehouse return --force --if-lease-id <lease_id>
//!   verify:   treehouse status --json
//! Idempotency: `--if-lease-id` is single-shot; on precondition failure we
//! verify via `status --json` and treat an absent lease as already-released.

pub mod cas;
pub mod pool;

// Re-export pool adapter types for convenient access.
pub use pool::{AntiEnv, AntiPool, AntiPoolError, PoolConfig};

use std::path::PathBuf;
use std::process::Command;
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
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Treehouse {
    bin: PathBuf,
}

impl Treehouse {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    fn run(
        &self,
        args: &[&str],
        cwd: &std::path::Path,
    ) -> Result<std::process::Output, WorkspaceError> {
        let out = Command::new(&self.bin)
            .args(args)
            .current_dir(cwd)
            .output()?;
        Ok(out)
    }

    /// Acquire a durable lease (plan §19). Uses `--json` for the lease identity.
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

    /// Release a lease idempotently (plan §19). A precondition failure is
    /// treated as already-released after checking `status`.
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
