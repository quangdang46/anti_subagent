//! Live Claude session — spawns `claude -p` with stream-json, reads NDJSON,
//! parses into AgentEvent. This is the real wiring that session.rs trait
//! defines but didn't have an implementation for.
//!
//! Architecture:
//!   claude stdout (NDJSON) → BufReader line-by-line → parse_claude_stream_line → AgentEvent
//!
//! The session owns the Child process and a background reader thread that
//! feeds parsed events into a bounded channel. Callers use drain_events()
//! to consume them non-blocking.

use crate::capabilities::CapabilityFlags;
use crate::events::{AgentEvent, parse_claude_stream_line};
use crate::session::{AgentSession, SessionId, SpawnResult};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// Live Claude session backed by a real `claude -p` process.
pub struct ClaudeSession {
    session_id: SessionId,
    child: Option<Child>,
    rx: Receiver<AgentEvent>,
    caps: CapabilityFlags,
}

impl ClaudeSession {
    /// Spawn a new Claude session with stream-json piping.
    ///
    /// Returns the session + initial SpawnResult. Events arrive asynchronously
    /// on the internal channel; call drain_events() to consume them.
    pub fn spawn(
        worktree: &std::path::Path,
        task: &str,
        peer_prompt: &str,
    ) -> Result<(Self, SpawnResult), String> {
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
                "acceptEdits",
                "--dangerously-skip-permissions",
                "--append-system-prompt",
                peer_prompt,
            ])
            .args(["--session-id", &session_id])
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

        // Feed the task via stdin then close it.
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(task.as_bytes())
                .map_err(|e| format!("stdin write failed: {e}"))?;
            drop(stdin);
        }

        // Wire up the NDJSON reader thread.
        let stdout = child.stdout.take().ok_or("no stdout pipe")?;
        let (tx, rx) = mpsc::channel::<AgentEvent>();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        if let Some(event) = parse_claude_stream_line(&line) {
                            if tx.send(event).is_err() {
                                break; // receiver dropped
                            }
                        }
                        // Unrecognized lines are silently skipped (protocol extensibility).
                    }
                    Err(_) => break, // stdout closed — process exited
                }
            }
        });

        let caps = CapabilityFlags {
            streaming: true,
            resume: true,
            interrupt: Some(true),
            permission: Some(true),
            reasoning: true,
            native_subagent: true,
        };

        let session = Self {
            session_id,
            child: Some(child),
            rx,
            caps,
        };
        let result = SpawnResult {
            session_id: session.session_id.clone(),
            capabilities: caps,
        };
        Ok((session, result))
    }
}

impl AgentSession for ClaudeSession {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn capabilities(&self) -> &CapabilityFlags {
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

    fn send(&mut self, input: &str) -> Result<(), String> {
        // For stream-json mode, we can't easily send follow-up after initial task.
        // This is a known limitation — full follow-up requires session resume.
        // For now, return an error to signal this needs the resume path.
        let _ = input;
        Err("follow-up not yet supported in stream-json mode; use resume path".into())
    }

    fn interrupt(&mut self) -> Result<(), String> {
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
                child.kill().map_err(|e| format!("interrupt failed: {e}"))
            }
        } else {
            Err("no active process".into())
        }
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_result_has_session_id() {
        // This test verifies the struct layout; actual spawn requires `claude` binary.
        let caps = CapabilityFlags::none();
        let result = SpawnResult {
            session_id: "test-123".into(),
            capabilities: caps,
        };
        assert_eq!(result.session_id, "test-123");
    }

    #[test]
    fn claude_session_trait_compatibility() {
        // Verify ClaudeSession would satisfy AgentSession trait bounds.
        // We can't actually spawn in CI without the binary, but we can
        // check the type implements the trait at compile time.
        fn _assert_send<T: Send>() {}
        fn _assert_session<T: AgentSession>() {}
        _assert_send::<ClaudeSession>();
        _assert_session::<ClaudeSession>();
    }
}
