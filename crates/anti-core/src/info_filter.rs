//! Information Boundary Filter — construction boundary for hierarchy isolation.
//!
//! The spawned agent must NOT be able to infer that it is a child/subagent.
//! This is NOT a text sanitizer — it is a type-level boundary:
//!
//!   InternalAgentState → [INFO FILTER] → SessionConfig
//!
//! InternalAgentState fields (parent_id, supervisor_id, role, etc.) are
//! NEVER referenced when building SessionConfig. The dangerous fields
//! simply do not exist in provider-facing types.

use crate::agent::{AgentConfig, AgentId};
use crate::provider::{PersistenceHandle, ProviderKind, SessionConfig};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ─── Internal State (NEVER serialized to provider) ────────────────────────────

/// Internal agent state — the control plane's view.
/// These fields are NEVER passed to SessionConfig, env vars, or CLI args.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalAgentState {
    pub agent_id: AgentId,
    pub parent_id: Option<AgentId>,
    pub supervisor_id: Option<AgentId>,
    pub role: crate::model::Role,
    pub disposition: Option<crate::model::Disposition>,
    pub governance_state: Option<String>,
    pub handoff_context: Option<String>,
    pub spawn_reason: Option<String>,
}

// ─── Agent Context (what gets serialized) ─────────────────────────────────────

/// Agent context — ONLY the fields the provider needs to see.
/// No hierarchy, no governance, no orchestration metadata.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub task: String,
    pub workspace: PathBuf,
    pub peer_prompt: Option<String>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
}

// ─── InfoFilter Functions ─────────────────────────────────────────────────────

/// Sanitize workspace path to prevent hierarchy inference.
/// /worktrees/lead-peer-engineer-login → /worktrees/w-{hash}
pub fn sanitize_workspace_path(raw: &Path) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let raw_str = raw.to_string_lossy();

    // Check if path contains hierarchy-related keywords
    let has_hierarchy_keywords = raw_str.contains("lead")
        || raw_str.contains("peer")
        || raw_str.contains("supervisor")
        || raw_str.contains("engineer")
        || raw_str.contains("reviewer")
        || raw_str.contains("architect")
        || raw_str.contains("scout");

    if !has_hierarchy_keywords {
        return raw.to_path_buf(); // No sanitization needed
    }

    // Hash the path and replace with neutral naming
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    let hash = hasher.finish();

    // Find the worktree root (parent of the hierarchy-named dir)
    if let Some(parent) = raw.parent() {
        parent.join(format!("w-{hash:016x}"))
    } else {
        PathBuf::from(format!("/worktrees/w-{hash:016x}"))
    }
}

/// Build SessionConfig from AgentContext only.
/// InternalAgentState fields are NEVER referenced.
pub fn build_session_config(context: &AgentContext, config: &AgentConfig) -> SessionConfig {
    SessionConfig {
        cwd: context.workspace.clone(),
        task: context.task.clone(),
        system_prompt: context.peer_prompt.clone(),
        model: context.model.clone(),
        mcp_servers: config.mcp_servers.clone(),
        permission_mode: config.permission_mode.clone(),
        persist: true,
    }
}

/// Build PersistenceHandle — strips hierarchy fields.
/// Only provider, session_id, and native_handle are preserved.
pub fn build_persistence_handle(
    provider: ProviderKind,
    session_id: &str,
    native_handle: Option<serde_json::Value>,
) -> PersistenceHandle {
    PersistenceHandle {
        provider,
        session_id: session_id.to_string(),
        native_handle,
        metadata: None, // deliberately empty — no cwd/model leakage
    }
}

/// Build AgentContext from SpawnRequest fields ONLY.
/// This is the construction boundary: internal metadata is not included.
pub fn build_agent_context(
    task: &str,
    workspace: &Path,
    peer_prompt: Option<&str>,
    model: Option<&str>,
) -> AgentContext {
    AgentContext {
        task: task.to_string(),
        workspace: sanitize_workspace_path(workspace),
        peer_prompt: peer_prompt.map(|p| p.to_string()),
        model: model.map(|m| m.to_string()),
        thinking_level: None,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Disposition, Role};

    #[test]
    fn sanitize_path_with_hierarchy_keywords() {
        let raw = Path::new("/worktrees/lead-peer-engineer-login");
        let sanitized = sanitize_workspace_path(raw);
        let s = sanitized.to_string_lossy();
        assert!(s.contains("w-"));
        assert!(!s.contains("lead"));
        assert!(!s.contains("peer"));
        assert!(!s.contains("engineer"));
    }

    #[test]
    fn sanitize_path_without_keywords_unchanged() {
        let raw = Path::new("/worktrees/feature-xyz");
        let sanitized = sanitize_workspace_path(raw);
        assert_eq!(sanitized, raw);
    }

    #[test]
    fn build_session_config_no_hierarchy() {
        let context = AgentContext {
            task: "fix the bug".into(),
            workspace: PathBuf::from("/tmp/workspace"),
            peer_prompt: Some("You are a peer.".into()),
            model: Some("claude-sonnet".into()),
            thinking_level: None,
        };
        let config = AgentConfig::default();

        let session = build_session_config(&context, &config);

        // SessionConfig has NO parent/supervisor/role fields — only:
        // cwd, task, system_prompt, model, mcp_servers, permission_mode, persist
        // This is a type-level guarantee, not a runtime check.
        assert_eq!(session.task, "fix the bug");
        assert_eq!(session.cwd, PathBuf::from("/tmp/workspace"));
        assert_eq!(session.system_prompt.as_deref(), Some("You are a peer."));
        assert_eq!(session.model.as_deref(), Some("claude-sonnet"));
        assert!(session.persist);
    }

    #[test]
    fn build_persistence_handle_strips_hierarchy() {
        let handle = build_persistence_handle(
            ProviderKind::Claude,
            "session-123",
            Some(serde_json::json!({"key": "value"})),
        );

        // PersistenceHandle has: provider, session_id, native_handle, metadata
        // NO parent_id, supervisor_id, role, etc.
        assert_eq!(handle.provider, ProviderKind::Claude);
        assert_eq!(handle.session_id, "session-123");
        assert!(handle.native_handle.is_some());
        assert!(handle.metadata.is_none()); // deliberately empty
    }

    #[test]
    fn build_agent_context_neutral() {
        let context = build_agent_context(
            "implement feature X",
            Path::new("/worktrees/lead-peer-engineer-feature"),
            Some("You are working with the project owner."),
            None,
        );

        // Workspace should be sanitized
        assert!(context.workspace.to_string_lossy().contains("w-"));
        assert!(!context.workspace.to_string_lossy().contains("lead"));
        assert!(!context.workspace.to_string_lossy().contains("peer"));

        // Task and prompt should be preserved
        assert_eq!(context.task, "implement feature X");
        assert!(context.peer_prompt.is_some());
    }

    #[test]
    fn internal_state_not_in_session_config() {
        let internal = InternalAgentState {
            agent_id: AgentId::new(),
            parent_id: Some(AgentId::new()),
            supervisor_id: Some(AgentId::new()),
            role: Role::Lead,
            disposition: Some(Disposition::Engineer),
            governance_state: Some("active".into()),
            handoff_context: None,
            spawn_reason: Some("delegation".into()),
        };

        let context = AgentContext {
            task: "do work".into(),
            workspace: PathBuf::from("/tmp"),
            peer_prompt: None,
            model: None,
            thinking_level: None,
        };

        let config = AgentConfig::default();
        let session = build_session_config(&context, &config);

        // TYPE-LEVEL BOUNDARY: SessionConfig has these fields:
        //   cwd, task, system_prompt, model, mcp_servers, permission_mode, persist
        //
        // InternalAgentState has these fields:
        //   agent_id, parent_id, supervisor_id, role, disposition, governance_state,
        //   handoff_context, spawn_reason
        //
        // The two structs share ZERO fields. This is a compile-time guarantee,
        // not a runtime string check. The test verifies the construction works.
        assert_eq!(session.task, "do work");
        assert_eq!(session.cwd, PathBuf::from("/tmp"));

        // Internal state fields exist only in InternalAgentState
        assert!(internal.parent_id.is_some());
        assert!(internal.supervisor_id.is_some());
        assert_eq!(internal.role, Role::Lead);
    }
}
