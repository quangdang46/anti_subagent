//! Persistence: SQLite agent registry + append-only JSONL event log (plan §27-28).
//!
//! SQLite (WAL, busy_timeout, synchronous=NORMAL — slb pattern) is the
//! authoritative state. Events are written to JSONL first so the sequence is
//! durable and survives daemon restarts (deliberate divergence from herdr).

use anti_core::events::{Event, EventType};
use anti_core::model::{AgentRecord, AgentStatus};
use std::path::Path;

pub struct Store {
    conn: rusqlite::Connection,
    event_seq: u64,
    event_file: std::fs::File,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("event serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("state machine rejected transition: {0}")]
    Transition(#[from] anti_core::statemachine::TransitionError),
    #[error("work item transition rejected: {0}")]
    WorkTransition(#[from] anti_core::work::WorkTransitionError),
}

impl Store {
    pub fn open(state_dir: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(state_dir)?;
        let db_path = state_dir.join("state.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", "5000")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                role TEXT NOT NULL,
                disposition TEXT,
                harness TEXT NOT NULL,
                parent_id TEXT,
                pid INTEGER,
                workspace_lease_id TEXT,
                workspace_path TEXT,
                task_path TEXT,
                status TEXT NOT NULL,
                restart_count INTEGER NOT NULL DEFAULT 0,
                spawn_gen INTEGER NOT NULL DEFAULT 0,
                last_state_change_seq INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                type TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS config (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS work_items (
                id TEXT PRIMARY KEY,
                task_node_id TEXT NOT NULL DEFAULT '',
                peer_id TEXT NOT NULL,
                lead_id TEXT NOT NULL DEFAULT '',
                state TEXT NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1,
                max_revisions INTEGER NOT NULL DEFAULT 3,
                evidence_sha256 TEXT,
                evidence_path TEXT,
                evidence_at TEXT,
                review_verdict TEXT,
                submitted_at TEXT,
                review_deadline TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;

        // Seed event sequence from SQLite so restart preserves ordering.
        let seq: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
        let events_dir = state_dir.join("events");
        std::fs::create_dir_all(&events_dir)?;
        let event_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_dir.join("events.jsonl"))?;

        Ok(Self {
            conn,
            event_seq: seq as u64,
            event_file,
        })
    }

    // ---- agents ----

    pub fn insert_agent(&self, rec: &AgentRecord) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO agents (id, role, disposition, harness, parent_id, pid,
                 workspace_lease_id, workspace_path, task_path, status,
                 restart_count, spawn_gen, last_state_change_seq, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                rec.id,
                format!("{:?}", rec.role).to_lowercase(),
                rec.disposition.map(|d| format!("{:?}", d).to_lowercase()),
                format!("{:?}", rec.harness).to_lowercase(),
                rec.parent_id,
                rec.pid,
                rec.workspace.as_ref().map(|w| w.lease_id.clone()),
                rec.workspace.as_ref().map(|w| w.path.clone()),
                rec.task_path,
                format!("{:?}", rec.status),
                rec.restart_count,
                rec.spawn_gen,
                rec.last_state_change_seq,
                rec.created_at,
                rec.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<AgentRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, role, disposition, harness, parent_id, pid, workspace_lease_id,
                 workspace_path, task_path, status, restart_count, spawn_gen,
                 last_state_change_seq, created_at, updated_at FROM agents WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(AgentRecord {
                id: r.get(0)?,
                role: parse_role(&r.get::<_, String>(1)?),
                disposition: r.get::<_, Option<String>>(2)?.map(|d| parse_disposition(&d)),
                harness: parse_harness(&r.get::<_, String>(3)?),
                parent_id: r.get(4)?,
                pid: r.get(5)?,
                workspace: {
                    let lease_id: Option<String> = r.get(6)?;
                    let path: Option<String> = r.get(7)?;
                    match (lease_id, path) {
                        (Some(lease_id), Some(path)) => Some(anti_core::model::WorkspaceLease {
                            lease_id,
                            path: path.into(),
                            holder: String::new(),
                            generation: 0,
                        }),
                        _ => None,
                    }
                },
                task_path: r.get(8)?,
                status: parse_status(&r.get::<_, String>(9)?),
                restart_count: r.get(10)?,
                spawn_gen: r.get(11)?,
                last_state_change_seq: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?;
        rows.next().transpose().map_err(StoreError::Sqlite)
    }

    pub fn list_agents(&self) -> Result<Vec<AgentRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, role, disposition, harness, parent_id, pid, workspace_lease_id,
                 workspace_path, task_path, status, restart_count, spawn_gen,
                 last_state_change_seq, created_at, updated_at FROM agents")?;
        let rows = stmt.query_map([], |r| {
            Ok(AgentRecord {
                id: r.get(0)?,
                role: parse_role(&r.get::<_, String>(1)?),
                disposition: r.get::<_, Option<String>>(2)?.map(|d| parse_disposition(&d)),
                harness: parse_harness(&r.get::<_, String>(3)?),
                parent_id: r.get(4)?,
                pid: r.get(5)?,
                workspace: {
                    let lease_id: Option<String> = r.get(6)?;
                    let path: Option<String> = r.get(7)?;
                    match (lease_id, path) {
                        (Some(lease_id), Some(path)) => Some(anti_core::model::WorkspaceLease {
                            lease_id,
                            path: path.into(),
                            holder: String::new(),
                            generation: 0,
                        }),
                        _ => None,
                    }
                },
                task_path: r.get(8)?,
                status: parse_status(&r.get::<_, String>(9)?),
                restart_count: r.get(10)?,
                spawn_gen: r.get(11)?,
                last_state_change_seq: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::Sqlite)
    }

    /// Optimistic-lock transition (slb pattern): fails with zero rows if the
    /// expected status does not match, so two concurrent spawns can never
    /// both claim the same agent.
    pub fn transition(
        &self,
        id: &str,
        from: AgentStatus,
        to: AgentStatus,
    ) -> Result<(), StoreError> {
        anti_core::statemachine::check_transition(from, to)?;
        let changed = self.conn.execute(
            "UPDATE agents SET status = ?2, updated_at = datetime('now') WHERE id = ?1 AND status = ?3",
            rusqlite::params![id, format!("{:?}", to), format!("{:?}", from)],
        )?;
        if changed == 0 {
            return Err(StoreError::Transition(
                anti_core::statemachine::TransitionError::InvalidTransition { from, to },
            ));
        }
        Ok(())
    }

    pub fn update_status(&self, id: &str, status: AgentStatus) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE agents SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, format!("{:?}", status)],
        )?;
        if changed == 0 {
            return Err(StoreError::Transition(
                anti_core::statemachine::TransitionError::InvalidTransition {
                    from: AgentStatus::Created,
                    to: status,
                },
            ));
        }
        Ok(())
    }

    pub fn attach_pid(&self, id: &str, pid: u32) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE agents SET pid = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, pid],
        )?;
        Ok(())
    }

    pub fn clear_workspace(&self, id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE agents SET workspace_lease_id = NULL, workspace_path = NULL WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    pub fn set_workspace(&self, id: &str, lease_id: &str, path: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE agents SET workspace_lease_id = ?2, workspace_path = ?3, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, lease_id, path],
        )?;
        Ok(())
    }

    pub fn inc_restart(&self, id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE agents SET restart_count = restart_count + 1, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    /// Recovery state transitions (plan §17): CRASHED → RECOVERING → RUNNING
    /// keeps the same id/workspace/task — replacement is a governance
    /// decision, never an implicit respawn.
    pub fn begin_recovery(&self, id: &str) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE agents SET status = 'Recovering', updated_at = datetime('now') WHERE id = ?1 AND status = 'Crashed'",
            rusqlite::params![id],
        )?;
        if changed == 0 {
            return Err(StoreError::Transition(
                anti_core::statemachine::TransitionError::InvalidTransition {
                    from: AgentStatus::Crashed,
                    to: AgentStatus::Recovering,
                },
            ));
        }
        Ok(())
    }

    pub fn set_running(&self, id: &str, pid: u32) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE agents SET status = 'Running', pid = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, pid],
        )?;
        if changed == 0 {
            return Err(StoreError::Transition(
                anti_core::statemachine::TransitionError::InvalidTransition {
                    from: AgentStatus::Recovering,
                    to: AgentStatus::Running,
                },
            ));
        }
        Ok(())
    }

    /// Mark an exited process: RUNNING/BLOCKED → Completed (exit 0) or Crashed.
    pub fn mark_exit(&mut self, id: &str, exit_ok: bool) -> Result<(), StoreError> {
        let _rec = self
            .get_agent(id)?
            .ok_or(StoreError::Transition(
                anti_core::statemachine::TransitionError::InvalidTransition {
                    from: AgentStatus::Created,
                    to: AgentStatus::Completed,
                },
            ))?;
        let to = if exit_ok {
            AgentStatus::Completed
        } else {
            AgentStatus::Crashed
        };
        let changed = self.conn.execute(
            "UPDATE agents SET status = ?2, updated_at = datetime('now') WHERE id = ?1 AND status IN ('Running', 'Blocked', 'Starting')",
            rusqlite::params![id, format!("{:?}", to)],
        )?;
        if changed == 0 {
            return Ok(()); // already terminal — nothing to do
        }
        self.append_event(
            id,
            if exit_ok {
                EventType::AgentCompleted
            } else {
                EventType::AgentCrashed
            },
            serde_json::json!({}),
        )?;
        Ok(())
    }

    // ---- events ----

    pub fn append_event(&mut self, agent_id: &str, type_: EventType, payload: serde_json::Value) -> Result<Event, StoreError> {
        self.event_seq += 1;
        let ev = Event::new(self.event_seq, agent_id, type_, payload);
        let line = serde_json::to_string(&ev)?;
        use std::io::Write;
        self.event_file.write_all(line.as_bytes())?;
        self.event_file.write_all(b"\n")?;
        self.event_file.flush()?;
        self.conn.execute(
            "INSERT INTO events (seq, agent_id, type, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                ev.seq as i64,
                ev.agent_id,
                format!("{:?}", ev.type_),
                ev.payload.to_string(),
                ev.timestamp,
            ],
        )?;
        Ok(ev)
    }

    pub fn current_sequence(&self) -> u64 {
        self.event_seq
    }

    // ---- work items ----

    pub fn insert_work_item(&self, w: &anti_core::work::WorkItem) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO work_items (id, task_node_id, peer_id, lead_id, state, revision,
                 max_revisions, evidence_sha256, evidence_path, evidence_at,
                 review_verdict, submitted_at, review_deadline, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                w.id,
                w.task_node_id,
                w.peer_id,
                w.lead_id,
                format!("{:?}", w.state),
                w.revision,
                w.max_revisions,
                w.evidence.as_ref().map(|e| &e.sha256),
                w.evidence.as_ref().map(|e| &e.artifact_path),
                w.evidence.as_ref().map(|e| &e.produced_at),
                w.review_verdict,
                w.submitted_at,
                w.review_deadline,
                w.created_at,
                w.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_work_item(&self, id: &str) -> Result<Option<anti_core::work::WorkItem>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_node_id, peer_id, lead_id, state, revision, max_revisions,
                    evidence_sha256, evidence_path, evidence_at,
                    review_verdict, submitted_at, review_deadline, created_at, updated_at
             FROM work_items WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], |r| {
            Ok(anti_core::work::WorkItem {
                id: r.get(0)?,
                task_node_id: r.get(1)?,
                peer_id: r.get(2)?,
                lead_id: r.get(3)?,
                state: parse_work_state(&r.get::<_, String>(4)?),
                revision: r.get(5)?,
                max_revisions: r.get(6)?,
                evidence: {
                    let sha: Option<String> = r.get(7)?;
                    let path: Option<String> = r.get(8)?;
                    let at: Option<String> = r.get(9)?;
                    match (sha, path, at) {
                        (Some(sha256), Some(artifact_path), Some(produced_at)) => {
                            Some(anti_core::work::EvidenceRef { sha256, artifact_path, produced_at })
                        }
                        _ => None,
                    }
                },
                review_verdict: r.get(10)?,
                submitted_at: r.get(11)?,
                review_deadline: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?;
        rows.next().transpose().map_err(StoreError::Sqlite)
    }

    /// Optimistic-lock: chỉ cập nhật nếu state hiện tại khớp `expected`.
    pub fn update_work_state(
        &self,
        id: &str,
        expected: anti_core::work::WorkItemState,
        to: anti_core::work::WorkItemState,
    ) -> Result<(), StoreError> {
        anti_core::work::can_transition(expected, to);
        // Validate the transition first
        if !anti_core::work::can_transition(expected, to) {
            return Err(StoreError::WorkTransition(
                anti_core::work::WorkTransitionError::Invalid { from: expected, to },
            ));
        }
        let changed = self.conn.execute(
            "UPDATE work_items SET state = ?2, updated_at = datetime('now') WHERE id = ?1 AND state = ?3",
            rusqlite::params![id, format!("{:?}", to), format!("{:?}", expected)],
        )?;
        if changed == 0 {
            return Err(StoreError::WorkTransition(
                anti_core::work::WorkTransitionError::Invalid { from: expected, to },
            ));
        }
        Ok(())
    }

    pub fn list_work_items(
        &self,
        state: Option<anti_core::work::WorkItemState>,
    ) -> Result<Vec<anti_core::work::WorkItem>, StoreError> {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match state {
            Some(s) => (
                "SELECT id, task_node_id, peer_id, lead_id, state, revision, max_revisions,
                        evidence_sha256, evidence_path, evidence_at,
                        review_verdict, submitted_at, review_deadline, created_at, updated_at
                 FROM work_items WHERE state = ?1",
                vec![Box::new(format!("{:?}", s))],
            ),
            None => (
                "SELECT id, task_node_id, peer_id, lead_id, state, revision, max_revisions,
                        evidence_sha256, evidence_path, evidence_at,
                        review_verdict, submitted_at, review_deadline, created_at, updated_at
                 FROM work_items",
                vec![],
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |r| {
            Ok(anti_core::work::WorkItem {
                id: r.get(0)?,
                task_node_id: r.get(1)?,
                peer_id: r.get(2)?,
                lead_id: r.get(3)?,
                state: parse_work_state(&r.get::<_, String>(4)?),
                revision: r.get(5)?,
                max_revisions: r.get(6)?,
                evidence: {
                    let sha: Option<String> = r.get(7)?;
                    let path: Option<String> = r.get(8)?;
                    let at: Option<String> = r.get(9)?;
                    match (sha, path, at) {
                        (Some(sha256), Some(artifact_path), Some(produced_at)) => {
                            Some(anti_core::work::EvidenceRef { sha256, artifact_path, produced_at })
                        }
                        _ => None,
                    }
                },
                review_verdict: r.get(10)?,
                submitted_at: r.get(11)?,
                review_deadline: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::Sqlite)
    }

    /// Lấy các work item quá review_deadline mà vẫn ở Submitted — watchdog dùng.
    pub fn overdue_reviews(&self, now: &str) -> Result<Vec<anti_core::work::WorkItem>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_node_id, peer_id, lead_id, state, revision, max_revisions,
                    evidence_sha256, evidence_path, evidence_at,
                    review_verdict, submitted_at, review_deadline, created_at, updated_at
             FROM work_items
             WHERE state = 'Submitted' AND review_deadline IS NOT NULL AND review_deadline < ?1",
        )?;
        let rows = stmt.query_map([now], |r| {
            Ok(anti_core::work::WorkItem {
                id: r.get(0)?,
                task_node_id: r.get(1)?,
                peer_id: r.get(2)?,
                lead_id: r.get(3)?,
                state: parse_work_state(&r.get::<_, String>(4)?),
                revision: r.get(5)?,
                max_revisions: r.get(6)?,
                evidence: {
                    let sha: Option<String> = r.get(7)?;
                    let path: Option<String> = r.get(8)?;
                    let at: Option<String> = r.get(9)?;
                    match (sha, path, at) {
                        (Some(sha256), Some(artifact_path), Some(produced_at)) => {
                            Some(anti_core::work::EvidenceRef { sha256, artifact_path, produced_at })
                        }
                        _ => None,
                    }
                },
                review_verdict: r.get(10)?,
                submitted_at: r.get(11)?,
                review_deadline: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::Sqlite)
    }
}

fn parse_role(s: &str) -> anti_core::model::Role {
    match s {
        "supervisor" => anti_core::model::Role::Supervisor,
        "lead" => anti_core::model::Role::Lead,
        _ => anti_core::model::Role::Peer,
    }
}

fn parse_disposition(s: &str) -> anti_core::model::Disposition {
    match s {
        "architect" => anti_core::model::Disposition::Architect,
        "reviewer" => anti_core::model::Disposition::Reviewer,
        "scout" => anti_core::model::Disposition::Scout,
        "proofauditor" => anti_core::model::Disposition::ProofAuditor,
        "shadow" => anti_core::model::Disposition::Shadow,
        _ => anti_core::model::Disposition::Engineer,
    }
}

fn parse_harness(s: &str) -> anti_core::model::Harness {
    match s {
        "codex" => anti_core::model::Harness::Codex,
        "opencode" => anti_core::model::Harness::OpenCode,
        _ => anti_core::model::Harness::Claude,
    }
}

fn parse_status(s: &str) -> AgentStatus {
    match s {
        "Created" => AgentStatus::Created,
        "Starting" => AgentStatus::Starting,
        "Running" => AgentStatus::Running,
        "Blocked" => AgentStatus::Blocked,
        "Completed" => AgentStatus::Completed,
        "Failed" => AgentStatus::Failed,
        "Crashed" => AgentStatus::Crashed,
        "Stopped" => AgentStatus::Stopped,
        "Recovering" => AgentStatus::Recovering,
        "Replaced" => AgentStatus::Replaced,
        _ => AgentStatus::Created,
    }
}

fn parse_work_state(s: &str) -> anti_core::work::WorkItemState {
    match s {
        "Pending" => anti_core::work::WorkItemState::Pending,
        "InProgress" => anti_core::work::WorkItemState::InProgress,
        "Submitted" => anti_core::work::WorkItemState::Submitted,
        "Verified" => anti_core::work::WorkItemState::Verified,
        "Accepted" => anti_core::work::WorkItemState::Accepted,
        "NeedsRevision" => anti_core::work::WorkItemState::NeedsRevision,
        "Rejected" => anti_core::work::WorkItemState::Rejected,
        _ => anti_core::work::WorkItemState::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anti_core::work::*;

    #[test]
    fn work_item_insert_get_transition() {
        let dir = std::env::temp_dir().join(format!("anti-store-test-{}", std::process::id()));
        let s = Store::open(&dir).unwrap();
        let mut w = WorkItem::new("w1".into(), "peer-1".into());
        s.insert_work_item(&w).unwrap();
        w.transition(WorkItemState::InProgress).unwrap();
        s.update_work_state("w1", WorkItemState::Pending, WorkItemState::InProgress).unwrap();
        let got = s.get_work_item("w1").unwrap().unwrap();
        assert_eq!(got.state, WorkItemState::InProgress);
        assert_eq!(got.revision, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_item_list_and_overdue() {
        let dir = std::env::temp_dir().join(format!("anti-store-overdue-{}", std::process::id()));
        let s = Store::open(&dir).unwrap();
        let mut w = WorkItem::new("w2".into(), "peer-2".into());
        w.transition(WorkItemState::InProgress).unwrap();
        let evidence = EvidenceRef {
            sha256: "abc123".into(),
            artifact_path: "/tmp/out.txt".into(),
            produced_at: "2026-01-01T00:00:00Z".into(),
        };
        w.submit(evidence, 5).unwrap(); // 5 second deadline
        s.insert_work_item(&w).unwrap();
        let all = s.list_work_items(None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, WorkItemState::Submitted);
        // Overdue: deadline was 5s from creation, so "now" should be past it
        let overdue = s.overdue_reviews("2099-01-01T00:00:00Z").unwrap();
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].id, "w2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
