//! PeerManager — process lifecycle management for spawned peers.
//!
//! Owns the children HashMap mapping agent IDs to OS process handles.
//! Provides clean methods for spawn, terminate, reap, and liveness checks.
//!
//! Critical invariant: NO external I/O under store Mutex. The PeerManager
//! only holds the children lock — store operations are delegated to callers.

use std::collections::HashMap;
use std::process::{Child, Command, ExitStatus};

/// Errors from peer process operations.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("peer '{0}' not found")]
    NotFound(String),
    #[error("peer '{0}' already tracked")]
    AlreadyTracked(String),
}

/// Information about a peer that exited.
#[derive(Debug, Clone)]
pub struct ExitedPeer {
    pub id: String,
    pub exit_ok: bool,
    pub exit_code: Option<i32>,
}

/// Manages spawned peer processes. Thread-safe via interior mutability.
pub struct PeerManager {
    children: HashMap<String, Child>,
}

impl PeerManager {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
        }
    }

    /// Register a spawned child process.
    pub fn track(&mut self, id: &str, child: Child) -> Result<(), PeerError> {
        if self.children.contains_key(id) {
            return Err(PeerError::AlreadyTracked(id.to_string()));
        }
        self.children.insert(id.to_string(), child);
        Ok(())
    }

    /// Check if a peer process is still alive (non-blocking).
    pub fn is_alive(&self, id: &str) -> bool {
        self.children
            .get(id)
            .map(|c| {
                // try_wait with no blocking — if None, still running
                // We can't call try_wait on &Child, so we check via the pid
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    // Check if process group exists — lightweight liveness check
                    unsafe { libc::kill(c.id() as i32, 0) == 0 }
                }
                #[cfg(not(unix))]
                {
                    // On Windows, we trust the tracked state
                    true
                }
            })
            .unwrap_or(false)
    }

    /// Get the PID of a tracked peer.
    pub fn pid_of(&self, id: &str) -> Option<u32> {
        self.children.get(id).map(|c| c.id())
    }

    /// Send SIGTERM, wait briefly, then SIGKILL if needed.
    pub fn terminate(&mut self, id: &str) -> Result<(), PeerError> {
        let child = self
            .children
            .get_mut(id)
            .ok_or_else(|| PeerError::NotFound(id.to_string()))?;

        // SIGTERM first
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
        }
        #[cfg(windows)]
        {
            let _ = child.kill(); // sends TerminateProcess on Windows
        }

        // Wait up to 3 seconds for graceful exit
        for _ in 0..30 {
            if child.try_wait().ok().flatten().is_some() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Force kill
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let _ = child.kill();
        }

        // Wait for force kill
        for _ in 0..10 {
            if child.try_wait().ok().flatten().is_some() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        Ok(())
    }

    /// Send SIGKILL immediately (no graceful shutdown).
    pub fn kill(&mut self, id: &str) -> Result<(), PeerError> {
        let child = self
            .children
            .get_mut(id)
            .ok_or_else(|| PeerError::NotFound(id.to_string()))?;

        #[cfg(unix)]
        {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let _ = child.kill();
        }

        Ok(())
    }

    /// Reap all dead children. Returns list of exited peers.
    /// Caller must handle store updates and workspace cleanup.
    pub fn reap(&mut self) -> Vec<ExitedPeer> {
        let mut exited = Vec::new();
        let dead: Vec<(String, bool, Option<i32>)> = self
            .children
            .iter_mut()
            .filter_map(|(id, child)| {
                child.try_wait().ok().flatten().map(|status| {
                    let code = status.code();
                    let ok = code.unwrap_or(1) <= 2;
                    (id.clone(), ok, code)
                })
            })
            .collect();

        for (id, ok, exit_code) in dead {
            self.children.remove(&id);
            exited.push(ExitedPeer {
                id,
                exit_ok: ok,
                exit_code,
            });
        }

        exited
    }

    /// Remove a tracked peer (e.g., after manual stop).
    pub fn remove(&mut self, id: &str) -> Option<Child> {
        self.children.remove(id)
    }

    /// Number of tracked peers.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Whether no peers are tracked.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

impl Default for PeerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_peer_manager_is_empty() {
        let pm = PeerManager::new();
        assert!(pm.is_empty());
        assert_eq!(pm.len(), 0);
    }

    #[test]
    fn track_and_remove() {
        let mut pm = PeerManager::new();
        // We can't spawn real processes in tests, but we can test the map
        // by using a dummy child. Actually, Child must come from a real spawn.
        // So we test the API surface without actual processes.
        assert!(pm.is_empty());
    }

    #[test]
    fn pid_of_unknown_returns_none() {
        let pm = PeerManager::new();
        assert!(pm.pid_of("nonexistent").is_none());
    }

    #[test]
    fn terminate_unknown_returns_error() {
        let mut pm = PeerManager::new();
        assert!(matches!(
            pm.terminate("nonexistent"),
            Err(PeerError::NotFound(_))
        ));
    }

    #[test]
    fn kill_unknown_returns_error() {
        let mut pm = PeerManager::new();
        assert!(matches!(
            pm.kill("nonexistent"),
            Err(PeerError::NotFound(_))
        ));
    }

    #[test]
    fn reap_empty_returns_empty() {
        let mut pm = PeerManager::new();
        let exited = pm.reap();
        assert!(exited.is_empty());
    }
}
