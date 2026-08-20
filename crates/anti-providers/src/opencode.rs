//! OpenCode provider adapter — spawns `opencode serve` with HTTP + SSE.
//!
//! Stub implementation: command construction works, but HTTP/SSE session
//! management is not yet wired. Full implementation requires:
//! - `opencode serve --port N` lifecycle
//! - HTTP REST API for commands
//! - SSE event stream for responses
//! - Session/message/part model

use anti_core::events::AgentEvent;
use anti_core::provider::{
    AgentClient, AgentSession, PersistenceHandle, ProviderCapabilities, ProviderError,
    ProviderKind, SessionConfig,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// OpenCode provider client (factory).
pub struct OpenCodeAdapter;

impl AgentClient for OpenCodeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenCode
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::opencode()
    }

    fn create_session(
        &self,
        config: &SessionConfig,
    ) -> Result<Box<dyn AgentSession>, ProviderError> {
        let session = OpenCodeSession::spawn(config)?;
        Ok(Box::new(session))
    }

    fn resume_session(
        &self,
        handle: &PersistenceHandle,
    ) -> Result<Box<dyn AgentSession>, ProviderError> {
        let session = OpenCodeSession::resume(handle)?;
        Ok(Box::new(session))
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("opencode")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }
}

/// OpenCode running session (stub).
pub struct OpenCodeSession {
    session_id: String,
    rx: Receiver<AgentEvent>,
    caps: ProviderCapabilities,
}

impl OpenCodeSession {
    pub fn spawn(config: &SessionConfig) -> Result<Self, ProviderError> {
        let _ = config; // TODO: wire actual spawn
        let (tx, rx) = mpsc::channel();
        drop(tx);
        Ok(Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            rx,
            caps: ProviderCapabilities::opencode(),
        })
    }

    pub fn resume(handle: &PersistenceHandle) -> Result<Self, ProviderError> {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        Ok(Self {
            session_id: handle.session_id.clone(),
            rx,
            caps: ProviderCapabilities::opencode(),
        })
    }
}

impl AgentSession for OpenCodeSession {
    fn provider(&self) -> ProviderKind {
        ProviderKind::OpenCode
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
            provider: ProviderKind::OpenCode,
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
    fn opencode_adapter_kind() {
        let adapter = OpenCodeAdapter;
        assert_eq!(adapter.kind(), ProviderKind::OpenCode);
    }

    #[test]
    fn opencode_capabilities() {
        let adapter = OpenCodeAdapter;
        let caps = adapter.capabilities();
        assert!(caps.streaming);
        assert!(caps.followup);
        assert!(caps.native_subagents);
    }
}
