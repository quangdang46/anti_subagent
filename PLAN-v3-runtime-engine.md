# anti_subagent v3 — Runtime Engine Architecture Plan

> **Date:** 2026-08-17 · **Status:** Architecture locked — ready for Phase 1 implementation
> **Sources:** oh-my-codex (OMX), mcp_agent_mail analysis (deferred), ChatGPT analysis, treehouse_rust, current codebase audit

---

## 1. Executive Summary

anti_subagent is a **runtime control plane** that replaces native subagent execution with independently spawned CLI sessions while preserving durable task, ownership, communication, lifecycle, workspace, and verification semantics.

**Core thesis preserved:** Every worker is a full, autonomous agent with durable identity. The hierarchy is invisible to peers. No auto-accept.

**Key learnings from three sources:**

| Source | Key Pattern | anti_subagent Integration |
|---|---|---|
| **oh-my-codex (OMX)** | RuntimeEngine + AuthorityLease + DispatchLog | Runtime protocol layer |
| **mcp_agent_mail** | Agent identity, messaging, file reservations, guard | AgentMailAdapter (not rebuild) |
| **treehouse_rust** | Workspace lease, process lifecycle, cleanup | TreehouseAdapter (existing) |

**Architecture invariant:**
> anti_subagent is a runtime control plane that replaces native subagent execution with independently spawned CLI sessions while preserving durable task, ownership, communication, lifecycle, workspace, and verification semantics.

---

## 2. Architecture v3

```
                    HUMAN
                      │
                      ▼
                LEAD CLAUDE
                      │
                 anti IPC
                      │
                      ▼
                ┌───────────┐
                │   ANTI    │
                │  daemon   │
                └─────┬─────┘
                      │
                 spawn Peer
                      │
                      ▼
                CLAUDE PEER
                      │
              ┌───────┴────────┐
              │                │
          edit code        anti report
              │                │
              ▼                ▼
         Treehouse ────────► ANTI
          workspace            │
                               ├─ validate task ownership
                               ├─ git show <commit>
                               ├─ verify profile
                               ├─ persist evidence
                               └─ notify Lead
                                      │
                                      ▼
                                   LEAD
```

**No external messaging system.** Code lives in Git. Messages live in anti daemon.

### Component Boundaries

```
anti_subagent daemon
    │
    ├── PeerManager                    ← PROCESS lifecycle (spawn/wait/terminate)
    │   ├── spawn(spec) → PeerHandle   ← creates OS process
    │   ├── wait(handle) → ExitStatus  ← blocks until exit
    │   ├── terminate(handle)           ← sends SIGTERM/SIGKILL (OS-native)
    │   └── CrashDetector              ← PID liveness monitoring
    │
    ├── TreehouseAdapter               ← WORKSPACE lifecycle (lease/worktree/cleanup)
    │   ├── acquire(holder, repo) → Lease
    │   ├── release_if_lease(lease)     ← cleanup + release workspace
    │   └── status() → PoolStatus
    │
    ├── RuntimeEngine                  ← ORCHESTRATION protocol
    │   ├── AuthorityLease             ← who controls this session
    │   ├── DispatchLog                ← task dispatch tracking
    │   ├── DispatchOutcome            ← evidence-based completion
    │   └── LifecycleBus               ← event emission + handlers
    │
    ├── TaskStateMachine
    │   ├── Staged pipeline (RECEIVED → ... → ACCEPTED)
    │   ├── Transition guards
    │   └── Revision tracking
    │
    ├── EvidenceStore
    │   ├── SQLite (WAL mode)
    │   ├── Verification results
    │   ├── Git diff/test output snapshots
    │   └── SHA-256 integrity
    │
    ├── ReportHandler                  ← Peer→anti report channel (new)
    │   ├── handle_report(task_id, status, commit, ...)
    │   ├── validate task ownership (no self-declared peer_id)
    │   ├── git show <commit> verification
    │   ├── trigger verify profile
    │   └── persist evidence + notify Lead
    │
    └── Scheduler
        ├── Task decomposition
        ├── Peer assignment by disposition
        └── Resource-aware parallelism
```

### Two Lease Types (Critical Distinction)

```
                 Peer
                  │
       ┌──────────┴──────────┐
       ▼                     ▼
Authority                Treehouse
  Lease                    Lease
       │                     │
   control               workspace
```

| Lease | Owner | Purpose | Lifecycle |
|---|---|---|---|
| **AuthorityLease** | RuntimeEngine | Who controls this session/task | acquire → renew → release (stale detection) |
| **Treehouse Lease** | TreehouseAdapter | Workspace ownership | acquire → release (process-independent) |

> **AgentMail Identity removed from MVP.** Peer identity is resolved from task ownership at report time, not from a separate identity system. mcp_agent_mail becomes a future adapter when cross-hierarchy messaging is needed.

---

## 3. What anti_subagent DOES NOT build

| Component | Built by | anti_subagent integration |
|---|---|---|
| Workspace isolation | treehouse_rust | TreehouseAdapter (existing) |
| Process lifecycle | treehouse_rust | PeerManager delegates to Treehouse |
| Agent-to-agent messaging | mcp_agent_mail | **Deferred to post-MVP adapter** |
| Agent identity (mail) | mcp_agent_mail | **Deferred to post-MVP adapter** |
| File reservations (mail) | mcp_agent_mail | **Deferred to post-MVP adapter** |

**anti_subagent builds:**
- RuntimeEngine (orchestration protocol)
- AuthorityLease (session ownership)
- DispatchLog + DispatchOutcome (task tracking)
- TaskStateMachine (staged pipeline)
- EvidenceStore (verification evidence)
- LifecycleBus (event emission)
- ReportHandler (Peer→anti report channel)
- Scheduler (parallel peer management)

---

## 4. Implementation Phases

### Phase 0: Safety & Correctness (COMPLETED)
- ✅ Replace pkill -f with PID-based termination
- ✅ Crash cleanup lifecycle with structured events
- ✅ VerifyProfile-based verification (no arbitrary commands)
- ✅ TCP read/write timeouts prevent deadlock
- ✅ Lock-free spawn_peer (split into phases)
- ✅ reconcile_on_start split into 3 phases
- ✅ verify_work split into 3 phases
- ✅ Watchdog deadlock fixed (split lock into read/write phases)

### Phase 1: Runtime Protocol (P0)

#### 1.0 anti report CLI (Peer → anti channel)

**Objective:** Enable peers to report task status back to anti daemon via the existing Unix socket IPC. No self-declared peer_id — daemon resolves identity from task ownership.

**Peer sends (no peer_id):**
```rust
ReportTask {
    task_id: TaskId,
    status: ReportStatus,  // completed | failed | progress | question
    commit: Option<GitSha>,
    message: Option<String>,
    error: Option<String>,
}
```

**Handler validates:**
1. `task_id` exists in `work_items` table
2. Task is assigned to a peer (ownership check)
3. If `status == completed` + commit: `git show <commit>` in workspace
4. Run verify profile against workspace
5. Transition work state (→ Submitted on success, → NeedsRevision on failure)
6. Emit event, notify Lead via IPC

**CLI subcommand:**
```bash
anti report --task <id> --status <completed|failed|progress|question> \
    [--commit <sha>] [--error <msg>] [--message <msg>]
```

**Files:**
- `crates/anti-cli/src/main.rs` (add `Report` variant + handler)
- `crates/anti-daemon/src/ipc.rs` (add `ReportTask` request)
- `crates/anti-daemon/src/report.rs` (NEW — handler logic)

**Tests:**
- Report accepted with valid task + commit
- Report rejected: task not found
- Report rejected: task not assigned to caller
- Report rejected: commit doesn't exist in workspace
- Report triggers verify profile
- Progress/question reports pass through without verification

#### 1.1 DispatchLog + DispatchOutcome

**Objective:** Track task dispatch with evidence-based completion outcomes.

**DispatchStatus:**
```rust
enum DispatchStatus {
    Pending,
    Notified,
    Delivered,
    Completed,
    Failed,
    Deferred,
}
```

**DispatchOutcome (9 outcomes from OMX):**
```rust
enum DispatchOutcome {
    DeliveredConfirmed,
    DeliveredUnconfirmed,
    CompletedConfirmed,
    CompletedUnverified,
    TargetMissing,
    TargetUnavailable,
    PreflightFailed,
    SendFailed,
    Timeout,
    Cancelled,
}
```

**Files:**
- `crates/anti-core/src/dispatch.rs` (NEW)
- `crates/anti-daemon/src/store.rs` (add dispatch_events table)

**Tests:**
- Dispatch lifecycle transitions
- Outcome recording
- Concurrent dispatch safety

#### 1.2 AuthorityLease

**Objective:** Session ownership model with acquire/renew/release semantics.

```rust
struct AuthorityLease {
    owner: Option<String>,
    lease_id: Option<String>,
    leased_until: Option<String>,
    stale: bool,
    stale_reason: Option<String>,
}

impl AuthorityLease {
    fn acquire(&mut self, owner, lease_id, leased_until) -> Result<(), AuthorityError>;
    fn renew(&mut self, owner, lease_id, leased_until) -> Result<(), AuthorityError>;
    fn release(&mut self, owner) -> Result<(), AuthorityError>;
    fn check_staleness(&mut self) -> bool;
}
```

**Files:**
- `crates/anti-core/src/authority.rs` (NEW)
- `crates/anti-daemon/src/store.rs` (add authority_leases table)

**Tests:**
- Acquire/release semantics
- Stale detection
- Concurrent authority claims

### Phase 2: AgentMail Integration — DEFERRED

> **Architecture decision (2026-08-17):** mcp_agent_mail is deferred to post-MVP.
> Anti does not need external messaging for the core orchestration loop.
> Code lives in Git (workspace). Messages live in anti daemon (Unix socket IPC).
> When cross-hierarchy messaging, persistent queues, or human messaging is needed,
> add mcp_agent_mail as an adapter layer: `Peer → anti → mcp_agent_mail → Lead`.

**Trigger conditions for re-evaluation:**
- Restart recovery needs persistent message queues
- Human operator needs to send messages to agents
- Cross-hierarchy communication required
- Thread/history persistence beyond JSONL events

### Phase 3: Peer Lifecycle (P2)

#### 3.1 PeerManager with Treehouse integration

**Objective:** Full peer lifecycle with workspace isolation.

```
spawn Peer
  → Treehouse.acquire() → WorkspaceLease
  → spawn Claude/Codex process
  → Peer READY

terminate Peer
  → SIGTERM → wait → SIGKILL if needed
  → Treehouse.return_if_lease()
  → workspace released

crash Peer
  → Reaper detects exit
  → PeerCrashDetected event
  → Treehouse cleanup
  → crash evidence persisted
  → Lead notified
```

**Files:**
- `crates/anti-daemon/src/peer_manager.rs` (NEW or refactor existing)
- `crates/anti-daemon/src/main.rs` (integrate PeerManager)

**Tests:**
- Spawn/terminate/crash lifecycle
- Workspace cleanup on crash
- Concurrent peer management

### Phase 4: Verification (P3)

#### 4.1 Evidence-gated completion

**Objective:** No accept without verification. Evidence must be comprehensive.

```rust
struct VerificationResult {
    status: VerifyStatus,
    profile: VerifyProfile,
    test_output: Option<String>,
    build_output: Option<String>,
    diagnostics: Vec<String>,
    git_diff: Option<String>,
    git_sha: Option<String>,
    claims_verified: Vec<ClaimVerification>,
    timestamp: String,
}
```

**Flow:**
```
EXECUTED → VERIFYING → VERIFIED → ACCEPTED
              ↓
           REJECTED → FIXING → EXECUTING
```

**Files:**
- `crates/anti-core/src/work.rs` (already has VerifyProfile)
- `crates/anti-daemon/src/main.rs` (verify handler)

**Tests:**
- Verify pass/fail
- Evidence completeness
- Stale evidence detection

### Phase 5: Recovery & Replay (P4)

#### 5.1 Restart recovery

**Objective:** Daemon restart recovers from previous state.

```
SQLite → find RUNNING sessions → check PID liveness
  → dead → CRASHED → Treehouse cleanup → recovery event
  → alive → reconnect
```

#### 5.2 Lead handoff

**Objective:** Context degradation after compactions.

```
ContextDegradation detected
  → capture active tasks, decisions, lessons
  → spawn new Lead with HandoffContext
  → archive old Lead
```

**Files:**
- `crates/anti-daemon/src/recovery.rs` (NEW)
- `crates/anti-daemon/src/handoff.rs` (exists, needs integration)

---

## 5. File Change Map

### New Files

| File | Purpose |
|---|---|
| `crates/anti-daemon/src/report.rs` | ReportHandler — Peer→anti report channel |
| `crates/anti-core/src/dispatch.rs` | DispatchLog, DispatchOutcome |
| `crates/anti-core/src/authority.rs` | AuthorityLease, AuthorityError |
| `crates/anti-daemon/src/peer_manager.rs` | Peer lifecycle with Treehouse |
| `crates/anti-daemon/src/recovery.rs` | Restart recovery |
| `scripts/slp-e2e.sh` | E2E test script |

### Modified Files

| File | Changes |
|---|---|
| `crates/anti-core/src/lib.rs` | Add dispatch, authority modules |
| `crates/anti-core/src/events.rs` | Add lifecycle event types |
| `crates/anti-daemon/src/store.rs` | Add dispatch_events, authority_leases tables |
| `crates/anti-daemon/src/main.rs` | Integrate RuntimeEngine, PeerManager |
| `crates/anti-daemon/src/ipc.rs` | Add ReportTask, dispatch/authority IPC requests |
| `crates/anti-cli/src/main.rs` | Add `report` subcommand + dispatch/authority CLI commands |
| `crates/anti-workspace/src/lib.rs` | Add PeerManager integration |

### Deferred Files (post-MVP, when mcp_agent_mail adapter is needed)

| File | Purpose |
|---|---|
| `crates/anti-adapters/src/mail.rs` | AgentMailAdapter (future) |

---

## 6. Test Matrix

### Unit Tests (existing + new)

| Module | Tests | Status |
|---|---|---|
| anti-core/work.rs | 12 tests (state machine, transitions) | ✅ |
| anti-core/loopprev.rs | 4 tests (sliding window, hysteresis) | ✅ |
| anti-core/arbiter.rs | 2 tests (rubric scoring) | ✅ |
| anti-core/capsule.rs | 2 tests (budget enforcement) | ✅ |
| anti-core/dispatch.rs | NEW — dispatch lifecycle | TODO |
| anti-core/authority.rs | NEW — lease semantics | TODO |
| anti-daemon/store.rs | 2 tests (work items, overdue) | ✅ |
| anti-workspace/cas.rs | 3 tests (CAS write, lock) | ✅ |

### Integration Tests (T1-T64)

| Phase | Tests | Status |
|---|---|---|
| Phase 0 (T1-T6) | Safety, process, treehouse | ✅ |
| Verify Gate (T7-T10) | Accept/verify/fake claim | TODO |
| Evidence (T11-T13) | SHA mismatch, stale evidence | TODO |
| Writer/Verifier (T14-T18) | Disposition contracts | TODO |
| Staged Pipeline (T19-T23) | Transitions, fix loop | TODO |
| Lifecycle Events (T24-T27) | Event ordering, handler crash | TODO |
| Restart Recovery (T28-T30) | PID reuse, dead peer | TODO |
| Treehouse Safety (T31-T34) | Wrong lease, double return | TODO |
| Parallel Scheduling (T35-T37) | DAG, concurrency limits | TODO |
| Model Routing (T38) | Disposition × complexity | TODO |
| SLP Behavioral (T39-T42) | Peer autonomy, challenge | TODO |
| E2E (T43) | Full SLP flow | TODO |
| Chaos (T44-T60) | Failure injection | TODO |

---

## 7. Definition of Done

### Phase 0 (COMPLETED)
- ✅ 0 pkill -f
- ✅ 0 orphan worktrees
- ✅ 0 orphan peer processes
- ✅ 0 accidental Lead kills
- ✅ verify gate enforced
- ✅ No external I/O under store Mutex

### Phase 1 (Runtime Protocol)
- [ ] `anti report` CLI subcommand (no self-declared peer_id)
- [ ] `ReportTask` IPC request type
- [ ] ReportHandler with task ownership validation
- [ ] git show <commit> verification
- [ ] Verify profile integration on report
- [ ] DispatchLog with status lifecycle
- [ ] DispatchOutcome (9 evidence-based outcomes)
- [ ] AuthorityLease with acquire/renew/release
- [ ] Stale detection
- [ ] Unit tests pass

### Phase 2 (AgentMail Integration) — DEFERRED
- [ ] Re-evaluate when cross-hierarchy messaging is needed
- [ ] AgentMailAdapter trait (future)
- [ ] Send/receive protocol messages (future)
- [ ] Inbox/outbox (future)
- [ ] Ack lifecycle (future)

### Phase 3 (Peer Lifecycle)
- [ ] PeerManager with Treehouse
- [ ] Spawn/terminate/crash lifecycle
- [ ] Workspace cleanup on crash
- [ ] Concurrent peer management
- [ ] Integration tests pass

### Phase 4 (Verification)
- [ ] Verify pass/fail
- [ ] Evidence completeness
- [ ] Stale evidence detection
- [ ] E2E test: spawn → execute → verify → accept

### Phase 5 (Recovery)
- [ ] Restart recovery
- [ ] Lead handoff
- [ ] PID reuse safety

---

## 8. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Treehouse API breaking changes | Keep CLI fallback adapter |
| SQLite migration failures | Versioned migrations + backup |
| Windows named pipe transport | TCP loopback as fallback |
| Concurrent peer state corruption | Optimistic-lock + CAS writes |
| Evidence staleness | Git SHA + timestamp validation |
| Peer impersonation via self-declared ID | No peer_id in ReportTask; daemon resolves from task ownership |
| Peer bypasses `anti report` | Daemon reaps process → marks Crashed → Lead notified; report is optimization, not requirement |

---

## 9. Next Steps

1. **Implement Phase 1.0** — `anti report` CLI subcommand + ReportHandler
2. **Run integration tests T1-T6** — Verify Phase 0 safety still holds
3. **Implement Phase 1.1** — DispatchLog + AuthorityLease
4. **Implement Phase 3** (Peer Lifecycle) — PeerManager with Treehouse
5. **Run integration tests T7-T18** — Verify verify gate + evidence
6. **Implement Phase 4** (Verification) — Evidence-gated completion
7. **Run E2E test T43** — Full SLP flow: spawn → work → report → verify → accept
8. **Implement Phase 5** (Recovery) — Restart + handoff
9. **Run chaos tests T44-T60** — Failure injection
10. **Re-evaluate Phase 2** — Only if cross-hierarchy messaging needed

---

> **Bottom line:** anti_subagent v3 is a runtime control plane that replaces native subagent execution with independently spawned CLI sessions. Peers report back via `anti report` over the existing Unix socket IPC — no external messaging system needed. Code lives in Git. Messages live in the daemon. The peer's entire vocabulary is: task, workspace, anti report.
