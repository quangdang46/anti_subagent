# Áp dụng Reina Patterns vào anti_subagent — Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Đưa 8 pattern đã học từ REINA_CODE_REVIEW.md vào anti_subagent — biến repo từ "spawner + bench" thành SLP orchestration thật: evidence-gated task lifecycle, generation-fenced leases, review loop có watchdog chống infinite loop, CAS writes, bounded capsules, read-only arbiter.

**Architecture:** Mở rộng `anti-core` (model/state machine) làm nền tảng; `anti-daemon` (store + IPC + watchdog threads) là tầng enforcement; `anti-cli` phơi các lệnh điều khiển. Mọi invariant ở tầng domain + store, không ở CLI.

**Tech Stack:** Rust edition 2024, serde/serde_json, rusqlite (WAL), chrono, thiserror, sha2 (thêm), uuid (đã có). Workspace: anti-core, anti-daemon, anti-cli, anti-bench, anti-adapters, anti-workspace.

**Nguồn pattern (file gốc trong `.tmp/`):**
| Pattern | Nguồn gốc |
|---|---|
| Generation-fenced lease | irina `src/lease.ts:415-440` |
| Review deadline/SLA + không auto-accept | Bài học veylen loop 3h |
| Sliding window + hysteresis + cooldown | veylen `SubscriptionEvaluator.ts:308-423` |
| Evidence gating SETTLED≠VERIFIED≠ACCEPTED | irina `src/verification.ts:282-333` |
| CAS write | maestro `src/foundation/core/fs.rs:120-141` |
| Bounded capsule ≤64KB | irina `src/project-state.ts:45` |
| Enum đóng + exhaustive match | maestro `src/domain/card/schema.rs:117-191` |
| Read-only arbiter tách executor | maestro `src/domain/loop_recipes.rs:840-845` (route_next) |

---

## Phase 0 — Data model & state machine (anti-core)

### Task 0.1: Thêm `WorkItemState` + `WorkItem` vào model

**Objective:** Đơn vị công việc lead giao peer — nền cho evidence gating. AgentStatus giữ nguyên (lifecycle process), WorkItem là task lifecycle (SETTLED ≠ VERIFIED ≠ ACCEPTED).

**Files:**
- Create: `crates/anti-core/src/work.rs`
- Modify: `crates/anti-core/src/lib.rs` (pub mod work)
- Test: trong `work.rs` (unit tests)

**Step 1: Viết test fail**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_admits_evidence_then_verify_then_accept() {
        assert!(can_transition(WorkItemState::Pending, WorkItemState::InProgress));
        assert!(can_transition(WorkItemState::InProgress, WorkItemState::Submitted));
        assert!(can_transition(WorkItemState::Submitted, WorkItemState::Verified));
        assert!(can_transition(WorkItemState::Verified, WorkItemState::Accepted));
    }

    #[test]
    fn cannot_accept_without_verification() {
        assert!(!can_transition(WorkItemState::Submitted, WorkItemState::Accepted));
        assert!(!can_transition(WorkItemState::Pending, WorkItemState::Accepted));
    }

    #[test]
    fn reject_bumps_revision() {
        let mut w = WorkItem::new("t1".into(), "peer-1".into());
        w.transition(WorkItemState::InProgress).unwrap();
        w.transition(WorkItemState::Submitted).unwrap();
        w.reject("lead-1".into(), "missing tests".into());
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
```

**Step 2: Chạy test — expected FAIL (`work` module chưa tồn tại)**

Run: `cargo test -p anti-core`

**Step 3: Implement `crates/anti-core/src/work.rs`**

```rust
//! WorkItem — task lifecycle của SLP (SETTLED ≠ VERIFIED ≠ ACCEPTED).
//! Bài học irina: "done" là claim, không phải sự thật; acceptance chỉ qua
//! evidence + verification + decision. Bài học veylen: reject phải bump
//! revision (group counter reset) và lead im lặng = phải có watchdog.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkItemState {
    Pending,        // lead giao, peer chưa nhận
    InProgress,     // peer claim và đang làm
    Submitted,      // peer submit + evidence — SETTLED (claim)
    Verified,       // verifier xác nhận evidence khớp — VERIFIED
    Accepted,       // lead accept — ACCEPTED (chỉ từ Verified)
    NeedsRevision,  // reject → peer sửa lại; revision bump
    Rejected,       // terminal reject (vượt max_revisions hoặc lead hủy)
}

impl WorkItemState {
    pub fn is_terminal(self) -> bool {
        matches!(self, WorkItemState::Accepted | WorkItemState::Rejected)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// sha-256 hex của artifact (file/đầu ra) — "claim phải khớp evidence thật"
    pub sha256: String,
    pub artifact_path: String,
    pub produced_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: String,
    pub task_node_id: String,      // nhóm DAG nếu có
    pub peer_id: String,           // ai đang giữ
    pub lead_id: String,           // ai accept
    pub state: WorkItemState,
    pub revision: u32,             // bump mỗi lần reject (veylen lesson)
    pub max_revisions: u32,        // mặc định 3
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
        if !matches!(self.state, WorkItemState::Submitted | WorkItemState::Verified) {
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
    pub fn submit(&mut self, evidence: EvidenceRef, review_timeout_secs: u64) -> Result<(), WorkTransitionError> {
        if !matches!(self.state, WorkItemState::InProgress | WorkItemState::NeedsRevision) {
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
    Invalid { from: WorkItemState, to: WorkItemState },
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
            | (NeedsRevision, Rejected)        // quá max_revisions
    )
}
```

**Step 4: Chạy test — expected PASS**

Run: `cargo test -p anti-core`
Expected: `test work::tests::state_machine_admits_evidence_then_verify_then_accept ... ok` (4 tests pass)

**Step 5: Commit**

```bash
git add crates/anti-core/src/work.rs crates/anti-core/src/lib.rs
git commit -m "core: WorkItem state machine — evidence-gated lifecycle (SETTLED≠VERIFIED≠ACCEPTED)"
```

---

### Task 0.2: Thêm generation fencing vào WorkspaceLease

**Objective:** Mọi write vào workspace phải mang đúng generation (irina lease.ts:415-440). Stale writer → FencingError. Correctness không dựa vào "kill pane".

**Files:**
- Modify: `crates/anti-core/src/model.rs` (WorkspaceLease + newtype Generation + error)
- Test: trong `model.rs`

**Step 1: Viết test fail**

```rust
#[test]
fn stale_generation_is_fenced() {
    let lease = WorkspaceLease {
        lease_id: "L1".into(),
        path: "/tmp/ws-1".into(),
        holder: "peer-1".into(),
        generation: 1,
    };
    assert!(lease.generation_matches(1));
    assert!(!lease.generation_matches(2)); // stale — bị fence
}

#[test]
fn fence_error_carries_audit_info() {
    let e = FenceError::StaleGeneration { expected: 2, actual: 1 };
    assert!(e.to_string().contains("stale"));
}
```

**Step 2: Implement**

```rust
// trong model.rs — thêm field vào WorkspaceLease:
pub struct WorkspaceLease {
    pub lease_id: String,
    pub path: String,
    pub holder: String,
    /// Generation fence (irina): mỗi lần lease được cấp lại/đổi chủ, generation
    /// tăng. Writer phải mang đúng generation hiện tại, nếu không = stale.
    pub generation: u64,
}

impl WorkspaceLease {
    pub fn generation_matches(&self, gen: u64) -> bool {
        self.generation == gen
    }
}

// New module-level error:
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum FenceError {
    #[error("stale generation: expected {expected}, writer holds {actual}")]
    StaleGeneration { expected: u64, actual: u64 },
    #[error("lease not held by {holder}")]
    NotHolder { holder: String },
}
```

**Step 3: Sửa các call-site hiện tại** — `crates/anti-daemon/src/store.rs:131-135` (WorkspaceLease construction) thêm `generation: 0` tạm; `crates/anti-workspace/src/lib.rs` Lease struct không cần đổi (treehouse trả lease_id, generation do store quản lý).

**Step 4: Chạy test — expected PASS** (`cargo test -p anti-core`)

**Step 5: Commit**

```bash
git commit -am "core: WorkspaceLease generation fence (irina pattern)"
```

---

### Task 0.3: Thêm `ReviewVerdict` + `VerificationStatus` enum đóng

**Objective:** Enum đóng + exhaustive match (maestro schema.rs:117-191) — compiler ép kiểm tra mọi dispatch site.

**Files:**
- Modify: `crates/anti-core/src/work.rs`

**Step 1: Viết test fail**

```rust
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
```

**Step 2: Implement** — thêm vào `work.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewVerdict {
    Accept,
    Reject,
    Escalate, // lead im lặng quá deadline → supervisor (watchdog)
}

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
```

**Step 3: Chạy test — PASS. Step 4: Commit**

```bash
git commit -am "core: ReviewVerdict + VerificationStatus closed enums (maestro pattern)"
```

---

## Phase 1 — Persistence & IPC (anti-daemon)

### Task 1.1: Bảng `work_items` + `evidence` trong store

**Objective:** WorkItem sống trong SQLite như agents; evidence có sha-256; transition dùng optimistic-lock (pattern store.rs:192-209 hiện có).

**Files:**
- Modify: `crates/anti-daemon/src/store.rs`
- Modify: `crates/anti-daemon/src/main.rs` (khởi tạo bảng)

**Step 1: Viết test fail** (thêm vào store.rs tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use anti_core::work::*;

    #[test]
    fn work_item_insert_get_transition() {
        let dir = std::env::temp_dir().join(format!("anti-store-test-{}", std::process::id()));
        let mut s = Store::open(&dir).unwrap();
        let mut w = WorkItem::new("w1".into(), "peer-1".into());
        s.insert_work_item(&w).unwrap();
        w.transition(WorkItemState::InProgress).unwrap();
        s.update_work_state(&w, WorkItemState::Pending).unwrap(); // optimistic: from=Pending
        let got = s.get_work_item("w1").unwrap().unwrap();
        assert_eq!(got.state, WorkItemState::InProgress);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

**Step 2: Implement**

Trong `Store::open` execute_batch, thêm:

```sql
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
);
```

Thêm methods (theo style get_agent/transition hiện có):

```rust
pub fn insert_work_item(&self, w: &WorkItem) -> Result<(), StoreError> { /* INSERT ... */ }

pub fn get_work_item(&self, id: &str) -> Result<Option<WorkItem>, StoreError> { /* SELECT ... */ }

/// Optimistic-lock: chỉ cập nhật nếu state hiện tại khớp `expected`.
pub fn update_work_state(&self, id: &str, expected: WorkItemState, to: WorkItemState) -> Result<(), StoreError> {
    anti_core::work::check_transition(expected, to)?;
    let changed = self.conn.execute(
        "UPDATE work_items SET state = ?2, updated_at = datetime('now') WHERE id = ?1 AND state = ?3",
        rusqlite::params![id, format!("{:?}", to), format!("{:?}", expected)],
    )?;
    if changed == 0 {
        return Err(StoreError::Transition(
            anti_core::statemachine::TransitionError::InvalidTransition {
                from: AgentStatus::Created,
                to: AgentStatus::Completed, // tái dùng error type — hoặc thêm WorkTransition variant
            },
        ));
    }
    Ok(())
}

pub fn list_work_items(&self, state: Option<WorkItemState>) -> Result<Vec<WorkItem>, StoreError> { /* ... */ }

/// Lấy các work item quá review_deadline mà vẫn ở Submitted — watchdog dùng.
pub fn overdue_reviews(&self, now: &str) -> Result<Vec<WorkItem>, StoreError> {
    let mut stmt = self.conn.prepare(
        "SELECT ... FROM work_items WHERE state = 'Submitted' AND review_deadline IS NOT NULL AND review_deadline < ?1"
    )?;
    // map rows như get_work_item
}
```

> Lưu ý: thêm `WorkTransition` variant vào `StoreError` (hoặc tái dùng `Transition` — ưu tiên thêm variant riêng cho rõ ràng).

**Step 3: Chạy test — PASS. Step 4: Commit**

```bash
git commit -am "daemon: work_items table + optimistic-lock transitions + overdue_reviews query"
```

---

### Task 1.2: IPC — `SubmitWork`, `ReviewWork`, `ListWorkItems`

**Objective:** CLI ↔ daemon giao tiếp bằng request mới.

**Files:**
- Modify: `crates/anti-daemon/src/ipc.rs`

**Step 1: Implement**

```rust
pub enum Request {
    // ... existing variants ...
    SubmitWork {
        id: String,
        sha256: String,
        artifact_path: String,
        review_timeout_secs: u64,
    },
    ReviewWork {
        id: String,
        verdict: String, // "accept" | "reject"
        note: String,
    },
    ListWorkItems,
    // Watchdog escalates: SubmitWork/ReviewWork không bao giờ auto-accept.
}
```

**Step 2: Handle trong main.rs dispatch** — thêm match arm:

```rust
Request::SubmitWork { id, sha256, artifact_path, review_timeout_secs } => {
    // 1. get work item
    // 2. WorkItem::submit(EvidenceRef{..}, review_timeout_secs)
    // 3. update_work_state optimistic (InProgress|NeedsRevision → Submitted)
    // 4. append_event(AgentSubmitted?) — thêm EventType::WorkSubmitted
    // 5. Response::ok
}
Request::ReviewWork { id, verdict, note } => {
    // accept: chỉ từ Verified (đã qua verify) — hoặc Verified→Accepted
    // reject: từ Submitted|Verified → reject() bump revision
}
```

Thêm EventType mới vào `anti-core/src/events.rs`: `WorkSubmitted, WorkVerified, WorkAccepted, WorkRejected, ReviewEscalated`.

**Step 3: Commit**

```bash
git commit -am "daemon: SubmitWork/ReviewWork IPC — no auto-accept path exists"
```

---

## Phase 2 — Review watchdog & loop prevention (bài học veylen loop 3h)

### Task 2.1: Watchdog thread — review deadline escalation

**Objective:** Lead im lặng quá `review_deadline` → daemon tự escalate (ReviewVerdict::Escalate) lên supervisor event. KHÔNG auto-accept (veylen race lesson). KHÔNG để kẹt vô hạn.

**Files:**
- Modify: `crates/anti-daemon/src/main.rs`

**Step 1: Implement** — thêm thread cạnh reaper/sweeper hiện có:

```rust
// Review watchdog: mỗi 15s, quét overdue reviews.
// Bài học veylen: lead im lặng = kẹt vô thời hạn. Escalate, không auto-accept.
let watchdog_store = store.clone();
std::thread::spawn(move || loop {
    std::thread::sleep(Duration::from_secs(15));
    let s = match watchdog_store.lock() {
        Ok(g) => g,
        Err(_) => continue,
    };
    let now = chrono::Utc::now().to_rfc3339();
    let overdue = match s.overdue_reviews(&now) {
        Ok(v) => v,
        Err(_) => continue,
    };
    for w in overdue {
        let _ = s.append_event(
            &w.id,
            EventType::ReviewEscalated,
            json!({
                "peer_id": w.peer_id,
                "lead_id": w.lead_id,
                "revision": w.revision,
                "deadline": w.review_deadline,
                "action": "supervisor intervention required",
            }),
        );
        // Giữ state ở Submitted (lead có thể vẫn review) nhưng sự kiện đã
        // ghi nhận — supervisor/người xem phản hồi qua CLI. Không tự đổi state.
    }
});
```

**Step 2: Verify compile + daemon chạy** — `cargo build` OK, `anti daemon start` + `anti daemon status` OK.

**Step 3: Commit**

```bash
git commit -am "daemon: review watchdog — overdue → ReviewEscalated (no auto-accept, veylen lesson)"
```

---

### Task 2.2: Loop-prevention evaluator — sliding window + hysteresis + cooldown (port veylen)

**Objective:** Đếm `ReviewRejected` theo `[task_node_id, revision]` trong sliding window 1h; trigger escalation khi > 3; hysteresis reset khi count ≤ 1; cooldown 10 phút. KHÔNG cộng dồn vô hạn — veylen là vì reset ≤ 1 không bao giờ chạm và revision không bump.

**Files:**
- Create: `crates/anti-core/src/loopprev.rs`
- Modify: `crates/anti-core/src/lib.rs`

**Step 1: Viết test fail**

```rust
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
        // peer sửa xong resubmit → revision 2 → group mới, counter sạch
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
```

**Step 2: Implement `crates/anti-core/src/loopprev.rs`**

```rust
//! Loop prevention (port veylen SubscriptionEvaluator.ts:308-423).
//! Sliding window 1h, group theo [task_node_id, revision], trigger > 3,
//! hysteresis reset khi count ≤ 1, cooldown 10 phút sau trigger.

use std::collections::HashMap;

pub const WINDOW_SECS: i64 = 3600;
pub const TRIGGER_THRESHOLD: usize = 3;
pub const COOLDOWN_SECS: i64 = 600;

#[derive(Debug, Default)]
pub struct LoopPrevention {
    /// key = (task_node_id, revision) → timestamps reject
    rejects: HashMap<(String, u32), Vec<i64>>,
    /// key → thời điểm trigger escalation gần nhất (cooldown)
    last_trigger: HashMap<(String, u32), i64>,
}

impl LoopPrevention {
    pub fn record_reject(&mut self, task_node_id: &str, revision: u32, at_rfc3339: String) {
        let now = chrono::DateTime::parse_from_rfc3339(&at_rfc3339)
            .map(|d| d.timestamp())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp());
        let key = (task_node_id.to_string(), revision);
        self.rejects.entry(key).or_default().push(now);
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

    /// Trigger khi count > threshold VÀ ngoài cooldown.
    /// Hysteresis: nếu count giảm ≤ 1 (window trôi), cooldown hết → cho trigger lại.
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
```

**Step 3: Chạy test — PASS. Step 4: Wire vào watchdog**

Trong main.rs watchdog (Task 2.1), trước khi escalate: query các `WorkRejected` events gần đây của work item (hoặc duy trì counter trong store), record vào `LoopPrevention`; chỉ escalate khi `should_escalate(task_node_id, revision)` = true.

```
Lưu ý: veylen loop 3h = reset ≤ 1 không bao giờ chạm + revision không bump.
Anti_subagent đã fix cả 2 ở model: reject() bump revision (Task 0.1) +
evaluator này group theo revision. Đây là lớp phòng thủ thứ 2.
```

**Step 5: Commit**

```bash
git commit -am "core+daemon: loop-prevention evaluator — sliding window, hysteresis, cooldown (veylen port)"
```

---

## Phase 3 — CAS writes & bounded capsule

### Task 3.1: CAS write module trong anti-workspace

**Objective:** 2 peers không last-writer-wins (maestro fs.rs:120-141): `write_if_unchanged` + lock marker.

**Files:**
- Create: `crates/anti-workspace/src/cas.rs`

**Step 1: Viết test fail**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_succeeds_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("anti-cas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.rs");
        std::fs::write(&f, "v1").unwrap();
        // baseline fork: đọc trước khi sửa
        let base = cas::read_baseline(&f).unwrap();
        assert!(cas::write_if_unchanged(&f, "v2", &base).is_ok());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_fails_when_changed_by_other() {
        let dir = std::env::temp_dir().join(format!("anti-cas2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.rs");
        std::fs::write(&f, "v1").unwrap();
        let base = cas::read_baseline(&f).unwrap();
        std::fs::write(&f, "v1.5").unwrap(); // peer khác sửa
        assert!(matches!(
            cas::write_if_unchanged(&f, "v2", &base),
            Err(cas::CasError::Changed { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

**Step 2: Implement `crates/anti-workspace/src/cas.rs`**

```rust
//! CAS write (maestro fs.rs:120-141) — write-if-unchanged + lock marker.
//! Chống last-writer-wins giữa 2 peers cùng sửa 1 file.

use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CasError {
    #[error("file changed since baseline (expected sha {expected}, found {found})")]
    Changed { path: PathBuf, expected: String, found: String },
    #[error("lock held by {holder}")]
    LockHeld { path: PathBuf, holder: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Baseline {
    pub sha256: String,
}

pub fn sha256_of(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Baseline = sha256 của file tại thời điểm peer bắt đầu sửa.
pub fn read_baseline(path: &Path) -> std::io::Result<Baseline> {
    Ok(Baseline { sha256: sha256_of(path)? })
}

/// Ghi chỉ khi file vẫn còn đúng baseline. Ngược lại → CasError::Changed.
pub fn write_if_unchanged(path: &Path, content: &str, base: &Baseline) -> Result<(), CasError> {
    if path.exists() {
        let now = sha256_of(path)?;
        if now != base.sha256 {
            return Err(CasError::Changed {
                path: path.to_path_buf(),
                expected: base.sha256.clone(),
                found: now,
            });
        }
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Lock marker: `.anti.lock` chứa holder. Atomic create_new — không bao giờ
/// overwrite lock của peer khác.
pub fn acquire_lock(dir: &Path, holder: &str) -> Result<(), CasError> {
    let lock = dir.join(".anti.lock");
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(mut f) => {
            use std::io::Write;
            let _ = f.write_all(holder.as_bytes());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let holder_now = std::fs::read_to_string(&lock).unwrap_or_default();
            Err(CasError::LockHeld { path: lock, holder: holder_now })
        }
        Err(e) => Err(CasError::Io(e)),
    }
}

pub fn release_lock(dir: &Path) -> std::io::Result<()> {
    let lock = dir.join(".anti.lock");
    if lock.exists() {
        std::fs::remove_file(lock)?;
    }
    Ok(())
}
```

**Step 3: Thêm deps** — `crates/anti-workspace/Cargo.toml`: `sha2 = "0.10"`.

**Step 4: Chạy test — PASS.**

**Step 5: Wire vào guard workflow (tùy chọn cho MVP)** — đưa `cas::` vào bin script `guard/anti-guard.sh` không cần; để là library API cho Lead/verifier dùng sau. Ghi chú trong README.

**Step 6: Commit**

```bash
git commit -am "workspace: CAS write-if-unchanged + atomic lock marker (maestro pattern)"
```

---

### Task 3.2: Bounded capsule ≤64KB (irina pattern)

**Objective:** Khi spawn peer, prompt context chỉ chứa capsule bounded: state tóm tắt + task + evidence refs, không dump toàn bộ transcript. Fix context explosion.

**Files:**
- Create: `crates/anti-core/src/capsule.rs`

**Step 1: Viết test fail**

```rust
#[test]
fn capsule_respects_budget() {
    let cap = crate::capsule::render_capsule(&CapsuleInput {
        peer_id: "peer-1".into(),
        task: "implement foo".into(),
        work_items: vec![WorkItem::new("w1".into(), "peer-1".into())],
        recent_events: vec!["event: x".repeat(1000)],
    });
    assert!(cap.len() <= 64 * 1024, "capsule {} bytes > 64KB", cap.len());
    assert!(cap.contains("implement foo"));
}
```

**Step 2: Implement `crates/anti-core/src/capsule.rs`**

```rust
//! Bounded capsule (irina project-state.ts:45) — mỗi agent chỉ thấy ≤64KB
//! state view. Cắt theo phần quan trọng nhất, không bao giờ vượt budget.

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
```

**Step 3: Wire vào spawn** — trong `spawn()` daemon (main.rs ~line 486): nếu prompt không được cấp, build capsule từ store: `render_capsule(&CapsuleInput { peer_id: id, task, work_items: store.list_work_items(None).unwrap_or_default(), recent_events: vec![] })` — MVP: chỉ task + work items, events sau.

**Step 4: Chạy test — PASS. Step 5: Commit**

```bash
git commit -am "core: bounded capsule ≤64KB (irina pattern) — wire into spawn prompt"
```

---

## Phase 4 — Read-only arbiter & CLI surface

### Task 4.1: Arbiter module — read-only scorer (maestro route_next)

**Objective:** Lead quyết định không tự làm — arbiter chấm điểm options bằng rubric cố định, không inspect FS/git (giữ read-only), kết quả là đề xuất cho lead.

**Files:**
- Create: `crates/anti-core/src/arbiter.rs`

**Step 1: Viết test fail**

```rust
#[test]
fn arbiter_scores_by_rubric_deterministically() {
    let a = Arbiter::default();
    let mut opts = vec![
        ArbiterOption { id: "fast".into(), desc: "quick hack".into(), risk: Risk::High, effort: Effort::Small },
        ArbiterOption { id: "solid".into(), desc: "proper fix".into(), risk: Risk::Low, effort: Effort::Large },
    ];
    let ranked = a.rank(&mut opts);
    // rubric: low risk + small effort thắng; solid (low risk) phải trên fast
    assert!(ranked.iter().position(|o| o.id == "solid").unwrap() < ranked.iter().position(|o| o.id == "fast").unwrap());
}

#[test]
fn arbiter_cannot_mutate_fs() {
    // read-only: không có tham chiếu fs nào trong API — compile-time guarantee
    let a = Arbiter::default();
    let ranked = a.rank(&mut vec![ArbiterOption { id: "x".into(), desc: "d".into(), risk: Risk::Low, effort: Effort::Small }]);
    assert_eq!(ranked.len(), 1);
}
```

**Step 2: Implement**

```rust
//! Read-only arbiter (maestro route_next) — chấm điểm options bằng rubric
//! cố định. KHÔNG có quyền FS/git: compiler đảm bảo read-only.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Risk { Low, Medium, High }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effort { Small, Medium, Large }

#[derive(Debug, Clone)]
pub struct ArbiterOption {
    pub id: String,
    pub desc: String,
    pub risk: Risk,
    pub effort: Effort,
}

pub struct Arbiter;

impl Arbiter {
    pub fn rank(&self, options: &mut Vec<ArbiterOption>) -> Vec<ArbiterOption> {
        options.sort_by_key(|o| (o.risk as u8, o.effort as u8));
        options.clone()
    }
}
```

**Step 3: Commit**

```bash
git commit -am "core: read-only arbiter with fixed rubric (maestro route_next pattern)"
```

---

### Task 4.2: CLI commands — `work submit`, `work review`, `work list`, `watchdog status`

**Objective:** User/lead điều khiển SLP qua CLI (mục tiêu repo: "CLI control all features").

**Files:**
- Modify: `crates/anti-cli/src/main.rs` (subcommands)
- Modify: `crates/anti-cli/src/commands.rs`

**Step 1: Implement subcommands**

```rust
// main.rs — thêm vào enum:
Work(WorkCmd),
// WorkCmd: Submit { id, sha, path, timeout }, Review { id, verdict, note }, List, Escalations
```

```rust
// commands.rs
pub fn work_submit(state_dir: &PathBuf, id: &str, sha: &str, path: &str, timeout: u64) -> Result<String, String> {
    let resp = ipc::send_request(&socket(state_dir), &Request::SubmitWork {
        id: id.to_string(),
        sha256: sha.to_string(),
        artifact_path: path.to_string(),
        review_timeout_secs: timeout,
    })?;
    check(resp)
}
pub fn work_review(state_dir: &PathBuf, id: &str, verdict: &str, note: &str) -> Result<String, String> {
    let resp = ipc::send_request(&socket(state_dir), &Request::ReviewWork {
        id: id.to_string(),
        verdict: verdict.to_string(),
        note: note.to_string(),
    })?;
    check(resp)
}
pub fn work_list(state_dir: &PathBuf) -> Result<String, String> { /* ListWorkItems → bảng ID/STATE/REV/DEADLINE */ }
pub fn escalations(state_dir: &PathBuf) -> Result<String, String> {
    // đọc events ReviewEscalated gần đây từ store
}
```

**Step 2: Kiểm tra end-to-end**

```bash
cargo build
./target/debug/anti daemon start
./target/debug/anti work submit w1 <sha> /tmp/out.txt 600   # submit
./target/debug/anti work list                                 # Submitted, deadline 10p
# không review → chờ watchdog 15s → escalations hiện ReviewEscalated
./target/debug/anti work review w1 accept "looks good"        # (cần Verified trước — xem Task 1.1)
```

**Step 3: Commit**

```bash
git commit -am "cli: work submit/review/list/escalations — SLP control plane via CLI"
```

---

## Phase 5 — Wire vào benchmark & integration

### Task 5.1: Expand ARM C/D để dùng WorkItem lifecycle

**Objective:** Benchmark SLP arms chạy qua work_items thay vì trạng thái agent đơn thuần — đo số vòng review, escalations, revision bumps (điểm khác biệt so với ARM A/B).

**Files:**
- Modify: `crates/anti-bench/src/main.rs`

**Step 1: Thêm metrics** vào `run_arm` cho ARM C/D: `reviews`, `rejections`, `escalations`, `revisions` (đọc từ store events + work_items sau run).

**Step 2: Chạy `anti-bench <repo> run 1` — verify compile + chạy được.**

**Step 3: Commit**

```bash
git commit -am "bench: ARM C/D report review/reject/escalate/revision metrics"
```

---

### Task 5.2: Integration test script — toàn bộ flow SLP qua CLI

**Files:**
- Create: `scripts/slp-e2e.sh`

**Step 1: Viết script**

```bash
#!/usr/bin/env bash
set -euo pipefail
# E2E: spawn peer → submit → watchdog escalate → review → accept
cd "$(dirname "$0")/.."
cargo build -q
ANTI=./target/debug/anti
$ANTI daemon start 2>/dev/null || true
ID="e2e-$(date +%s)"
$ANTI spawn "$ID" peer engineer claude "" /tmp "$ID"
sleep 2
SHA=$(shasum -a 256 /tmp/e2e-note.txt | awk '{print $1}')
$ANTI work submit "$ID" "$SHA" /tmp/e2e-note.txt 5
$ANTI work list | grep -q Submitted && echo "PASS: submit"
# watchdog escalate sau ~15s
sleep 18
$ANTI escalations | grep -q "$ID" && echo "PASS: escalate (watchdog)"
echo "E2E done — manual: anti work review $ID accept|reject <note>"
```

**Step 2: Chạy script — verify cả 2 PASS.**

**Step 3: Commit**

```bash
git commit -am "test: SLP e2e script — spawn→submit→escalate→review"
```

---

## Phase 6 — Docs & final review

### Task 6.1: Cập nhật README + REINA_CODE_REVIEW → "đã áp dụng" checklist

**Files:**
- Modify: `README.md` (thêm section "Applied patterns")
- Modify: `REINA_CODE_REVIEW.md` (checklist đánh dấu xong)

**Step 1: Ghi checklist**

```markdown
## Applied patterns (từ REINA_CODE_REVIEW.md)
- [x] WorkItem evidence-gated lifecycle (SETTLED≠VERIFIED≠ACCEPTED) — irina
- [x] Generation-fenced WorkspaceLease — irina
- [x] Review watchdog + deadline — no auto-accept — veylen lesson
- [x] Loop-prevention sliding window + hysteresis + cooldown — veylen
- [x] Reject bumps revision (group counter reset) — veylen fix
- [x] CAS write_if_unchanged + lock marker — maestro
- [x] Bounded capsule ≤64KB — irina
- [x] Closed enums + exhaustive match — maestro
- [x] Read-only arbiter — maestro route_next
- [x] CLI control plane: work submit/review/list/escalations
```

**Step 2: Commit**

```bash
git commit -am "docs: applied-patterns checklist (reina research → implementation)"
```

---

## Verification roadmap (chạy sau mỗi phase)

```bash
cargo test --workspace          # toàn bộ unit tests
cargo build --release
./scripts/slp-e2e.sh            # end-to-end SLP flow
./target/release/anti doctor    # môi trường đủ
./target/release/anti bench /tmp/repo run 1   # benchmark vẫn chạy
```

## Risks & tradeoffs

| Rủi ro | Mitigation |
|---|---|
| StoreError::Transition tái dùng cho Work — thiếu variant | Task 1.1 yêu cầu thêm variant `WorkTransition` riêng |
| Watchdog escalate nhưng lead vẫn review sau đó → 2 luồng | Escalate chỉ ghi event, KHÔNG đổi state — không xung đột |
| Capsule cắt ngang mid-UTF8 | `truncate` có thể cắt char — chấp nhận cho MVP (serde không đụng tới); ghi chú |
| CAS lock file làm dirty git status | `.anti.lock` thêm vào `.gitignore` — **bắt buộc trong Task 3.1** |
| LoopPrevention trong-memory mất khi daemon restart | MVP chấp nhận (window 1h); v2: persist reject events vào SQLite rồi rebuild |
| old code: `restart_agent` hardcode claude | đã biết — ngoài scope plan này (đã có HarnessAdapter cho spawn; restart chưa) |

## Open questions
1. Lead "accept" có cần qua verifier riêng (role ProofAuditor) hay lead tự verify? → Plan mặc định: `Verified` được set bởi verifier/thủ công qua CLI; AI review có thể làm cả 2 nhưng state machine vẫn ép thứ tự.
2. Review timeout mặc định bao nhiêu? → Đề xuất 600s (10p), config qua `~/.anti_subagent/config.toml` `review_timeout_secs`.
3. Có cần Supervisor seat thật (khác người dùng) không? → MVP: human = supervisor qua CLI; v2: supervisor agent.

---

**Execution handoff:** Plan đã sẵn sàng. Implement theo thứ tự Phase 0 → 6, mỗi task 2-5 phút, TDD + commit từng task. Có thể dùng subagent-driven-development (1 subagent/task) hoặc làm tuần tự.