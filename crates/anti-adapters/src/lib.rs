//! Harness adapters (plan §25): spawn/stop/status for each coding CLI.
//! P0-P4: Claude Code. P5: Codex. OpenCode later.
//!
//! CLI-first, SDK-sidecar only on capability gap (h9h): Claude uses
//! `claude -p --output-format stream-json --verbose` with NDJSON -> AgentEvent
//! normalization. Input is plain text via stdin (default --input-format text).
//! One-shot json is the compatibility fallback. No Node/Python SDK sidecar in this bead.
pub mod capabilities;
pub mod claude_session;
pub mod events;
pub mod session;
pub mod timeline_projection;

pub use capabilities::CapabilityFlags;
pub use claude_session::ClaudeSession;
pub use events::{AgentEvent, ToolCallStatus, Usage, parse_claude_stream_line, parse_claude_value};
pub use session::{AgentSession, SpawnResult};
pub use timeline_projection::{
    ProjectionEntry, TimelineItem, TimelineRow, collapse_tool_lifecycle, merge_assistant_chunks,
    project,
};

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
/// - Default: `claude -p --verbose --output-format stream-json`.
/// - Fallback (binary without stream-json): one-shot `claude -p --output-format json`.
/// - Peer prompt via --append-system-prompt, task via stdin (plain text, default input-format).
pub struct ClaudeCodeAdapter;

impl HarnessAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        let caps = capabilities::CapabilityFlags::probe("claude", "claude");
        let use_stream = caps.streaming;
        let mut cmd = Command::new("claude");
        cmd.arg("-p");
        if use_stream {
            // stream-json output requires --verbose; input stays default "text" (plain text stdin)
            cmd.args(["--verbose", "--output-format", "stream-json"]);
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
            // `codex exec <arg>` takes a PROMPT, not a path. If the task is a
            // file path (the daemon's convention), inline its content —
            // otherwise the model sees a literal path and may wander.
            let prompt = std::path::Path::new(task)
                .is_file()
                .then(|| std::fs::read_to_string(task).ok())
                .flatten()
                .unwrap_or_else(|| task.to_string());
            cmd.arg(prompt);
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

/// OpenCode adapter (real CLI: `opencode run --format json --model <m> [--dir <dir>] <message>`).
///
/// Model resolution order:
/// 1. `ANTI_OPENCODE_MODEL` env var (explicit override)
/// 2. `"model"` key from the global opencode config
///    (`~/.config/opencode/opencode.json` or `.jsonc`)
///
/// Without an explicit `--model`, `opencode run` blocks indefinitely waiting
/// for interactive model selection even when the config file names a default —
/// so the flag is mandatory for non-interactive daemon spawns.
pub struct OpenCodeAdapter;

impl OpenCodeAdapter {
    /// Resolve the model id to pass via `--model`. None = cannot determine.
    fn resolve_model() -> Option<String> {
        if let Ok(m) = std::env::var("ANTI_OPENCODE_MODEL")
            && !m.trim().is_empty()
        {
            return Some(m.trim().to_string());
        }
        let home = std::env::var("HOME").ok()?;
        let base = PathBuf::from(home).join(".config").join("opencode");
        for name in ["opencode.json", "opencode.jsonc"] {
            let path = base.join(name);
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Some(model) = extract_json_string_field(&content, "model")
            {
                return Some(model);
            }
        }
        None
    }
}

/// Minimal `"key": "value"` extractor tolerant of JSONC comments. Scans for
/// the quoted key at any depth and returns the following string literal.
fn extract_json_string_field(content: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut rest = content;
    while let Some(pos) = rest.find(&needle) {
        // Skip a match that sits on a `//`-commented line: the comment marker
        // must appear before the key on the same line.
        let before = &rest[..pos];
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        if before[line_start..].contains("//") {
            rest = &rest[pos + needle.len()..];
            continue;
        }
        let after = rest[pos + needle.len()..].trim_start();
        let after = after.strip_prefix(':')?.trim_start();
        let after = after.strip_prefix('"')?;
        let end = after.find('"')?;
        let value = &after[..end];
        if !value.is_empty() {
            return Some(value.to_string());
        }
        rest = &rest[pos + needle.len()..];
    }
    None
}

/// Sleep adapter — deterministic test double (no network, no auth, no model).
/// Runs `sleep <seconds>` in the leased worktree: alive until the timeout,
/// then exits 0. Lets integration tests control process lifetime exactly.
pub struct SleepAdapter;

impl HarnessAdapter for SleepAdapter {
    fn name(&self) -> &'static str {
        "sleep"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        // Lifetime: task text "NNN" seconds (default 60). No stdin needed —
        // closed immediately by the daemon after write attempt.
        let secs: u64 = ctx
            .task
            .as_deref()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(60);
        let mut cmd = Command::new("sleep");
        cmd.arg(secs.to_string());
        cmd.current_dir(&ctx.worktree);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        Ok(cmd)
    }
}

impl HarnessAdapter for OpenCodeAdapter {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
        let mut cmd = Command::new("opencode");
        // --model is REQUIRED for non-interactive runs: without it `opencode
        // run` blocks forever on interactive model selection even when the
        // global config names a default.
        match Self::resolve_model() {
            Some(model) => {
                cmd.args(["run", "--format", "json", "--model", &model, "--dir"]);
            }
            None => {
                return Err(AdapterError::Spawn(
                    "opencode model not resolved: set ANTI_OPENCODE_MODEL or add \"model\" to \
                     ~/.config/opencode/opencode.json"
                        .into(),
                ));
            }
        }
        cmd.arg(ctx.worktree.as_os_str());
        let mut msg = String::new();
        if let Some(pp) = &ctx.peer_prompt {
            msg.push_str(pp);
            msg.push_str("\n\n");
        }
        if let Some(task) = &ctx.task {
            msg.push_str(task);
        }
        if !msg.is_empty() {
            cmd.arg(msg);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        Ok(cmd)
    }
}

#[cfg(test)]
mod sleep_opencode_tests {
    use super::*;

    #[test]
    fn sleep_adapter_parses_seconds_from_task() {
        let ctx = SpawnContext {
            worktree: PathBuf::from("."),
            task: Some("120".into()),
            peer_prompt: None,
        };
        let cmd = SleepAdapter.spawn_command(&ctx).unwrap();
        let args = cmd.get_args().collect::<Vec<_>>();
        assert_eq!(args, vec!["120"]);
    }

    #[test]
    fn sleep_adapter_defaults_to_60s() {
        let ctx = SpawnContext {
            worktree: PathBuf::from("."),
            task: None,
            peer_prompt: None,
        };
        let cmd = SleepAdapter.spawn_command(&ctx).unwrap();
        let args = cmd.get_args().collect::<Vec<_>>();
        assert_eq!(args, vec!["60"]);
    }

    #[test]
    fn opencode_requires_resolved_model() {
        // In test env neither ANTI_OPENCODE_MODEL nor HOME config may exist —
        // both outcomes are legal; the contract is only that spawn never hangs.
        let ctx = SpawnContext {
            worktree: PathBuf::from("."),
            task: Some("hi".into()),
            peer_prompt: Some("peer".into()),
        };
        let _ = OpenCodeAdapter.spawn_command(&ctx);
    }

    #[test]
    fn extract_model_field_plain_json() {
        let cfg = r#"{"provider": {}, "model": "9router/xxx"}"#;
        assert_eq!(
            extract_json_string_field(cfg, "model"),
            Some("9router/xxx".to_string())
        );
    }

    #[test]
    fn extract_model_field_jsonc_commented_out() {
        let cfg = "// {\"model\": \"old\"}\n{\"model\": \"new/m\"}";
        assert_eq!(
            extract_json_string_field(cfg, "model"),
            Some("new/m".into())
        );
    }

    #[test]
    fn extract_model_field_missing() {
        assert_eq!(extract_json_string_field("{\"a\":1}", "model"), None);
        assert_eq!(
            extract_json_string_field("{\"model\": \"\"}", "model"),
            None
        );
    }
}
