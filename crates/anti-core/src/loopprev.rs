//! Loop prevention (port veylen SubscriptionEvaluator.ts:308-423).
//! Sliding window 1h, group theo [task_node_id, revision], trigger > 3,
//! hysteresis reset when count ≤ 1, cooldown 10 minutes after trigger.

use std::collections::HashMap;

pub const WINDOW_SECS: i64 = 3600;
pub const TRIGGER_THRESHOLD: usize = 3;
pub const COOLDOWN_SECS: i64 = 600;

#[derive(Debug, Default)]
pub struct LoopPrevention {
    /// key = (task_node_id, revision) → timestamps reject
    rejects: HashMap<(String, u32), Vec<i64>>,
    /// key → timestamp of last escalation trigger (cooldown)
    last_trigger: HashMap<(String, u32), i64>,
}

impl LoopPrevention {
    pub fn record_reject(&mut self, task_node_id: &str, revision: u32, at_rfc3339: String) {
        let now = chrono::DateTime::parse_from_rfc3339(&at_rfc3339)
            .map(|d| d.timestamp())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp());
        let key = (task_node_id.to_string(), revision);
        self.rejects.entry(key.clone()).or_default().push(now);
        // prune window
        if let Some(v) = self.rejects.get_mut(&key) {
            v.retain(|t| now - *t <= WINDOW_SECS);
        }
    }

    pub fn count_in_window(&self, task_node_id: &str, revision: u32) -> usize {
        self.rejects
            .get(&(task_node_id.to_string(), revision))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Trigger when count > threshold AND outside cooldown.
    /// Hysteresis: if count drops ≤ 1 (window slides), cooldown expires → allow re-trigger.
    pub fn should_escalate(&mut self, task_node_id: &str, revision: u32) -> bool {
        let key = (task_node_id.to_string(), revision);
        let count = self.count_in_window(task_node_id, revision);
        if count <= TRIGGER_THRESHOLD {
            return false;
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(last) = self.last_trigger.get(&key) {
            if now - *last < COOLDOWN_SECS {
                return false; // cooldown
            }
        }
        self.last_trigger.insert(key, now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(secs_ago: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(secs_ago)).to_rfc3339()
    }

    #[test]
    fn reject_count_groups_by_task_and_revision() {
        let mut e = LoopPrevention::default();
        e.record_reject("task-1", 1, ts(60));
        e.record_reject("task-1", 1, ts(50));
        assert!(!e.should_escalate("task-1", 1)); // 2 < 3
        e.record_reject("task-1", 1, ts(40));
        e.record_reject("task-1", 1, ts(30));
        assert!(e.should_escalate("task-1", 1)); // 4 > 3
    }

    #[test]
    fn revision_bump_resets_group() {
        let mut e = LoopPrevention::default();
        for i in 0..5 {
            e.record_reject("task-1", 1, ts(60 - i * 10));
        }
        assert!(e.should_escalate("task-1", 1));
        // peer fixes and resubmits → revision 2 → new group, counter clean
        assert!(!e.should_escalate("task-1", 2));
    }

    #[test]
    fn hysteresis_resets_when_quiet() {
        let mut e = LoopPrevention::default();
        e.record_reject("task-1", 1, ts(4000)); // ngoài window 1h
        assert!(!e.should_escalate("task-1", 1)); // 0 trong window → reset
        e.record_reject("task-1", 1, ts(10));
        assert_eq!(e.count_in_window("task-1", 1), 1);
    }

    #[test]
    fn cooldown_blocks_retrigger() {
        let mut e = LoopPrevention::default();
        for i in 0..6 {
            e.record_reject("task-1", 1, ts(100 - i * 5));
        }
        assert!(e.should_escalate("task-1", 1));
        assert!(!e.should_escalate("task-1", 1)); // trong cooldown 10p
    }
}
