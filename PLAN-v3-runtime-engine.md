# anti_subagent v3 — Runtime Engine Architecture Plan

> **Date:** 2026-08-17 · **Status:** Draft for team review
> **Sources:** oh-my-codex (OMX), mcp_agent_mail, ChatGPT analysis, treehouse_rust, current codebase audit

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
                anti CLI / daemon
                      │
              ┌───────┴────────┐
              │                │
          Workflow          Policy
              │                │
              └───────┬────────┘
                      ▼
                 Scheduler
                      │
             ┌────────┴────────┐
             ▼                 ▼
          Peer A             Peer B
             │                 │
       ┌─────┴─────┐     ┌─────┴─────┐
       │            │     │           │
  Claude CLI   Treehouse  Codex CLI Treehouse
       │                         │
       └──────────┬──────────────┘
                  ▼
             Evidence
                  │
                  ▼
              Verifier
                  │
                  ▼
               ACCEPT


              MCP Agent Mail
                     ▲
                     │
        ┌────────────┼────────────┐
        │            │            │
      Lead         Peer A       Peer B
```

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
    ├── AgentMailAdapter               ← messaging (via mcp_agent_mail)
    │   ├── send/receive protocol messages
    │   ├── inbox/outbox
    │   └── thread correlation
    │
    └── Scheduler
        ├── Task decomposition
        ├── Peer assignment by disposition
        └── Resource-aware parallelism
```

### Three Lease Types (Critical Distinction)

```
                 Peer
                  │
       ┌──────────┼──────────┐
       ▼          ▼          ▼
Authority     Agent Mail   Treehouse
  Lease        Identity       Lease
       │          │          │
   control       talk     workspace
```

| Lease | Owner | Purpose | Lifecycle |
|---|---|---|---|
| **AuthorityLease** | RuntimeEngine | Who controls this session/task | acquire → renew → release (stale detection) |
| **AgentMail Identity** | mcp_agent_mail | Agent-to-agent messaging identity | persistent, cross-session |
| **Treehouse Lease** | TreehouseAdapter | Workspace ownership | acquire → release (process-independent) |

---

## 3. What anti_subagent DOES NOT build

| Component | Built by | anti_subagent integration |
|---|---|---|
| Agent messaging | mcp_agent_mail | AgentMailAdapter (thin wrapper) |
| File reservations | mcp_agent_mail | AgentMailAdapter |
| Agent directory | mcp_agent_mail | AgentMailAdapter |
| Message history | mcp_agent_mail | AgentMailAdapter |
| Workspace isolation | treehouse_rust | TreehouseAdapter (existing) |
| Process lifecycle | treehouse_rust | PeerManager delegates to Treehouse |

**anti_subagent builds:**
- RuntimeEngine (orchestration protocol)
- AuthorityLease (session ownership)
- DispatchLog + DispatchOutcome (task tracking)
- TaskStateMachine (staged pipeline)
- EvidenceStore (verification evidence)
- LifecycleBus (event emission)
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

### Phase 2: AgentMail Integration (P1)

#### 2.1 AgentMailAdapter

**Objective:** Thin adapter to mcp_agent_mail for messaging.

```rust
trait AgentMailAdapter {
    fn send_task(&self, from: &str, to: &str, task: &str) -> Result<MessageId>;
    fn send_fix_request(&self, from: &str, to: &str, task: &str) -> Result<MessageId>;
    fn send_verify_result(&self, from: &str, to: &str, result: &VerificationResult) -> Result<MessageId>;
    fn notify_crash(&self, peer_id: &str, crash_info: &CrashInfo) -> Result<MessageId>;
    fn notify_completion(&self, peer_id: &str, task_id: &str) -> Result<MessageId>;
    fn fetch_inbox(&self, agent: &str) -> Result<Vec<Message>>;
    fn acknowledge(&self, agent: &str, message_id: &str) -> Result<()>;
}
```

**Files:**
- `crates/anti-adapters/src/mail.rs` (NEW)
- Integration with mcp_agent_mail MCP server

**Tests:**
- Send/receive protocol messages
- Inbox filtering
- Ack lifecycle

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
| `crates/anti-core/src/dispatch.rs` | DispatchLog, DispatchOutcome |
| `crates/anti-core/src/authority.rs` | AuthorityLease, AuthorityError |
| `crates/anti-daemon/src/peer_manager.rs` | Peer lifecycle with Treehouse |
| `crates/anti-daemon/src/recovery.rs` | Restart recovery |
| `crates/anti-adapters/src/mail.rs` | AgentMailAdapter |
| `scripts/slp-e2e.sh` | E2E test script |

### Modified Files

| File | Changes |
|---|---|
| `crates/anti-core/src/lib.rs` | Add dispatch, authority modules |
| `crates/anti-core/src/events.rs` | Add lifecycle event types |
| `crates/anti-daemon/src/store.rs` | Add dispatch_events, authority_leases tables |
| `crates/anti-daemon/src/main.rs` | Integrate RuntimeEngine, PeerManager |
| `crates/anti-daemon/src/ipc.rs` | Add dispatch/authority IPC requests |
| `crates/anti-cli/src/main.rs` | Add dispatch/authority CLI commands |
| `crates/anti-workspace/src/lib.rs` | Add PeerManager integration |

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
- [ ] DispatchLog with status lifecycle
- [ ] DispatchOutcome (9 evidence-based outcomes)
- [ ] AuthorityLease with acquire/renew/release
- [ ] Stale detection
- [ ] Unit tests pass

### Phase 2 (AgentMail Integration)
- [ ] AgentMailAdapter trait
- [ ] Send/receive protocol messages
- [ ] Inbox/outbox
- [ ] Ack lifecycle
- [ ] Integration tests pass

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
| mcp_agent_mail integration complexity | Thin adapter, not rebuild |
| Windows named pipe transport | TCP loopback as fallback |
| Concurrent peer state corruption | Optimistic-lock + CAS writes |
| Evidence staleness | Git SHA + timestamp validation |

---

## 9. Next Steps

1. **Implement Phase 1** (Runtime Protocol) — DispatchLog + AuthorityLease
2. **Implement Phase 2** (AgentMail Integration) — AgentMailAdapter
3. **Run integration tests T1-T18** — Verify Phase 0 safety
4. **Implement Phase 3** (Peer Lifecycle) — PeerManager with Treehouse
5. **Run integration tests T19-T42** — Verify staged pipeline
6. **Implement Phase 4** (Verification) — Evidence-gated completion
7. **Run E2E test T43** — Full SLP flow
8. **Implement Phase 5** (Recovery) — Restart + handoff
9. **Run chaos tests T44-T60** — Failure injection

---

> **Bottom line:** anti_subagent v3 is a runtime control plane that makes independent CLI sessions behave as first-class agents with authority, task lifecycle, crash recovery, evidence-gated completion, and trustworthy verification.
