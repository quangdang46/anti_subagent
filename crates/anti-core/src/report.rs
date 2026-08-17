//! Report channel — Peer→anti status reporting.
//!
//! The ONLY way peers communicate back to the daemon. Peers call
//! `anti report --task <id> --status <status>` which sends a ReportTask
//! over the existing Unix socket IPC.
//!
//! Key invariant: NO peer_id in the report. The daemon resolves peer
//! identity from task ownership (work_items table). This prevents
//! impersonation — a peer cannot report on another peer's task.
//!
//! Code lives in Git. Messages live in the daemon. The peer's entire
//! vocabulary is: task, workspace, anti report.

use serde::{Deserialize, Serialize};

/// Status a peer can report back to the daemon.
///
/// Exhaustive enum — adding a new variant forces handling at every
/// dispatch site (maestro pattern). No catch-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReportStatus {
    /// Peer completed the task successfully. Requires commit SHA.
    Completed,
    /// Peer failed. Requires error message.
    Failed,
    /// Progress update — no state change on work item.
    Progress,
    /// Peer has a question for the Lead.
    Question,
}

impl ReportStatus {
    /// Parse from CLI string, rejecting unknown values.
    pub fn from_str(s: &str) -> Result<Self, ReportError> {
        match s.to_ascii_lowercase().as_str() {
            "completed" => Ok(ReportStatus::Completed),
            "failed" => Ok(ReportStatus::Failed),
            "progress" => Ok(ReportStatus::Progress),
            "question" => Ok(ReportStatus::Question),
            other => Err(ReportError::InvalidStatus(other.to_string())),
        }
    }

    /// Whether this status requires a git commit SHA.
    pub fn requires_commit(&self) -> bool {
        matches!(self, ReportStatus::Completed)
    }

    /// Whether this status triggers verification.
    pub fn triggers_verify(&self) -> bool {
        matches!(self, ReportStatus::Completed)
    }
}

/// Data payload a peer sends to report task status.
///
/// This is the domain struct. The IPC request (in ipc.rs) mirrors
/// these fields. No peer_id — daemon resolves from task ownership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportTask {
    pub task_id: String,
    pub status: ReportStatus,
    /// Git SHA — only required when status == Completed.
    pub commit: Option<String>,
    /// Human-readable text — used for Progress and Question.
    pub message: Option<String>,
    /// Error details — only relevant when status == Failed.
    pub error: Option<String>,
}

/// Errors from report handling.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ReportError {
    #[error("invalid status string: '{0}'")]
    InvalidStatus(String),

    #[error("task_id must not be empty")]
    InvalidTaskId(String),

    #[error("work item '{0}' not found")]
    TaskNotFound(String),

    #[error("work item '{0}' has no assigned peer")]
    TaskNotAssigned(String),

    #[error("commit SHA is required when status is Completed")]
    CommitRequired,

    #[error("commit '{0}' not found in workspace")]
    CommitNotFound(String),

    #[error("verification failed: {0}")]
    VerifyFailed(String),

    #[error("state transition failed: {0}")]
    TransitionFailed(String),

    #[error("store error: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_status_from_str_valid() {
        assert_eq!(
            ReportStatus::from_str("completed").unwrap(),
            ReportStatus::Completed
        );
        assert_eq!(
            ReportStatus::from_str("FAILED").unwrap(),
            ReportStatus::Failed
        );
        assert_eq!(
            ReportStatus::from_str("Progress").unwrap(),
            ReportStatus::Progress
        );
        assert_eq!(
            ReportStatus::from_str("question").unwrap(),
            ReportStatus::Question
        );
    }

    #[test]
    fn report_status_from_str_invalid() {
        assert!(ReportStatus::from_str("unknown").is_err());
        assert!(ReportStatus::from_str("").is_err());
        assert!(ReportStatus::from_str("DONE").is_err());
    }

    #[test]
    fn completed_requires_commit() {
        assert!(ReportStatus::Completed.requires_commit());
        assert!(!ReportStatus::Failed.requires_commit());
        assert!(!ReportStatus::Progress.requires_commit());
        assert!(!ReportStatus::Question.requires_commit());
    }

    #[test]
    fn completed_triggers_verify() {
        assert!(ReportStatus::Completed.triggers_verify());
        assert!(!ReportStatus::Failed.triggers_verify());
        assert!(!ReportStatus::Progress.triggers_verify());
        assert!(!ReportStatus::Question.triggers_verify());
    }

    #[test]
    fn report_task_roundtrip() {
        let task = ReportTask {
            task_id: "42".into(),
            status: ReportStatus::Completed,
            commit: Some("abc123".into()),
            message: None,
            error: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        let parsed: ReportTask = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, "42");
        assert_eq!(parsed.status, ReportStatus::Completed);
        assert_eq!(parsed.commit.as_deref(), Some("abc123"));
    }

    #[test]
    fn report_status_exhaustive_match() {
        // Adding a new variant without updating this match = compile error.
        // That's the purpose — forces all dispatch sites to handle new statuses.
        fn describe(s: ReportStatus) -> &'static str {
            match s {
                ReportStatus::Completed => "done",
                ReportStatus::Failed => "error",
                ReportStatus::Progress => "update",
                ReportStatus::Question => "help needed",
            }
        }
        assert_eq!(describe(ReportStatus::Completed), "done");
        assert_eq!(describe(ReportStatus::Question), "help needed");
    }

    #[test]
    fn report_task_with_all_fields() {
        let task = ReportTask {
            task_id: "7".into(),
            status: ReportStatus::Failed,
            commit: None,
            message: Some("halfway done".into()),
            error: Some("compile error".into()),
        };
        let json = serde_json::to_string(&task).unwrap();
        let parsed: ReportTask = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, ReportStatus::Failed);
        assert_eq!(parsed.message.as_deref(), Some("halfway done"));
        assert_eq!(parsed.error.as_deref(), Some("compile error"));
        assert!(parsed.commit.is_none());
    }
}
