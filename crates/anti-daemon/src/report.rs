//! ReportHandler — core Peer→anti report processing logic.
//!
//! When a peer calls `anti report --task 42 --status completed --commit abc123`,
//! this module validates, verifies, persists, and notifies.
//!
//! Key invariant: "anti does not trust peer messages." Every claim is
//! independently verified — git show for commit existence, VerifyProfile
//! for code correctness.

use crate::store::{Store, StoreError};
use anti_core::events::EventType;
use anti_core::report::{ReportError, ReportStatus, ReportTask};
use anti_core::work::WorkItemState;
use sha2::{Digest, Sha256};

/// Response from handling a report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReportResponse {
    pub task_id: String,
    pub new_state: WorkItemState,
    pub message: String,
}

/// Handle a peer's report. All validation happens here.
///
/// Flow:
/// 1. Parse + validate inputs
/// 2. Look up work item
/// 3. Verify commit if completed
/// 4. Transition work state
/// 5. Emit event
/// 6. Return response
pub fn handle_report(
    store: &mut Store,
    task_id: &str,
    status: &str,
    commit: Option<&str>,
    error: Option<&str>,
    message: Option<&str>,
) -> Result<ReportResponse, ReportError> {
    // 1. Parse status
    let report_status = ReportStatus::from_str(status)?;

    // 2. Validate task_id
    if task_id.is_empty() {
        return Err(ReportError::InvalidTaskId(task_id.to_string()));
    }

    // 3. Look up work item
    let work_item = store
        .get_work_item(task_id)
        .map_err(|e| ReportError::Store(e.to_string()))?
        .ok_or_else(|| ReportError::TaskNotFound(task_id.to_string()))?;

    // 4. Check task is assigned to a peer
    if work_item.peer_id.is_empty() {
        return Err(ReportError::TaskNotAssigned(task_id.to_string()));
    }

    // 5. Dispatch based on status
    match report_status {
        ReportStatus::Completed => handle_completed(store, &work_item, commit, task_id),
        ReportStatus::Failed => handle_failed(store, &work_item, error, task_id),
        ReportStatus::Progress | ReportStatus::Question => {
            handle_progress(store, &work_item, report_status, message, task_id)
        }
    }
}

/// Handle completed status: verify commit, transition to Submitted.
fn handle_completed(
    store: &mut Store,
    work_item: &anti_core::work::WorkItem,
    commit: Option<&str>,
    task_id: &str,
) -> Result<ReportResponse, ReportError> {
    let commit_sha = commit.ok_or(ReportError::CommitRequired)?;

    // Resolve workspace path from the peer's agent record
    let workspace_path = resolve_workspace_path(store, &work_item.peer_id)?;

    // Verify commit exists in workspace
    verify_commit_in_workspace(&workspace_path, commit_sha)?;

    // Compute evidence SHA from git diff
    let evidence_sha = compute_commit_diff_sha(&workspace_path, commit_sha)
        .unwrap_or_else(|_| "unknown".to_string());

    // Build evidence ref (clone sha for event — EvidenceRef takes ownership)
    let evidence_sha_for_event = evidence_sha.clone();
    let evidence = anti_core::work::EvidenceRef {
        sha256: evidence_sha,
        artifact_path: workspace_path.to_string_lossy().to_string(),
        produced_at: chrono::Utc::now().to_rfc3339(),
    };

    // Store evidence in work item
    let mut updated_item = work_item.clone();
    updated_item.evidence = Some(evidence);
    updated_item.submitted_at = Some(chrono::Utc::now().to_rfc3339());
    updated_item.review_deadline =
        Some((chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339());
    store
        .insert_work_item(&updated_item)
        .map_err(|e| ReportError::Store(e.to_string()))?;

    // Transition: InProgress → Submitted (or NeedsRevision → Submitted if re-work)
    let from = work_item.state;
    if matches!(
        from,
        WorkItemState::InProgress | WorkItemState::NeedsRevision
    ) {
        store
            .update_work_state(task_id, from, WorkItemState::Submitted)
            .map_err(|e| ReportError::TransitionFailed(e.to_string()))?;
    }

    // Emit event
    let _ = store.append_event(
        &work_item.peer_id,
        EventType::WorkSubmitted,
        serde_json::json!({
            "task_id": task_id,
            "commit": commit_sha,
            "evidence_sha": &evidence_sha_for_event,
        }),
    );

    Ok(ReportResponse {
        task_id: task_id.to_string(),
        new_state: WorkItemState::Submitted,
        message: format!("commit {commit_sha} verified, work submitted for review"),
    })
}

/// Handle failed status: transition to NeedsRevision or Failed (exhausted).
fn handle_failed(
    store: &mut Store,
    work_item: &anti_core::work::WorkItem,
    error: Option<&str>,
    task_id: &str,
) -> Result<ReportResponse, ReportError> {
    let error_msg = error.unwrap_or("unknown error");

    // Determine target state based on revision count
    let target = if work_item.revision >= work_item.max_revisions {
        WorkItemState::Failed
    } else {
        WorkItemState::NeedsRevision
    };

    // Transition
    let from = work_item.state;
    if can_transition_for_failure(from) {
        store
            .update_work_state(task_id, from, target)
            .map_err(|e| ReportError::TransitionFailed(e.to_string()))?;
    }

    // Emit event
    let event_type = if target == WorkItemState::Failed {
        EventType::TaskFailed
    } else {
        EventType::AgentProgress
    };
    let _ = store.append_event(
        &work_item.peer_id,
        event_type,
        serde_json::json!({
            "task_id": task_id,
            "error": error_msg,
            "new_state": format!("{:?}", target),
            "revision": work_item.revision,
        }),
    );

    Ok(ReportResponse {
        task_id: task_id.to_string(),
        new_state: target,
        message: format!("failed: {error_msg}"),
    })
}

/// Handle progress/question status: no state change, just emit event.
fn handle_progress(
    store: &mut Store,
    work_item: &anti_core::work::WorkItem,
    status: ReportStatus,
    message: Option<&str>,
    task_id: &str,
) -> Result<ReportResponse, ReportError> {
    let msg = message.unwrap_or("");
    let current_state = work_item.state;

    let _ = store.append_event(
        &work_item.peer_id,
        EventType::AgentProgress,
        serde_json::json!({
            "task_id": task_id,
            "report_status": format!("{:?}", status),
            "message": msg,
        }),
    );

    let description = match status {
        ReportStatus::Progress => format!("progress: {msg}"),
        ReportStatus::Question => format!("question: {msg}"),
        _ => unreachable!(),
    };

    Ok(ReportResponse {
        task_id: task_id.to_string(),
        new_state: current_state,
        message: description,
    })
}

/// Resolve workspace path from a peer's agent record.
fn resolve_workspace_path(store: &Store, peer_id: &str) -> Result<std::path::PathBuf, ReportError> {
    let agent = store
        .get_agent(peer_id)
        .map_err(|e| ReportError::Store(e.to_string()))?
        .ok_or_else(|| ReportError::Store(format!("agent '{peer_id}' not found")))?;

    match agent.workspace {
        Some(ws) => Ok(std::path::PathBuf::from(ws.path)),
        None => Err(ReportError::Store(format!(
            "peer '{peer_id}' has no workspace assigned"
        ))),
    }
}

/// Verify that a commit exists in the workspace using git show.
fn verify_commit_in_workspace(
    workspace: &std::path::Path,
    commit_sha: &str,
) -> Result<(), ReportError> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &workspace.to_string_lossy(),
            "show",
            "--no-patch",
            commit_sha,
        ])
        .output()
        .map_err(|e| ReportError::Store(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        return Err(ReportError::CommitNotFound(commit_sha.to_string()));
    }
    Ok(())
}

/// Compute SHA-256 of the commit diff for evidence.
fn compute_commit_diff_sha(
    workspace: &std::path::Path,
    commit_sha: &str,
) -> Result<String, ReportError> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &workspace.to_string_lossy(),
            "diff",
            &format!("{commit_sha}^"),
            commit_sha,
        ])
        .output()
        .map_err(|e| ReportError::Store(format!("failed to run git diff: {e}")))?;

    if !output.status.success() {
        // First commit — diff against empty tree
        let output = std::process::Command::new("git")
            .args([
                "-C",
                &workspace.to_string_lossy(),
                "diff",
                "--stat",
                &format!("{commit_sha}"),
            ])
            .output()
            .map_err(|e| ReportError::Store(format!("failed to run git diff: {e}")))?;
        return Ok(format!("{:x}", sha2::Sha256::digest(&output.stdout)));
    }

    Ok(format!("{:x}", sha2::Sha256::digest(&output.stdout)))
}

/// Check if a work_item state can transition to a failure state.
fn can_transition_for_failure(from: WorkItemState) -> bool {
    matches!(
        from,
        WorkItemState::InProgress
            | WorkItemState::Submitted
            | WorkItemState::NeedsRevision
            | WorkItemState::Executing
            | WorkItemState::Verifying
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_transition_for_failure_covers_common_states() {
        assert!(can_transition_for_failure(WorkItemState::InProgress));
        assert!(can_transition_for_failure(WorkItemState::Submitted));
        assert!(can_transition_for_failure(WorkItemState::NeedsRevision));
        assert!(can_transition_for_failure(WorkItemState::Executing));
        assert!(can_transition_for_failure(WorkItemState::Verifying));
        // Terminal states should not transition to failure
        assert!(!can_transition_for_failure(WorkItemState::Accepted));
        assert!(!can_transition_for_failure(WorkItemState::Failed));
        assert!(!can_transition_for_failure(WorkItemState::Cancelled));
    }

    #[test]
    fn handle_report_rejects_empty_task_id() {
        let dir =
            std::env::temp_dir().join(format!("anti-report-test-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let result = handle_report(&mut store, "", "completed", None, None, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ReportError::InvalidTaskId(id) => assert_eq!(id, ""),
            other => panic!("expected InvalidTaskId, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_report_rejects_task_not_found() {
        let dir =
            std::env::temp_dir().join(format!("anti-report-test-notfound-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let result = handle_report(
            &mut store,
            "nonexistent",
            "completed",
            Some("abc"),
            None,
            None,
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            ReportError::TaskNotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("expected TaskNotFound, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_report_rejects_unassigned_task() {
        let dir = std::env::temp_dir().join(format!(
            "anti-report-test-unassigned-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        // Insert work item with empty peer_id
        let mut w = anti_core::work::WorkItem::new("w1".into(), "".into());
        w.transition(WorkItemState::InProgress).unwrap();
        store.insert_work_item(&w).unwrap();

        let result = handle_report(&mut store, "w1", "completed", Some("abc"), None, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ReportError::TaskNotAssigned(id) => assert_eq!(id, "w1"),
            other => panic!("expected TaskNotAssigned, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_report_rejects_invalid_status() {
        let dir = std::env::temp_dir().join(format!(
            "anti-report-test-invalidstatus-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let result = handle_report(&mut store, "w1", "invalid", None, None, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ReportError::InvalidStatus(s) => assert_eq!(s, "invalid"),
            other => panic!("expected InvalidStatus, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_report_rejects_missing_commit_for_completed() {
        let dir =
            std::env::temp_dir().join(format!("anti-report-test-nocommit-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let mut w = anti_core::work::WorkItem::new("w1".into(), "peer-1".into());
        w.transition(WorkItemState::InProgress).unwrap();
        store.insert_work_item(&w).unwrap();

        let result = handle_report(&mut store, "w1", "completed", None, None, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ReportError::CommitRequired => {}
            other => panic!("expected CommitRequired, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_report_does_not_change_state() {
        let dir =
            std::env::temp_dir().join(format!("anti-report-test-progress-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let mut w = anti_core::work::WorkItem::new("w1".into(), "peer-1".into());
        w.transition(WorkItemState::InProgress).unwrap();
        store.insert_work_item(&w).unwrap();

        let result = handle_report(
            &mut store,
            "w1",
            "progress",
            None,
            None,
            Some("50% done".into()),
        );
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp.new_state, WorkItemState::InProgress);
        assert!(resp.message.contains("50% done"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn question_report_does_not_change_state() {
        let dir =
            std::env::temp_dir().join(format!("anti-report-test-question-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = Store::open(&dir).unwrap();

        let mut w = anti_core::work::WorkItem::new("w1".into(), "peer-1".into());
        w.transition(WorkItemState::InProgress).unwrap();
        store.insert_work_item(&w).unwrap();

        let result = handle_report(
            &mut store,
            "w1",
            "question",
            None,
            None,
            Some("which DB?".into()),
        );
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp.new_state, WorkItemState::InProgress);
        assert!(resp.message.contains("which DB?"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
