//! Hybrid wait engine (herdr-adapted, plan §21): event-gated polling —
//! a status snapshot is taken only when the event ring changed or the
//! deadline is near, never in a tight loop.

use crate::store::Store;
use anti_core::events::EventType;
use anti_core::model::AgentStatus;
use std::time::{Duration, Instant};

pub fn wait_for_status(
    store: &Store,
    id: &str,
    until: AgentStatus,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<AgentStatus, String> {
    let deadline = Instant::now() + timeout;
    let start_seq = store.current_sequence();
    let mut last_seq = start_seq;

    loop {
        // Status snapshot — reads are cheap; only re-snapshotted when the
        // event log advanced (or on the first tick).
        let rec = store
            .get_agent(id)
            .map_err(|e| format!("read agent {id}: {e}"))?
            .ok_or_else(|| format!("agent {id} not found"))?;
        if rec.status == until {
            return Ok(rec.status);
        }
        if rec.status.is_terminal() && until != rec.status {
            return Ok(rec.status);
        }

        let now_seq = store.current_sequence();
        if now_seq != last_seq {
            last_seq = now_seq;
            continue; // event advanced — re-snapshot immediately
        }

        if Instant::now() >= deadline {
            return Err(format!("timeout after {timeout:?} waiting for {id} to reach {until:?} (current: {:?})", rec.status));
        }
        std::thread::sleep(poll_interval);
    }
}

/// Snapshot the current status without waiting.
pub fn snapshot(store: &Store, id: &str) -> Result<AgentStatus, String> {
    store
        .get_agent(id)
        .map_err(|e| format!("read agent {id}: {e}"))?
        .map(|r| r.status)
        .ok_or_else(|| format!("agent {id} not found"))
}

/// Returns true if the event type marks a state change worth probing.
pub fn is_state_change(t: &EventType) -> bool {
    matches!(
        t,
        EventType::AgentStarted
            | EventType::AgentBlocked
            | EventType::AgentCompleted
            | EventType::AgentFailed
            | EventType::AgentCrashed
            | EventType::AgentRestarted
            | EventType::AgentStopped
            | EventType::AgentReplaced
    )
}
