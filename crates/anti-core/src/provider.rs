//! Provider abstraction layer — unified traits for all agent runtimes.
//!
//! Every provider (Claude, Codex, OpenCode) implements AgentClient (factory)
//! and AgentSession (conversation). The control plane interacts only through
//! these traits — never directly with provider-specific protocols.
//!
//! Architecture:
//!   AgentClient::create_session() → Box<dyn AgentSession>
//!   AgentSession::drain_events() → Vec<AgentEvent>
//!   AgentEvent flows through normalized pipeline

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─── Provider Identity ────────────────────────────────────────────────────────

/// Which provider runtime backs this agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Claude,
    Codex,
    OpenCode,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
            ProviderKind::OpenCode => "opencode",
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ─── Capability Flags ─────────────────────────────────────────────────────────

/// What a provider runtime supports — discovered at runtime, not assumed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Structured event streaming (NDJSON / SSE / JSON-RPC).
    pub streaming: bool,
    /// Session resume via handle (claude --resume, codex thread/resume).
    pub resume: bool,
    /// Mid-turn interrupt (SIGINT, SDK interrupt).
    pub interrupt: bool,
    /// Bidirectional permission (canUseTool callback, approval requests).
    pub permission: bool,
    /// Thinking/reasoning blocks in event stream.
    pub reasoning: bool,
    /// Provider may spawn its own sub-agents (Task tool, child sessions).
    pub native_subagents: bool,
    /// Follow-up to existing session without re-spawning.
    pub followup: bool,
    /// Dynamic thinking level configuration.
    pub thinking_config: bool,
}

impl ProviderCapabilities {
    /// No capabilities (unknown or missing binary).
    pub fn none() -> Self {
        Self::default()
    }

    /// Claude capabilities (probed from binary).
    pub fn claude() -> Self {
        Self {
            streaming: true,
            resume: true,
            interrupt: true,
            permission: true,
            reasoning: true,
            native_subagents: true,
            followup: false, // stream-json doesn't support follow-up
            thinking_config: true,
        }
    }

    /// Codex capabilities (app-server mode).
    pub fn codex() -> Self {
        Self {
            streaming: true,
            resume: true,
            interrupt: true,
            permission: true,
            reasoning: true,
            native_subagents: true,
            followup: true,
            thinking_config: false,
        }
    }

    /// OpenCode capabilities (server mode).
    pub fn opencode() -> Self {
        Self {
            streaming: true,
            resume: true,
            interrupt: true,
            permission: true,
            reasoning: true,
            native_subagents: true,
            followup: true,
            thinking_config: false,
        }
    }
}

// ─── Persistence ──────────────────────────────────────────────────────────────

/// Opaque handle for resuming a session after process death.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceHandle {
    pub provider: ProviderKind,
    pub session_id: String,
    /// Provider-specific resume data (JSON-serialized).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_handle: Option<serde_json::Value>,
    /// Metadata for the control plane (cwd, model, config).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ─── Session Configuration ────────────────────────────────────────────────────

/// Configuration for creating a new agent session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Working directory for the agent process.
    pub cwd: PathBuf,
    /// The task prompt (fed via stdin or first message).
    pub task: String,
    /// System prompt override (peer prompt, role instructions).
    pub system_prompt: Option<String>,
    /// Model override (provider-specific).
    pub model: Option<String>,
    /// MCP servers to inject.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Permission mode (acceptEdits, bypassPermissions, etc.).
    pub permission_mode: Option<String>,
    /// Whether to persist the session for later resume.
    pub persist: bool,
}

/// MCP server configuration for injection into a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(flatten)]
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpTransport {
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    #[serde(rename = "sse")]
    Sse { url: String },
}

impl McpTransport {
    pub fn type_(&self) -> &'static str {
        match self {
            McpTransport::Stdio { .. } => "stdio",
            McpTransport::Sse { .. } => "sse",
        }
    }
}

// ─── AgentClient Trait (Factory) ──────────────────────────────────────────────

/// Factory for creating agent sessions. Each provider implements this.
///
/// The control plane calls `create_session()` to get a live `AgentSession`.
/// The factory handles binary resolution, authentication checks, and
/// capability probing.
pub trait AgentClient: Send + Sync {
    /// Which provider this client creates sessions for.
    fn kind(&self) -> ProviderKind;

    /// Runtime capabilities of this provider.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Create a new session with the given configuration.
    fn create_session(
        &self,
        config: &SessionConfig,
    ) -> Result<Box<dyn AgentSession>, ProviderError>;

    /// Resume a previously persisted session.
    fn resume_session(
        &self,
        handle: &PersistenceHandle,
    ) -> Result<Box<dyn AgentSession>, ProviderError>;

    /// Check if this provider is available (binary exists, auth valid).
    fn is_available(&self) -> bool;
}

// ─── AgentSession Trait (Conversation) ────────────────────────────────────────

/// A running agent session — abstracts Claude/Codex/OpenCode runtimes.
///
/// The session owns its process, event stream, and lifecycle.
/// Callers interact through this trait, not the underlying protocol.
pub trait AgentSession: Send {
    /// Which provider backs this session.
    fn provider(&self) -> ProviderKind;

    /// Unique session identifier.
    fn session_id(&self) -> &str;

    /// Runtime capabilities for this session.
    fn capabilities(&self) -> &ProviderCapabilities;

    /// Drain pending events (non-blocking).
    fn drain_events(&mut self) -> Vec<crate::events::AgentEvent>;

    /// Send a follow-up message to the running session.
    /// Returns error if provider doesn't support follow-up.
    fn send(&mut self, input: &str) -> Result<(), ProviderError>;

    /// Interrupt the current turn.
    fn interrupt(&mut self) -> Result<(), ProviderError>;

    /// Close the session (release process, keep identity).
    fn close(&mut self) -> Result<(), ProviderError>;

    /// Get persistence handle for resume.
    fn persistence_handle(&self) -> Option<PersistenceHandle>;
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider not available: {0}")]
    NotAvailable(String),
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("follow-up not supported")]
    FollowUpNotSupported,
    #[error("interrupt not supported")]
    InterruptNotSupported,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ─── Tool Name Normalization ──────────────────────────────────────────────────

/// Normalize provider-specific tool names to a common set.
pub fn normalize_tool_name(provider: ProviderKind, raw_name: &str) -> &'static str {
    match provider {
        ProviderKind::Claude => normalize_claude_tool(raw_name),
        ProviderKind::Codex => normalize_codex_tool(raw_name),
        ProviderKind::OpenCode => normalize_opencode_tool(raw_name),
    }
}

fn normalize_claude_tool(name: &str) -> &'static str {
    match name {
        "Bash" | "bash" | "shell" | "exec_command" => "Shell",
        "Read" | "read" | "read_file" | "view_file" => "Read",
        "Write" | "write" | "write_file" | "create_file" => "Write",
        "Edit" | "MultiEdit" | "multi_edit" | "apply_patch" => "Edit",
        "WebSearch" | "web_search" | "Grep" | "grep" | "Glob" | "glob" => "Search",
        "WebFetch" | "web_fetch" | "WebFetchTool" => "Fetch",
        "Task" | "Agent" => "SubAgent",
        _ => {
            if name.contains("mcp") || name.contains("MCP") {
                "Mcp"
            } else {
                "Unknown"
            }
        }
    }
}

fn normalize_codex_tool(name: &str) -> &'static str {
    match name {
        "commandExecution" | "shell" | "bash" => "Shell",
        "read" | "read_file" => "Read",
        "write" | "write_file" => "Write",
        "fileChange" | "apply_patch" | "edit" => "Edit",
        "webSearch" | "search" | "grep" | "glob" => "Search",
        "webFetch" | "fetch" => "Fetch",
        "subAgentActivity" | "collabAgentToolCall" => "SubAgent",
        "mcpToolCall" => "Mcp",
        _ => "Unknown",
    }
}

fn normalize_opencode_tool(name: &str) -> &'static str {
    match name {
        "shell" | "bash" | "exec_command" => "Shell",
        "read" | "read_file" => "Read",
        "write" | "write_file" | "create_file" => "Write",
        "edit" | "apply_patch" | "apply_diff" => "Edit",
        "grep" | "search" | "glob" => "Search",
        "fetch" | "web_fetch" => "Fetch",
        "task" | "subtask" => "SubAgent",
        "skill" => "Mcp", // skills map to MCP-like tools
        _ => "Unknown",
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_display() {
        assert_eq!(ProviderKind::Claude.to_string(), "claude");
        assert_eq!(ProviderKind::Codex.to_string(), "codex");
        assert_eq!(ProviderKind::OpenCode.to_string(), "opencode");
    }

    #[test]
    fn provider_kind_as_str() {
        assert_eq!(ProviderKind::Claude.as_str(), "claude");
        assert_eq!(ProviderKind::Codex.as_str(), "codex");
        assert_eq!(ProviderKind::OpenCode.as_str(), "opencode");
    }

    #[test]
    fn capabilities_none_defaults() {
        let caps = ProviderCapabilities::none();
        assert!(!caps.streaming);
        assert!(!caps.resume);
        assert!(!caps.interrupt);
        assert!(!caps.permission);
        assert!(!caps.reasoning);
        assert!(!caps.native_subagents);
        assert!(!caps.followup);
        assert!(!caps.thinking_config);
    }

    #[test]
    fn capabilities_claude() {
        let caps = ProviderCapabilities::claude();
        assert!(caps.streaming);
        assert!(caps.resume);
        assert!(caps.interrupt);
        assert!(caps.permission);
        assert!(caps.reasoning);
        assert!(caps.native_subagents);
        assert!(!caps.followup); // stream-json doesn't support follow-up
        assert!(caps.thinking_config);
    }

    #[test]
    fn capabilities_codex() {
        let caps = ProviderCapabilities::codex();
        assert!(caps.streaming);
        assert!(caps.followup); // app-server supports follow-up
    }

    #[test]
    fn capabilities_opencode() {
        let caps = ProviderCapabilities::opencode();
        assert!(caps.streaming);
        assert!(caps.followup);
    }

    #[test]
    fn normalize_claude_tools() {
        assert_eq!(normalize_tool_name(ProviderKind::Claude, "Bash"), "Shell");
        assert_eq!(normalize_tool_name(ProviderKind::Claude, "Read"), "Read");
        assert_eq!(normalize_tool_name(ProviderKind::Claude, "Write"), "Write");
        assert_eq!(normalize_tool_name(ProviderKind::Claude, "Edit"), "Edit");
        assert_eq!(
            normalize_tool_name(ProviderKind::Claude, "Task"),
            "SubAgent"
        );
        assert_eq!(
            normalize_tool_name(ProviderKind::Claude, "WebSearch"),
            "Search"
        );
        assert_eq!(
            normalize_tool_name(ProviderKind::Claude, "WebFetch"),
            "Fetch"
        );
    }

    #[test]
    fn normalize_codex_tools() {
        assert_eq!(
            normalize_tool_name(ProviderKind::Codex, "commandExecution"),
            "Shell"
        );
        assert_eq!(
            normalize_tool_name(ProviderKind::Codex, "fileChange"),
            "Edit"
        );
        assert_eq!(
            normalize_tool_name(ProviderKind::Codex, "subAgentActivity"),
            "SubAgent"
        );
    }

    #[test]
    fn normalize_opencode_tools() {
        assert_eq!(
            normalize_tool_name(ProviderKind::OpenCode, "shell"),
            "Shell"
        );
        assert_eq!(
            normalize_tool_name(ProviderKind::OpenCode, "task"),
            "SubAgent"
        );
    }

    #[test]
    fn unknown_tools_return_unknown() {
        assert_eq!(
            normalize_tool_name(ProviderKind::Claude, "some_new_tool"),
            "Unknown"
        );
    }

    #[test]
    fn persistence_handle_roundtrip() {
        let handle = PersistenceHandle {
            provider: ProviderKind::Claude,
            session_id: "test-123".into(),
            native_handle: Some(serde_json::json!({"key": "value"})),
            metadata: None,
        };
        let json = serde_json::to_string(&handle).unwrap();
        let back: PersistenceHandle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, ProviderKind::Claude);
        assert_eq!(back.session_id, "test-123");
    }

    #[test]
    fn session_config_builder() {
        let config = SessionConfig {
            cwd: PathBuf::from("/tmp/workspace"),
            task: "fix the bug".into(),
            system_prompt: Some("You are a peer.".into()),
            model: None,
            mcp_servers: vec![],
            permission_mode: Some("acceptEdits".into()),
            persist: true,
        };
        assert_eq!(config.cwd, PathBuf::from("/tmp/workspace"));
        assert!(config.persist);
    }

    #[test]
    fn mcp_server_config_roundtrip() {
        let config = McpServerConfig {
            name: "my-server".into(),
            transport: McpTransport::Stdio {
                command: "node".into(),
                args: vec!["server.js".into()],
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "my-server");
        assert_eq!(back.transport.type_(), "stdio");
    }
}
