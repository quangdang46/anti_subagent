//! Codex provider adapter — spawns `codex app-server` with JSON-RPC 2.0.
//!
//! Stub implementation: command construction works, but JSON-RPC session
//! management is not yet wired. Full implementation requires:
//! - `codex app-server` lifecycle (thread/turn/item model)
//! - JSON-RPC 2.0 transport over stdio
//! - Reasoning via `item/reasoning/summaryTextDelta`
//! - Child agents via `subAgentActivity` items

use anti_core::events::AgentEvent;
use anti_core::provider::{
    AgentClient, AgentSession, PersistenceHandle, ProviderCapabilities, ProviderError,
    ProviderKind, SessionConfig,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Codex provider client (factory).
pub struct CodexAdapter;

impl AgentClient for CodexAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::codex()
    }

    fn create_session(
        &self,
        config: &SessionConfig,
    ) -> Result<Box<dyn AgentSession>, ProviderError> {
        let session = CodexSession::spawn(config)?;
        Ok(Box::new(session))
    }

    fn resume_session(
        &self,
        handle: &PersistenceHandle,
    ) -> Result<Box<dyn AgentSession>, ProviderError> {
        let session = CodexSession::resume(handle)?;
        Ok(Box::new(session))
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("codex")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }
}

/// Codex running session (stub).
pub struct CodexSession {
    session_id: String,
    rx: Receiver<AgentEvent>,
    caps: ProviderCapabilities,
}

impl CodexSession {
    pub fn spawn(config: &SessionConfig) -> Result<Self, ProviderError> {
        // Build command: codex exec --json --skip-git-repo-check
        let _ = config; // TODO: wire actual spawn
        let (tx, rx) = mpsc::channel();
        // Stub: immediately close channel
        drop(tx);
        Ok(Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            rx,
            caps: ProviderCapabilities::codex(),
        })
    }

    pub fn resume(handle: &PersistenceHandle) -> Result<Self, ProviderError> {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        Ok(Self {
            session_id: handle.session_id.clone(),
            rx,
            caps: ProviderCapabilities::codex(),
        })
    }
}

impl AgentSession for CodexSession {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.caps
    }

    fn drain_events(&mut self) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        events
    }

    fn send(&mut self, input: &str) -> Result<(), ProviderError> {
        let _ = input;
        Err(ProviderError::FollowUpNotSupported) // TODO: implement
    }

    fn interrupt(&mut self) -> Result<(), ProviderError> {
        Err(ProviderError::InterruptNotSupported) // TODO: implement
    }

    fn close(&mut self) -> Result<(), ProviderError> {
        Ok(())
    }

    fn persistence_handle(&self) -> Option<PersistenceHandle> {
        Some(PersistenceHandle {
            provider: ProviderKind::Codex,
            session_id: self.session_id.clone(),
            native_handle: None,
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_adapter_kind() {
        let adapter = CodexAdapter;
        assert_eq!(adapter.kind(), ProviderKind::Codex);
    }

    #[test]
    fn codex_capabilities() {
        let adapter = CodexAdapter;
        let caps = adapter.capabilities();
        assert!(caps.streaming);
        assert!(caps.followup);
        assert!(caps.native_subagents);
    }
}
