//! Provider-native subagent tracking — SidechainTracker.
//!
//! Tracks lifecycle of subagents created by the provider itself (Task tool,
//! child sessions). These are DISTINCT from managed agents (Peers) which are
//! tracked by AgentManager.
//!
//! Visibility: Provider-native subagents are visible to the owning Peer
//! and the control plane. They are NOT hidden — hiding would break the
//! provider runtime.

use crate::events::AgentEvent;
use crate::provider::ProviderKind;
use std::collections::HashMap;

// ─── Subagent State ───────────────────────────────────────────────────────────

/// Status of a provider-native subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

/// State of a tracked provider-native subagent.
#[derive(Debug, Clone)]
pub struct SubagentState {
    /// Provider-specific ID (parentToolUseId for Claude, agentThreadId for Codex).
    pub id: String,
    /// Human-readable name.
    pub name: Option<String>,
    /// Subagent type (task, agent, etc.).
    pub sub_agent_type: Option<String>,
    /// Current status.
    pub status: SubagentStatus,
    /// Timeline events from the subagent.
    pub timeline: Vec<AgentEvent>,
}

// ─── Sidechain Tracker ────────────────────────────────────────────────────────

/// Issue #5: mirrors guard/rules.toml `deny_stems`. A tool whose normalized
/// name contains any of these stems is delegation-shaped and must never run
/// inside a peer session.
fn is_delegation_shaped(tool_name: &str) -> bool {
    const DENY_STEMS: [&str; 14] = [
        "agent",
        "subagent",
        "task",
        "workflow",
        "cron",
        "schedul",
        "worktree",
        "delegate",
        "spawn",
        "dispatch",
        "handoff",
        "remote",
        "sendmessage",
        "monitor",
    ];
    let normalized: String = tool_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    // mcp__* passthroughs are explicitly allowed by the rules file.
    if tool_name.starts_with("mcp__") {
        return false;
    }
    DENY_STEMS.iter().any(|stem| normalized.contains(stem))
}

/// Tracks provider-native subagents within a Peer's scope.
pub struct SidechainTracker {
    active: HashMap<String, SubagentState>,
}

impl SidechainTracker {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    /// Handle a provider event and emit normalized subagent events.
    ///
    /// Returns events to emit to the control plane.
    pub fn handle_event(&mut self, provider: ProviderKind, event: &AgentEvent) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        // Issue #5: ANY tool call whose name is delegation-shaped is a guard
        // violation regardless of provider — surface it before tracking.
        if let AgentEvent::ToolCallStart {
            call_id, tool_name, ..
        } = event
        {
            if is_delegation_shaped(tool_name) {
                events.push(AgentEvent::GuardViolation {
                    tool_name: tool_name.clone(),
                    call_id: Some(call_id.clone()),
                });
            }
        }

        match provider {
            ProviderKind::Claude => {
                // Claude: detect Task tool usage via tool calls
                if let AgentEvent::ToolCallStart {
                    call_id, tool_name, ..
                } = event
                {
                    if tool_name == "Task" || tool_name == "Agent" {
                        let state = SubagentState {
                            id: call_id.clone(),
                            name: None,
                            sub_agent_type: Some("task".into()),
                            status: SubagentStatus::Running,
                            timeline: Vec::new(),
                        };
                        self.active.insert(call_id.clone(), state);

                        events.push(AgentEvent::SubagentStarted {
                            id: call_id.clone(),
                            title: Some(format!("Claude subagent {call_id}")),
                        });
                    }
                }

                // Claude: detect completion via tool results
                if let AgentEvent::ToolCallComplete { call_id, .. } = event {
                    if self.active.contains_key(call_id) {
                        events.push(AgentEvent::SubagentCompleted {
                            id: call_id.clone(),
                            status: crate::events::SubagentStatus::Completed,
                        });
                        self.active.remove(call_id);
                    }
                }

                // Claude: detect failure
                if let AgentEvent::ToolCallFailed { call_id, .. } = event {
                    if self.active.contains_key(call_id) {
                        events.push(AgentEvent::SubagentCompleted {
                            id: call_id.clone(),
                            status: crate::events::SubagentStatus::Failed,
                        });
                        self.active.remove(call_id);
                    }
                }
            }
            ProviderKind::Codex => {
                // Codex: detect subAgentActivity items (via tool calls)
                if let AgentEvent::ToolCallStart {
                    call_id, tool_name, ..
                } = event
                {
                    if tool_name.contains("subAgent") || tool_name.contains("collabAgent") {
                        let state = SubagentState {
                            id: call_id.clone(),
                            name: None,
                            sub_agent_type: Some("subagent".into()),
                            status: SubagentStatus::Running,
                            timeline: Vec::new(),
                        };
                        self.active.insert(call_id.clone(), state);

                        events.push(AgentEvent::SubagentStarted {
                            id: call_id.clone(),
                            title: Some(format!("Codex subagent {call_id}")),
                        });
                    }
                }

                if let AgentEvent::ToolCallComplete { call_id, .. } = event {
                    if self.active.contains_key(call_id) {
                        events.push(AgentEvent::SubagentCompleted {
                            id: call_id.clone(),
                            status: crate::events::SubagentStatus::Completed,
                        });
                        self.active.remove(call_id);
                    }
                }
            }
            ProviderKind::OpenCode => {
                // OpenCode: detect task/subtask tool calls
                if let AgentEvent::ToolCallStart {
                    call_id, tool_name, ..
                } = event
                {
                    if tool_name == "task" || tool_name == "subtask" {
                        let state = SubagentState {
                            id: call_id.clone(),
                            name: None,
                            sub_agent_type: Some("task".into()),
                            status: SubagentStatus::Running,
                            timeline: Vec::new(),
                        };
                        self.active.insert(call_id.clone(), state);

                        events.push(AgentEvent::SubagentStarted {
                            id: call_id.clone(),
                            title: Some(format!("OpenCode subagent {call_id}")),
                        });
                    }
                }

                if let AgentEvent::ToolCallComplete { call_id, .. } = event {
                    if self.active.contains_key(call_id) {
                        events.push(AgentEvent::SubagentCompleted {
                            id: call_id.clone(),
                            status: crate::events::SubagentStatus::Completed,
                        });
                        self.active.remove(call_id);
                    }
                }
            }
        }

        events
    }

    /// Get all active subagents.
    pub fn active(&self) -> &HashMap<String, SubagentState> {
        &self.active
    }

    /// Get count of active subagents.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Remove a completed/failed subagent from tracking.
    pub fn remove(&mut self, id: &str) -> Option<SubagentState> {
        self.active.remove(id)
    }

    /// Clear all tracked subagents.
    pub fn clear(&mut self) {
        self.active.clear();
    }
}

impl Default for SidechainTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_starts_empty() {
        let tracker = SidechainTracker::new();
        assert_eq!(tracker.active_count(), 0);
        assert!(tracker.active().is_empty());
    }

    #[test]
    fn claude_task_detected() {
        let mut tracker = SidechainTracker::new();
        let event = AgentEvent::ToolCallStart {
            call_id: "task-1".into(),
            tool_name: "Task".into(),
            input: serde_json::json!({"prompt": "do stuff"}),
        };

        let events = tracker.handle_event(ProviderKind::Claude, &event);
        // Issue #5: a delegation-shaped call yields BOTH a GuardViolation
        // (control-plane signal) and the SubagentStarted tracking event.
        assert_eq!(events.len(), 2);
        assert!(
            matches!(events[0], AgentEvent::GuardViolation { ref tool_name, .. } if tool_name == "Task")
        );
        assert!(matches!(events[1], AgentEvent::SubagentStarted { .. }));
        assert_eq!(tracker.active_count(), 1);
    }

    #[test]
    fn claude_task_completion() {
        let mut tracker = SidechainTracker::new();

        // Start
        tracker.handle_event(
            ProviderKind::Claude,
            &AgentEvent::ToolCallStart {
                call_id: "task-1".into(),
                tool_name: "Task".into(),
                input: serde_json::Value::Null,
            },
        );

        // Complete
        let events = tracker.handle_event(
            ProviderKind::Claude,
            &AgentEvent::ToolCallComplete {
                call_id: "task-1".into(),
                output: Some("done".into()),
            },
        );

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AgentEvent::SubagentCompleted {
                status: crate::events::SubagentStatus::Completed,
                ..
            }
        ));
        assert_eq!(tracker.active_count(), 0);
    }

    #[test]
    fn codex_subagent_detected() {
        let mut tracker = SidechainTracker::new();
        let event = AgentEvent::ToolCallStart {
            call_id: "sub-1".into(),
            tool_name: "subAgentActivity".into(),
            input: serde_json::Value::Null,
        };

        let events = tracker.handle_event(ProviderKind::Codex, &event);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], AgentEvent::GuardViolation { .. }));
        assert!(matches!(events[1], AgentEvent::SubagentStarted { .. }));
    }

    #[test]
    fn opencode_task_detected() {
        let mut tracker = SidechainTracker::new();
        let event = AgentEvent::ToolCallStart {
            call_id: "task-1".into(),
            tool_name: "task".into(),
            input: serde_json::Value::Null,
        };

        let events = tracker.handle_event(ProviderKind::OpenCode, &event);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], AgentEvent::GuardViolation { .. }));
        assert!(matches!(events[1], AgentEvent::SubagentStarted { .. }));
    }

    #[test]
    fn guard_violation_on_delegation_shaped_tools() {
        let mut tracker = SidechainTracker::new();
        for (tool, denied) in [
            ("Task", true),
            ("Workflow", true),
            ("SendMessage", true),
            ("Read", false),
            ("mcp__ffs__dispatch", false), // mcp__ passthrough allowed
        ] {
            let event = AgentEvent::ToolCallStart {
                call_id: "c1".into(),
                tool_name: tool.into(),
                input: serde_json::Value::Null,
            };
            let events = tracker.handle_event(ProviderKind::Claude, &event);
            assert_eq!(
                events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::GuardViolation { .. })),
                denied,
                "{tool} violation mismatch"
            );
        }
    }

    #[test]
    fn non_subagent_tools_ignored() {
        let mut tracker = SidechainTracker::new();
        let event = AgentEvent::ToolCallStart {
            call_id: "bash-1".into(),
            tool_name: "Bash".into(),
            input: serde_json::Value::Null,
        };

        let events = tracker.handle_event(ProviderKind::Claude, &event);
        assert!(events.is_empty());
        assert_eq!(tracker.active_count(), 0);
    }

    #[test]
    fn tracker_clear() {
        let mut tracker = SidechainTracker::new();
        tracker.handle_event(
            ProviderKind::Claude,
            &AgentEvent::ToolCallStart {
                call_id: "task-1".into(),
                tool_name: "Task".into(),
                input: serde_json::Value::Null,
            },
        );
        assert_eq!(tracker.active_count(), 1);

        tracker.clear();
        assert_eq!(tracker.active_count(), 0);
    }
}
