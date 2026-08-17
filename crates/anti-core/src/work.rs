//! WorkItem — task lifecycle của SLP (SETTLED ≠ VERIFIED ≠ ACCEPTED).
//!
//! Bài học irina: "done" là claim, không phải sự thật; acceptance chỉ qua
//! evidence + verification + decision.
//! Bài học veylen: reject phải bump revision (group counter reset) và
//! lead im lặng = phải có watchdog.

use serde::{Deserialize, Serialize};

/// Lifecycle states cho một work item — staged pipeline.
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

/// Tham chiếu evidence — sha-256 hex của artifact (file/đầu ra).
/// "claim phải khớp evidence thật"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// sha-256 hex của artifact — claim phải khớp evidence thật
    pub sha256: String,
    pub artifact_path: String,
    pub produced_at: String,
}

/// WorkItem — đơn vị công việc lead giao peer.
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

    /// Reject: chỉ được từ Submitted/Verified; bump revision;
    /// quá max_revisions → Rejected terminal.
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

    /// Submit: gắn evidence + tính review_deadline (watchdog sẽ escalate nếu
    /// lead im lặng — KHÔNG auto-accept, bài học veylen race AUTO-ACCEPT).
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

/// Closed enum: lead decision sau khi review — exhaustive match đảm bảo
/// mọi dispatch site xử lý đủ 3 trường hợp (maestro pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewVerdict {
    Accept,
    Reject,
    Escalate, // lead im lặng quá deadline → supervisor (watchdog)
}

/// Closed enum: verification lifecycle — exhaustive match bắt buộc
/// mọi code path xử lý đủ 6 trạng thái (maestro pattern).
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

/// Bảng chuyển trạng thái — mọi thứ không liệt kê = bất hợp pháp.
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
        // Nếu thêm variant mới, match này phải fail compile — đó là mục đích
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
