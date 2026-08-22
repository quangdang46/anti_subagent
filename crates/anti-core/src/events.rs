//! Persisted append-only event log (plan §20).
//!
//! Differs from herdr deliberately: events are persisted to JSONL and the
//! sequence survives daemon restarts. Each event is a single
//! write-then-sync so a crash can only truncate a partial tail, never
//! reorder or lose acknowledged events.
//!
//! This module contains two event systems:
//! 1. `EventType` / `Event` — lifecycle events persisted to JSONL
//! 2. `AgentEvent` — normalized provider events flowing through the pipeline

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ─── Lifecycle Events (persisted to JSONL) ────────────────────────────────────

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
    PermissionRequested,
    PermissionResolved,
    GuardViolated,
    /// B3: provider stream event with no lifecycle meaning (audit-only).
    ProviderEvent,
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
    pub fn new(
        seq: u64,
        agent_id: impl Into<String>,
        type_: EventType,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            seq,
            timestamp: Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            type_,
            payload,
        }
    }
}

// ─── Normalized Provider Events (flow through pipeline) ───────────────────────

/// Tool call lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

/// Normalized tool detail — provider-specific tool info mapped to common types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ToolDetail {
    Shell {
        command: String,
    },
    Read {
        file_path: String,
    },
    Write {
        file_path: String,
    },
    Edit {
        file_path: String,
    },
    Search {
        query: String,
    },
    Fetch {
        url: String,
    },
    Mcp {
        server: String,
        tool: String,
    },
    SubAgent {
        description: String,
        child_id: Option<String>,
    },
    Plan {
        content: String,
    },
    Unknown,
}

/// Token/cost usage reported on turn completion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub total_cost_usd: Option<f64>,
}

/// Permission request from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

/// Normalized agent event — flows through the entire pipeline.
///
/// Every provider (Claude, Codex, OpenCode) normalizes its output to these
/// events. The control plane, event bridge, and wait engine consume only
/// this enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    // ─── Content ───
    /// Assistant text delta (streaming chunk).
    AssistantDelta { text: String, message_id: String },
    /// Complete assistant message.
    AssistantMessage { text: String, message_id: String },
    /// System message (info, warnings).
    SystemMessage { text: String },

    // ─── Tool Calls ───
    /// Tool call started.
    ToolCallStart {
        call_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    /// Tool call status update.
    ToolCallUpdate {
        call_id: String,
        status: ToolStatus,
        detail: Option<serde_json::Value>,
    },
    /// Tool call completed.
    ToolCallComplete {
        call_id: String,
        output: Option<String>,
    },
    /// Tool call failed.
    ToolCallFailed { call_id: String, error: String },

    // ─── Turn Lifecycle ───
    /// Turn started.
    TurnStarted { turn_id: Option<String> },
    /// Turn completed successfully.
    TurnCompleted { usage: Option<Usage> },
    /// Turn failed.
    TurnFailed { error: String },
    /// Turn was interrupted/canceled.
    TurnCanceled { reason: String },

    // ─── Subagents ───
    /// Native subagent started (Task tool, child session).
    SubagentStarted { id: String, title: Option<String> },
    /// Native subagent progress update.
    SubagentProgress {
        id: String,
        timeline_item: Box<AgentEvent>,
    },
    /// Native subagent completed.
    SubagentCompleted { id: String, status: SubagentStatus },
    /// Issue #5: a delegation-shaped tool call was detected in a peer
    /// session — the guard denied it and the control plane must see it.
    GuardViolation {
        tool_name: String,
        call_id: Option<String>,
    },

    // ─── Permissions ───
    /// Permission request from provider.
    PermissionRequested { request: PermissionRequest },
    /// Permission resolved (allow/deny).
    PermissionResolved { request_id: String, allowed: bool },

    // ─── System ───
    /// Usage updated (token count, cost).
    UsageUpdated { usage: Usage },
    /// Context compaction occurred.
    Compaction { status: CompactionStatus },
    /// Generic error.
    Error { message: String },
}

/// Subagent completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Completed,
    Failed,
    Canceled,
}

/// Compaction status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    Started,
    Completed,
    Failed,
}
