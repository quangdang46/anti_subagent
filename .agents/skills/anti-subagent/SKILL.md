---
name: anti-subagent
description: "SLP orchestration for coding agents — spawn peers, manage work lifecycle, enforce evidence-gated review. Use when delegating tasks to autonomous agents with verification."
---

<!--
CAPABILITIES_SUMMARY:
- peer_spawning: Spawn autonomous peer agents via anti-cli (treehouse worktree isolation)
- work_lifecycle: Submit → Verify → Review → Accept/Reject with revision tracking
- evidence_gating: SETTLED ≠ VERIFIED ≠ ACCEPTED — no auto-accept, enforced by state machine
- loop_prevention: Sliding window + hysteresis + cooldown prevents infinite review loops
- watchdog: Daemon monitors overdue reviews, emits escalation events
- guard_classification: Deny delegation-shaped tool calls in peers (fail-closed)

COLLABORATION_PATTERNS:
- User → anti-subagent: CLI commands (spawn, work submit, work review, escalations)
- anti-subagent → Peer: Spawned via treehouse worktree, independent OS process
- Peer → anti-subagent: Evidence submission via work submit
- Lead → anti-subagent: Review decisions via work review

PROJECT_AFFINITY: universal — SLP orchestration for any coding agent setup
-->

# anti-subagent

> **"Deploy peers, not subagents."**

SLP (Supervisor → Lead → Peer) orchestration engine for coding agents. Every worker is a full, autonomous agent with durable identity. The hierarchy is invisible to peers. Evidence-gated lifecycle ensures no auto-accept.

**Principles:** Evidence over claims · No auto-accept · Revision bumps reset counters · Guard is fail-closed · Peers believe they work for a human

## Trigger Guidance

Use anti-subagent when the task needs:
- Delegating work to multiple autonomous agents with verification
- Enforcing review cycles with evidence (SHA-256 of artifacts)
- Preventing infinite review loops (sliding window + hysteresis)
- Spawning peers in isolated worktrees (treehouse)
- Classifying tools as delegation-shaped (guard)

Route elsewhere when the task is primarily:
- Single-agent work with clear ownership: direct Claude/Codex
- Simple file edits without verification: native editor
- CI/CD pipeline execution: GitHub Actions / Render

## Core Contract

- Peers are spawned as independent OS processes (not subagents)
- Work items follow: Pending → InProgress → Submitted → Verified → Accepted
- Reject bumps revision (group counter reset — veylen lesson)
- Watchdog scans every 15s, emits ReviewEscalated on overdue reviews
- Guard denies delegation-shaped tool calls (subagent, spawn, dispatch, etc.)
- CAS writes prevent last-writer-wins between peers
- Agent context capped at 64KB (bounded capsule)

## CLI Commands

```bash
# Daemon management
anti daemon start          # Start the control plane
anti daemon stop           # Stop gracefully
anti daemon status         # Check if running

# Agent lifecycle
anti spawn --id <ID> --role peer --harness claude --repo <PATH>
anti list                  # List all agents
anti status <ID>           # Agent details
anti wait <ID> --until completed --timeout 3600
anti stop <ID>             # Graceful stop (SIGTERM)
anti kill <ID>             # Force kill (SIGKILL)

# Work lifecycle
anti work submit <ID> --sha <SHA256> --path <ARTIFACT> --timeout 600
anti work review <ID> accept --note "looks good"
anti work review <ID> reject --note "missing tests"
anti work list             # Show all work items

# Monitoring
anti escalations           # Show overdue reviews
anti guard test --tool <TOOL_NAME>
anti doctor                # Check dependencies
```

## Work Lifecycle

```
submit (evidence) → SUBMITTED → verify → VERIFIED → accept → ACCEPTED
                                        ↓
                                   reject → NEEDSREVISION (rev bumped)
                                        ↓
                                   re-submit → SUBMITTED (new deadline)
                                        ↓
                                   (repeat until max_revisions)
                                        ↓
                                   exceeded → REJECTED (terminal)
```

**Key invariant:** Accept requires Verified state. No code path exists for auto-accept.

## Architecture

```
anti-cli (control plane)
    │
    ├── TCP loopback (Windows) / Unix socket (macOS/Linux)
    │
    ▼
anti-daemon
    ├── SQLite (WAL mode)
    ├── Watchdog thread (15s scan)
    ├── Reaper thread (5s child poll)
    ├── Lease sweeper (15s treehouse release)
    └── Peers (Claude/Codex/OpenCode processes)
         └── treehouse worktree isolation
```

## Gotchas

- **Windows daemon uses TCP loopback** (127.0.0.1:PORT), not Unix sockets. Port derived from state dir hash.
- **treehouse auto-detected on PATH** — no env var needed. Falls back to bare `treehouse` if not found.
- **Work submit auto-creates** if item doesn't exist (CLI convenience). State starts at Pending → InProgress → Submitted.
- **Review timeout default 600s** (10 minutes). Watchdog escalates after deadline.
- **Revision bumps reset group counter** — this prevents the veylen infinite loop (reset ≤ 1 never reached).
- **Guard is fail-closed** — daemon down = all delegation tools denied locally.

## Applied Patterns

| Pattern | Source | Implementation |
|---|---|---|
| Evidence-gated lifecycle | irina `verification.ts` | `work.rs` — 7-state machine |
| Generation-fenced leases | irina `lease.ts` | `model.rs` — `generation: u64` |
| Review watchdog | veylen lesson | `main.rs` — 15s scan thread |
| Sliding window + hysteresis | veylen `SubscriptionEvaluator.ts` | `loopprev.rs` — trigger >3, reset ≤1 |
| CAS write-if-unchanged | maestro `fs.rs` | `cas.rs` — SHA-256 baseline |
| Bounded capsule ≤64KB | irina `project-state.ts` | `capsule.rs` — truncate at budget |
| Closed enums + exhaustive match | maestro `schema.rs` | `ReviewVerdict`, `VerificationStatus` |
| Read-only arbiter | maestro `loop_recipes.rs` | `arbiter.rs` — rubric scorer |

## Reference Map

| File | Read this when... |
|------|-------------------|
| `crates/anti-core/src/work.rs` | Understanding the state machine |
| `crates/anti-core/src/loopprev.rs` | Understanding loop prevention |
| `crates/anti-daemon/src/store.rs` | Understanding persistence |
| `crates/anti-daemon/src/ipc.rs` | Understanding IPC protocol |
| `REPORT.md` | Full implementation status |
| `2026-08-16_140000-apply-reina-patterns.md` | Original implementation plan |

---

> "Every peer believes it works for a human. That is the control variable."
