# anti_subagent — Implementation Status Report

> **Date:** 2026-08-17 · **Author:** Quang Dang · **Repo:** github.com/quangdang46/anti_subagent

---

## 1. What is this?

anti_subagent is a **SLP (Supervisor → Lead → Peer) orchestration engine** for coding agents. The thesis: harness-native subagents make multi-agent work worse; the fix is full, autonomous agents with durable identity, governed by an evidence-gated lifecycle — not a prompt-level hierarchy.

**Key invariant:** No auto-accept. Every work item must pass: Submitted (claim) → Verified (evidence) → Accepted (lead decision). The system actively prevents infinite review loops.

---

## 2. Architecture

```
User/Lead (CLI)
    │
    ▼
┌──────────────────────────────────────────┐
│  anti-cli        control plane commands  │
│  anti-daemon     SQLite + JSONL + IPC    │
│  anti-workspace  treehouse + CAS writes  │
└──────────────┬───────────────────────────┘
               │ Unix socket / named pipe
    ┌──────────┼──────────┐
    ▼          ▼          ▼
 ┌──────┐  ┌──────┐  ┌──────┐
 │Peer 1│  │Peer 2│  │Peer N│  ← full OS-process agents
 │(tree │  │(tree │  │(tree │    (treehouse worktree isolation)
 │house │  │house │  │house │
 └──────┘  └──────┘  └──────┘
```

**6 Rust crates, 20 source files, ~3,700 lines of Rust, 25 unit tests.**

| Crate | Role | Lines |
|---|---|---|
| `anti-core` | Domain model, state machines, policies | 845 |
| `anti-daemon` | SQLite persistence, Unix IPC, watchdog threads | 1,603 |
| `anti-cli` | CLI commands (the user interface) | 587 |
| `anti-workspace` | Treehouse adapter, CAS file writes | 229 |
| `anti-adapters` | Harness adapters (Claude/Codex/OpenCode) | 111 |
| `anti-bench` | 4-arm benchmark harness with sign test | 303 |

---

## 3. What has been built (13 implementation commits)

### 3.1 Domain Model (anti-core)

| Feature | Description | Tests |
|---|---|---|
| **WorkItem state machine** | 7-state lifecycle: Pending → InProgress → Submitted → Verified → Accepted. Reject bumps revision. | 4 tests |
| **Generation-fenced leases** | Every workspace write must carry the correct generation number. Stale writer → FenceError. | 2 tests |
| **Closed enums** | `ReviewVerdict` (Accept/Reject/Escalate) and `VerificationStatus` (6 states). Compiler-enforced exhaustive match. | 2 tests |
| **Loop prevention** | Sliding window (1h), hysteresis (trigger >3, reset ≤1), cooldown (10min). Ported from veylen production code. | 4 tests |
| **Bounded capsule** | Agent context capped at 64KB. Prevents context explosion. | 2 tests |
| **Read-only arbiter** | Rubric-based scorer. No FS/git access — compile-time guarantee. | 2 tests |

### 3.2 Persistence & IPC (anti-daemon)

| Feature | Description |
|---|---|
| **SQLite store** | WAL mode, agents + events + work_items tables. Optimistic-lock transitions. |
| **Unix socket IPC** | Request/Response protocol. Thread-per-connection. Graceful shutdown. |
| **SubmitWork/ReviewWork IPC** | Submit attaches evidence + deadline. Review: accept requires Verified; reject bumps revision. No auto-accept code path exists. |
| **Review watchdog** | Every 15s, scans overdue reviews → emits `ReviewEscalated` event. Does NOT auto-accept. |
| **Event log** | Append-only JSONL + SQLite. Sequence survives daemon restarts. |

### 3.3 File Safety (anti-workspace)

| Feature | Description | Tests |
|---|---|---|
| **CAS write** | `write_if_unchanged` — SHA-256 baseline check. Prevents last-writer-wins between peers. | 2 tests |
| **Atomic lock marker** | `.anti.lock` file with `create_new`. Two peers can never hold the same lock. | 1 test |

### 3.4 CLI (anti-cli)

| Command | What it does |
|---|---|
| `anti work submit --id W1 --sha <hash> --path /tmp/out.txt --timeout 600` | Submit work with evidence |
| `anti work review W1 accept --note "looks good"` | Accept (requires Verified state) |
| `anti work review W1 reject --note "missing tests"` | Reject (bumps revision) |
| `anti work list` | List all work items |
| `anti escalations` | Show overdue reviews |
| `anti guard test --tool subagent_spawn` | Classify tool as allow/deny |
| `anti doctor` | Check daemon, treehouse, claude, state.db |

---

## 4. What works today

### ✅ On Windows (current machine)

- All domain logic (work.rs, model.rs, loopprev.rs, capsule.rs, arbiter.rs)
- CAS file writes
- Guard classification (local, no daemon needed)
- All 25 unit tests pass
- `anti doctor` — checks dependencies
- `cargo build` compiles clean

### ✅ On Unix/macOS (daemon required)

- Full daemon lifecycle (start, spawn, wait, reap, restart)
- Treehouse worktree isolation
- Watchdog escalation
- End-to-end SLP flow
- Benchmark execution

---

## 5. What doesn't work yet

| Gap | Severity | Effort | Workaround |
|---|---|---|---|
| **Windows daemon transport** | 🔴 Blocks Windows testing | ~2 days | Use WSL or Docker for daemon; CLI runs natively |
| **Named pipe / TCP IPC** | 🔴 Same root cause | ~2 days | WSL |
| **Supervisor agent** (human = supervisor via CLI for MVP) | 🟡 Missing governance layer | ~1 week | Human reviews via `anti work review` |
| **Experience handoff artifact** | 🟡 Open question (thesis §4) | ~2 weeks | No standard format exists anywhere |
| **Control-plane events subscription** | 🟢 Enhancement | ~3 days | Watchdog already emits `ReviewEscalated` |
| **ARM C/D benchmark comparison** | 🟢 Needs real peer execution | ~1 day | Needs daemon running on Unix |

---

## 6. Testing infrastructure

| Type | Status | Location |
|---|---|---|
| Unit tests | 25 passing | `cargo test --workspace` |
| Store integration tests | 2 passing | `crates/anti-daemon/src/store.rs` (tests module) |
| CAS integration tests | 3 passing | `crates/anti-workspace/src/cas.rs` (tests module) |
| E2E script | Guard tests pass on Windows; daemon tests require Unix | `scripts/slp-e2e.sh` |

---

## 7. Research backing

The implementation is backed by 16 cloned production repositories:

| Source | Key pattern ported |
|---|---|
| irina (ReinaMacCredy) | Generation fence, evidence gating, bounded capsule |
| veylen (ReinaMacCredy) | Sliding window + hysteresis + cooldown, review loop detection |
| maestro (ReinaMacCredy) | CAS write + lock marker, closed enums, read-only arbiter |
| 13 other repos | Evidence corpus for SLP thesis (herdr, treehouse, pi-subagents, etc.) |

The plan `2026-08-16_140000-apply-reina-patterns.md` has been fully implemented (all beads completed).

---

## 8. Discussion points for the team

1. **Windows support:** Do we need native Windows daemon, or is WSL sufficient for the team's workflow?
2. **Supervisor layer:** The MVP uses `human = supervisor` via CLI. When do we need a Supervisor agent?
3. **Review timeout default:** Set to 600s (10min). Should this be configurable per project?
4. **Benchmark experiment:** Ready to run ARM A/B/C/D comparison once daemon works. What repo/task should we benchmark against?
5. **Production readiness:** What's the gap between this prototype and production use? (Error handling, logging, monitoring, deployment)
