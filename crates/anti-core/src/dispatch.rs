//! DispatchLog — task dispatch tracking with evidence-based outcomes.
//!
//! Derived from oh-my-codex (OMX) DispatchLog pattern.
//! Answers: "What happened when we tried to deliver this task to a peer?"
//!
//! Append-only audit trail. Every dispatch attempt gets recorded
//! with its outcome. Granularity lets the scheduler make informed
//! decisions about retry, escalation, or abandonment.

use serde::{Deserialize, Serialize};

/// Lifecycle status of a dispatch attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchStatus {
    /// Task created, not yet sent to peer.
    Pending,
    /// Peer has been notified of the task.
    Notified,
    /// Peer acknowledged receipt.
    Delivered,
    /// Peer completed successfully.
    Completed,
    /// Peer reported failure.
    Failed,
    /// Peer deferred (will try later).
    Deferred,
    /// Dispatch cancelled by orchestrator.
    Cancelled,
}

/// Evidence-based outcome of a dispatch attempt.
///
/// 10 outcomes covering every distinct success/failure path.
/// Granularity prevents lossy categorization — e.g. TargetMissing
/// (peer doesn't exist) vs TargetUnavailable (peer exists but busy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DispatchOutcome {
    /// Peer confirmed receipt.
    DeliveredConfirmed,
    /// Sent but no confirmation received.
    DeliveredUnconfirmed,
    /// Completed and verified.
    CompletedConfirmed,
    /// Completed but verification not yet run.
    CompletedUnverified,
    /// Peer process doesn't exist.
    TargetMissing,
    /// Peer exists but busy or unreachable.
    TargetUnavailable,
    /// Pre-dispatch checks failed (workspace not ready, etc.).
    PreflightFailed,
    /// IPC/transport failure.
    SendFailed,
    /// Delivery timed out.
    Timeout,
    /// Dispatch cancelled by orchestrator.
    Cancelled,
}

impl DispatchStatus {
    pub fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "pending" => Ok(DispatchStatus::Pending),
            "notified" => Ok(DispatchStatus::Notified),
            "delivered" => Ok(DispatchStatus::Delivered),
            "completed" => Ok(DispatchStatus::Completed),
            "failed" => Ok(DispatchStatus::Failed),
            "deferred" => Ok(DispatchStatus::Deferred),
            "cancelled" => Ok(DispatchStatus::Cancelled),
            _ => Err(()),
        }
    }
}

impl DispatchOutcome {
    pub fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "delivered_confirmed" => Ok(DispatchOutcome::DeliveredConfirmed),
            "delivered_unconfirmed" => Ok(DispatchOutcome::DeliveredUnconfirmed),
            "completed_confirmed" => Ok(DispatchOutcome::CompletedConfirmed),
            "completed_unverified" => Ok(DispatchOutcome::CompletedUnverified),
            "target_missing" => Ok(DispatchOutcome::TargetMissing),
            "target_unavailable" => Ok(DispatchOutcome::TargetUnavailable),
            "preflight_failed" => Ok(DispatchOutcome::PreflightFailed),
            "send_failed" => Ok(DispatchOutcome::SendFailed),
            "timeout" => Ok(DispatchOutcome::Timeout),
            "cancelled" => Ok(DispatchOutcome::Cancelled),
            _ => Err(()),
        }
    }
}

/// A single dispatch event — append-only audit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchEvent {
    pub id: String,
    pub task_id: String,
    pub peer_id: String,
    pub status: DispatchStatus,
    pub outcome: Option<DispatchOutcome>,
    pub created_at: String,
    pub updated_at: String,
}

/// Valid status transitions for dispatch lifecycle.
pub fn can_transition_dispatch(from: DispatchStatus, to: DispatchStatus) -> bool {
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        (DispatchStatus::Pending, DispatchStatus::Notified)
            | (DispatchStatus::Pending, DispatchStatus::Failed)
            | (DispatchStatus::Pending, DispatchStatus::Cancelled)
            | (DispatchStatus::Notified, DispatchStatus::Delivered)
            | (DispatchStatus::Notified, DispatchStatus::Failed)
            | (DispatchStatus::Notified, DispatchStatus::Deferred)
            | (DispatchStatus::Notified, DispatchStatus::Cancelled)
            | (DispatchStatus::Delivered, DispatchStatus::Completed)
            | (DispatchStatus::Delivered, DispatchStatus::Failed)
            | (DispatchStatus::Delivered, DispatchStatus::Cancelled)
            | (DispatchStatus::Deferred, DispatchStatus::Notified)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_lifecycle_happy_path() {
        assert!(can_transition_dispatch(
            DispatchStatus::Pending,
            DispatchStatus::Notified
        ));
        assert!(can_transition_dispatch(
            DispatchStatus::Notified,
            DispatchStatus::Delivered
        ));
        assert!(can_transition_dispatch(
            DispatchStatus::Delivered,
            DispatchStatus::Completed
        ));
    }

    #[test]
    fn dispatch_failure_paths() {
        assert!(can_transition_dispatch(
            DispatchStatus::Pending,
            DispatchStatus::Failed
        ));
        assert!(can_transition_dispatch(
            DispatchStatus::Notified,
            DispatchStatus::Failed
        ));
        assert!(can_transition_dispatch(
            DispatchStatus::Delivered,
            DispatchStatus::Failed
        ));
    }

    #[test]
    fn dispatch_cancelled_from_any_active() {
        assert!(can_transition_dispatch(
            DispatchStatus::Pending,
            DispatchStatus::Cancelled
        ));
        assert!(can_transition_dispatch(
            DispatchStatus::Notified,
            DispatchStatus::Cancelled
        ));
        assert!(can_transition_dispatch(
            DispatchStatus::Delivered,
            DispatchStatus::Cancelled
        ));
    }

    #[test]
    fn dispatch_deferred_can_retry() {
        assert!(can_transition_dispatch(
            DispatchStatus::Deferred,
            DispatchStatus::Notified
        ));
    }

    #[test]
    fn dispatch_terminal_no_return() {
        assert!(!can_transition_dispatch(
            DispatchStatus::Completed,
            DispatchStatus::Pending
        ));
        assert!(!can_transition_dispatch(
            DispatchStatus::Failed,
            DispatchStatus::Notified
        ));
        assert!(!can_transition_dispatch(
            DispatchStatus::Cancelled,
            DispatchStatus::Pending
        ));
    }

    #[test]
    fn dispatch_status_roundtrip() {
        let s = DispatchStatus::Notified;
        let json = serde_json::to_string(&s).unwrap();
        let parsed: DispatchStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, parsed);
    }

    #[test]
    fn dispatch_outcome_roundtrip() {
        let o = DispatchOutcome::CompletedConfirmed;
        let json = serde_json::to_string(&o).unwrap();
        let parsed: DispatchOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, parsed);
    }

    #[test]
    fn dispatch_event_roundtrip() {
        let event = DispatchEvent {
            id: "d1".into(),
            task_id: "t1".into(),
            peer_id: "p1".into(),
            status: DispatchStatus::Completed,
            outcome: Some(DispatchOutcome::CompletedConfirmed),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: DispatchEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "d1");
        assert_eq!(parsed.status, DispatchStatus::Completed);
        assert_eq!(parsed.outcome, Some(DispatchOutcome::CompletedConfirmed));
    }

    #[test]
    fn dispatch_status_exhaustive_match() {
        fn describe(s: DispatchStatus) -> &'static str {
            match s {
                DispatchStatus::Pending => "new",
                DispatchStatus::Notified => "sent",
                DispatchStatus::Delivered => "acked",
                DispatchStatus::Completed => "done",
                DispatchStatus::Failed => "error",
                DispatchStatus::Deferred => "later",
                DispatchStatus::Cancelled => "killed",
            }
        }
        assert_eq!(describe(DispatchStatus::Completed), "done");
    }

    #[test]
    fn dispatch_outcome_exhaustive_match() {
        fn describe(o: DispatchOutcome) -> &'static str {
            match o {
                DispatchOutcome::DeliveredConfirmed => "confirmed",
                DispatchOutcome::DeliveredUnconfirmed => "unconfirmed",
                DispatchOutcome::CompletedConfirmed => "verified",
                DispatchOutcome::CompletedUnverified => "unverified",
                DispatchOutcome::TargetMissing => "gone",
                DispatchOutcome::TargetUnavailable => "busy",
                DispatchOutcome::PreflightFailed => "preflight",
                DispatchOutcome::SendFailed => "send",
                DispatchOutcome::Timeout => "timeout",
                DispatchOutcome::Cancelled => "cancelled",
            }
        }
        assert_eq!(describe(DispatchOutcome::Timeout), "timeout");
    }
}
