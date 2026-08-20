//! Agent identity model — the core invariant.
//!
//! Agent identity is decoupled from process/session. An AgentId survives
//! process death; the session can be resumed by UUID; the agent can be
//! transferred to a different provider without losing identity.
//!
//! Architecture:
//!   AgentId (persistent) ≠ SessionId (runtime) ≠ Pid (ephemeral)

use crate::model::{AgentStatus, Disposition, Role};
use crate::provider::{PersistenceHandle, ProviderKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─── Identity Types ───────────────────────────────────────────────────────────

/// Unique agent identifier — survives process death, session resume, provider transfer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for AgentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for AgentId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Workspace identifier — groups agents working in the same codebase.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    pub fn new(path: &std::path::Path) -> Self {
        // Hash the path for a stable, short ID
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        Self(format!("ws-{:016x}", hasher.finish()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ─── Agent Configuration ──────────────────────────────────────────────────────

/// Agent-specific configuration (model, prompt, MCP servers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub system_prompt: Option<String>,
    pub permission_mode: Option<String>,
    pub mcp_servers: Vec<crate::provider::McpServerConfig>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: None,
            thinking_level: None,
            system_prompt: None,
            permission_mode: Some("acceptEdits".into()),
            mcp_servers: vec![],
        }
    }
}

// ─── Agent Record ─────────────────────────────────────────────────────────────

/// The durable agent record — persisted to JSON, survives process death.
///
/// This is the source of truth for agent state. The control plane reads
/// and writes this record; the runtime session is ephemeral metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    /// Unique agent identity — permanent.
    pub agent_id: AgentId,

    /// Parent agent (SLP hierarchy — INTERNAL ONLY, never exposed to agent).
    pub parent_id: Option<AgentId>,

    /// Agent role in the SLP hierarchy.
    pub role: Role,

    /// Agent disposition (Engineer, Architect, Reviewer, etc.).
    pub disposition: Option<Disposition>,

    /// Which provider backs this agent.
    pub provider: ProviderKind,

    /// Workspace this agent operates in.
    pub workspace_id: WorkspaceId,

    /// Current lifecycle status.
    pub status: AgentStatus,

    /// Runtime session handle (for resume).
    pub persistence_handle: Option<PersistenceHandle>,

    /// Agent-specific configuration.
    pub config: AgentConfig,

    /// Process ID (ephemeral — None when process is dead).
    pub pid: Option<u32>,

    /// When the agent was created.
    pub created_at: DateTime<Utc>,

    /// Last activity timestamp.
    pub last_activity_at: DateTime<Utc>,

    /// When the agent was archived (soft-delete).
    pub archived_at: Option<DateTime<Utc>>,

    /// Restart count (for crash-loop detection).
    pub restart_count: u32,
}

impl AgentRecord {
    /// Create a new agent record.
    pub fn new(
        role: Role,
        provider: ProviderKind,
        workspace_id: WorkspaceId,
        config: AgentConfig,
    ) -> Self {
        let now = Utc::now();
        Self {
            agent_id: AgentId::new(),
            parent_id: None,
            role,
            disposition: None,
            provider,
            workspace_id,
            status: AgentStatus::Created,
            persistence_handle: None,
            config,
            pid: None,
            created_at: now,
            last_activity_at: now,
            archived_at: None,
            restart_count: 0,
        }
    }

    /// Transition to a new status (validates state machine).
    pub fn transition(&mut self, new_status: AgentStatus) -> Result<(), StatusError> {
        if !self.status.can_transition_to(new_status) {
            return Err(StatusError::InvalidTransition {
                from: self.status,
                to: new_status,
            });
        }
        self.status = new_status;
        self.last_activity_at = Utc::now();
        Ok(())
    }

    /// Mark as archived (soft-delete).
    pub fn archive(&mut self) {
        self.status = AgentStatus::Replaced; // Replaced = archived in lifecycle
        self.archived_at = Some(Utc::now());
        self.last_activity_at = Utc::now();
    }

    /// Check if agent is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// Get the agent's working directory (from persistence metadata).
    pub fn cwd(&self) -> Option<PathBuf> {
        self.persistence_handle
            .as_ref()
            .and_then(|h| h.metadata.as_ref())
            .and_then(|m| m.get("cwd"))
            .and_then(|c| c.as_str())
            .map(PathBuf::from)
    }
}

// ─── Status State Machine ─────────────────────────────────────────────────────

/// Status transition error.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StatusError {
    #[error("invalid status transition: {from:?} → {to:?}")]
    InvalidTransition { from: AgentStatus, to: AgentStatus },
}

impl AgentStatus {
    /// Check if a transition is valid.
    pub fn can_transition_to(self, target: AgentStatus) -> bool {
        use AgentStatus::*;
        matches!(
            (self, target),
            // Normal lifecycle
            (Created, Starting)
                | (Starting, Running)
                // Error/retry
                | (Running, Failed)
                | (Failed, Starting)
                // Process release (Completed = process done successfully)
                | (Running, Completed)
                // Crash/recovery
                | (Running, Crashed)
                | (Crashed, Recovering)
                | (Recovering, Running)
                // Stop
                | (Running, Stopped)
                // Replace
                | (Running, Replaced)
        )
    }
}

// ─── Spawn Request ────────────────────────────────────────────────────────────

/// Request to spawn a new agent.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub role: Role,
    pub disposition: Option<Disposition>,
    pub provider: ProviderKind,
    pub workspace_id: WorkspaceId,
    pub config: AgentConfig,
    pub parent_id: Option<AgentId>,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_roundtrip() {
        let id = AgentId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: AgentId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn agent_id_display() {
        let id = AgentId("test-123".into());
        assert_eq!(format!("{id}"), "test-123");
    }

    #[test]
    fn workspace_id_from_path() {
        let ws = WorkspaceId::new(std::path::Path::new("/tmp/workspace"));
        assert!(ws.as_str().starts_with("ws-"));
    }

    #[test]
    fn agent_record_new() {
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));
        let record = AgentRecord::new(Role::Peer, ProviderKind::Claude, ws, AgentConfig::default());
        assert_eq!(record.status, AgentStatus::Created);
        assert!(record.parent_id.is_none());
        assert!(record.archived_at.is_none());
    }

    #[test]
    fn agent_record_transition_valid() {
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));
        let mut record =
            AgentRecord::new(Role::Peer, ProviderKind::Claude, ws, AgentConfig::default());
        assert!(record.transition(AgentStatus::Starting).is_ok());
        assert!(record.transition(AgentStatus::Running).is_ok());
    }

    #[test]
    fn agent_record_transition_invalid() {
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));
        let mut record =
            AgentRecord::new(Role::Peer, ProviderKind::Claude, ws, AgentConfig::default());
        // Created -> Running is invalid
        assert!(record.transition(AgentStatus::Running).is_err());
    }

    #[test]
    fn agent_record_archive() {
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));
        let mut record =
            AgentRecord::new(Role::Peer, ProviderKind::Claude, ws, AgentConfig::default());
        record.archive();
        assert_eq!(record.status, AgentStatus::Replaced);
        assert!(record.archived_at.is_some());
    }

    #[test]
    fn agent_record_serialization_roundtrip() {
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));
        let record = AgentRecord::new(Role::Peer, ProviderKind::Claude, ws, AgentConfig::default());
        let json = serde_json::to_string(&record).unwrap();
        let back: AgentRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record.agent_id, back.agent_id);
        assert_eq!(record.status, back.status);
    }

    #[test]
    fn status_transition_table() {
        // Valid transitions
        assert!(AgentStatus::Created.can_transition_to(AgentStatus::Starting));
        assert!(AgentStatus::Starting.can_transition_to(AgentStatus::Running));
        assert!(AgentStatus::Running.can_transition_to(AgentStatus::Completed));
        assert!(AgentStatus::Running.can_transition_to(AgentStatus::Failed));
        assert!(AgentStatus::Failed.can_transition_to(AgentStatus::Starting));

        // Invalid transitions
        assert!(!AgentStatus::Created.can_transition_to(AgentStatus::Running));
        assert!(!AgentStatus::Completed.can_transition_to(AgentStatus::Running));
    }
}
