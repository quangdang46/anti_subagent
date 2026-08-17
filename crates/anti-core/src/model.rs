//! Durable identity model — the anti_subagent core invariant.
//!
//! Plan §16: identity is persisted BEFORE spawn (firstmate lesson). A peer
//! that crashes and restarts keeps the same id; replacement is an explicit
//! governance decision, never an implicit respawn.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Supervisor,
    Lead,
    Peer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    Engineer,
    Architect,
    Reviewer,
    Scout,
    ProofAuditor,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    OpenCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceLease {
    pub lease_id: String,
    pub path: String,
    pub holder: String,
    /// Generation fence (irina pattern): mỗi lần lease được cấp lại/đổi chủ,
    /// generation tăng. Writer phải mang đúng generation hiện tại; stale → fence.
    pub generation: u64,
}

impl WorkspaceLease {
    pub fn generation_matches(&self, expected: u64) -> bool {
        self.generation == expected
    }
}

/// Fence error — stale generation or wrong holder.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum FenceError {
    #[error("stale generation: expected {expected}, writer holds {actual}")]
    StaleGeneration { expected: u64, actual: u64 },
    #[error("lease not held by {holder}")]
    NotHolder { holder: String },
}

/// Lifecycle states (plan §17). Transitions are enforced by optimistic-lock
/// UPDATE: `UPDATE agents SET status=? WHERE id=? AND status=<expected>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentStatus {
    Created,
    Starting,
    Running,
    Blocked,
    Completed,
    Failed,
    Crashed,
    Stopped,
    Recovering,
    Replaced,
}

impl AgentStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            AgentStatus::Completed
                | AgentStatus::Failed
                | AgentStatus::Stopped
                | AgentStatus::Replaced
        )
    }
}

/// The durable AgentRecord (plan §16).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub role: Role,
    pub disposition: Option<Disposition>,
    pub harness: Harness,
    pub parent_id: Option<String>,
    pub pid: Option<u32>,
    pub workspace: Option<WorkspaceLease>,
    pub task_path: Option<String>,
    pub status: AgentStatus,
    pub restart_count: u32,
    pub spawn_gen: u32,
    pub last_state_change_seq: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states() {
        assert!(AgentStatus::Completed.is_terminal());
        assert!(AgentStatus::Failed.is_terminal());
        assert!(!AgentStatus::Running.is_terminal());
    }

    #[test]
    fn stale_generation_is_fenced() {
        let lease = WorkspaceLease {
            lease_id: "L1".into(),
            path: "/tmp/ws-1".into(),
            holder: "peer-1".into(),
            generation: 1,
        };
        assert!(lease.generation_matches(1));
        assert!(!lease.generation_matches(2)); // stale — bị fence
    }

    #[test]
    fn fence_error_carries_audit_info() {
        let e = FenceError::StaleGeneration { expected: 2, actual: 1 };
        assert!(e.to_string().contains("stale"));
    }
}
