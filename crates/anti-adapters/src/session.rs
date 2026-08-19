//! AgentSession trait — the per-agent runtime handle.
//!
//! Anti manages session identity (`session_id`) directly; we do NOT reuse
//! Paseo's `AgentPersistenceHandle`. Each session owns its process, stdin,
//! and event stream.

use crate::capabilities::CapabilityFlags;
use crate::events::AgentEvent;

/// Unique session id (UUID, stored on AgentRecord).
pub type SessionId = String;

/// A running agent session — abstracts Claude/Codex/OpenCode runtimes.
pub trait AgentSession: Send {
    fn session_id(&self) -> &str;
    fn capabilities(&self) -> &CapabilityFlags;

    /// Drain pending events (non-blocking).
    fn drain_events(&mut self) -> Vec<AgentEvent>;

    /// Send a follow-up message to the running session.
    /// Returns an error if `capabilities().interrupt` is None and the
    /// session does not support mid-turn input.
    fn send(&mut self, input: &str) -> Result<(), String>;

    /// Interrupt the current turn, if supported.
    fn interrupt(&mut self) -> Result<(), String>;

    /// Kill the session's process.
    fn kill(&mut self);
}

/// Result of spawning a session (holds the live session + initial events if any).
pub struct SpawnResult {
    pub session_id: String,
    pub capabilities: CapabilityFlags,
}
