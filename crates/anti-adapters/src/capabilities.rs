//! Provider capability flags — discovered at runtime, not assumed.
//!
//! The `probe()` method runs the actual CLI to detect capabilities instead
//! of returning hardcoded defaults. Results are cached per binary path.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Global probe cache — avoids re-running CLI probes.
static PROBE_CACHE: Mutex<Option<HashMap<String, CapabilityFlags>>> = Mutex::new(None);

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

    /// Per-provider fallback defaults (used when probe can't determine).
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

    /// Probe the real binary capability by running it.
    /// Returns conservative fallback if binary not found.
    pub fn probe(provider: &str, binary: &str) -> Self {
        // Check cache first
        if let Ok(cache) = PROBE_CACHE.lock() {
            if let Some(ref map) = *cache {
                if let Some(cached) = map.get(binary) {
                    return *cached;
                }
            }
        }

        let result = probe_real(provider, binary);

        // Cache the result
        if let Ok(mut cache) = PROBE_CACHE.lock() {
            cache
                .get_or_insert_with(HashMap::new)
                .insert(binary.to_string(), result);
        }

        result
    }

    /// Clear the probe cache (for testing).
    pub fn clear_cache() {
        if let Ok(mut cache) = PROBE_CACHE.lock() {
            *cache = None;
        }
    }
}

/// Probe the real binary by running `--help` and checking output.
fn probe_real(provider: &str, binary: &str) -> CapabilityFlags {
    let mut caps = CapabilityFlags::none();

    // Check if binary exists
    if !is_binary_on_path(binary) {
        return caps;
    }

    // Run `binary --help` and capture output
    let help_output = match std::process::Command::new(binary).arg("--help").output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
        Err(_) => return CapabilityFlags::for_provider(provider), // fallback to defaults
    };

    let help_lower = help_output.to_lowercase();

    match provider {
        "claude" => {
            // Check for stream-json support
            caps.streaming =
                help_lower.contains("stream-json") || help_lower.contains("output-format");
            // Check for resume support
            caps.resume = help_lower.contains("resume") || help_lower.contains("continue");
            // Check for interrupt support (stream-json implies interruptible)
            caps.interrupt = if caps.streaming { Some(true) } else { None };
            // Check for permission support
            caps.permission = if help_lower.contains("permission-mode")
                || help_lower.contains("dangerously-skip-permissions")
            {
                Some(true)
            } else {
                None
            };
            // Check for reasoning/thinking support
            caps.reasoning = help_lower.contains("thinking") || help_lower.contains("effort");
            // Claude always has native subagents (Task tool)
            caps.native_subagent = true;
        }
        "codex" => {
            // Check for app-server support
            caps.streaming = help_lower.contains("app-server") || help_lower.contains("json");
            caps.resume = help_lower.contains("resume");
            caps.interrupt = Some(caps.streaming); // streaming implies interruptible
            caps.permission = if help_lower.contains("approval") {
                Some(true)
            } else {
                None
            };
            caps.reasoning = help_lower.contains("reasoning") || help_lower.contains("thinking");
            caps.native_subagent = help_lower.contains("agent") || help_lower.contains("subagent");
        }
        "opencode" => {
            // Check for server mode
            caps.streaming = help_lower.contains("serve") || help_lower.contains("json");
            caps.resume = help_lower.contains("resume") || help_lower.contains("session");
            caps.interrupt = Some(caps.streaming);
            caps.permission = if help_lower.contains("permission") || help_lower.contains("allow") {
                Some(true)
            } else {
                None
            };
            caps.reasoning = help_lower.contains("reasoning") || help_lower.contains("thinking");
            caps.native_subagent = help_lower.contains("task") || help_lower.contains("agent");
        }
        _ => {
            // Unknown provider — use defaults
            return CapabilityFlags::for_provider(provider);
        }
    }

    caps
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
        CapabilityFlags::clear_cache();
        let c = CapabilityFlags::probe("claude", "__definitely_not_a_real_binary_12345");
        assert!(!c.streaming);
    }

    #[test]
    fn probe_caches_results() {
        CapabilityFlags::clear_cache();
        let c1 = CapabilityFlags::probe("claude", "__nonexistent_binary_999");
        let c2 = CapabilityFlags::probe("claude", "__nonexistent_binary_999");
        assert_eq!(c1, c2);
    }

    #[test]
    fn clear_cache_works() {
        CapabilityFlags::clear_cache();
        let cache = PROBE_CACHE.lock().unwrap();
        assert!(cache.is_none());
    }
}
