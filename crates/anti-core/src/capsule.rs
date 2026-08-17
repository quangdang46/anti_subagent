//! Bounded capsule (irina project-state.ts:45) — each agent sees ≤64KB
//! state view. Truncates at most important parts, never exceeds budget.

use crate::work::WorkItem;

pub const CAPSULE_BUDGET: usize = 64 * 1024;

pub struct CapsuleInput {
    pub peer_id: String,
    pub task: String,
    pub work_items: Vec<WorkItem>,
    pub recent_events: Vec<String>,
}

pub fn render_capsule(input: &CapsuleInput) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# ANTI CAPSULE — peer {}\n## TASK\n{}\n## WORK ITEMS\n",
        input.peer_id, input.task
    ));
    for w in &input.work_items {
        out.push_str(&format!(
            "- {} [{}] rev={} evidence={}\n",
            w.id,
            format!("{:?}", w.state),
            w.revision,
            w.evidence.as_ref().map(|e| &e.sha256[..8.min(e.sha256.len())]).unwrap_or("-"),
        ));
    }
    out.push_str("## RECENT EVENTS\n");
    for e in &input.recent_events {
        out.push_str(e);
        out.push('\n');
        if out.len() > CAPSULE_BUDGET {
            out.truncate(CAPSULE_BUDGET);
            out.push_str("\n...[truncated]\n");
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::{EvidenceRef, WorkItem, WorkItemState};

    #[test]
    fn capsule_respects_budget() {
        let cap = render_capsule(&CapsuleInput {
            peer_id: "peer-1".into(),
            task: "implement foo".into(),
            work_items: vec![WorkItem::new("w1".into(), "peer-1".into())],
            recent_events: vec!["event: x".repeat(1000)],
        });
        assert!(cap.len() <= 64 * 1024, "capsule {} bytes > 64KB", cap.len());
        assert!(cap.contains("implement foo"));
    }

    #[test]
    fn capsule_shows_evidence_prefix() {
        let mut w = WorkItem::new("w2".into(), "peer-2".into());
        w.transition(WorkItemState::InProgress).unwrap();
        w.evidence = Some(EvidenceRef {
            sha256: "abcdef1234567890".into(),
            artifact_path: "/tmp/out.txt".into(),
            produced_at: "2026-01-01T00:00:00Z".into(),
        });
        let cap = render_capsule(&CapsuleInput {
            peer_id: "peer-2".into(),
            task: "review PR".into(),
            work_items: vec![w],
            recent_events: vec![],
        });
        assert!(cap.contains("abcdef12"));
    }
}
