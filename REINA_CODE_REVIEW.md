# REINA CODE REVIEW — Phân tích sâu 5 repo của ReinaMacCredy
> Tạo: 2026-08-16 | Nguồn: clone trong `.tmp/` | Mục đích: học hỏi + copy pattern vào anti_subagent

---

## 1. VEYLEN — Hệ thống SLP thật (Root→Supervisor→Lead→Peer)

### 1.1 Kiến trúc 3 tầng

```
root.md (ROLE_INSTRUCTIONS: root/child_owner/participant/compiler/arbiter/verifier)
        │
        ▼
ORCHESTRATION ENGINE — Command→Event→Projection
  OrchestrationEngine.ts (sagas) · decider.ts (invariants) · projector.ts
        │                                   │
        ▼                                   ▼
GOVERNANCE PLANE                  SIGNAL PLANE
  supervisedGovernance.ts          BuiltInSubscriptions.ts
  AgentSeat, RootLease,            builtin-review-loop-v1 (ReviewRejected>3)
  AuthorityReceipt, Mission,       SubscriptionEvaluator.ts (sliding window,
  LeadRotation                     hysteresis, cooldown, dedup)
        │                                   │
        ▼                                   ▼
SUPERVISED RUNTIME DAEMON — reconcile loop (30s / 1s nếu RLM active)
  SupervisedRuntimeDaemon.ts: 8 phase mỗi vòng:
  1. ensureBuiltIns  2. orchestration-ingestion  3. lease-expiration
  4. rlm reconcile   5. harness-patches          6. subscriptions
  7. deliveries      8. health
        │                   │                   │
        ▼                   ▼                   ▼
  LEAD SEAT           SUPERVISOR SEAT        PEER SEATS
  task_node_review     concern wake           supervised.peer.bind
  review.accept        intervention.propose   supervised.run.submit
```

### 1.2 Data model cốt lõi

| Entity | File | Vai trò |
|---|---|---|
| `SupervisorSeat` | supervision.ts:114 | Seat giám sát theo `concern` (primary/context/delivery) |
| `LeadSeat` | supervision.ts:131 | Seat chủ phòng, versioning qua `predecessorThreadIds` |
| `PeerBinding` | supervision.ts:145 | Ràng buộc peer → 1 lead + root thread |
| `RootHolderSeatId` | supervision.ts:27 | **Root là lease, không phải role cố định** (Lead hoặc Supervisor đều giữ được) |
| `WorkClaim` | supervised.ts:296 | 1 revision chỉ có 1 active claim; `expiresAt` |
| `CapabilityLease` | supervised.ts:310 | Leased capability |
| `RlmEpisode` | supervised.ts:444 | Episode N branches (2-16), mỗi branch 1 provider session |
| `TaskNodeRevision` | supervised.ts | Bất biến, append-only, mọi mutation có `expectedRevision` |

### 1.3 Luồng SLP từng bước

1. **Root tạo task** — saga `supervised.task-graph.create`, DAG validate bằng DFS cycle detection (OrchestrationEngine.ts:1304-1328)
2. **Delegate** — `supervised.task.delegate` → run + workClaim → peer nhận thread
3. **Peer làm** — `supervised.run.submit` (saga atomic): evidence.publish → run reviewing → tạo review revision → task-node.commit → claim.release → wake lead
4. **Lead review** — `supervised.review.accept`; invariant: run reviewing + không active claim + actor = leadSeatId + evidence đúng scope
5. **Accept** — auto unblock dependents: taskNode planned mà dep accepted → ready
6. **RLM** — branches terminal → synthesis prompt → root session → episode completed

### 1.4 ☠️ NGUYÊN NHÂN LOOP REVIEW 3 TIẾNG (reina kể trong Discord)

**Kịch bản "reject liên tục":**
1. Peer submit → run reviewing → lead KHÔNG có tool `review.reject` — chỉ accept hoặc im lặng
2. Reject duy nhất qua `intervention.reconcile` (status `rejected`) → sinh `ReviewRejected`
3. **`builtin-review-loop-v1`** đếm: sliding window 1h, count theo `[taskNodeId, graphRevision]`, trigger khi > 3, hysteresis reset khi ≤ 1, cooldown 10 phút
4. Trigger → wake supervisor → propose intervention → lead reconcile rejected → **ReviewRejected mới** → loop vô hạn

**Điểm tử vong:**
- **A. Hysteresis reset KHÔNG BAO GIỜ xảy ra**: reset chỉ khi count ≤ 1 trong window 1h; vòng reject cứ 10 phút 1 lần → count luôn > 1 → không bao giờ reset
- **B. `graphRevision` không tăng khi reject** (SupervisedRuntimeDaemon.ts:322) → group `[taskNodeId, graphRevision]` không đổi → count cộng dồn mãi
- **C. Race AUTO-ACCEPT** (SupervisedRuntimeDaemon.ts:1161-1181): daemon tự `run-succeeded` khi `runRevision > run.revision` — KHÔNG cần lead accept → run succeeded nhưng taskNode chưa accepted → trạng thái mâu thuẫn
- **D. Lead im lặng = kẹt vô thời hạn**: không có review timeout/SLA → không watchdog nào

### 1.5 TOP 10 snippet đáng copy (veylen)

| # | Snippet | Đường dẫn | Lý do |
|---|---|---|---|
| 1 | Sliding-window + hysteresis + cooldown evaluator | `apps/server/src/supervised/signal/SubscriptionEvaluator.ts:308-423` | Mẫu chống loop chuẩn |
| 2 | Optimistic concurrency `expectedRevision` | `packages/contracts/src/supervised.ts:301-308` | Chống lost-update |
| 3 | Idempotency key deterministic (sha256) | `SupervisedSignalDelivery.ts:47-48` | Replay an toàn |
| 4 | Claim serialization | `decider.ts:683-691` | 2 peer không làm chung 1 revision |
| 5 | Lease expiry reconcile loop | `SupervisedRuntimeDaemon.ts:1747-1782` | Không dựa agent tự release |
| 6 | BFS recovery path state machine | `SupervisedRuntimeDaemon.ts:124-172` | Chống kẹt vĩnh viễn sau crash |
| 7 | Saga atomic submit | `OrchestrationEngine.ts:1750-1796` | Evidence+publish+transition trong 1 saga |
| 8 | Bounded authority wake text | `SupervisedSignalDelivery.ts:154-170` | Signal "grants no new authority" |
| 9 | DAG validation trước commit | `OrchestrationEngine.ts:1304-1328` | Chống graph loop từ đầu |
| 10 | Dead-letter + redrive delivery | `SupervisedRuntimeDaemon.ts:1501-1553` | Delivery lỗi không mất |

**Pitfalls:** supervision.ts là legacy (TODO xoá 2027-08-09, học từ supervisedGovernance.ts); compiler/arbiter/verifier CHỈ là ROLE_INSTRUCTIONS trong root.md, không phải pipeline server-side; đừng copy AUTO-ACCEPT race; thiếu review timeout là lỗ hổng lớn nhất.

---

## 2. IRINA — Chief of Staff (config-as-code trên Herdr)

### 2.1 Kiến trúc

```
YOU (vision/priority/decisions)
  │
  ▼
IRINA GATEWAY — durable store: inbox, lease, portfolio, capsules,
  evidence, acceptance, audit (CAS records, version n+1)
  │
  ▼
IRINA STAFF — 1 logical writer `staff`, 1 Attention Frame
  (lease generation-fenced; pin: codex gpt-5.6-sol xhigh --yolo)
  │
  ├── Project A Lane (≤4 lanes)  ├── Project B Lane  ├── Project C Lane
  │   Capsule A (≤64KB)          │   Capsule B       │   direct Worker
  │   Lead A ──────── Worker     │   Lead B          │
  │   └── Reviewer               │   └── Reviewer    │
```

### 2.2 Pattern đáng học

| Pattern | File | Chi tiết |
|---|---|---|
| **Generation-fenced lease** | `src/lease.ts:415-440` | Mọi write phải mang đúng generation; stale → FencingError + audit. Correctness không dựa vào kill pane |
| **Bounded view / Capsule** | `src/project-state.ts:45` | Mỗi agent chỉ thấy 1 capsule ≤64KB — fix context explosion |
| **Evidence gating** | `src/delegation.ts` + `src/verification.ts:282-333` | handback/lifecycle/done đều là claim; acceptance chỉ qua evidence (sha-256) + verification + decision |
| **Attention Frame** | `src/project-attention.ts:292-363` | Chỉ claim khi slot IDLE + lease ACTIVE; checkpoint durabler trước mỗi item |
| **Compare attention priority** | `src/project-attention.ts:756` | priority class → owner priority → deadline → fan-out → created_at — deterministic anti-starvation |
| **Model routing fail-closed** | `src/router.ts:81-116` | RouteModel yêu cầu capability snapshot fresh (ttl ≤ 60s) nếu không → unavailable |
| **validateLaunchRecord** | `src/router.ts:176-199` | Chỉ codex + yolo + external_effects_allowed; cấm field tên secret |
| **Staff switch** | `src/staffswitch.ts:31-46` | REQUESTED → DRAINING → CANDIDATE_LAUNCHED → CANDIDATE_VERIFIED → CUTOVER → COMPLETE |

**State machines quan trọng:** Lane `QUEUED→READY→EXECUTING→CLOSING→DONE`; Verification `OPEN→EVIDENCE_READY→VERIFYING→VERIFIED|FAILED|UNCERTAIN`; Acceptance `PENDING→ACCEPTED|REPAIR_REQUIRED|REJECTED`. **SETTLED ≠ VERIFIED ≠ ACCEPTED** — nguyên tắc bất biến.

**Concurrency policy:** 1 Staff writer, 1 Attention Frame, ≤1 Lead/lane, ≤4 lanes, direct delegations 1, lead delegations ≤2.

**Escalation packet chuẩn** (authority/DEFAULTS.md): problem → project + authority revision → evidence → options → recommended → impact of waiting → required authority → safe default.

---

## 3. MAESTRO — Rust local-first task harness

### 3.1 Kiến trúc 4 tầng
```
interfaces (cli/mcp/tui/hooks/shell) → operations → domain → foundation/core
```

### 3.2 Card model

```rust
pub struct Card {
    schema_version, id, card_type (CardType enum đóng), title, status,
    parent: Option<String>, deps: Vec<Dep>, lane, claimed_by: Option<String>,
    claimed_at, created_at, updated_at,
    extra: serde_yaml::Mapping,          // type-specific payload
    #[serde(flatten)] unknown: serde_yaml::Mapping,  // forward tolerance!
}
```

- **CardType enum đóng + exhaustive match** — thêm variant = compiler ép review mọi dispatch site (ARCHITECTURE.md:75-79)
- **`parent` là hierarchy KHÔNG phải execution blocker** — readiness chỉ từ Task status + blockers
- **Coarse status DERIVED, không lưu** (query.rs:57-62) — board status không thể desync

### 3.3 TaskRecord + proof gating

- TaskState: `draft→exploring→ready→in_progress→needs_verification→verified` (terminal: rejected/abandoned/superseded)
- Stamp `agent#session` từ env (`MAESTRO_SESSION_ID`, `CODEX_THREAD_ID`, `CLAUDE_CODE_SESSION_ID`...) + fallback `cli-<date>`
- **verify** = `check_claims(claims, evidence)`: claims khớp evidence thật (event JSONL hoặc file); mọi PostToolUse status:ok thành claim `"{tool} {input_hash}"`
- **QA gate feature**: baseline ghi TRƯỚC khi sửa code (registry.rs:1059); freshness qua amend_log_position; close cần contract sweep (AcceptanceSweepRun)

### 3.4 Loop-recipes — 15 recipe (SLP compiler/arbiter/verifier trong maestro)

| Recipe | Vai trò |
|---|---|
| `work` | 1 task: claim → implement → proof → verify → gate kế |
| `feature-fanout` | Song song slice độc lập; **conductor giữ verify+close** |
| `adversarial-review` | Skeptic độc lập refute claim rủi ro cao |
| `conflict-handoff` | 2 session đè file: link + conflict notice + worktree merge-back |
| `generate-filter` | Sinh nhiều option, chấm rubric cố định, lock survivor |
| `ship` | Chỉ qua close/commit/push khi có authority + evidence |
| `unattended` | 1 safe unit/tick, hard stop external ship |

- **Compiler = parse + validate recipe**: `#[serde(deny_unknown_fields)]` trên MỌI struct — YAML lạ bị từ chối
- **Arbiter = `route_next`** — read-only scorer, không inspect FS/git (loop_recipes.rs:840-845)
- **Verifier = proof/QA gates + readiness ladder L0→L3**; L2 yêu cầu `verifier_split` (worker tạo proof, conductor verify); L3 yêu cầu budget/kill_switch/heartbeat
- **Anti-bypass**: `FORBIDDEN_BYPASS_PHRASES` chặn "bypass acceptance/proof/qa", "launch workers", "start a daemon"

### 3.5 TOP 10 snippet Rust đáng copy

| # | Snippet | Đường dẫn |
|---|---|---|
| 1 | CAS write (write_string_if_unchanged + lock marker) | `src/foundation/core/fs.rs:120-141` |
| 2 | Card envelope forward tolerance (extra + flatten unknown) | `src/domain/card/schema.rs:20-87` |
| 3 | Enum đóng + exhaustive match | `src/domain/card/schema.rs:117-191` |
| 4 | Deterministic salted hash-id | `src/domain/card/store.rs:153-178` |
| 5 | Path guard chống traversal | `src/domain/card/store.rs:56-61` |
| 6 | Session token từ env agent runtime | `src/foundation/core/session.rs:16-23` |
| 7 | DAG readiness gộp 3 nguồn blocker | `src/domain/task/readiness.rs:191-244` |
| 8 | Wave projection tương lai (BFS) | `src/domain/task/readiness.rs:335-382` |
| 9 | Claim matching bền whitespace | `src/domain/proof/claims.rs:27-52` |
| 10 | Gate QA là hàm thuần | `src/domain/feature/qa.rs:120-160` |

**Pitfalls:** không lưu trạng thái derived; symlink là kẻ thù số 1 (bail thay vì báo absent); CAS per-file KHÔNG hứa transaction cross-file; "done" phải tốn công (verify = claim khớp evidence); recipe là control grammar không phải markdown khuyên nhủ.

---

## 4. MAESTRO-ORCHESTRATE (josstei) — KHÁC HẲN

- **KHÔNG phải của ReinaMacCredy** — là `@josstei/maestro` v1.6.4, Node.js MCP server, 39 specialist agents, workflow 4 phase (Design→Planning→Execution→Completion)
- HARD-GATE: validate_plan, phase transition, Critical/Major findings chặn completion
- Đáng đọc: `docs/flow.md` (41 bước orchestration), `src/core/policy-rules.js` (DENY/ASK — chặn `rm -rf`, `git reset --hard`, heredoc), `src/state/session-state.js` (validate containment state_dir)

---

## 5. KHUYẾN NGHỊ CHO ANTI_SUBAGENT (tổng hợp 3 repo)

### Bắt buộc copy:
1. **Lease + generation fencing** (irina lease.ts:415-440) — single-writer fence, mọi write mang generation
2. **Review loop watchdog + timeout/SLA** — bài học lớn nhất từ lỗi loop 3h của veylen: KHÔNG CÓ subset tiêu đề nào phải có:
   - Review deadline (lead im lặng N phút → auto escalate)
   - Hysteresis reset theo số trigger gần nhất, không theo count tuyệt đối
   - Bump revision khi reject (để group counter reset)
   - KHÔNG auto-accept khi chưa có lead accept
3. **Sliding window + hysteresis + cooldown evaluator** (veylen SubscriptionEvaluator.ts:308-423) — chống loop
4. **CAS write** (maestro fs.rs:120-141) — multi-agent không last-writer-wins
5. **Evidence gating**: SETTLED ≠ VERIFIED ≠ ACCEPTED; done/closed phải có evidence sha-256
6. **Bounded capsule ≤64KB** — mỗi agent chỉ thấy 1 bounded view
7. **Enum đóng + exhaustive match** — compiler ép kiểm tra
8. **Read-only arbiter tách khỏi executor** (maestro route_next)

### Tránh:
- ❌ AUTO-ACCEPT race (veylen daemon auto succeed)
- ❌ Hysteresis reset ≤ 1 với window 1h
- ❌ Không có review deadline
- ❌ last-writer-wins ghi file
- ❌ Agent tự quyết autonomy (cần thang L0-L3 + bằng chứng)