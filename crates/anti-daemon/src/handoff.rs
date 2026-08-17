//! Lead handoff — context degradation recovery.
//!
//! When a Lead's context degrades after ~5-7 compactions, an experience
//! handoff transfers leadership to a new Lead with preserved lessons.

use serde::{Deserialize, Serialize};

/// Reason for lead handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HandoffReason {
    /// Context degradation after too many compactions
    ContextDegradation { compactions: u32 },
    /// Manual handoff command
    ManualHandoff,
    /// Supervisor decision (on-demand governance)
    SupervisorDecision,
}

/// Context to transfer during handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffContext {
    /// Tasks currently in progress
    pub active_tasks: Vec<TaskSummary>,
    /// Reviews awaiting decisions
    pub pending_reviews: Vec<ReviewSummary>,
    /// Rationale for decisions made
    pub decisions: Vec<Decision>,
    /// Unresolved issues
    pub open_questions: Vec<Question>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub state: String,
    pub assigned_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub id: String,
    pub task_id: String,
    pub status: String,
    pub deadline: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub description: String,
    pub rationale: String,
    pub alternatives_considered: Vec<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub blocking: bool,
    pub context: Option<String>,
}

/// Lesson learned from experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub id: String,
    pub description: String,
    pub category: LessonCategory,
    pub confidence: f32, // 0.0 - 1.0
    pub learned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LessonCategory {
    AntiPattern,
    BestPractice,
    ToolInsight,
    ProcessImprovement,
}

/// Complete handoff artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeadHandoff {
    pub from_lead: String,
    pub to_lead: String,
    pub reason: HandoffReason,
    pub context: HandoffContext,
    pub lessons: Vec<Lesson>,
    pub created_at: String,
}

impl LeadHandoff {
    pub fn new(
        from_lead: String,
        to_lead: String,
        reason: HandoffReason,
        context: HandoffContext,
        lessons: Vec<Lesson>,
    ) -> Self {
        Self {
            from_lead,
            to_lead,
            reason,
            context,
            lessons,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Serialize to JSON for storage/transfer.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_roundtrip() {
        let handoff = LeadHandoff::new(
            "lead-1".into(),
            "lead-2".into(),
            HandoffReason::ContextDegradation { compactions: 7 },
            HandoffContext {
                active_tasks: vec![],
                pending_reviews: vec![],
                decisions: vec![],
                open_questions: vec![],
            },
            vec![],
        );

        let json = handoff.to_json().unwrap();
        let restored = LeadHandoff::from_json(&json).unwrap();

        assert_eq!(restored.from_lead, "lead-1");
        assert_eq!(restored.to_lead, "lead-2");
        assert!(matches!(
            restored.reason,
            HandoffReason::ContextDegradation { compactions: 7 }
        ));
    }

    #[test]
    fn lesson_categories() {
        let lesson = Lesson {
            id: "l1".into(),
            description: "Avoid polling".into(),
            category: LessonCategory::AntiPattern,
            confidence: 0.9,
            learned_at: chrono::Utc::now().to_rfc3339(),
        };

        let json = serde_json::to_string(&lesson).unwrap();
        let restored: Lesson = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored.category, LessonCategory::AntiPattern));
    }
}
