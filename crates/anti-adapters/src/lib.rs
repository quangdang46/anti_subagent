//! Harness adapters (plan §25): spawn/stop/status for each coding CLI.
//! P0-P4: Claude Code. P5: Codex. OpenCode later.

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

/// Claude Code adapter: `claude -p --output-format json --permission-mode
/// acceptEdits --dangerously-skip-permissions --append-system-prompt <peer>`
/// with the task fed via stdin.
pub struct ClaudeCodeAdapter;

impl HarnessAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        let mut cmd = Command::new("claude");
        cmd.args([
            "-p",
            "--output-format",
            "json",
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

/// Codex adapter (plan §25): `codex exec --json --skip-git-repo-check
/// [--resume <id>]` with the task as the first argument.
pub struct CodexAdapter;

impl HarnessAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        let mut cmd = Command::new("codex");
        cmd.args(["exec", "--json", "--skip-git-repo-check"]);
        if let Some(task) = &ctx.task {
            cmd.arg(task);
        }
        cmd.current_dir(&ctx.worktree);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        Ok(cmd)
    }
}
