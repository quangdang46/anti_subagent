//! WorkItem — task lifecycle of SLP (SETTLED ≠ VERIFIED ≠ ACCEPTED).
//!
//! Lesson from irina: "done" is a claim, not truth; acceptance only through
//! evidence + verification + decision.
//! Lesson from veylen: reject must bump revision (group counter reset) and
//! silent lead = must have watchdog.

use serde::{Deserialize, Serialize};

/// Lifecycle states for a work item — staged pipeline.
/// Path: RECEIVED → EXPLORED → PLANNED → EXECUTING → EXECUTED → VERIFYING → VERIFIED → ACCEPTED
/// Failure paths: EXECUTING→FAILED, VERIFYING→REJECTED→FIXING→EXECUTING
/// Terminal states: ACCEPTED, REJECTED, CANCELLED, EXHAUSTED
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkItemState {
    // Legacy states (backward compatible)
    Pending,       // lead giao, peer chưa nhận
    InProgress,    // peer claim và đang làm
    Submitted,     // peer submit + evidence — SETTLED (claim)
    Verified,      // verifier xác nhận evidence khớp — VERIFIED
    Accepted,      // lead accept — ACCEPTED (chỉ từ Verified)
    NeedsRevision, // reject → peer sửa lại; revision bump
    Rejected,      // terminal reject (vượt max_revisions hoặc lead hủy)

    // Staged pipeline states (Phase 1)
    Received,      // task assigned to peer
    Explored,      // peer has investigated codebase
    Planned,       // implementation plan created
    Executing,     // peer actively coding
    Executed,      // code written, awaiting verification
    Verifying,     // verification in progress
    Failed,        // execution failed (terminal)
    Fixing,        // peer fixing issues after rejection
    Exhausted,     // max_revisions exceeded (terminal)
    Cancelled,     // task cancelled (terminal)
}

impl WorkItemState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkItemState::Accepted
                | WorkItemState::Rejected
                | WorkItemState::Failed
                | WorkItemState::Exhausted
                | WorkItemState::Cancelled
        )
    }

    /// Map legacy states to staged pipeline states
    pub fn to_staged(self) -> Self {
        match self {
            WorkItemState::Pending => WorkItemState::Received,
            WorkItemState::InProgress => WorkItemState::Executing,
            WorkItemState::Submitted => WorkItemState::Executed,
            WorkItemState::Verified => WorkItemState::Verified,
            WorkItemState::Accepted => WorkItemState::Accepted,
            WorkItemState::NeedsRevision => WorkItemState::Fixing,
            WorkItemState::Rejected => WorkItemState::Rejected,
            other => other,
        }
    }
}

/// Evidence reference — sha-256 hex of artifact (file/output).
/// "claim must match actual evidence"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// sha-256 hex of artifact — claim must match actual evidence
    pub sha256: String,
    pub artifact_path: String,
    pub produced_at: String,
}

/// Comprehensive evidence record for audit trail.
/// Replaces simple EvidenceRef with full verification evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    // Integrity
    pub artifact_sha256: String,
    pub artifact_path: String,

    // Verification evidence
    pub test_output: Option<String>,
    pub test_exit_code: Option<i32>,
    pub build_output: Option<String>,
    pub build_exit_code: Option<i32>,
    pub lint_output: Option<String>,
    pub diagnostics: Vec<String>,

    // Git state at verification time
    pub git_sha: Option<String>,
    pub git_diff: Option<String>,
    pub git_status: Option<String>,

    // Acceptance criteria verification
    pub claims: Vec<ClaimVerification>,

    // Metadata
    pub produced_at: String,
    pub verified_at: Option<String>,
    pub verified_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimVerification {
    pub claim: String,
    pub status: VerifyClaimStatus,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyClaimStatus {
    Verified,
    Partial,
    Missing,
}

impl EvidenceRecord {
    pub fn new(artifact_sha256: String, artifact_path: String) -> Self {
        Self {
            artifact_sha256,
            artifact_path,
            test_output: None,
            test_exit_code: None,
            build_output: None,
            build_exit_code: None,
            lint_output: None,
            diagnostics: Vec::new(),
            git_sha: None,
            git_diff: None,
            git_status: None,
            claims: Vec::new(),
            produced_at: chrono::Utc::now().to_rfc3339(),
            verified_at: None,
            verified_by: None,
        }
    }

    /// Convert to legacy EvidenceRef for backward compatibility
    pub fn to_evidence_ref(&self) -> EvidenceRef {
        EvidenceRef {
            sha256: self.artifact_sha256.clone(),
            artifact_path: self.artifact_path.clone(),
            produced_at: self.produced_at.clone(),
        }
    }
}

/// WorkItem — unit of work assigned by lead to peer.
/// Task lifecycle (SETTLED ≠ VERIFIED ≠ ACCEPTED).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: String,
    pub task_node_id: String, // nhóm DAG nếu có
    pub peer_id: String,      // ai đang giữ
    pub lead_id: String,      // ai accept
    pub state: WorkItemState,
    pub revision: u32,      // bump mỗi lần reject (veylen lesson)
    pub max_revisions: u32, // mặc định 3
    pub evidence: Option<EvidenceRef>,
    pub review_verdict: Option<String>, // lead note
    pub submitted_at: Option<String>,
    pub review_deadline: Option<String>, // RFC3339 — watchdog dựa vào đây
    pub created_at: String,
    pub updated_at: String,
}

impl WorkItem {
    pub fn new(id: String, peer_id: String) -> Self {
        Self {
            id,
            task_node_id: String::new(),
            peer_id,
            lead_id: String::new(),
            state: WorkItemState::Pending,
            revision: 1,
            max_revisions: 3,
            evidence: None,
            review_verdict: None,
            submitted_at: None,
            review_deadline: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn transition(&mut self, to: WorkItemState) -> Result<(), WorkTransitionError> {
        if can_transition(self.state, to) {
            self.state = to;
            self.updated_at = chrono::Utc::now().to_rfc3339();
            Ok(())
        } else {
            Err(WorkTransitionError::Invalid {
                from: self.state,
                to,
            })
        }
    }

    /// Reject: only from Submitted/Verified; bump revision;
    /// exceeds max_revisions → Rejected terminal.
    pub fn reject(&mut self, lead_id: &str, verdict: &str) -> Result<(), WorkTransitionError> {
        if !matches!(
            self.state,
            WorkItemState::Submitted | WorkItemState::Verified
        ) {
            return Err(WorkTransitionError::Invalid {
                from: self.state,
                to: WorkItemState::NeedsRevision,
            });
        }
        self.review_verdict = Some(verdict.to_string());
        self.lead_id = lead_id.to_string();
        self.revision += 1; // veylen lesson: bump revision để loop counter reset
        if self.revision > self.max_revisions {
            self.state = WorkItemState::Rejected;
        } else {
            self.state = WorkItemState::NeedsRevision;
        }
        self.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }

    /// Submit: attach evidence + calculate review_deadline (watchdog will escalate if
    /// lead is silent — NO auto-accept, lesson from veylen race AUTO-ACCEPT).
    pub fn submit(
        &mut self,
        evidence: EvidenceRef,
        review_timeout_secs: u64,
    ) -> Result<(), WorkTransitionError> {
        if !matches!(
            self.state,
            WorkItemState::InProgress | WorkItemState::NeedsRevision
        ) {
            return Err(WorkTransitionError::Invalid {
                from: self.state,
                to: WorkItemState::Submitted,
            });
        }
        self.evidence = Some(evidence);
        self.submitted_at = Some(chrono::Utc::now().to_rfc3339());
        self.review_deadline = Some(
            (chrono::Utc::now() + chrono::Duration::seconds(review_timeout_secs as i64))
                .to_rfc3339(),
        );
        self.state = WorkItemState::Submitted;
        self.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkTransitionError {
    #[error("invalid work transition from {from:?} to {to:?}")]
    Invalid {
        from: WorkItemState,
        to: WorkItemState,
    },
}

/// Closed enum: lead decision after review — exhaustive match ensures
/// all dispatch sites handle all 3 cases (maestro pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewVerdict {
    Accept,
    Reject,
    Escalate, // lead im lặng quá deadline → supervisor (watchdog)
}

/// Closed enum: verification lifecycle — exhaustive match required
/// for all code paths to handle all 6 states (maestro pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationStatus {
    Open,
    EvidenceReady,
    Verifying,
    Verified,
    Failed,
    Uncertain,
}

/// Verification profiles — predefined check sets, NOT arbitrary commands.
/// Prevents execution escape hatch (caller can't inject arbitrary CLI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerifyProfile {
    /// cargo fmt --check + cargo clippy + cargo test + cargo build
    Full,
    /// cargo fmt --check + cargo clippy + cargo test
    Check,
    /// cargo test only
    Test,
    /// cargo build only
    Build,
    /// Custom profile defined in project config (.anti_subagent/verify.toml)
    Named(String),
}

impl VerifyProfile {
    /// Returns the cargo commands to run for this profile.
    pub fn commands(&self) -> Vec<&'static str> {
        match self {
            VerifyProfile::Full => vec![
                "cargo fmt --check",
                "cargo clippy -- -D warnings",
                "cargo test",
                "cargo build",
            ],
            VerifyProfile::Check => vec![
                "cargo fmt --check",
                "cargo clippy -- -D warnings",
                "cargo test",
            ],
            VerifyProfile::Test => vec!["cargo test"],
            VerifyProfile::Build => vec!["cargo build"],
            VerifyProfile::Named(_) => vec![], // loaded from config at runtime
        }
    }
}

/// Result of a verification run — comprehensive evidence for audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub status: VerifyStatus,
    pub profile: VerifyProfile,
    pub test_output: Option<String>,
    pub test_exit_code: Option<i32>,
    pub build_output: Option<String>,
    pub build_exit_code: Option<i32>,
    pub diagnostics: Vec<String>,
    pub git_diff: Option<String>,
    pub git_sha: Option<String>,
    pub claims_verified: Vec<ClaimVerification>,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyStatus {
    Pass,
    Fail,
    Incomplete,
}

impl VerificationResult {
    pub fn new(profile: VerifyProfile) -> Self {
        Self {
            status: VerifyStatus::Incomplete,
            profile,
            test_output: None,
            test_exit_code: None,
            build_output: None,
            build_exit_code: None,
            diagnostics: Vec::new(),
            git_diff: None,
            git_sha: None,
            claims_verified: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Transition table — anything not listed is illegal.
pub fn can_transition(from: WorkItemState, to: WorkItemState) -> bool {
    use WorkItemState::*;
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        // Legacy transitions (backward compatible)
        (Pending, InProgress)
            | (InProgress, Submitted)
            | (InProgress, Rejected)
            | (Submitted, Verified)
            | (Submitted, NeedsRevision)
            | (Verified, Accepted)
            | (Verified, NeedsRevision)
            | (NeedsRevision, InProgress)
            | (NeedsRevision, Rejected)
        // Staged pipeline transitions
            | (Received, Explored)
            | (Explored, Planned)
            | (Planned, Executing)
            | (Executing, Executed)
            | (Executing, Failed)
            | (Executed, Verifying)
            | (Verifying, Verified)
            | (Verifying, Rejected)
            | (Verifying, Fixing)
            | (Fixing, Executing)
            | (Rejected, Exhausted)
            | (Fixing, Exhausted)
        // Cancellation from any non-terminal state
            | (Received, Cancelled)
            | (Explored, Cancelled)
            | (Planned, Cancelled)
            | (Executing, Cancelled)
            | (Executed, Cancelled)
            | (Verifying, Cancelled)
            | (Fixing, Cancelled)
            | (Pending, Cancelled)
            | (InProgress, Cancelled)
            | (Submitted, Cancelled)
            | (Verified, Cancelled)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_admits_evidence_then_verify_then_accept() {
        assert!(can_transition(
            WorkItemState::Pending,
            WorkItemState::InProgress
        ));
        assert!(can_transition(
            WorkItemState::InProgress,
            WorkItemState::Submitted
        ));
        assert!(can_transition(
            WorkItemState::Submitted,
            WorkItemState::Verified
        ));
        assert!(can_transition(
            WorkItemState::Verified,
            WorkItemState::Accepted
        ));
    }

    #[test]
    fn cannot_accept_without_verification() {
        assert!(!can_transition(
            WorkItemState::Submitted,
            WorkItemState::Accepted
        ));
        assert!(!can_transition(
            WorkItemState::Pending,
            WorkItemState::Accepted
        ));
    }

    #[test]
    fn reject_bumps_revision() {
        let mut w = WorkItem::new("t1".into(), "peer-1".into());
        w.transition(WorkItemState::InProgress).unwrap();
        w.transition(WorkItemState::Submitted).unwrap();
        w.reject("lead-1".into(), "missing tests".into()).unwrap();
        assert_eq!(w.state, WorkItemState::NeedsRevision);
        assert_eq!(w.revision, 2); // reject bump revision để group counter reset
    }

    #[test]
    fn terminal_states_are_closed() {
        assert!(WorkItemState::Accepted.is_terminal());
        assert!(WorkItemState::Rejected.is_terminal());
        assert!(!WorkItemState::Submitted.is_terminal());
    }

    #[test]
    fn verdict_roundtrip() {
        let v = ReviewVerdict::Accept;
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<ReviewVerdict>(&s).unwrap(), v);
    }

    #[test]
    fn verification_status_is_exhaustively_matched() {
        // If adding a new variant, this match must fail compile — that's the purpose
        fn describe(s: VerificationStatus) -> &'static str {
            match s {
                VerificationStatus::Open => "no evidence yet",
                VerificationStatus::EvidenceReady => "claim filed",
                VerificationStatus::Verifying => "checking sha",
                VerificationStatus::Verified => "matches artifact",
                VerificationStatus::Failed => "mismatch",
                VerificationStatus::Uncertain => "needs human",
            }
        }
        assert_eq!(describe(VerificationStatus::Verified), "matches artifact");
    }
}
