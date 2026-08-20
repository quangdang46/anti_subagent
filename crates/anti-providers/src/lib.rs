//! Provider adapters — concrete implementations of AgentClient/AgentSession.
//!
//! Each provider (Claude, Codex, OpenCode) has its own adapter that:
//! - Spawns the provider process
//! - Reads provider-specific output (NDJSON, JSON-RPC, SSE)
//! - Normalizes events to AgentEvent
//! - Manages session lifecycle

pub mod claude;
pub mod codex;
pub mod opencode;

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;
pub use opencode::OpenCodeAdapter;
