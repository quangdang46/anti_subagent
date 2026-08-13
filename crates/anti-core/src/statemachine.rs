//! Lifecycle state machine (plan §17), enforced by optimistic-lock UPDATE.

use crate::model::AgentStatus;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: AgentStatus, to: AgentStatus },
}

/// The canonical transition table. Everything not listed here is illegal.
pub fn can_transition(from: AgentStatus, to: AgentStatus) -> bool {
    use AgentStatus::*;
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        (Created, Starting)
            | (Starting, Running)
            | (Starting, Failed)
            | (Running, Blocked)
            | (Blocked, Running)
            | (Running, Completed)
            | (Running, Crashed)
            | (Crashed, Recovering)
            | (Recovering, Running)
            | (Recovering, Replaced)
            | (Running, Stopped)
            | (Blocked, Stopped)
            | (Running, Failed)
    )
}

pub fn check_transition(from: AgentStatus, to: AgentStatus) -> Result<(), TransitionError> {
    if can_transition(from, to) {
        Ok(())
    } else {
        Err(TransitionError::InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentStatus::*;

    #[test]
    fn happy_path() {
        assert!(can_transition(Created, Starting));
        assert!(can_transition(Starting, Running));
        assert!(can_transition(Running, Completed));
    }

    #[test]
    fn illegal_skips() {
        assert!(!can_transition(Created, Running));
        assert!(!can_transition(Created, Completed));
        assert!(!can_transition(Completed, Running));
    }

    #[test]
    fn crash_recovery_path() {
        assert!(can_transition(Running, Crashed));
        assert!(can_transition(Crashed, Recovering));
        assert!(can_transition(Recovering, Running));
        assert!(can_transition(Recovering, Replaced));
    }
}
