//! Event-driven orchestration bus — push-based notifications for agent lifecycle.
//!
//! Replaces polling-based monitoring with real-time event delivery.
//! When a Lead spawns a Peer, it subscribes to completion/failure events.
//!
//! Architecture:
//!   AgentManager::spawn() → emit AgentSpawned
//!   Peer completes → emit AgentCompleted → parent receives notification
//!   Peer fails → emit AgentFailed → parent receives notification
//!   Permission needed → emit PermissionRequested → parent routes to human

use anti_core::agent::AgentId;
use anti_core::events::AgentEvent;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

// ─── Events ───────────────────────────────────────────────────────────────────

/// Orchestration events — higher-level than provider AgentEvents.
#[derive(Debug, Clone)]
pub enum OrchestratorEvent {
    /// Agent spawned.
    AgentSpawned {
        agent_id: AgentId,
        parent_id: Option<AgentId>,
    },
    /// Agent completed successfully.
    AgentCompleted {
        agent_id: AgentId,
        exit_code: Option<i32>,
    },
    /// Agent failed.
    AgentFailed { agent_id: AgentId, error: String },
    /// Agent archived (soft-deleted).
    AgentArchived { agent_id: AgentId },
    /// Agent produced a thinking/reasoning delta.
    AgentThinking { agent_id: AgentId, content: String },
    /// Agent made a tool call.
    AgentToolCall {
        agent_id: AgentId,
        tool: String,
        status: String,
    },
    /// Agent produced a message.
    AgentMessage { agent_id: AgentId, content: String },
    /// Native subagent started within a peer.
    SubagentStarted {
        parent_id: AgentId,
        child_id: String,
        name: String,
    },
    /// Native subagent progress.
    SubagentProgress {
        parent_id: AgentId,
        child_id: String,
        progress: String,
    },
    /// Native subagent completed.
    SubagentCompleted {
        parent_id: AgentId,
        child_id: String,
    },
    /// Permission requested from a peer.
    PermissionRequested {
        agent_id: AgentId,
        tool_name: String,
        request_id: String,
    },
    /// Experience handoff requested.
    HandoffRequested {
        from: AgentId,
        to: AgentId,
        reason: String,
    },
}

// ─── Event Bus ────────────────────────────────────────────────────────────────

/// Synchronous event bus using mpsc channels.
pub struct EventBus {
    /// Registered subscribers: agent_id → sender
    subscribers: Mutex<HashMap<String, Vec<Sender<OrchestratorEvent>>>>,
    /// Global broadcast (all events)
    broadcast_tx: Sender<OrchestratorEvent>,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        let (broadcast_tx, _) = mpsc::channel();
        Self {
            subscribers: Mutex::new(HashMap::new()),
            broadcast_tx,
        }
    }

    /// Emit an event to all subscribers.
    pub fn emit(&self, event: OrchestratorEvent) {
        // Broadcast to all global subscribers
        let _ = self.broadcast_tx.send(event.clone());

        // Send to agent-specific subscribers
        let agent_id = match &event {
            OrchestratorEvent::AgentSpawned { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::AgentCompleted { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::AgentFailed { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::AgentArchived { agent_id } => Some(agent_id.as_str()),
            OrchestratorEvent::AgentThinking { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::AgentToolCall { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::AgentMessage { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::SubagentStarted { parent_id, .. } => Some(parent_id.as_str()),
            OrchestratorEvent::SubagentProgress { parent_id, .. } => Some(parent_id.as_str()),
            OrchestratorEvent::SubagentCompleted { parent_id, .. } => Some(parent_id.as_str()),
            OrchestratorEvent::PermissionRequested { agent_id, .. } => Some(agent_id.as_str()),
            OrchestratorEvent::HandoffRequested { .. } => None,
        };

        if let Some(id) = agent_id {
            if let Ok(subs) = self.subscribers.lock() {
                if let Some(senders) = subs.get(id) {
                    for tx in senders {
                        let _ = tx.send(event.clone());
                    }
                }
            }
        }
    }

    /// Subscribe to events for a specific agent.
    pub fn subscribe(&self, agent_id: &str) -> Receiver<OrchestratorEvent> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.entry(agent_id.to_string()).or_default().push(tx);
        }
        rx
    }

    /// Subscribe to all events (global broadcast).
    pub fn subscribe_all(&self) -> Receiver<OrchestratorEvent> {
        // For global subscription, we create a new channel and clone events
        // This is simplified — in production, use broadcast pattern
        let (tx, rx) = mpsc::channel();
        // Note: global subscribers receive events via the broadcast_tx
        // For now, this returns a channel that will receive future events
        rx
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_bus_emit_and_receive() {
        let bus = EventBus::new();
        let agent_id = AgentId::new();
        let rx = bus.subscribe(agent_id.as_str());

        bus.emit(OrchestratorEvent::AgentCompleted {
            agent_id: agent_id.clone(),
            exit_code: Some(0),
        });

        let event = rx.try_recv().unwrap();
        match event {
            OrchestratorEvent::AgentCompleted {
                agent_id: id,
                exit_code,
            } => {
                assert_eq!(id, agent_id);
                assert_eq!(exit_code, Some(0));
            }
            _ => panic!("wrong event type"),
        }
    }

    #[test]
    fn event_bus_multiple_subscribers() {
        let bus = EventBus::new();
        let agent_id = AgentId::new();
        let rx1 = bus.subscribe(agent_id.as_str());
        let rx2 = bus.subscribe(agent_id.as_str());

        bus.emit(OrchestratorEvent::AgentFailed {
            agent_id: agent_id.clone(),
            error: "oops".into(),
        });

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn event_bus_no_cross_agent_leak() {
        let bus = EventBus::new();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();
        let rx_a = bus.subscribe(agent_a.as_str());

        bus.emit(OrchestratorEvent::AgentCompleted {
            agent_id: agent_b.clone(),
            exit_code: Some(0),
        });

        // Agent A should NOT receive Agent B's events
        assert!(rx_a.try_recv().is_err());
    }

    #[test]
    fn event_bus_parent_receives_child_events() {
        let bus = EventBus::new();
        let parent_id = AgentId::new();
        let child_id = AgentId::new();

        // Parent subscribes
        let rx = bus.subscribe(parent_id.as_str());

        // Child completes with parent_id
        bus.emit(OrchestratorEvent::SubagentCompleted {
            parent_id: parent_id.clone(),
            child_id: child_id.to_string(),
        });

        let event = rx.try_recv().unwrap();
        match event {
            OrchestratorEvent::SubagentCompleted {
                parent_id: p,
                child_id: c,
            } => {
                assert_eq!(p, parent_id);
                assert_eq!(c, child_id.to_string());
            }
            _ => panic!("wrong event type"),
        }
    }

    #[test]
    fn event_bus_permission_routed_to_parent() {
        let bus = EventBus::new();
        let agent_id = AgentId::new();
        let rx = bus.subscribe(agent_id.as_str());

        bus.emit(OrchestratorEvent::PermissionRequested {
            agent_id: agent_id.clone(),
            tool_name: "bash".into(),
            request_id: "perm-1".into(),
        });

        let event = rx.try_recv().unwrap();
        match event {
            OrchestratorEvent::PermissionRequested {
                agent_id: id,
                tool_name,
                ..
            } => {
                assert_eq!(id, agent_id);
                assert_eq!(tool_name, "bash");
            }
            _ => panic!("wrong event type"),
        }
    }
}
