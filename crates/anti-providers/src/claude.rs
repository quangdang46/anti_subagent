//! Claude provider adapter — spawns `claude -p` with stream-json, reads NDJSON,
//! normalizes to AgentEvent.

use anti_core::events::AgentEvent;
use anti_core::provider::{
    AgentClient, AgentSession, PersistenceHandle, ProviderCapabilities, ProviderError,
    ProviderKind, SessionConfig,
};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// Convert anti-adapters AgentEvent to anti-core AgentEvent.
fn convert_event(evt: anti_adapters::AgentEvent) -> Option<AgentEvent> {
    use anti_adapters::AgentEvent as A;
    match evt {
        A::AssistantDelta { text, message_id } => {
            Some(AgentEvent::AssistantDelta { text, message_id })
        }
        A::AssistantMessage { text, message_id } => {
            Some(AgentEvent::AssistantMessage { text, message_id })
        }
        A::SystemMessage { text } => Some(AgentEvent::SystemMessage { text }),
        A::ToolCallStart {
            call_id,
            tool_name,
            input,
        } => Some(AgentEvent::ToolCallStart {
            call_id,
            tool_name,
            input,
        }),
        A::ToolCallUpdate {
            call_id,
            status,
            detail,
        } => {
            use anti_core::events::ToolStatus;
            let status = match status {
                anti_adapters::ToolCallStatus::Running => ToolStatus::Running,
                anti_adapters::ToolCallStatus::Completed => ToolStatus::Completed,
                anti_adapters::ToolCallStatus::Failed => ToolStatus::Failed,
                anti_adapters::ToolCallStatus::Canceled => ToolStatus::Canceled,
            };
            Some(AgentEvent::ToolCallUpdate {
                call_id,
                status,
                detail,
            })
        }
        A::ToolCallComplete { call_id, output } => {
            Some(AgentEvent::ToolCallComplete { call_id, output })
        }
        A::ToolCallFailed { call_id, error } => Some(AgentEvent::ToolCallFailed { call_id, error }),
        A::TurnCompleted { usage } => {
            let usage = usage.map(|u| anti_core::events::Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cached_input_tokens: None,
                total_cost_usd: u.total_cost_usd,
            });
            Some(AgentEvent::TurnCompleted { usage })
        }
        A::TurnFailed { error } => Some(AgentEvent::TurnFailed { error }),
        A::PermissionRequested {
            request_id,
            tool_name,
            input,
        } => Some(AgentEvent::PermissionRequested {
            request: anti_core::events::PermissionRequest {
                id: request_id,
                tool_name,
                input,
            },
        }),
        A::PermissionResolved { request_id } => Some(AgentEvent::PermissionResolved {
            request_id,
            allowed: true,
        }),
    }
}

/// Claude provider client (factory).
pub struct ClaudeAdapter;

impl AgentClient for ClaudeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::claude()
    }

    fn create_session(
        &self,
        config: &SessionConfig,
    ) -> Result<Box<dyn AgentSession>, ProviderError> {
        let session = ClaudeSession::spawn(config)?;
        Ok(Box::new(session))
    }

    fn resume_session(
        &self,
        handle: &PersistenceHandle,
    ) -> Result<Box<dyn AgentSession>, ProviderError> {
        // Resume by re-spawning with --resume <session_id>
        let session = ClaudeSession::resume(handle)?;
        Ok(Box::new(session))
    }

    fn is_available(&self) -> bool {
        // Check if `claude` binary exists on PATH
        std::process::Command::new("claude")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }
}

/// Claude running session.
pub struct ClaudeSession {
    session_id: String,
    child: Option<Child>,
    rx: Receiver<AgentEvent>,
    caps: ProviderCapabilities,
}

impl ClaudeSession {
    /// Spawn a new Claude session.
    pub fn spawn(config: &SessionConfig) -> Result<Self, ProviderError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut cmd = Command::new("claude");
        cmd.arg("-p")
            .args([
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
            ])
            .args([
                "--permission-mode",
                config.permission_mode.as_deref().unwrap_or("acceptEdits"),
                "--dangerously-skip-permissions",
            ]);

        if let Some(ref prompt) = config.system_prompt {
            cmd.args(["--append-system-prompt", prompt]);
        }

        cmd.args(["--session-id", &session_id])
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .map_err(|e| ProviderError::SpawnFailed(e.to_string()))?;

        // Feed task via stdin
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(config.task.as_bytes());
            drop(stdin);
        }

        // Wire up NDJSON reader
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::SpawnFailed("no stdout pipe".into()))?;
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        if let Some(evt) = anti_adapters::parse_claude_stream_line(&line) {
                            if let Some(converted) = convert_event(evt) {
                                if tx.send(converted).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            session_id,
            child: Some(child),
            rx,
            caps: ProviderCapabilities::claude(),
        })
    }

    /// Resume an existing session.
    pub fn resume(handle: &PersistenceHandle) -> Result<Self, ProviderError> {
        // For resume, we need the cwd from metadata
        let cwd = handle
            .metadata
            .as_ref()
            .and_then(|m| m.get("cwd"))
            .and_then(|c| c.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        let mut cmd = Command::new("claude");
        cmd.arg("-p")
            .args([
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
            ])
            .args([
                "--permission-mode",
                "acceptEdits",
                "--dangerously-skip-permissions",
            ])
            .args(["--resume", &handle.session_id])
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .map_err(|e| ProviderError::SpawnFailed(e.to_string()))?;

        // For resume, send empty prompt to continue
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(b"");
            drop(stdin);
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::SpawnFailed("no stdout pipe".into()))?;
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        if let Some(evt) = anti_adapters::parse_claude_stream_line(&line) {
                            if let Some(converted) = convert_event(evt) {
                                if tx.send(converted).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            session_id: handle.session_id.clone(),
            child: Some(child),
            rx,
            caps: ProviderCapabilities::claude(),
        })
    }
}

impl AgentSession for ClaudeSession {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Claude
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
        // Stream-json mode doesn't support follow-up after initial task
        let _ = input;
        Err(ProviderError::FollowUpNotSupported)
    }

    fn interrupt(&mut self) -> Result<(), ProviderError> {
        if let Some(ref mut child) = self.child {
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGINT);
                }
                Ok(())
            }
            #[cfg(not(unix))]
            {
                child
                    .kill()
                    .map_err(|e| ProviderError::Protocol(e.to_string()))
            }
        } else {
            Err(ProviderError::Protocol("no active process".into()))
        }
    }

    fn close(&mut self) -> Result<(), ProviderError> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    fn persistence_handle(&self) -> Option<PersistenceHandle> {
        Some(PersistenceHandle {
            provider: ProviderKind::Claude,
            session_id: self.session_id.clone(),
            native_handle: None,
            metadata: None,
        })
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_adapter_kind() {
        let adapter = ClaudeAdapter;
        assert_eq!(adapter.kind(), ProviderKind::Claude);
    }

    #[test]
    fn claude_capabilities() {
        let adapter = ClaudeAdapter;
        let caps = adapter.capabilities();
        assert!(caps.streaming);
        assert!(caps.resume);
        assert!(caps.reasoning);
        assert!(caps.native_subagents);
    }

    #[test]
    fn claude_session_provider() {
        // Just verify the type compiles
        fn _assert_session<T: AgentSession>() {}
        _assert_session::<ClaudeSession>();
    }

    #[test]
    fn claude_session_send() {
        let session = ClaudeSession {
            session_id: "test".into(),
            child: None,
            rx: mpsc::channel().1,
            caps: ProviderCapabilities::claude(),
        };
        // Can't test send without a real process, but verify trait compiles
        let mut session = session;
        let result = session.send("test");
        assert!(result.is_err());
    }
}
