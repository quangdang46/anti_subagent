//! Harness adapters (plan §25): spawn/stop/status for each coding CLI.
//! P0-P4: Claude Code. P5: Codex. OpenCode later.
//!
//! CLI-first, SDK-sidecar only on capability gap (h9h): Claude uses
//! `claude -p --input-format stream-json --output-format stream-json` when
//! supported, with NDJSON -> AgentEvent normalization. One-shot json is the
//! compatibility fallback. No Node/Python SDK sidecar in this bead.

pub mod capabilities;
pub mod events;
pub mod session;

pub use capabilities::CapabilityFlags;
pub use events::{AgentEvent, ToolCallStatus, Usage, parse_claude_stream_line, parse_claude_value};
pub use session::{AgentSession, SpawnResult};

use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("binary not found: {0}")]
    BinaryNotFound(String),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A harness adapter — the extension surface for new coding CLIs.
pub trait HarnessAdapter {
    /// Build the spawn command for a peer.
    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError>;
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct SpawnContext {
    /// Leased worktree the peer works in.
    pub worktree: PathBuf,
    /// The peer's task prompt (fed via stdin for -p style CLIs).
    pub task: Option<String>,
    /// Per-arm peer prompt (plan §34: concealment is a benchmark variable).
    pub peer_prompt: Option<String>,
}

/// Claude Code adapter. CLI-first (h9h):
/// - Default: `claude -p --input-format stream-json --output-format stream-json --session-id <uuid>`.
/// - Fallback (binary without stream-json): one-shot `claude -p --output-format json`.
/// - Peer prompt and task are injected via `--append-system-prompt` and stdin.
pub struct ClaudeCodeAdapter;

impl HarnessAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        // Capability probe: if stream-json is supported, use it.
        let caps = capabilities::CapabilityFlags::probe("claude", "claude");
        let use_stream = caps.streaming;
        let mut cmd = Command::new("claude");
        cmd.arg("-p");
        if use_stream {
            cmd.args([
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
            ]);
        } else {
            cmd.args(["--output-format", "json"]);
        }
        cmd.args([
            "--permission-mode",
            "acceptEdits",
            "--dangerously-skip-permissions",
            "--append-system-prompt",
            ctx.peer_prompt
                .as_deref()
                .unwrap_or("You are a peer working on this repository with the project owner. Work independently."),
        ]);
        cmd.current_dir(&ctx.worktree);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        Ok(cmd)
    }
}

/// Codex adapter (real CLI: `codex exec --json --skip-git-repo-check [-C <dir>] [--dangerously-bypass-approvals-and-sandbox]`).
/// Task is passed via arg or stdin; peer_prompt is not natively supported
/// (codex uses system-level AGENTS.md); written as warm-up note if present.
pub struct CodexAdapter;

impl HarnessAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        let mut cmd = Command::new("codex");
        cmd.args(["exec", "--json", "--skip-git-repo-check", "-C"]);
        cmd.arg(ctx.worktree.as_os_str());
        if let Some(task) = &ctx.task {
            cmd.arg(task);
        }
        if let Some(pp) = &ctx.peer_prompt {
            // Prepend peer prompt as part of task (codex has no --append-system-prompt)
            cmd.args(["-c", &format!("peer_prompt={pp}")]);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        Ok(cmd)
    }
}

/// OpenCode adapter (real CLI: `opencode run --format json [-c --session <id>] [<message>...]`).
/// Task is passed as positional args or stdin; peer prompt prepended to message when present.
pub struct OpenCodeAdapter;

impl HarnessAdapter for OpenCodeAdapter {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        let mut cmd = Command::new("opencode");
        cmd.args(["run", "--format", "json"]);
        let mut msg = String::new();
        if let Some(pp) = &ctx.peer_prompt {
            msg.push_str(pp);
            msg.push_str("\n\n");
        }
        if let Some(task) = &ctx.task {
            msg.push_str(task);
        }
        if !msg.is_empty() {
            cmd.arg("--");
            cmd.arg(msg);
        }
        cmd.args(["--dir"]);
        cmd.arg(ctx.worktree.as_os_str());
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        Ok(cmd)
    }
}
