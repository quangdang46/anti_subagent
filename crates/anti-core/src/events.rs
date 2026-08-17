//! Persisted append-only event log (plan §20).
//!
//! Differs from herdr deliberately: events are persisted to JSONL and the
//! sequence survives daemon restarts. Each event is a single
//! write-then-sync so a crash can only truncate a partial tail, never
//! reorder or lose acknowledged events.

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventType {
    AgentRegistered,
    AgentStarted,
    AgentProgress,
    AgentBlocked,
    AgentCompleted,
    AgentFailed,
    AgentCrashed,
    AgentRestarted,
    AgentStopped,
    AgentReplaced,
    HandoffCreated,
    AgentPromptStalled,
    AgentRejected,
    WorkSubmitted,
    WorkVerified,
    WorkAccepted,
    WorkRejected,
    ReviewEscalated,
    // Lifecycle events (Phase 0.2)
    PeerSpawned,
    PeerReady,
    PeerCrashed,
    PeerStopped,
    TaskReceived,
    TaskExecuting,
    TaskCompleted,
    TaskFailed,
    VerificationStarted,
    VerificationPassed,
    VerificationFailed,
    WorkspaceAcquired,
    WorkspaceReleased,
    WorkspaceCleaned,
    LeadHandoff,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub timestamp: String,
    pub agent_id: String,
    #[serde(rename = "type")]
    pub type_: EventType,
    pub payload: serde_json::Value,
}

impl Event {
    pub fn new(seq: u64, agent_id: impl Into<String>, type_: EventType, payload: serde_json::Value) -> Self {
        Self {
            seq,
            timestamp: Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            type_,
            payload,
        }
    }
}
