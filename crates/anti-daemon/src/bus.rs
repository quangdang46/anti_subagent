//! Lifecycle event bus — typed events with handler dispatch.
//!
//! Events are emitted by various components (PeerManager, TaskStateMachine, etc.)
//! and dispatched to registered handlers. Handlers can perform side effects like
//! cleanup, persistence, or notifications.

use anti_core::events::{Event, EventType};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Handler trait for lifecycle events.
pub trait EventHandler: Send + Sync {
    fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error>>;
}

/// Event bus with typed handlers.
pub struct EventBus {
    handlers: Mutex<HashMap<String, Vec<Box<dyn EventHandler>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            handlers: Mutex::new(HashMap::new()),
        }
    }

    /// Register a handler for an event type.
    pub fn on(&self, event_type: &str, handler: Box<dyn EventHandler>) {
        let mut handlers = self.handlers.lock().unwrap();
        handlers
            .entry(event_type.to_string())
            .or_default()
            .push(handler);
    }

    /// Emit an event to all registered handlers.
    pub fn emit(&self, event: &Event) {
        let handlers = self.handlers.lock().unwrap();
        let key = format!("{:?}", event.type_);

        if let Some(event_handlers) = handlers.get(&key) {
            for handler in event_handlers {
                if let Err(e) = handler.handle(event) {
                    eprintln!("Event handler error for {:?}: {}", event.type_, e);
                }
            }
        }
    }
}

/// Recovery handler — cleans up workspace on peer crash.
/// Note: actual cleanup is done in reap_children() directly.
/// This handler is for logging/audit purposes.
pub struct RecoveryHandler;

impl EventHandler for RecoveryHandler {
    fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error>> {
        if event.type_ == EventType::PeerCrashed {
            eprintln!(
                "[RECOVERY] Peer {} crashed — workspace cleanup required",
                event.agent_id
            );
            if let Some(lease_id) = event
                .payload
                .get("workspace_lease_id")
                .and_then(|v| v.as_str())
            {
                eprintln!("[RECOVERY] Lease ID: {}", lease_id);
            }
        }
        Ok(())
    }
}

/// Evidence handler — persists verification results.
pub struct EvidenceHandler;

impl EventHandler for EvidenceHandler {
    fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error>> {
        if event.type_ == EventType::VerificationPassed {
            // Log verification success
            eprintln!(
                "Verification passed for task {}: {:?}",
                event.agent_id, event.payload
            );
        }
        Ok(())
    }
}

/// Notification handler — logs events for audit trail.
pub struct NotificationHandler;

impl EventHandler for NotificationHandler {
    fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!(
            "[{}] {:?} for {}: {:?}",
            event.timestamp, event.type_, event.agent_id, event.payload
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn event_bus_dispatches_to_handlers() {
        let bus = EventBus::new();
        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        struct CountHandler(Arc<Mutex<i32>>);
        impl EventHandler for CountHandler {
            fn handle(&self, _event: &Event) -> Result<(), Box<dyn std::error::Error>> {
                *self.0.lock().unwrap() += 1;
                Ok(())
            }
        }

        bus.on("PeerCrashed", Box::new(CountHandler(counter_clone)));

        let event = Event::new(1, "test", EventType::PeerCrashed, json!({}));
        bus.emit(&event);

        assert_eq!(*counter.lock().unwrap(), 1);
    }
}
