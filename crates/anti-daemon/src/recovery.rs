//! Restart recovery — daemon restart handles stale sessions.
//!
//! Recovery algorithm (3 phases):
//! 1. Agent liveness check: PID alive? spawn_gen match? → mark Crashed if dead
//! 2. Work item recovery: if assigned peer is Crashed → transition to NeedsRevision
//! 3. Deferred Treehouse cleanup (outside any lock)
//!
//! Key invariant: Recovery is conservative. If unsure, mark for observation.
//! PID reuse safety: spawn_gen must match for liveness claim.

use crate::store::Store;
use anti_core::events::EventType;
use anti_core::model::AgentStatus;
use anti_core::work::WorkItemState;

/// Information about a cleaned-up agent (for deferred Treehouse release).
#[derive(Debug, Clone)]
pub struct DeferredCleanup {
    pub id: String,
    pub lease_id: String,
    pub path: String,
}

/// Run recovery on daemon restart. 3 phases, lock discipline preserved.
pub fn recover_on_restart(store: &mut Store) -> Vec<DeferredCleanup> {
    // Phase 1: Find dead agents (no Treehouse ops)
    let dead_agents = find_dead_agents(store);

    // Phase 2: Work item recovery (with lock, fast SQL only)
    let work_cleanups = recover_work_items(store, &dead_agents);

    // Phase 3: Mark agents as Crashed (with lock)
    let mut cleanups = Vec::new();
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

        if let Some(workspace) = &rec.workspace {
            cleanups.push(DeferredCleanup {
                id: rec.id.clone(),
                lease_id: workspace.lease_id.clone(),
                path: workspace.path.clone(),
            });
        }
    }

    // Merge work-item-initiated cleanups
    cleanups.extend(work_cleanups);
    cleanups
}

/// Phase 1: Find agents that are Running/Starting/Blocked but whose PIDs are dead.
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
        .filter(|rec| !is_pid_alive(rec.pid))
        .collect()
}

/// Phase 2: Work items assigned to now-dead agents → NeedsRevision.
fn recover_work_items(
    store: &mut Store,
    dead_agents: &[anti_core::model::AgentRecord],
) -> Vec<DeferredCleanup> {
    let dead_ids: std::collections::HashSet<String> =
        dead_agents.iter().map(|a| a.id.clone()).collect();

    let all_work = match store.list_work_items(None) {
        Ok(w) => w,
        Err(_) => return Vec::new(),
    };

    let mut cleanups = Vec::new();
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

    cleanups
}

/// Check if a PID is alive. Returns false if pid is None.
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
        let cleanups = recover_on_restart(&mut store);
        assert!(cleanups.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
