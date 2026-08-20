//! Event bridge — connects adapter AgentEvents to daemon lifecycle.
//!
//! When a Claude peer is spawned with stream-json, its stdout produces
//! NDJSON → AgentEvent. This module reads those events and sends them
//! through a channel. The daemon's reaper thread drains the channel and
//! persists events to the Store, making provider activity observable
//! to the control plane.
//!
//! Architecture:
//!   claude stdout → NDJSON reader → AgentEvent → channel → reaper drains → Store.append_event()

use anti_adapters::{AgentEvent, parse_claude_stream_line};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// A bridged event: agent_id + parsed AgentEvent.
#[derive(Debug)]
pub struct BridgedEvent {
    pub agent_id: String,
    pub event: AgentEvent,
}

/// Bridge that reads adapter events from a child process stdout and sends
/// them through a channel for the daemon to persist.
pub struct EventBridge;

impl EventBridge {
    /// Spawn a background reader that bridges adapter events to the channel.
    ///
    /// Takes ownership of child.stdout — the child must have been spawned
    /// with `Stdio::piped()` for stdout.
    ///
    /// Returns the receiver end. The caller (daemon reaper) drains this
    /// channel and persists events to the Store.
    pub fn spawn(child: &mut Child, agent_id: &str) -> Receiver<BridgedEvent> {
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                // Return a channel that's immediately closed.
                let (_, rx) = mpsc::channel();
                return rx;
            }
        };
        let agent_id = agent_id.to_string();
        let (tx, rx) = mpsc::channel::<BridgedEvent>();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line_result in reader.lines() {
                let line = match line_result {
                    Ok(l) => l,
                    Err(_) => break, // stdout closed
                };
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let Some(event) = parse_claude_stream_line(&line) else {
                    continue; // unrecognized — skip
                };

                let bridged = BridgedEvent {
                    agent_id: agent_id.clone(),
                    event,
                };
                if tx.send(bridged).is_err() {
                    break; // receiver dropped
                }
            }
        });

        rx
    }

    /// Drain pending events from a receiver into a vector.
    pub fn drain(rx: &Receiver<BridgedEvent>) -> Vec<BridgedEvent> {
        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_empty_channel() {
        let (_, rx) = mpsc::channel::<BridgedEvent>();
        let events = EventBridge::drain(&rx);
        assert!(events.is_empty());
    }

    #[test]
    fn drain_collects_events() {
        let (tx, rx) = mpsc::channel();
        tx.send(BridgedEvent {
            agent_id: "a1".into(),
            event: AgentEvent::TurnCompleted { usage: None },
        })
        .unwrap();
        tx.send(BridgedEvent {
            agent_id: "a1".into(),
            event: AgentEvent::AssistantMessage {
                text: "hello".into(),
                message_id: "m1".into(),
            },
        })
        .unwrap();
        drop(tx);

        let events = EventBridge::drain(&rx);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].event, AgentEvent::TurnCompleted { .. }));
        assert!(matches!(
            events[1].event,
            AgentEvent::AssistantMessage { .. }
        ));
    }
}
