//! AuthorityLease — session ownership model.
//!
//! Answers: "Who controls this session/task right now?"
//! Prevents two Leads from claiming the same peer session simultaneously.
//!
//! Per-task, not per-peer. Lifecycle: acquire → renew → release.
//! Stale detection via leased_until timestamp comparison.

use serde::{Deserialize, Serialize};

/// Session ownership lease — who controls a task right now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityLease {
    /// Agent ID that holds the lease (None = unheld).
    pub owner: Option<String>,
    /// Unique lease identifier (UUID).
    pub lease_id: Option<String>,
    /// Expiry timestamp (ISO 8601).
    pub leased_until: Option<String>,
    /// True if lease has expired.
    pub stale: bool,
    /// Why the lease is stale.
    pub stale_reason: Option<String>,
}

/// Errors from lease operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthorityError {
    #[error("lease already held by {0}")]
    AlreadyHeld(String),
    #[error("lease not held")]
    NotHeld,
    #[error("lease expired")]
    Expired,
    #[error("owner mismatch: expected {expected}, got {actual}")]
    OwnerMismatch { expected: String, actual: String },
}

impl AuthorityLease {
    /// Create a new unheld lease.
    pub fn new() -> Self {
        Self {
            owner: None,
            lease_id: None,
            leased_until: None,
            stale: false,
            stale_reason: None,
        }
    }

    /// Acquire the lease. Only succeeds if currently unheld or stale.
    pub fn acquire(
        &mut self,
        owner: &str,
        lease_id: &str,
        leased_until: &str,
    ) -> Result<(), AuthorityError> {
        match &self.owner {
            Some(current) if current == owner => {
                // Re-entrant: same owner re-acquires
                self.lease_id = Some(lease_id.to_string());
                self.leased_until = Some(leased_until.to_string());
                self.stale = false;
                self.stale_reason = None;
                Ok(())
            }
            Some(current) => {
                if self.stale {
                    // Stale lease can be overridden
                    self.owner = Some(owner.to_string());
                    self.lease_id = Some(lease_id.to_string());
                    self.leased_until = Some(leased_until.to_string());
                    self.stale = false;
                    self.stale_reason = None;
                    Ok(())
                } else {
                    Err(AuthorityError::AlreadyHeld(current.clone()))
                }
            }
            None => {
                // Unheld — acquire
                self.owner = Some(owner.to_string());
                self.lease_id = Some(lease_id.to_string());
                self.leased_until = Some(leased_until.to_string());
                self.stale = false;
                self.stale_reason = None;
                Ok(())
            }
        }
    }

    /// Renew the lease. Only succeeds if same owner and not stale.
    pub fn renew(
        &mut self,
        owner: &str,
        lease_id: &str,
        leased_until: &str,
    ) -> Result<(), AuthorityError> {
        match &self.owner {
            Some(current) if current == owner => {
                if self.stale {
                    return Err(AuthorityError::Expired);
                }
                self.lease_id = Some(lease_id.to_string());
                self.leased_until = Some(leased_until.to_string());
                Ok(())
            }
            Some(current) => Err(AuthorityError::OwnerMismatch {
                expected: current.clone(),
                actual: owner.to_string(),
            }),
            None => Err(AuthorityError::NotHeld),
        }
    }

    /// Release the lease. Only succeeds if same owner.
    pub fn release(&mut self, owner: &str) -> Result<(), AuthorityError> {
        match &self.owner {
            Some(current) if current == owner => {
                self.owner = None;
                self.lease_id = None;
                self.leased_until = None;
                self.stale = false;
                self.stale_reason = None;
                Ok(())
            }
            Some(current) => Err(AuthorityError::OwnerMismatch {
                expected: current.clone(),
                actual: owner.to_string(),
            }),
            None => Err(AuthorityError::NotHeld),
        }
    }

    /// Check staleness by comparing leased_until to current time.
    /// Returns true if stale.
    pub fn check_staleness(&mut self, now_rfc3339: &str) -> bool {
        if let Some(ref until) = self.leased_until {
            if until.as_str() < now_rfc3339 {
                self.stale = true;
                self.stale_reason = Some(format!("lease expired at {until}, now is {now_rfc3339}"));
                return true;
            }
        }
        false
    }

    /// True if the lease is available (unheld or stale).
    pub fn is_available(&self) -> bool {
        self.owner.is_none() || self.stale
    }
}

impl Default for AuthorityLease {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_when_unheld() {
        let mut lease = AuthorityLease::new();
        assert!(
            lease
                .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
                .is_ok()
        );
        assert_eq!(lease.owner.as_deref(), Some("lead-a"));
        assert_eq!(lease.lease_id.as_deref(), Some("lease-1"));
        assert!(!lease.stale);
    }

    #[test]
    fn acquire_reentrant_same_owner() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        assert!(
            lease
                .acquire("lead-a", "lease-2", "2099-02-01T00:00:00Z")
                .is_ok()
        );
        assert_eq!(lease.lease_id.as_deref(), Some("lease-2"));
    }

    #[test]
    fn acquire_rejected_when_held_by_other() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        let result = lease.acquire("lead-b", "lease-2", "2099-01-01T00:00:00Z");
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthorityError::AlreadyHeld(holder) => assert_eq!(holder, "lead-a"),
            other => panic!("expected AlreadyHeld, got {other:?}"),
        }
    }

    #[test]
    fn acquire_when_stale_allows_override() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2020-01-01T00:00:00Z")
            .unwrap();
        lease.check_staleness("2026-01-01T00:00:00Z");
        assert!(lease.stale);
        assert!(
            lease
                .acquire("lead-b", "lease-2", "2099-01-01T00:00:00Z")
                .is_ok()
        );
        assert_eq!(lease.owner.as_deref(), Some("lead-b"));
        assert!(!lease.stale);
    }

    #[test]
    fn renew_by_same_owner() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        assert!(
            lease
                .renew("lead-a", "lease-1", "2099-06-01T00:00:00Z")
                .is_ok()
        );
        assert_eq!(lease.leased_until.as_deref(), Some("2099-06-01T00:00:00Z"));
    }

    #[test]
    fn renew_rejected_by_different_owner() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        let result = lease.renew("lead-b", "lease-2", "2099-01-01T00:00:00Z");
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthorityError::OwnerMismatch { expected, actual } => {
                assert_eq!(expected, "lead-a");
                assert_eq!(actual, "lead-b");
            }
            other => panic!("expected OwnerMismatch, got {other:?}"),
        }
    }

    #[test]
    fn renew_rejected_when_stale() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2020-01-01T00:00:00Z")
            .unwrap();
        lease.check_staleness("2026-01-01T00:00:00Z");
        assert!(matches!(
            lease.renew("lead-a", "lease-2", "2099-01-01T00:00:00Z"),
            Err(AuthorityError::Expired)
        ));
    }

    #[test]
    fn renew_rejected_when_not_held() {
        let mut lease = AuthorityLease::new();
        assert!(matches!(
            lease.renew("lead-a", "lease-1", "2099-01-01T00:00:00Z"),
            Err(AuthorityError::NotHeld)
        ));
    }

    #[test]
    fn release_by_owner() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        assert!(lease.release("lead-a").is_ok());
        assert!(lease.owner.is_none());
        assert!(lease.is_available());
    }

    #[test]
    fn release_rejected_by_non_owner() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        let result = lease.release("lead-b");
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthorityError::OwnerMismatch { expected, actual } => {
                assert_eq!(expected, "lead-a");
                assert_eq!(actual, "lead-b");
            }
            other => panic!("expected OwnerMismatch, got {other:?}"),
        }
    }

    #[test]
    fn release_rejected_when_not_held() {
        let mut lease = AuthorityLease::new();
        assert!(matches!(
            lease.release("lead-a"),
            Err(AuthorityError::NotHeld)
        ));
    }

    #[test]
    fn stale_detection() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2020-01-01T00:00:00Z")
            .unwrap();
        assert!(!lease.check_staleness("2019-01-01T00:00:00Z")); // not stale yet
        assert!(lease.check_staleness("2026-01-01T00:00:00Z")); // stale!
        assert!(lease.stale);
        assert!(lease.stale_reason.is_some());
    }

    #[test]
    fn is_available_unheld() {
        let lease = AuthorityLease::new();
        assert!(lease.is_available());
    }

    #[test]
    fn is_available_stale() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2020-01-01T00:00:00Z")
            .unwrap();
        lease.check_staleness("2026-01-01T00:00:00Z");
        assert!(lease.is_available());
    }

    #[test]
    fn not_available_when_held() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        assert!(!lease.is_available());
    }

    #[test]
    fn lease_roundtrip() {
        let mut lease = AuthorityLease::new();
        lease
            .acquire("lead-a", "lease-1", "2099-01-01T00:00:00Z")
            .unwrap();
        let json = serde_json::to_string(&lease).unwrap();
        let parsed: AuthorityLease = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.owner.as_deref(), Some("lead-a"));
        assert_eq!(parsed.lease_id.as_deref(), Some("lease-1"));
    }
}
