//! Provider capability flags — discovered at runtime, not assumed.
//!
//! Mirrors Paseo's AgentCapabilityFlags but trimmed to what the CLI protocol
//! actually exposes. `Option<bool>` fields are `Some` when known supported/
//! unsupported, `None` when capability is unknown (e.g. old binary).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityFlags {
    /// `claude -p --output-format stream-json` supported
    pub streaming: bool,
    /// `--continue` / `--resume <sessionId>` supported
    pub resume: bool,
    /// Mid-turn interrupt via stdin JSON
    pub interrupt: Option<bool>,
    /// Bidirectional permission via stdin/stdout JSON
    pub permission: Option<bool>,
    /// Thinking/reasoning blocks in stream
    pub reasoning: bool,
    /// Provider may spawn its own sub-agents
    pub native_subagent: bool,
}

impl CapabilityFlags {
    /// Conservative default — no capabilities.
    pub fn none() -> Self {
        Self {
            streaming: false,
            resume: false,
            interrupt: None,
            permission: None,
            reasoning: false,
            native_subagent: false,
        }
    }

    /// Per-provider defaults (mirrors Paseo claude/codex/opencode definitions).
    pub fn for_provider(provider: &str) -> Self {
        match provider {
            "claude" => Self {
                streaming: true,
                resume: true,
                interrupt: Some(true),
                permission: Some(true),
                reasoning: true,
                native_subagent: true,
            },
            "codex" => Self {
                streaming: false,
                resume: false,
                interrupt: None,
                permission: None,
                reasoning: false,
                native_subagent: false,
            },
            "opencode" => Self {
                streaming: false,
                resume: false,
                interrupt: None,
                permission: None,
                reasoning: false,
                native_subagent: false,
            },
            "copilot" | "pi" => Self {
                streaming: true,
                resume: true,
                interrupt: Some(true),
                permission: Some(true),
                reasoning: true,
                native_subagent: false,
            },
            _ => Self::none(),
        }
    }

    /// Probe the real binary capability (best-effort).
    /// Returns conservative fallback if binary not found.
    pub fn probe(provider: &str, binary: &str) -> Self {
        let binary_exists = is_binary_on_path(binary);
        if !binary_exists {
            return Self::none();
        }
        // Binary exists: use provider default (actual stream-json probe can be
        // added later by trying `binary --help` and grepping for --output-format).
        Self::for_provider(provider)
    }
}

fn is_binary_on_path(binary: &str) -> bool {
    let paths = std::env::var("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&paths) {
        let p = dir.join(binary);
        if p.exists() {
            return true;
        }
        #[cfg(windows)]
        {
            let p_exe = dir.join(format!("{binary}.exe"));
            if p_exe.exists() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_has_no_capabilities() {
        let c = CapabilityFlags::none();
        assert!(!c.streaming);
        assert!(!c.resume);
        assert_eq!(c.interrupt, None);
    }

    #[test]
    fn claude_defaults_have_streaming_and_resume() {
        let c = CapabilityFlags::for_provider("claude");
        assert!(c.streaming);
        assert!(c.resume);
        assert_eq!(c.interrupt, Some(true));
    }

    #[test]
    fn probe_unknown_binary_returns_none() {
        let c = CapabilityFlags::probe("claude", "__definitely_not_a_real_binary_12345");
        assert!(!c.streaming);
    }
}
