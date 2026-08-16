//! WorkItem — task lifecycle của SLP (SETTLED ≠ VERIFIED ≠ ACCEPTED).
//!
//! Bài học irina: "done" là claim, không phải sự thật; acceptance chỉ qua
//! evidence + verification + decision.
//! Bài học veylen: reject phải bump revision (group counter reset) và
//! lead im lặng = phải có watchdog.

use serde::{Deserialize, Serialize};

/// Lifecycle states cho một work item — phân biệt SETTLED (submitted),
/// VERIFIED (evidence checked), ACCEPTED (lead approved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkItemState {
    Pending,       // lead giao, peer chưa nhận
    InProgress,    // peer claim và đang làm
    Submitted,     // peer submit + evidence — SETTLED (claim)
    Verified,      // verifier xác nhận evidence khớp — VERIFIED
    Accepted,      // lead accept — ACCEPTED (chỉ từ Verified)
    NeedsRevision, // reject → peer sửa lại; revision bump
    Rejected,      // terminal reject (vượt max_revisions hoặc lead hủy)
}

impl WorkItemState {
    pub fn is_terminal(self) -> bool {
        matches!(self, WorkItemState::Accepted | WorkItemState::Rejected)
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

/// Bảng chuyển trạng thái — mọi thứ không liệt kê = bất hợp pháp.
pub fn can_transition(from: WorkItemState, to: WorkItemState) -> bool {
    use WorkItemState::*;
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        (Pending, InProgress)
            | (InProgress, Submitted)
            | (InProgress, Rejected)           // lead hủy giữa chừng
            | (Submitted, Verified)
            | (Submitted, NeedsRevision)       // reject round 1
            | (Verified, Accepted)
            | (Verified, NeedsRevision)        // reject sau verify
            | (NeedsRevision, InProgress)      // peer sửa lại
            | (NeedsRevision, Rejected) // quá max_revisions
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
}
