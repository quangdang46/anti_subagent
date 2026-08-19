//! Attention policy (mirrors Paseo AttentionState, plan §5.6).
//!
//! When a peer finishes/errors/requests-permission, anti-daemon marks it as
//! requiring attention (triaging signal for supervisor/human via
//! `anti list --attention`). Cleared on ack or when the supervisor acts.

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttentionReason {
    Finished,
    Error,
    Permission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionState {
    pub requires_attention: bool,
    pub reason: Option<AttentionReason>,
    pub timestamp: Option<String>,
}

impl Default for AttentionState {
    fn default() -> Self {
        Self::none()
    }
}

impl AttentionState {
    pub fn none() -> Self {
        Self {
            requires_attention: false,
            reason: None,
            timestamp: None,
        }
    }

    pub fn new(reason: AttentionReason) -> Self {
        Self {
            requires_attention: true,
            reason: Some(reason),
            timestamp: Some(Utc::now().to_rfc3339()),
        }
    }

    pub fn clear(&mut self) {
        *self = Self::none();
    }

    /// Priority for triage (higher = more urgent): Permission 3 > Error 2 > Finished 1.
    pub fn priority(&self) -> u8 {
        match (self.requires_attention, self.reason) {
            (true, Some(AttentionReason::Permission)) => 3,
            (true, Some(AttentionReason::Error)) => 2,
            (true, Some(AttentionReason::Finished)) => 1,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn none_requires_no_attention() {
        let s = AttentionState::none();
        assert!(!s.requires_attention);
        assert_eq!(s.priority(), 0);
    }
    #[test]
    fn priority_ordering() {
        assert!(
            AttentionState::new(AttentionReason::Permission).priority()
                > AttentionState::new(AttentionReason::Error).priority()
        );
        assert!(
            AttentionState::new(AttentionReason::Error).priority()
                > AttentionState::new(AttentionReason::Finished).priority()
        );
    }
    #[test]
    fn clear_resets() {
        let mut s = AttentionState::new(AttentionReason::Error);
        s.clear();
        assert!(!s.requires_attention);
    }
}
