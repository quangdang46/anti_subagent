# anti_subagent v2 — Comprehensive Implementation Plan

> **Date:** 2026-08-17 · **Status:** Draft for team review
> **Sources:** OMC analysis, ChatGPT architecture review, treehouse_rust API, current codebase audit

---

## 1. Executive Summary

anti_subagent is an **SLP (Supervisor → Lead → Peer) orchestration daemon** for coding agents. After analyzing oh-my-claudecode (OMC) and treehouse_rust, we identified critical bugs and architectural gaps that must be addressed before production use.

**Core thesis preserved:** Every worker is a full, autonomous agent with durable identity. The hierarchy is invisible to peers. No auto-accept.

**Key learnings from OMC:**
- Writer-Verifier separation (no self-approve)
- Staged pipeline (plan → exec → verify → fix loop)
- Evidence-based verification (not just "agent said done")
- Hook system for lifecycle management
- Model routing by task complexity

**Key learnings from treehouse_rust:**
- Delegate workspace/process lifecycle to Treehouse (not pkill -f)
- Treehouse owns the tree; anti_subagent owns the agent
- Lease identity survives process death (safe cleanup)

---

## 2. Current State Audit

### 2.1 What Works

| Component | Status | Notes |
|---|---|---|
| WorkItem state machine | ✅ | 7 states, revision bumping, evidence gating |
| Watchdog thread | ✅ | 15s scan, ReviewEscalated events |
| Loop prevention | ✅ | Sliding window + hysteresis + cooldown |
| CAS writes | ✅ | SHA-256 baseline + atomic lock |
| Bounded capsule | ✅ | 64KB context cap |
| Read-only arbiter | ✅ | Rubric scorer, no FS access |
| Windows TCP transport | ✅ | Loopback on 127.0.0.1 |
| Treehouse auto-detect | ✅ | PATH detection + .exe fallback |
| Guard classification | ✅ | Deny delegation-shaped tools |

### 2.2 Critical Bugs

| Bug | Severity | Impact |
|---|---|---|
| `pkill -f` regex match | 🔴 P0 | Can kill current Claude session |
| No cleanup on peer crash | 🔴 P0 | Orphaned worktrees, leaked processes |
| No verify stage before accept | 🔴 P0 | "Done" claim accepted without evidence |
| Re-submit after reject | ✅ Fixed | INSERT OR REPLACE |
| Embedded repos in git | ✅ Fixed | .gitignore |

### 2.3 Architectural Gaps

| Gap | Source | Priority |
|---|---|---|
| No staged pipeline | OMC analysis | P1 |
| No evidence model (just SHA-256) | OMC + ChatGPT | P1 |
| No writer-verifier separation | OMC analysis | P1 |
| No disposition contracts | ChatGPT analysis | P2 |
| No lifecycle event bus | ChatGPT analysis | P2 |
| No model routing | OMC analysis | P3 |
| No parallel scheduler | OMC analysis | P3 |
| No restart recovery | ChatGPT analysis | P3 |
| No Lead handoff | anti_subagent thesis | P4 |

---

## 3. Architecture v2

### 3.1 Target Architecture

```
                         HUMAN
                           │
                           ▼
                     ┌──────────┐
                     │SUPERVISOR│ ← read-only, on-demand
                     └────┬─────┘
                          │
                          ▼
                     ┌──────────┐
                     │   LEAD   │ ← planning, delegation, integration
                     └────┬─────┘
                          │
           ┌──────────────┼──────────────┐
           │              │              │
        ┌──▼──┐       ┌──▼──┐       ┌──▼──┐
        │PEER │       │PEER │       │PEER │ ← autonomous agents
        │Eng. │       │Scout│       │Rev. │
        └──┬──┘       └──┬──┘       └──┬──┘
           │              │              │
           └──────────────┼──────────────┘
                          │
                    ┌─────▼─────┐
                    │  VERIFY   │ ← evidence-based, read-only
                    └─────┬─────┘
                          │
                    ┌─────┴─────┐
                    │           │
                  PASS         FAIL
                    │           │
                    ▼           ▼
                  ACCEPT       FIX
                               │
                               └──────→ VERIFY
```

### 3.2 Component Boundaries

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
    ├── LifecycleBus
    │   ├── Event emission
    │   ├── Handler dispatch
    │   └── Recovery triggers
    │
    └── Scheduler
        ├── Task decomposition
        ├── Peer assignment by disposition
        └── Resource-aware parallelism
```

**Boundary contract:**
- PeerManager owns process lifecycle (spawn, wait, terminate by PID)
- TreehouseAdapter owns workspace lifecycle (lease, worktree, cleanup)
- On peer crash: PeerManager detects → emits PeerCrashed → handler calls TreehouseAdapter.release_if_lease()
- Do NOT let Treehouse become the "kill API" — it is a workspace manager, not a process manager

---

## 4. Implementation Phases

### Phase 0: Safety & Correctness (P0) — 1 week

#### 4.0.1 Fix pkill -f → Treehouse lifecycle

**Current (DANGEROUS):**
```rust
// crates/anti-daemon/src/main.rs:629
let _ = std::process::Command::new("pkill")
    .args(["-f", &format!("claude -p.*{}", worktree.path.display())])
    .status();
```

**Target:**
```rust
// Delegate to Treehouse for workspace cleanup
let treehouse = Treehouse::new(resolve_treehouse());
let _ = treehouse.return_if_lease(&lease_id, &worktree_path, &repo_path);
```

**Files to modify:**
- `crates/anti-daemon/src/main.rs` — replace pkill with Treehouse.return
- `crates/anti-workspace/src/lib.rs` — add return_if_lease method

**Verification:**
- Spawn peer → kill peer process → verify worktree cleaned up
- Verify current session not affected

#### 4.0.2 Crash cleanup lifecycle

**Current:**
```rust
// Reaper marks CRASHED but no cleanup
fn mark_exit(&mut self, id: &str, exit_ok: bool) { ... }
```

**Target:**
```rust
// Full crash lifecycle
PeerCrashDetected {
    peer_id,
    pid,
    exit_code,
    workspace_lease_id,
} → {
    1. capture exit information
    2. Treehouse.return (cleanup workspace)
    3. persist crash evidence
    4. notify Lead
    5. Lead decides: retry / replace / abort
}
```

**Files to modify:**
- `crates/anti-daemon/src/main.rs` — crash handler
- `crates/anti-core/src/events.rs` — new event types

#### 4.0.3 Verify stage before accept

**Current:**
```rust
// accept requires Verified state, but verify is manual
Request::ReviewWork { id, verdict: "accept", note } => {
    if w.state != WorkItemState::Verified {
        return err("accept requires Verified state");
    }
    // ... accept
}
```

**Target — VerificationProfile (NOT arbitrary commands):**
```rust
/// Predefined verification profiles — no arbitrary commands from caller.
/// Verifier runs the profile, not the commands.
pub enum VerifyProfile {
    /// cargo fmt --check + cargo clippy + cargo test + cargo build
    Full,
    /// cargo fmt --check + cargo clippy + cargo test
    Check,
    /// cargo test only
    Test,
    /// cargo build only
    Build,
    /// Custom profile defined in project config (not arbitrary CLI)
    Named(String),
}

struct VerificationResult {
    status: VerifyStatus, // PASS | FAIL | INCOMPLETE
    profile: VerifyProfile,
    test_output: Option<String>,
    build_output: Option<String>,
    diagnostics: Vec<String>,
    git_diff: Option<String>,
    claims_verified: Vec<String>,
    timestamp: String,
}

// Verify command — caller selects profile, not commands
Request::VerifyWork {
    id: String,
    profile: VerifyProfile,
}

// After verify → state becomes Verified
// Then accept is allowed
```

**Why profiles, not commands:**
- Prevents execution escape hatch (caller can't inject arbitrary CLI)
- Verifier runs standardized checks, not caller-specified commands
- Evidence is comparable across runs (same profile = same checks)
- Configurable via project config (`.anti_subagent/verify.toml`) but not per-call

**Files to modify:**
- `crates/anti-core/src/work.rs` — VerifyProfile, VerificationResult
- `crates/anti-daemon/src/ipc.rs` — VerifyWork request
- `crates/anti-daemon/src/main.rs` — verify handler
- `crates/anti-cli/src/main.rs` — verify command with profile flag

---

### Phase 1: Execution Architecture (P1) — 2 weeks

#### 4.1.1 Staged task state machine

**Current states:**
```
Pending → InProgress → Submitted → Verified → Accepted
```

**Target states:**
```
RECEIVED → EXPLORED → PLANNED → EXECUTING → EXECUTED → VERIFYING → VERIFIED → ACCEPTED
                                      │                        │
                                      └── FAILED               └── REJECTED → FIXING → EXECUTING
                                                                         │
                                                                         └── EXHAUSTED → CANCELLED
```

**Files to modify:**
- `crates/anti-core/src/work.rs` — expand WorkItemState
- `crates/anti-daemon/src/store.rs` — migration for new states

#### 4.1.2 Evidence model

**Current:**
```rust
struct EvidenceRef {
    sha256: String,
    artifact_path: String,
    produced_at: String,
}
```

**Target:**
```rust
struct EvidenceRecord {
    // Integrity
    artifact_sha256: String,
    artifact_path: String,
    
    // Verification evidence
    test_output: Option<String>,
    test_exit_code: Option<i32>,
    build_output: Option<String>,
    build_exit_code: Option<i32>,
    lint_output: Option<String>,
    diagnostics: Vec<String>,
    
    // Git state at verification time
    git_sha: Option<String>,
    git_diff: Option<String>,
    git_status: Option<String>,
    
    // Acceptance criteria verification
    claims: Vec<ClaimVerification>,
    
    // Metadata
    produced_at: String,
    verified_at: Option<String>,
    verified_by: Option<String>,  // peer_id of verifier
}

struct ClaimVerification {
    claim: String,
    status: VerifyStatus,  // VERIFIED | PARTIAL | MISSING
    evidence: Option<String>,
}
```

**Files to modify:**
- `crates/anti-core/src/work.rs` — new structs
- `crates/anti-daemon/src/store.rs` — new table

#### 4.1.3 Writer-Verifier separation

**Disposition contracts:**

| Disposition | CAN | CANNOT |
|---|---|---|
| Engineer | read, write, test, edit | approve own work |
| Architect | read, design, plan | modify implementation |
| Scout | read, search, inspect | modify source |
| Reviewer | inspect, test, challenge | silently modify |
| ProofAuditor | read, test, inspect git/logs | modify source |
| Shadow | read, observe | modify anything |

**Files to create:**
- `crates/anti-core/src/disposition.rs` — disposition contracts

---

### Phase 2: SLP-Native Orchestration (P2) — 2 weeks

#### 4.2.1 Disposition contracts (formal)

```rust
pub struct DispositionContract {
    pub name: Disposition,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub can_approve_own_work: bool,
    pub requires_evidence: bool,
    pub max_concurrent: usize,
}

impl DispositionContract {
    pub fn engineer() -> Self { ... }
    pub fn proof_auditor() -> Self { ... }
    pub fn scout() -> Self { ... }
    pub fn reviewer() -> Self { ... }
}
```

#### 4.2.2 Lifecycle event bus

```rust
pub enum LifecycleEvent {
    // Peer events
    PeerSpawned { peer_id, pid, workspace },
    PeerReady { peer_id },
    PeerCrashed { peer_id, exit_code },
    PeerStopped { peer_id },
    
    // Task events
    TaskReceived { task_id },
    TaskExecuting { task_id, peer_id },
    TaskCompleted { task_id, peer_id },
    TaskFailed { task_id, peer_id, error },
    
    // Verification events
    VerificationStarted { task_id },
    VerificationPassed { task_id },
    VerificationFailed { task_id, findings },
    
    // Workspace events
    WorkspaceAcquired { lease_id, path },
    WorkspaceReleased { lease_id },
    WorkspaceCleaned { lease_id },
    
    // Lead events
    LeadHandoff { from_lead, to_lead, reason },
}
```

**Files to create:**
- `crates/anti-core/src/events.rs` — expand with lifecycle events
- `crates/anti-daemon/src/bus.rs` — event bus implementation

---

### Phase 3: Scheduling & Routing (P3) — 2 weeks

#### 4.3.1 Model routing (config-based, not hard-coded)

```rust
/// Capability tier — disposition × complexity → required capability level.
/// Model name resolution is config/provider-specific, NOT hard-coded in core.
pub enum CapabilityTier {
    /// Quick lookups, search, narrow checks
    Lightweight,
    /// Standard implementation, debugging, reviews
    Standard,
    /// Architecture, deep analysis, complex refactors
    Heavyweight,
}

pub struct ModelRoute {
    pub disposition: Disposition,
    pub complexity: Complexity,     // LOW | MEDIUM | HIGH
    pub capability: CapabilityTier, // resolved from disposition × complexity
    pub provider: String,           // "claude" | "codex" | "opencode" (config)
    pub model: String,              // resolved from provider + capability (config)
}

/// Route resolution: disposition × complexity → capability → model
/// Model names come from provider config, NOT from anti-core.
impl ModelRoute {
    pub fn resolve(disposition: Disposition, complexity: Complexity, config: &ProviderConfig) -> Self {
        let capability = match (&disposition, &complexity) {
            (Scout, _) => CapabilityTier::Lightweight,
            (Engineer, Complexity::Low) => CapabilityTier::Standard,
            (Engineer, Complexity::High) => CapabilityTier::Heavyweight,
            (ProofAuditor, _) => CapabilityTier::Heavyweight,
            _ => CapabilityTier::Standard,
        };
        let (provider, model) = config.resolve(&capability);
        Self { disposition, complexity, capability, provider, model }
    }
}

/// Provider config — loaded from .anti_subagent/providers.toml
/// NOT hard-coded in anti-core
pub struct ProviderConfig {
    pub providers: HashMap<String, ProviderTier>,
}

pub struct ProviderTier {
    pub lightweight: String,  // model name for lightweight tier
    pub standard: String,     // model name for standard tier
    pub heavyweight: String,  // model name for heavyweight tier
}
```

#### 4.3.2 Parallel scheduler

```rust
pub struct Scheduler {
    max_concurrent_peers: usize,
    task_graph: DagGraph,
    resource_monitor: ResourceMonitor,
}

impl Scheduler {
    pub fn schedule(&self, tasks: Vec<Task>) -> Schedule {
        // 1. Build dependency graph
        // 2. Find independent sets
        // 3. Check resource availability
        // 4. Assign peers with workload balancing
        // 5. Return schedule with parallel groups
    }
}
```

---

### Phase 4: Reliability (P4) — 2 weeks

#### 4.4.1 Restart recovery

```rust
pub fn recover_on_restart(store: &mut Store) {
    // 1. Find all RUNNING sessions
    let running = store.find_by_status(AgentStatus::Running);
    
    for agent in running {
        // 2. Check PID liveness
        let alive = check_pid_alive(agent.pid);
        
        if !alive {
            // 3. Dead process → recover
            store.mark_crashed(&agent.id);
            let lease = store.get_workspace_lease(&agent.id);
            
            // 4. Cleanup via Treehouse
            if let Some(lease) = lease {
                treehouse.return_if_lease(&lease.lease_id, &lease.path, &repo);
            }
            
            // 5. Notify Lead
            emit_event(LifecycleEvent::PeerCrashed {
                peer_id: agent.id,
                exit_code: None,
            });
        }
    }
}
```

#### 4.4.2 Lead handoff

```rust
pub struct LeadHandoff {
    pub from_lead: String,
    pub to_lead: String,
    pub reason: HandoffReason,
    pub context: HandoffContext,
    pub lessons: Vec<Lesson>,
}

pub enum HandoffReason {
    ContextDegradation { compactions: u32 },
    ManualHandoff,
    SupervisorDecision,
}

pub struct HandoffContext {
    pub active_tasks: Vec<Task>,
    pub pending_reviews: Vec<Review>,
    pub decisions: Vec<Decision>,
    pub open_questions: Vec<Question>,
}
```

---

## 5. File Change Map

### Phase 0 (Safety)

| File | Action | Description |
|---|---|---|
| `crates/anti-daemon/src/main.rs` | MODIFY | Replace pkill with Treehouse.return |
| `crates/anti-daemon/src/main.rs` | MODIFY | Add crash cleanup handler |
| `crates/anti-core/src/work.rs` | MODIFY | Add VerificationResult struct |
| `crates/anti-daemon/src/ipc.rs` | MODIFY | Add VerifyWork request |
| `crates/anti-cli/src/main.rs` | MODIFY | Add verify command |
| `crates/anti-cli/src/commands.rs` | MODIFY | Add verify handler |

### Phase 1 (Execution)

| File | Action | Description |
|---|---|---|
| `crates/anti-core/src/work.rs` | MODIFY | Expand WorkItemState |
| `crates/anti-core/src/work.rs` | CREATE | EvidenceRecord struct |
| `crates/anti-daemon/src/store.rs` | MODIFY | Migration + new queries |
| `crates/anti-core/src/disposition.rs` | CREATE | Disposition contracts |

### Phase 2 (Orchestration)

| File | Action | Description |
|---|---|---|
| `crates/anti-core/src/events.rs` | MODIFY | Lifecycle events |
| `crates/anti-daemon/src/bus.rs` | CREATE | Event bus |
| `crates/anti-daemon/src/scheduler.rs` | CREATE | Task scheduler |

### Phase 3 (Scheduling)

| File | Action | Description |
|---|---|---|
| `crates/anti-core/src/routing.rs` | CREATE | Model routing |
| `crates/anti-daemon/src/scheduler.rs` | MODIFY | Parallel scheduling |

### Phase 4 (Reliability)

| File | Action | Description |
|---|---|---|
| `crates/anti-daemon/src/main.rs` | MODIFY | Recovery logic |
| `crates/anti-daemon/src/handoff.rs` | CREATE | Lead handoff |

---

## 6. Test Plan

### Phase 0 Tests

```rust
#[test]
fn spawn_peer_then_return_cleans_workspace() {
    // 1. Spawn peer
    // 2. Kill peer process
    // 3. Verify worktree cleaned up via Treehouse
    // 4. Verify current session unaffected
}

#[test]
fn crash_triggers_cleanup_and_notification() {
    // 1. Spawn peer
    // 2. Kill peer process (simulating crash)
    // 3. Wait for reaper to detect
    // 4. Verify cleanup event emitted
    // 5. Verify Lead notified
}

#[test]
fn verify_stage_produces_evidence() {
    // 1. Create work item
    // 2. Submit with artifact
    // 3. Run verify with test commands
    // 4. Verify evidence recorded
    // 5. Verify state = Verified
}
```

### Phase 1 Tests

```rust
#[test]
fn staged_pipeline_transitions_correctly() {
    // 1. RECEIVED → EXPLORED → PLANNED → EXECUTING
    // 2. EXECUTING → EXECUTED → VERIFYING → VERIFIED
    // 3. VERIFIED → ACCEPTED
}

#[test]
fn verify_fail_triggers_fix_loop() {
    // 1. VERIFYING → REJECTED
    // 2. REJECTED → FIXING → EXECUTING
    // 3. Loop bounded by max_revisions
}
```

---

## 7. Migration Strategy

### 7.1 Database Migration

```sql
-- Add new columns to work_items
ALTER TABLE work_items ADD COLUMN verify_status TEXT;
ALTER TABLE work_items ADD COLUMN verify_test_output TEXT;
ALTER TABLE work_items ADD COLUMN verify_build_output TEXT;
ALTER TABLE work_items ADD COLUMN verify_diagnostics TEXT;
ALTER TABLE work_items ADD COLUMN verify_git_sha TEXT;
ALTER TABLE work_items ADD COLUMN verify_git_diff TEXT;
ALTER TABLE work_items ADD COLUMN verify_claims TEXT;

-- New table for evidence records
CREATE TABLE evidence_records (
    id TEXT PRIMARY KEY,
    work_item_id TEXT NOT NULL,
    artifact_sha256 TEXT NOT NULL,
    test_output TEXT,
    test_exit_code INTEGER,
    build_output TEXT,
    build_exit_code INTEGER,
    git_sha TEXT,
    git_diff TEXT,
    claims_verified TEXT,
    produced_at TEXT NOT NULL,
    verified_at TEXT,
    verified_by TEXT
);
```

### 7.2 Backward Compatibility

- Existing work items without verification → state stays Submitted
- New verify command required before accept
- Old accept path blocked until verify implemented

---

## 8. Risk Assessment

| Risk | Mitigation | Impact |
|---|---|---|
| Treehouse API breaking changes | Keep CLI fallback | Medium |
| SQLite migration failures | Versioned migrations + backup | High |
| Performance regression | Benchmark before/after | Medium |
| Breaking existing tests | Run full test suite after each phase | High |
| Windows compatibility | Test on Windows for each phase | Medium |

---

## 9. Success Criteria

### Phase 0
- [ ] No unsafe pattern-based process termination anywhere in production code
- [ ] PeerManager owns process lifecycle, TreehouseAdapter owns workspace lifecycle
- [ ] Crash cleanup: PeerManager detects → PeerCrashed event → TreehouseAdapter releases
- [ ] VerifyProfile-based verification (no arbitrary command execution)
- [ ] All existing tests pass
- [ ] New safety tests pass

### Phase 1
- [ ] Staged pipeline implemented
- [ ] Evidence model complete
- [ ] Disposition contracts enforced
- [ ] E2E test: spawn → execute → verify → accept

### Phase 2
- [ ] Lifecycle event bus operational
- [ ] Disposition contracts enforced
- [ ] Event logging complete

### Phase 3
- [ ] Model routing by disposition + complexity
- [ ] Parallel scheduling working
- [ ] Resource monitoring

### Phase 4
- [ ] Restart recovery verified
- [ ] Lead handoff mechanism
- [ ] Supervisor on-demand

---

## 10. Appendix: OMC Patterns Learned

| Pattern | OMC Implementation | anti_subagent v2 |
|---|---|---|
| Writer-Verifier separation | executor codes, verifier checks (read-only) | Engineer codes, ProofAuditor verifies (read-only) |
| Staged pipeline | plan → prd → exec → verify → fix | RECEIVED → EXPLORE → PLAN → EXECUTE → VERIFY → ACCEPT |
| Evidence verification | Fresh test output, lsp_diagnostics | test_output, build_output, git_diff, diagnostics |
| Hook system | 20 hooks on 11 lifecycle events | LifecycleBus with 15+ event types |
| Model routing | haiku → sonnet → opus by complexity | disposition × complexity → capability tier → config-resolved model |
| Kill safety | SubagentStop hook cleanup | Treehouse.return() |
| State persistence | .omc/state/ with PID-aware liveness | SQLite daemon with PID recovery |
| Commit protocol | Git trailers (Constraint/Rejected/Confidence) | Evidence trail in SQLite |

---

> **Next step:** Review this plan with team. Start Phase 0 fixes immediately after approval.
