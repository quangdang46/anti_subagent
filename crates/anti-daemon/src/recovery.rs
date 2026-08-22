//! Unified restart recovery — daemon restart handles stale sessions.
//!
//! Recovery algorithm (4 phases):
//! 1. Treehouse gc: reclaim orphaned worktrees (leases, dead owners, missing paths)
//! 2. Agent liveness: PID alive + start time match → mark Crashed if dead
//! 3. Work item recovery: if assigned peer is Crashed → transition to NeedsRevision
//! 4. Queue cleanup: placeholder for future queue-based scheduling
//!
//! Key invariant: Recovery is conservative. If unsure, mark for observation.
//! PID reuse safety: owner_started_at must match for liveness claim.

use crate::store::Store;
use anti_core::events::EventType;
use anti_core::model::AgentStatus;
use anti_core::work::WorkItemState;
use anti_workspace::Treehouse;

/// Run unified recovery on daemon restart.
///
/// Phase 1 is treehouse gc (automatic, reclaims orphaned worktrees).
/// Phases 2-3 reconcile agents and work items in the store.
pub fn recover_on_restart(store: &mut Store, treehouse: &Treehouse) {
    // Phase 1: GC orphaned worktrees via treehouse.
    // This handles expired leases, dead-owner worktrees, and missing paths.
    // heal_state runs automatically on every pool.lock() acquisition.
    match treehouse.gc(std::path::Path::new("."), None) {
        Ok(result) => {
            if !result.reclaimed.is_empty() {
                eprintln!(
                    "[RECOVERY] GC reclaimed {} worktrees ({} bytes)",
                    result.reclaimed.len(),
                    result.freed_bytes
                );
            }
        }
        Err(e) => {
            eprintln!("[RECOVERY] GC failed (non-fatal): {e}");
        }
    }

    // Phase 2: Find agents that were Running/Starting/Blocked but whose
    // processes are dead (PID not alive or start time mismatch).
    let dead_agents = find_dead_agents(store);

    // Phase 3: Work item recovery (with lock, fast SQL only)
    recover_work_items(store, &dead_agents);

    // Phase 4: Mark dead agents as Crashed and emit events.
    for rec in &dead_agents {
        let _ = store.mark_exit(&rec.id, false);
        let payload = serde_json::json!({
            "exit_code": null,
            "workspace_lease_id": rec.workspace.as_ref().map(|w| &w.lease_id),
            "workspace_path": rec.workspace.as_ref().map(|w| &w.path),
            "crash_evidence": {
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "reason": "daemon_restart_dead_process",
                "spawn_gen": rec.spawn_gen,
            },
        });
        let _ = store.append_event(&rec.id, EventType::PeerCrashed, payload);
        eprintln!(
            "[RECOVERY] Marked dead agent {} (pid {:?}, gen {}) as Crashed",
            rec.id, rec.pid, rec.spawn_gen
        );
    }
}

/// Find agents that appear alive in the store but whose processes are dead.
/// Uses store.is_agent_alive which checks both PID existence and start time.
fn find_dead_agents(store: &Store) -> Vec<anti_core::model::AgentRecord> {
    let agents = match store.list_agents() {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };

    agents
        .into_iter()
        .filter(|rec| {
            matches!(
                rec.status,
                AgentStatus::Running | AgentStatus::Blocked | AgentStatus::Starting
            )
        })
        .filter(|rec| {
            // Dead = liveness check fails. is_agent_alive verifies PID exists
            // AND (when recorded) the process start time still matches, so a
            // reused PID can never masquerade as the original peer.
            !store
                .is_agent_alive(&rec.id)
                .unwrap_or_else(|_| !is_pid_alive(rec.pid))
        })
        .collect()
}

/// Work items assigned to now-dead agents → NeedsRevision (or Failed if max revisions exceeded).
fn recover_work_items(store: &mut Store, dead_agents: &[anti_core::model::AgentRecord]) {
    let dead_ids: std::collections::HashSet<String> =
        dead_agents.iter().map(|a| a.id.clone()).collect();

    let all_work = match store.list_work_items(None) {
        Ok(w) => w,
        Err(_) => return,
    };

    for mut work in all_work {
        if dead_ids.contains(&work.peer_id)
            && matches!(
                work.state,
                WorkItemState::InProgress | WorkItemState::Submitted
            )
        {
            let target = if work.revision >= work.max_revisions {
                WorkItemState::Failed
            } else {
                WorkItemState::NeedsRevision
            };
            let from = work.state;
            if let Err(e) = store.update_work_state(&work.id, from, target) {
                eprintln!("[RECOVERY] Failed to transition work item {}: {e}", work.id);
                continue;
            }
            let _ = store.append_event(
                &work.peer_id,
                EventType::AgentProgress,
                serde_json::json!({
                    "task_id": work.id,
                    "recovery": true,
                    "new_state": format!("{:?}", target),
                    "reason": "peer_crashed_on_restart",
                }),
            );
            eprintln!(
                "[RECOVERY] Work item {} → {:?} (peer {} crashed)",
                work.id, target, work.peer_id
            );
        }
    }
}

/// Check if a PID is alive. Returns false if pid is None.
/// Fallback for when store.is_agent_alive is unavailable.
fn is_pid_alive(pid: Option<u32>) -> bool {
    let pid = match pid {
        Some(p) => p,
        None => return false,
    };

    #[cfg(unix)]
    {
        // kill(pid, 0) — signal 0 checks existence without sending a signal
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On Windows, trust the tracked state (no kill(pid, 0) equivalent)
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_pid_alive_none_returns_false() {
        assert!(!is_pid_alive(None));
    }

    #[test]
    fn is_pid_alive_current_process() {
        // Current process is definitely alive
        let pid = std::process::id();
        assert!(is_pid_alive(Some(pid)));
    }

    #[test]
    fn is_pid_alive_invalid_pid() {
        // PID 1 is init — alive on Unix, may not exist on macOS test env
        // Just verify the function doesn't panic on any input
        let _ = is_pid_alive(Some(u32::MAX));
        let _ = is_pid_alive(Some(1));
        let _ = is_pid_alive(Some(0));
    }

    #[test]
    fn recover_on_restart_handles_empty_store() {
        let dir =
            std::env::temp_dir().join(format!("anti-recovery-test-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();
        let env = anti_workspace::AntiEnv::new(dir.clone());
        let treehouse = Treehouse::new(env, anti_workspace::PoolConfig::default());
        recover_on_restart(&mut store, &treehouse);
        // Should complete without error on empty store
        let _ = std::fs::remove_dir_all(&dir);
    }
}
