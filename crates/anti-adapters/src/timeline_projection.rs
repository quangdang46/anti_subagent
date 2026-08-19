//! Timeline projection (mirrors Paseo timeline-projection.ts).
//!
//! Two collapses:
//! 1. Tool lifecycle: merge entries with same callId into one entry with sourceSeqRanges.
//! 2. Assistant chunks: merge consecutive assistant_message chunks into one.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Simplified timeline item (enough for projection tests).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TimelineItem {
    Assistant { text: String },
    ToolCall { call_id: String, name: String },
    System(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineRow {
    pub seq: u64,
    pub timestamp: String,
    pub item: TimelineItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionEntry {
    pub seq_start: u64,
    pub seq_end: u64,
    pub item: TimelineItem,
    pub collapsed: Vec<String>,
    pub source_seqs: Vec<u64>,
}

pub fn collapse_tool_lifecycle(rows: &[TimelineRow]) -> Vec<ProjectionEntry> {
    let mut out: Vec<ProjectionEntry> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();

    for row in rows {
        let call_id = if let TimelineItem::ToolCall { call_id, .. } = &row.item {
            Some(call_id.clone())
        } else {
            None
        };

        if let Some(cid) = call_id {
            if let Some(&pos) = index.get(&cid) {
                // Merge into existing entry
                let e = &mut out[pos];
                e.seq_end = e.seq_end.max(row.seq);
                if !e.collapsed.contains(&"tool_lifecycle".to_string()) {
                    e.collapsed.push("tool_lifecycle".to_string());
                }
                e.source_seqs.push(row.seq);
                continue;
            }
            index.insert(cid, out.len());
        }

        out.push(ProjectionEntry {
            seq_start: row.seq,
            seq_end: row.seq,
            item: row.item.clone(),
            collapsed: vec![],
            source_seqs: vec![row.seq],
        });
    }
    out
}

pub fn merge_assistant_chunks(entries: Vec<ProjectionEntry>) -> Vec<ProjectionEntry> {
    if entries.is_empty() {
        return entries;
    }
    let mut out: Vec<ProjectionEntry> = vec![];
    for e in entries {
        if let Some(last) = out.last_mut() {
            let both_assistant = matches!(last.item, TimelineItem::Assistant { .. })
                && matches!(e.item, TimelineItem::Assistant { .. });
            let contiguous = last.seq_end + 1 == e.seq_start;
            if both_assistant && contiguous {
                let (prev_text, cur_text) = match (&last.item, &e.item) {
                    (TimelineItem::Assistant { text: a }, TimelineItem::Assistant { text: b }) => {
                        (a.clone(), b.clone())
                    }
                    _ => unreachable!(),
                };
                last.item = TimelineItem::Assistant {
                    text: format!("{prev_text}{cur_text}"),
                };
                last.seq_end = e.seq_end;
                if !last.collapsed.contains(&"assistant_merge".to_string()) {
                    last.collapsed.push("assistant_merge".to_string());
                }
                last.source_seqs.extend_from_slice(&e.source_seqs);
                continue;
            }
        }
        out.push(e);
    }
    out
}

/// Full projection: tool lifecycle collapse, then assistant merge.
pub fn project(rows: &[TimelineRow]) -> Vec<ProjectionEntry> {
    let step1 = collapse_tool_lifecycle(rows);
    merge_assistant_chunks(step1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_lifecycle_same_call_collapsed() {
        let rows = vec![
            TimelineRow {
                seq: 1,
                timestamp: "t".into(),
                item: TimelineItem::ToolCall { call_id: "c1".into(), name: "bash".into() },
            },
            TimelineRow {
                seq: 2,
                timestamp: "t".into(),
                item: TimelineItem::ToolCall { call_id: "c1".into(), name: "bash".into() },
            },
        ];
        let proj = project(&rows);
        assert_eq!(proj.len(), 1);
        assert!(proj[0].collapsed.contains(&"tool_lifecycle".to_string()));
    }

    #[test]
    fn assistant_chunks_merged() {
        let rows = vec![
            TimelineRow { seq: 1, timestamp: "t".into(), item: TimelineItem::Assistant { text: "hi ".into() } },
            TimelineRow { seq: 2, timestamp: "t".into(), item: TimelineItem::Assistant { text: "there".into() } },
        ];
        let proj = project(&rows);
        assert_eq!(proj.len(), 1);
        if let TimelineItem::Assistant { text } = &proj[0].item {
            assert_eq!(text, "hi there");
        } else {
            panic!("wrong item");
        }
    }

    #[test]
    fn non_contiguous_assistant_not_merged() {
        let rows = vec![
            TimelineRow { seq: 1, timestamp: "t".into(), item: TimelineItem::Assistant { text: "a".into() } },
            TimelineRow { seq: 3, timestamp: "t".into(), item: TimelineItem::Assistant { text: "b".into() } },
        ];
        let proj = project(&rows);
        assert_eq!(proj.len(), 2);
    }
}
