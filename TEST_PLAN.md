# anti_subagent — Comprehensive Test Plan

> **60 test scenarios** across 13 categories.
> Priority: T1-T18 must pass before Phase 1. T19-T42 must pass for SLP validation.

---

## Phase 0 — Safety / Process / Treehouse

### T1: Spawn peer
- Spawn peer → acquire Treehouse lease → create worktree → peer READY
- Verify: PID valid, lease exists, worktree path correct, SQLite = RUNNING, no impact on current session

### T2: Terminate peer
- RUNNING → terminate(peer) → process exits → Treehouse.return_if_lease()
- Verify: process dead, worktree cleaned, lease released, state = STOPPED, no orphan

### T3: Kill Peer doesn't kill Lead
- Lead PID ≠ Peer PID, terminate(Peer)
- Verify: Peer dead, Lead alive, current session alive
- Test with process names to prove no pattern matching

### T4: Peer crash
- spawn Peer → SIGTERM → Reaper detects → PeerCrashDetected
- Verify: exit info captured, Treehouse cleanup, crash evidence persisted, LifecycleEvent emitted, state = CRASHED

### T5: Crash while holding Treehouse lease
- RUNNING + lease + worktree + process → crash → recovery
- Verify: correct lease_id cleanup, not wrong workspace, worktree released

### T6: Crash between process death and cleanup
- process exits → daemon crashes → daemon restarts
- Verify: RUNNING → detect dead PID → CRASHED → Treehouse cleanup → recovery

---

## Verify Gate

### T7: Cannot ACCEPT without VERIFY
- Submitted → accept → DENIED (requires Verified)

### T8: VERIFY success
- Submitted → VerifyWork → tests pass → EvidenceRecord → VERIFIED
- Verify: test output, exit code, build output, git SHA, git diff, diagnostics, claims, timestamp

### T9: VERIFY fail
- cargo test exit 1 → REJECTED (not VERIFIED)

### T10: Fake "done" claim
- Peer claims done, but cargo test fails → NOT VERIFIED, NOT ACCEPTED

---

## Evidence Integrity

### T11: SHA mismatch
- artifact SHA = ABC → mutate → SHA = XYZ → verification rejected

### T12: Old evidence
- commit A → verify PASS → commit B → accept → DENIED (evidence stale)

### T13: Code changed after verification
- VERIFY PASS → git diff changes → ACCEPT DENIED

---

## Writer / Verifier Separation

### T14: Engineer can modify
- Engineer: read, write, edit, test → all allowed

### T15: Engineer cannot self-approve
- Engineer → implement → verify own work → DENIED

### T16: ProofAuditor cannot modify
- ProofAuditor: edit source, git commit → DENIED
- ProofAuditor: read, test, inspect git → allowed

### T17: Architect read-only
- Architect: read, plan → OK; write source → DENIED

### T18: Scout read-only
- Scout: search, inspect → OK; modify → DENIED

---

## Staged Pipeline

### T19: Happy path
- RECEIVED → EXPLORED → PLANNED → EXECUTING → EXECUTED → VERIFYING → VERIFIED → ACCEPTED

### T20: Skip stage
- RECEIVED → EXECUTING → DENIED

### T21: Verify before execution
- RECEIVED → VERIFYING → DENIED

### T22: Fix loop
- EXECUTING → VERIFYING → REJECTED → FIXING → EXECUTING → VERIFYING → VERIFIED → ACCEPTED

### T23: Infinite loop protection
- max_revisions reached → EXHAUSTED → CANCELLED

---

## Lifecycle Event Bus

### T24: Spawn events
- WorkspaceAcquired → PeerSpawned → PeerReady

### T25: Normal completion
- TaskExecuting → TaskCompleted → PeerStopped → WorkspaceReleased → WorkspaceCleaned

### T26: Crash event ordering
- PeerCrashed → WorkspaceReleased → WorkspaceCleaned

### T27: Event handler crash
- handler panic → bus survives, other handlers execute, daemon doesn't crash

---

## Restart Recovery

### T28: Restart with healthy peers
- Peer RUNNING → daemon restart → PID alive → Peer remains RUNNING

### T29: Restart with dead Peer
- SQLite = RUNNING, PID dead → CRASHED → Treehouse cleanup → recovery

### T30: PID reuse
- old PID 1234 dies, new process gets PID 1234 → daemon MUST NOT conclude old Peer alive

---

## Treehouse Lease Safety

### T31: Wrong lease ID
- return_if_lease(WRONG) → workspace NOT cleaned

### T32: Wrong workspace path
- lease_id = A, path = workspace B → DENIED

### T33: Double return
- return lease twice → idempotent, no corruption, no panic

### T34: Stale lease
- daemon crash → stale lease → Treehouse GC reclaims

---

## Parallel Scheduling

### T35: Independent tasks parallel
- A, B, C independent → run in parallel

### T36: Dependent tasks sequential
- A → B → A completes, then B starts

### T37: Resource limit
- max_concurrent = 2, A B C D → A B, then C D

---

## Model Routing

### T38: Disposition × complexity matrix
- Scout + LOW → lightweight
- Engineer + LOW → standard
- Engineer + HIGH → heavyweight
- ProofAuditor + HIGH → heavyweight
- Provider resolved from config, not hard-coded

---

## SLP Behavioral Tests

### T39: Peer doesn't know hierarchy
- Peer believes interacting with human, not "subagent"

### T40: Peer can challenge premise
- Given wrong task → Peer challenges with evidence

### T41: Lead doesn't implement
- Lead plans, delegates, integrates, verifies

### T42: ProofAuditor rejects bad implementation
- Engineer claims PASS, ProofAuditor finds FAIL → REJECTED → FIX

---

## E2E Orchestration

### T43: Full SLP flow
- Human → Supervisor → Lead → Scout → Architect → Engineer → submit → ProofAuditor verify → FAIL → fix → verify → PASS → ACCEPT
- Verify: Treehouse lease, isolated worktree, process lifecycle, SQLite state

---

## Chaos / Evil Tests

### T44: Treehouse daemon unavailable
- Spawn fails gracefully, no orphan, state = FAILED

### T45: Treehouse CLI timeout
- Spawn timeout → cleanup attempt → state = FAILED

### T46: Treehouse malformed response
- Parse error → state = FAILED, no crash

### T47: Peer process hangs
- Hanging peer → watchdog escalates → terminate → cleanup

### T48: Peer ignores terminate
- SIGTERM → wait → SIGKILL → cleanup

### T49: Peer crashes during verification
- Verify crash → REJECTED → no stale VERIFIED state

### T50: Daemon crashes during cleanup
- Restart → detect dead → re-attempt cleanup

### T51: SQLite locked
- Concurrent writes → retry/backoff, no panic

### T52: SQLite unavailable
- Fallback to in-memory, log warning

### T53: Event handler panics
- Panic caught, other handlers execute

### T54: Duplicate PeerSpawn
- Duplicate ID → error, not overwrite

### T55: Duplicate Verify
- Already verified → skip or error

### T56: Duplicate Accept
- Already accepted → skip or error

### T57: Two Leads attempt same task
- Conflict detection → one wins, other notified

### T58: Two peers attempt same lease
- Lease conflict → one fails gracefully

### T59: Evidence artifact disappears
- Missing artifact → verification fails, not crash

### T60: Git dirty after verification
- Git changes post-verify → stale evidence detected

---

## Definition of Done

### Phase 0 (T1-T6)
- ✅ 0 pkill -f
- ✅ 0 orphan worktrees
- ✅ 0 orphan peer processes
- ✅ 0 accidental Lead kills
- ✅ verify gate enforced

### Phase 1 (T7-T23)
- ✅ State transitions deterministic
- ✅ Evidence immutable/versioned
- ✅ Writer ≠ verifier
- ✅ Fix loop bounded

### Phase 2 (T24-T27)
- ✅ Lifecycle events complete
- ✅ Disposition permissions enforced

### Phase 3 (T28-T37)
- ✅ DAG dependencies correct
- ✅ Concurrency bounded
- ✅ Routing deterministic/configurable

### Phase 4 (T38-T42)
- ✅ Restart recovery
- ✅ PID reuse safe
- ✅ Lead handoff

### E2E (T43)
- ✅ spawn → execute → fail → fix → verify → accept

### Chaos (T44-T60)
- ✅ All fail-safe, no infinite retry
- ✅ Graceful degradation
- ✅ No data loss

---

> **Gate:** T1-T18 must pass before Phase 1. T19-T42 must pass for SLP validation.
> T43-T60 determine production readiness.
