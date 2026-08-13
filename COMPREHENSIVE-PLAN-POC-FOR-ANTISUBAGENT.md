# COMPREHENSIVE-PLAN-POC-FOR-ANTISUBAGENT

> **Status:** research-backed implementation/design plan, 2026-08-13.
> **Method:** deep code archaeology (not README summaries) of 12+ repositories, every architectural claim anchored to `file:symbol`. Claims are tagged **[VERIFIED-FROM-SOURCE]**, **[INFERENCE]**, **[THESIS-REQUIREMENT]**, or **[OPEN-QUESTION]**.
> **Output:** the POC is **not** implemented here. This document must be sufficient for another engineer to implement the POC without rediscovering the architecture.
> **Revision 2026-08-13b (review pass):** 3-arm → 4-arm benchmark (adds SLP-disclosed) to separate the confounded variables; concealment reclassified as a benchmark variable with per-arm `--peer-prompt`; guard blast radius capped to the delegation surface; stall timeout configurable (default 60s); Windows IPC promoted to a P0 gate; benchmark blinding + pre-registered sign-test added; cost estimate added; `locks` table contradiction removed.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Research Baseline](#2-research-baseline)
3. [Anti-Subagent Thesis](#3-anti-subagent-thesis)
4. [Repository Research Summary](#4-repository-research-summary)
5. [slb Deep Analysis](#5-slb-deep-analysis)
6. [herdr Deep Analysis](#6-herdr-deep-analysis)
7. [treehouse Deep Analysis](#7-treehouse-deep-analysis)
8. [firstmate Deep Analysis](#8-firstmate-deep-analysis)
9. [Additional Repository Findings](#9-additional-repository-findings)
10. [Cross-Repository Comparison](#10-cross-repository-comparison)
11. [Reuse/Adapt/Depend/Reimplement Decisions](#11-reuseadaptdependreimplement-decisions)
12. [License Analysis](#12-license-analysis)
13. [Final Architecture](#13-final-architecture)
14. [Component Architecture](#14-component-architecture)
15. [Process Model](#15-process-model)
16. [Identity Model](#16-identity-model)
17. [Lifecycle State Machine](#17-lifecycle-state-machine)
18. [Spawn Protocol](#18-spawn-protocol)
19. [Workspace Protocol](#19-workspace-protocol)
20. [Event Protocol](#20-event-protocol)
21. [Wait Protocol](#21-wait-protocol)
22. [Guard/Security Model](#22-guardsecurity-model)
23. [Recovery Model](#23-recovery-model)
24. [Handoff Model](#24-handoff-model)
25. [Harness Adapter Model](#25-harness-adapter-model)
26. [CLI Specification](#26-cli-specification)
27. [Data Model](#27-data-model)
28. [Persistence Model](#28-persistence-model)
29. [Failure Modes](#29-failure-modes)
30. [Threat Model](#30-threat-model)
31. [POC Scope](#31-poc-scope)
32. [Explicit Non-Goals](#32-explicit-non-goals)
33. [Implementation Phases](#33-implementation-phases)
34. [Benchmark Architecture](#34-benchmark-architecture)
35. [Benchmark Tasks](#35-benchmark-tasks)
36. [Metrics](#36-metrics)
37. [Failure Injection](#37-failure-injection)
38. [Definition of Done](#38-definition-of-done)
39. [Open Questions](#39-open-questions)
40. [Risks](#40-risks)
41. [Future Extension: Protocol, Not MCP](#41-future-extension-protocol-not-mcp)
42. [Final Recommendation](#42-final-recommendation)
43. [Cost Estimate](#43-cost-estimate)

---

## 1. Executive Summary

The thesis of `anti_subagent` is that **harness-native subagents make long-horizon multi-agent coding work worse**, and the fix is an architecture where every worker is a full, independent agent with durable identity — organized as **Supervisor → Lead → Peer (SLP)**.

Deep code archaeology of 12 repositories (slb, herdr, treehouse, firstmate, gnap, multipi, opengoat, swarm-protocol, agent-orchestrator, mcp_agent_mail(_rust), pi-subagents, maestro-orchestrate) produced four findings that reshape the plan:

1. **The substrate exists but the supervisor layer does not.** slb (approval notary + fail-closed hook), herdr (hybrid event-gated wait, agent-state derivation), treehouse (durable worktree lease), agent-orchestrator (daemon-owned CI feedback routing), mcp_agent_mail_rust (file reservation + guard) each solve one slice. **No repo ships a read-only on-demand Supervisor above a Lead with instruction-patching and Lead replacement.** This is the confirmed gap. [VERIFIED-FROM-SOURCE]

2. **The strongest counterexample to the thesis is not native subagents — it is the full-agent flat fleet.** agent-orchestrator ships a working orchestrator→worker fleet that engineers away F1 (livestock) and F3 (context-burn) *by construction* — real subprocess workers, worktree isolation, daemon-owned CI/merge/review routing — while workers openly know an orchestrator exists. opengoat similarly proves **durable full-agent identity is achievable with a fully visible hierarchy**. Both challenge the thesis's load-bearing "identity concealment" claim. [VERIFIED-FROM-SOURCE]

3. **Therefore the experiment must be four-arm, not three.** Native Subagent vs Flat Full-Agent vs SLP-concealed vs SLP-disclosed. The original 3-arm design conflated two independent variables into ARM B ("no SLP hierarchy, peers know each other" = simultaneously *flat* AND *disclosed*), so no arm represented the counterexamples' *hierarchical-but-disclosed* configuration (opengoat §9.3, agent-orchestrator §9.5). Four arms separate (a) independent full agents, (b) the SLP hierarchy, and (c) hierarchy visibility into isolatable effects. [THESIS-REQUIREMENT] per research baseline §19 and §39.1.

4. **The POC should be a Rust CLI that spawns real coding-agent subprocesses, DEPENDING on treehouse for workspace lease and slb-derived patterns for the guard, ADAPTING herdr's hybrid wait, and clean-room implementing the SLP-specific layer** (identity registry, guard, handoff, events). Agent Mail code cannot be copied (license excludes Anthropic). **Scope decision: CLI-only — no MCP layer is built or planned.** [VERIFIED-FROM-SOURCE + LICENSE]

The plan below gives the exact architecture, protocols, state machine, CLI, benchmark, and failure-injection matrix.

---

## 2. Research Baseline

Source material (read in full):
- `ChatGPT-Thesis và Research Summary-20260813-1456.md` — thesis statement, SLP, POC direction, CLI-first decision, 4-arm benchmark (3-arm in the original baseline; expanded to 4 in this revision).
- `Claude-System check-20260813-1447.md` — round-2 research: opengoat (identity rebuttal), gnap/swarm-protocol (flat coordination axis), agent-orchestrator, multipi, mcp_agent_mail_rust.
- `RESEARCH_REPOS.md` (in repo) — first 12-repo corpus.

Key decisions locked by the baseline:
- **CLI-only, no MCP.** The POC is a Rust CLI; no MCP server is built or planned (removed from scope per explicit decision).
- **Harness-agnostic**: anti_subagent spawns executables (`claude`, `codex`, `opencode`), never harness-native subagent tools.
- **4-arm experiment**: A=Native Subagent, B=Flat Full-Agent, C=SLP-concealed, D=SLP-disclosed (see §34).
- **Peer = independent OS process**, not a function call.
- **State persisted BEFORE spawn** (firstmate lesson).
- **Fail-closed guard** (slb lesson).

---

## 3. Anti-Subagent Thesis

**Claim (H0):** For long-horizon, multi-file coding tasks, independent full-agent peers outperform native subagents on correctness and context cost, and the SLP hierarchy adds measurable value beyond mere independence.

**Four failure modes:**
| # | Failure | Definition |
|---|---|---|
| F1 | Livestock | Worker is a function call; no autonomy, no durable identity |
| F2 | Reflexive agreement | Worker over-agrees with orchestrator (or resists) to please requester |
| F3 | Context burned on polling | Orchestrator polls "done yet", wastes context, misses state changes |
| F4 | Identity deception | Worker knows it is subordinate → stops acting like an owner |

**SLP invariants:**
- Peer is a full agent (plainly spawned, addressed as a human, free to disagree).
- Lead owns outcome, never presolves, never implements.
- Supervisor is on-demand, read-only, above the Lead, patches instructions, can replace a degrading Lead.
- Hierarchy is invisible to workers.
- Handoff/state is durable.

**Challenge from the corpus (must be answered in the benchmark):** agent-orchestrator and opengoat show full-agent fleets work with a *visible* hierarchy. So "invisible hierarchy" is a **hypothesis, not a proven requirement** — and identity concealment is **not a settled architecture default; it is a benchmark variable.** The POC's production model (peer prompts, guard config) must be parameterized so each arm can toggle concealment on or off without structural change (see §16, §22, §34, §39.1).

---

## 4. Repository Research Summary

| Repo | Role in corpus | Verdict for POC |
|---|---|---|
| `Dicklesworthstone/slb` (Go) | Guard + approval + state machine | **ADAPT/COPY** guard patterns |
| `herdrdev/herdr` (Rust) | Agent state derivation + hybrid wait | **ADAPT** wait/state |
| `kunchenguid/treehouse` (Go) | Worktree lease + atomic state | **DEPEND** (CLI subprocess) |
| `kunchenguid/firstmate` (bash) | Fleet supervision; incident | **ADAPT** lesson (metadata-before-spawn) |
| `farol-team/gnap` | Git-native flat coordination | **ADOPT as reference** (flat axis) |
| `Ch3w3y/multipi` | Native-subagent pipeline | **DO NOT USE** (counterexample) |
| `marian2js/opengoat` | Durable identity, visible hierarchy | **ADAPT peer mechanics; DROP disclosure model** |
| `phuryn/swarm-protocol` | Flat state-sync (MCP+Postgres) | **DO NOT USE for POC** (Postgres SPOF); adopt state-sync concept |
| `Untrivial-ai/agent-orchestrator` | Second substrate fleet | **ADAPT** substrate; **DO NOT USE** visible-hierarchy model |
| `Dicklesworthstone/mcp_agent_mail_rust` | File lease + guard + ledger | **CLEAN-ROOM ADOPT** (license blocks copy) |
| `edxeth/pi-subagents` | Harness-native subagent machinery | **REFERENCE** for F3/F4 fences + their trust leaks |
| `josstei/maestro-orchestrate` | Native-subagent orchestrator | **REFERENCE** counterexample |

Note on repo ownership: the prompt listed `Dicklesworthstone/{herdr,treehouse,firstmate}` — those are **404**; the real owners are `herdrdev/herdr`, `kunchenguid/treehouse`, `kunchenguid/firstmate`. [VERIFIED-FROM-SOURCE]

---

## 5. slb Deep Analysis

Source: `slb` v0.4.0, MIT+AI-Rider, Go 1.24, cobra/viper/modernc.org/sqlite.

### 5.1 Execution path (CLI → daemon → guard → execution → state)
```
slb run "<cmd>" --session-id <id>
  → cli/run.go:66 RunE
  → core/request.go:129 CreateRequest
  → core/patterns.go:180 ClassifyCommand
  → (safe short-circuit) OR (unmatched → escalate to DANGEROUS)
  → db/requests.go:28 CreateRequest (status=pending, hash)
  → run.go:166 poll loop (500ms)
  → core/execute.go:99 ExecuteApprovedRequest (5 gates)
  → core/command.go RunCommand (client-side shell)
```

### 5.2 Mechanisms (file:symbol → behavior → anti_subagent mapping)

| slb mechanism | Anchor | Behavior | anti_subagent use |
|---|---|---|---|
| **Risk-tier classification** | `core/patterns.go:180` `ClassifyCommand` | Normalize (strip `sudo`/`env`/`bash -c`, shell-aware split `&&;|`), match SAFE→CRITICAL→DANGEROUS→CAUTION, compound = highest-risk segment | **ADAPT** as spawn-risk classifier (swap pattern set for spawn shapes) |
| **Conservative parse upgrade** | `core/patterns.go:395` `applyParseUpgrade` | On parse error, bump tier one level; `HasUnmatchedSegment` forces escalation (GH#9) | **COPY** — fail-closed default for unknown shapes |
| **Unmatched → DANGEROUS** | `core/request.go:198` | Submitted commands never auto-allow unmatched | **COPY** — the single most important safety default |
| **Optimistic-lock claim** | `db/requests.go:302` `UpdateRequestStatus` | `UPDATE ... WHERE status=<expected>`; rowsAffected==0 → `ErrInvalidTransition` | **COPY** — first-executor-wins for spawn approval |
| **5 execution gates** | `core/execute.go:99` | G1 approved, G2 TTL, G3 hash, G4 tier-consistency, G5 claim | **COPY** as pre-spawn gates |
| **Command hash** | `db/requests.go:541` `ComputeCommandHash` | SHA-256(raw+cwd+argv+shell), verified at execution | **COPY** — bind exact spawn command |
| **State machine** | `core/statemachine.go` + `db/requests.go` | `validTransitions` map; TTL 30min/10min | **COPY** (port DB-enforcing layer only; pure layer duplicated) |
| **HMAC review signature** | `db/reviews.go:258` | HMAC-SHA256(sessionKey, requestID+decision+timestamp) | **COPY** — tamper-proof approval/spawn records |
| **Fail-closed hook** | `internal/cli/hook.go` `slb_guard.py` | Hook queries daemon (50ms); unreachable → offline classify; unknown → `ask` (fail-closed) | **COPY** — the PreToolUse guard generation |
| **Hook generation** | `internal/integrations/claudehooks.go` | Generates `.claude/hooks.json` `pre_bash` → `slb patterns test` | **ADAPT** — generate native hook config per harness |
| **Daemon = notary, not executor** | `daemon.go:1-4` | Executes client-side; daemon verifies only | **ADAPT** — anti_subagent's daemon spawns, not executes |
| **Config hierarchy** | `config/loader.go:27` | defaults < user < project < env < flags | **COPY** |
| **`daemon.Verifier.VerifyAndMarkExecuting`** | `daemon/verifier.go:111` | Dead code (never wired into RunDaemon) | **DO NOT USE** |

### 5.3 Risks to avoid
- Two divergent gate paths (`core/execute.go` vs `daemon/verifier.go`) — merge into one superset.
- `checkApproval` SQL bug (`daemon/hook_query.go:128-136`): queries columns that don't exist; any SQL error → silent `(false,"")`. Copy carefully.
- `cfg.Patterns.<tier>.patterns` never loaded into engine — dead config. Keep engine = builtins + SQLite.
- `slb run` executes via `/bin/sh -c` — hash binds raw string, env-var indirection unguarded.
- Caution tier is auto-approve in concept but blocking `ask` in hook — decide tier semantics.
- Windows: sockets are Unix-only; anti_subagent needs different IPC transport.

---

## 6. herdr Deep Analysis

Source: `herdrdev/herdr` v0.8.0, Apache-2.0, Rust, 28.4k stars.

### 6.1 Architecture
PTY-managing daemon. Per-pane `TerminalState` derives effective agent state from **visible-blocker > hook > screen-fallback** (`src/terminal/state.rs:2125 recompute_effective_state`). Per-pane detection task (`src/pane.rs:677`) samples foreground process group + screen on ~300ms ticks, publishes `AppEvent` via mpsc to main loop, which bumps `state_change_seq` and pushes `EventEnvelope` into in-memory `EventHub` (512 events). JSON-line socket API. **Events NOT persisted** — seq restarts at 0 on daemon restart.

### 6.2 State machine (matches the requested model)
`AgentState` (`src/detect/mod.rs:11`): `Idle | Working | Blocked | Unknown`. API adds `Done` = Idle + unseen (`api_helpers.rs:99`). `ManagedAgent` phases: `Pending → Blocked → Active` (`state.rs:1949`), with settle 3s + deadline timeout.

### 6.3 Wait subsystem — HYBRID (event-gated polling) [VERIFIED]
`wait_for_resolved_agent` (`src/api/wait.rs:348`): loop replays `EventHub.events_after(last_seq)`, sets `should_probe` on matching events, does synchronous `AgentGet` snapshot only when the ring changed or at deadline, **sleeps 100ms unconditionally**. `CONNECTION_POLL_INTERVAL = 100ms` (server.rs:28). `prompt_agent` two-phase (`wait.rs:177`): Phase 0 snapshot+dispatch; Phase 1 stall detection (any state change within 5s else `agent_prompt_stalled`); Phase 2 settled-state wait with `last_event_sequence` reset to pre-submit (wait.rs:286).

### 6.4 Recovery — NOT supervised
Crash → `PaneDied` or foreground disappearance → Idle + `Done` + released. Recovery = shell respawn + agent's own `--resume <id>` argv injection (`src/agent_resume.rs`, `app/agent_resume.rs:205`). **No retry/backoff loop.** herdr does not auto-restart a crashed agent.

### 6.5 anti_subagent mapping
| herdr primitive | Map to |
|---|---|
| `AgentState`/`AgentStatus` | Target subagent state model |
| `recompute_effective_state` precedence | Deriving subagent state from multiple sources |
| `foreground_job` process-group probe (5s/30s) | Process discovery for "subagent spawned a child" |
| `agent.wait`/`prompt --wait` two-phase | **Wait substrate** — stall detection + event-gated polling |
| `EventHub` ring + seq | Event log (but anti_subagent must **persist** it) |
| `ManagedAgent` phases | Spawn health-check (did it come up?) |
| `Idle+unseen→Done` | Completion inference |
| `AgentSessionRef` + resume plan | Crash recovery reference |

### 6.6 Risks to avoid
- **EventHub not persisted** — anti_subagent MUST persist events for recovery.
- seq bumps only on state transition — a wait gated purely on seq can miss title/progress.
- `agent_not_running` aborts wait on identity divergence — subagent relocation kills the waiter.
- `Unknown` status: default `until=[Idle,Done,Blocked]`; an agent stuck Unknown hangs — always pass timeout.
- Detection heuristic: missed pattern → Idle fallback → fabricated false `Done`.
- Blocking per-connection threads (100ms sleeps) — N waits = N threads.

---

## 7. treehouse Deep Analysis

Source: `kunchenguid/treehouse`, MIT, Go 1.25, 1.4k stars.

### 7.1 Architecture
Single cobra CLI, **no daemon**. Per-repo pool `~/.treehouse/<repo>-<hash>/`, worktree `<pool>/<n>/<repoName>/`. All state = one JSON file `treehouse-state.json`, mutated only under `WithStateLock` (flock/LockFileEx), written atomically (temp + fsync + rename). Git ops shell out to `git` binary.

### 7.2 Core primitives (file:symbol → behavior)
- `pool.go:92 acquire` — under one lock: ReadState → healState → skip `Destroying||Leased||ownerAlive||inUse||dirty` → ResetWorktree OR AddWorktree → markAcquired → WriteState → release lock → post_create hooks.
- `pool.go:199 markAcquired` — durable lease (LeaseID 128-bit crypto) vs short-lived owner reservation (PID+CreateTime).
- `state.go:36 newLeaseID` — crypto/rand 128-bit → 32 hex chars. **Immutable.**
- `pool.go:252 ReleaseConditional` — under one lock: verify `--if-lease-id`/`--if-lease-holder` preconditions → terminate → ResetWorktree → clearLease → WriteState. **ABA-safe.**
- `state.go:101 recoverCorruptState` — crash recovery: rebuilds entries **marked Leased** with `recoveredLeaseHolder`, prints loud warning. Fail-closed to "leased".
- `git.go:164 ResetWorktree` — `checkout --detach --force`, `reset --hard`, `clean -fd` (**no `-x`** → ignored files survive).
- `process/detect.go:44 FindProcessesInWorktree` — gopsutil cwd scan; `terminate.go:38 filterProtectedProcesses` — excludes own ancestor chain.
- `destroy.go` — two-phase reservation: re-classify + stamp Destroying under lock, run hooks lock-free, re-lock and only remove same-reservation worktrees; restores owner on failure.

### 7.3 Decision for anti_subagent
**[VERIFIED-FROM-SOURCE] DEPEND as CLI subprocess. Do not vendor, do not reimplement.**
Reasons: (a) lease + atomic-state + crash-recovery is ~2.6k LOC battle-tested concurrency; (b) clean subprocess boundary `get --lease --json` / `return --if-lease-id`; (c) treehouse explicitly scopes itself as substrate, not policy (`VISION.md:3`).

If anti_subagent were Go: ADAPT `pool.go acquire`+`ReleaseConditional`+`state.go` atomic write (~600 LOC). But since Rust + clean CLI boundary exist, **DEPEND** is strictly better.

### 7.4 Risks
- **Leases never expire** — a crashed Supervisor must carry lease inventory across restarts (the "experience handoff" concern at FS level).
- `clean -fd` wipes untracked-not-ignored files on return — extract deliverables before release.
- Windows `TerminateProcess` has no grace.
- `destroy --all` skips exit-0 semantics differ; per-risk opt-in flags.
- No daemon → same-machine peers only (fine for POC).
- `--if-lease-id` return is single-shot; caller must be idempotent (capture ID from `get --json`, treat precondition-failure as already-released after `status --json` check).

---

## 8. firstmate Deep Analysis

Source: `kunchenguid/firstmate`, MIT, bash distro.

### 8.1 Correct spawn path
`fm-spawn.sh` writes `state/<id>.meta` (the identity axis) at `fm-spawn.sh:2655-2703`, then `spawn_send_literal "$T" "$LAUNCH"` (`:2820`) into a backend pane. **State before spawn.** `fm-brief.sh:169-171` writes `data/<id>/brief.md`. `--relaunch` (`:2639-2712`) is the restart path: relaunches into the *recorded* endpoint/worktree, refusing identity-axis changes.

### 8.2 Fleet metadata is the key invariant
`fm-fleet-snapshot.sh:410` iterates `$STATE/*.meta`; `fm-fleet-view.sh:67` shows "No live task metadata found" when `.tasks` empty. `FM_SUP_IN_FLIGHT` = count of `state/*.meta` (`fm-supervision-lib.sh:46-49`). **A task that never got a `.meta` is invisible to the entire fleet and the guard stack.**

### 8.3 The guard — `fm-subagent-pretool-check.sh` (the fix)
Wired in `.claude/settings.json` PreToolUse with **matcher `.*`** (a stem-enum matcher would reintroduce fail-open-by-enumeration). The script is the single owner of classification:
1. Get tool name (stdin jq or `--tool`); malformed/empty/missing jq → **fails open** (`:137-140`).
2. Normalize lowercase + strip non-alnum (`:145`).
3. `mcp__*` prefix → allow (`:151`).
4. Exact-name exclusions: `OBSERVE_ONLY_TOOLS` (observe/stop) + `PLAN_ONLY_TOOLS` (todo writes) (`:66,79`). Exact match only, never substring.
5. Stem scan: `DELEGATION_STEMS='agent subagent task workflow cron schedul worktree delegate spawn dispatch handoff remote sendmessage monitor'` (`:58`). Any substring hit = delegation-shaped.
6. Escape hatch: `FM_ALLOW_SUBAGENT=1` env only (`:171`) — set at launch, unforgeable in-session.
7. Scope gate: `fm_primary_scope_matches` (`fm-primary-scope-lib.sh:23-33`) — plain checkout or marked secondmate home = in scope; **linked worktree = out of scope** (crewmate delegation stays legal). Failure to confirm = inert (allow).
8. Deny: exit 2; Claude deny object to stderr only (`:205`), keep stdout empty (Claude ignores PreToolUse deny when stdout nonempty).

### 8.4 What firstmate got wrong (the incident)
The docs name it (`docs/subagent-guard.md:19-24`): the bypass "made the in-flight-work branch of the guard stack structurally inert." Chain:
1. Primary used Claude's built-in delegation tool instead of `fm-spawn.sh`.
2. No `state/<id>.meta`, no `data/<id>/brief.md` written.
3. Fleet blindness: `fm-fleet-view.sh` iterates metas → zero → supervision view empty.
4. Loss on restart: subagent work lived in the primary's process tree; 2 of 4 workers died on restart.
5. Silent supervision collapse: `FM_SUP_NEEDED` driven by meta counts → inert → watch down 73 min undetected.

**Root cause:** failure detection keyed on task metadata cannot catch a failure whose signature is precisely the *absence* of that metadata. Fix = move the fence to the harness tool surface (PreToolUse shape deny) so unaccounted work cannot be created.

### 8.5 Why the fix works
- Closes the surface, not the symptom; shape-based future-proof (16 known tools + hypothetical future names denied).
- Preserves legitimate surface (observe/stop, plan-only, MCP).
- Scope gate = the SLP shape test (supervisor home vs worker worktree).
- Layer-2 hardening: untracked `permissions.deny` removes tools from schema entirely.
- Env escape hatch unforgeable in-session.

### 8.6 anti_subagent application
**Decision: ADAPT** the guard shape (deny-by-shape, matcher `.*`, scope gate, env escape hatch), but the durable fix firstmate's docs identify (`docs/subagent-guard.md:373-381`) — treating "primary doing project-shaped work with zero `state/*.meta`" as *suspicious* not *idle* — is exactly anti_subagent's SLP claim. anti_subagent should implement **durable-identity-keyed supervision** from the start.

### 8.7 Risks
- Guard is a tool-shape heuristic, not reasoning; cannot judge "should this be delegated at all."
- Grok/OpenCode/Pi not wired in firstmate; anti_subagent needs per-harness adapters.
- Residual gap: unaccounted work via `Bash` reads as idle not suspicious.
- **Malformed-input transport risks fail-open (§8.3).** anti_subagent's guard deliberately *fails closed* (slb lineage, §22.3) — a deliberate divergence from firstmate, since the threat (native escape) is only detectable when the control plane is up.

---

## 9. Additional Repository Findings

### 9.1 gnap (`farol-team/gnap`, MIT, spec-only)
Zero implementation code (RFC draft). Git-native task/run/message/agent entities; commit convention `<agent-id>: <action>` is the audit log. Claim races resolved by git push-rejection retry (no atomicity). **Identity is nominal, self-declared, no cryptographic binding** (`ONBOARDING.md:77-79`). The repo itself admits "GNAP currently is an RFC, not a tool" (`docs/article.md:169-176`).
**Decision:** [VERIFIED] Adopt as **reference** for the durable state store concept (git as audit log + zero infra), NOT as the authority model (no supervision, no enforcement, forgeable identity). A flat fully-readable roster conflicts with invisible hierarchy unless filtered.

### 9.2 multipi (`Ch3w3y/multipi`)
Counterexample: pipeline with hard native-subagent gates, uses Anthropic's own research to argue *for* subagent isolation. Worker knows it's a subagent (tool literally named `subagent`).
**Decision:** DO NOT USE.

### 9.3 opengoat (`marian2js/opengoat`, MIT, ~36k LOC TS)
**The strongest identity rebuttal.** Every agent is told openly in its workspace `ROLE.md`: "You are part of an organization fully run by AI agents", plus "You report to: <manager>". Org visibility on disk: `syncWorkspaceReporteeLinks` symlinks `workspace/manager` → manager's workspace (`agent.service.ts:429-497`). Sessions persist per agent + session key (`session.service.ts`); restart continuity dual (OpenGoat transcript + provider session id threaded via `--resume`). Spawn = exec subprocess per provider, **never native subagent** (`cli-provider.ts:22-261`). Manager behavior = skills + prompts + org metadata, no internal planner loop.
**Key finding:** **durable full-agent identity is achievable with a fully visible hierarchy** → identity concealment is a *load-bearing choice* in the thesis, not a precondition. [VERIFIED]
**Decision:** ADAPT the peer-tier mechanics (per-agent workspace + ROLE.md, mechanical delegation permissions `board.service.ts:130-138`, session-key continuity, event-driven task dispatch). DROP the "AI-only org" disclosure model.

### 9.4 swarm-protocol (`phuryn/swarm-protocol`, MIT, TS)
Flat state-sync substrate, MCP+Postgres. Intents/claims/heartbeats/context-packages. `claim_work` single-claim enforced; `complete_claim` is **self-declared** (no proof gate); stale threshold 30min reporting only. **No supervisor, no lifecycle monitoring, no daemon.**
**Key finding:** state-sync (intents+claims+heartbeats+context packages+dep unblocking) does NOT need a hierarchy — it needs Postgres + MCP tools. The hierarchy's irreducible jobs are **governance** (verdict/acceptance, instruction-patching, Lead replacement). [VERIFIED]
**Decision:** DO NOT USE Postgres SPOF for POC. Adopt the *state-sync concept* (declarative shared state instead of orchestrator polling) as design reference for the flat arm.

### 9.5 agent-orchestrator (`Untrivial-ai/agent-orchestrator`, Apache-2.0, Go, 9.5k stars)
**The strongest counterexample.** Real subprocess workers in tmux (`agentruntime.BuildLaunchCommand` builds `claude --session-id ... --permission-mode ...`, `command.go:90`); supervisor wrapper posts exit state. **Explicit ban on built-in subagents in both system prompts** (`prompt.go:179,269`) + `SubagentStop` hook. Orchestrator role: "coordinate work, not perform implementation" (`prompt.go:161`). **Daemon-owned feedback routing** — `lifecycle/reactions.go:142 ApplyPRObservation` auto-nudges workers on CI-fail/review-changes/merge-conflict with dedup + caps. Reviewer is a real spawned agent on the worker's own worktree (`review/launcher.go:100`). SQLite durable facts, derived status, tmux-is-persistence, Boot Reconcile + StashUncommitted→preserve-ref→3-way-merge replay.
**What it does NOT ship:** Supervisor above the orchestrator (only `SpawnOrchestrator(clean=true)` → `RetireForReplacement`, service.go:373-427, user-driven not degradation-driven); **workers know an orchestrator exists** (`prompt.go:278-286` workerOrchestratorPrompt) — no invisible hierarchy; no council verdict protocol; no standardized handoff artifact.

**Benchmark implication:** agent-orchestrator's fleet is *hierarchical-but-disclosed*. ARM D of the 4-arm benchmark (§34) represents exactly this configuration, so the POC can measure whether concealment (ARM C) adds anything over a disclosed SLP (ARM D) — and whether disclosed SLP (ARM D) beats a disclosed flat fleet (ARM B). [VERIFIED + THESIS]
**Key finding:** a full-agent fleet with daemon-owned feedback engineers away F1+F3 by construction while leaving F4 untouched. [VERIFIED]
**Decision:** ADAPT the substrate (OBSERVE→UPDATE→DERIVE pipeline, durable-facts/derived-status, `sendOnce` dedup, reaper mass-death circuit breaker, `StashUncommitted` preserve/restore). **DO NOT USE** the visible-hierarchy worker model as-is — but keep the disclosed-SLP configuration as ARM D (§34), so the benchmark can quantify what concealment is actually worth. [VERIFIED + THESIS]

### 9.6 mcp_agent_mail(_rust) (`Dicklesworthstone/`, MIT + AI-Rider, 2.1k/126 stars)
**Identity:** semi-persistent, codename ("GreenCastle"), "ephemeral by design" (`README.md:186`), deliberately **rejects role/descriptive names** (`models.rs:585-640`). Flat, peer-known mesh: `list_agents`/`whois` expose the full swarm — collapses identity concealment.
**File reservation:** TTL lease (default 3600s, clamp [60s, 1yr]), exclusive default, broad-pattern rejection, generation-stamped archive artifacts, pre-commit guard reads archive JSON directly, **fail-closed** guard (`fail_closed`, GH#224), `AGENT_MAIL_GUARD_MODE=warn|block`, `AGENT_MAIL_BYPASS=1`.
**Persistence:** SQLite live + Git archive (write-behind queue, eventual consistency), lock hierarchy, idempotency keys (24h).
**[VERIFIED] License:** MIT with OpenAI/Anthropic rider — `LICENSE:1-25` explicitly grants **no rights to Anthropic PBC / OpenAI or anyone acting under their direction**. This is a **hard blocker** for copying code into a Claude-driven anti_subagent.
**Decision:** **CLEAN-ROOM ADOPT** (Tier 1): file-reservation + pre-commit guard + Git/SQLite ledger + signal/event subscription, implemented fresh from the described semantics. Do NOT adopt the flat codename mesh, contact graph, or auto-registration.

### 9.7 pi-subagents (`edxeth/pi-subagents`, MIT)
Harness-native subagent machinery. Orchestrator mode strips tools to `{subagent,...}`. `<subagent-boundary>` marker, context-pressure exit blocks resume, parent-owned timeout sidecar, budget-shrinking. **Its own source concedes the fences are prompt/trust-level, not hard** (`session-files.ts:356-361`: "a child that can write files can rewrite this entry in place").
**Decision:** REFERENCE for F3/F4 fences + documented trust leaks. **Do not reuse pi-subagents' context-pressure exit / budget-shrinking design in anti_subagent**: those assume the *parent* controls the child's context budget, which contradicts the full-agent model (§3); anti_subagent instead measures context consumption via its own event log (§36).

### 9.8 maestro-orchestrate (`josstei/maestro-orchestrate`, Apache-2.0)
39-specialist orchestrator → native subagents. TechLead persona never implements (`architecture.md:12`). Hard gates (validate_plan, blockers, no-orchestrator-code). Workers = native subagents, ephemeral.
**Decision:** REFERENCE counterexample (short-horizon benchmarks may not distinguish — see §35).

---

## 10. Cross-Repository Comparison

Legend: ✅ = present & verified, 🟡 = partial, ❌ = absent.

| Capability | slb | herdr | treehouse | firstmate | agent-orch | **anti_subagent** |
|---|---|---|---|---|---|---|
| CLI | ✅ cobra | ✅ | ✅ cobra | ✅ bash | ✅ | ✅ (design) |
| Daemon | ✅ notary | ✅ | ❌ no-daemon | ✅ watcher | ✅ | ✅ (design) |
| Subprocess mgmt | ❌ | ✅ PTY | ❌ | ✅ panes | ✅ tmux | ✅ (design) |
| Agent lifecycle | 🟡 request-only | ✅ state machine | ❌ | ✅ meta | ✅ derived | ✅ (design) |
| Persistent identity | 🟡 session | 🟡 pane | 🟡 lease | ✅ meta | ✅ SQLite facts | ✅ (design) |
| State persistence | ✅ SQLite | 🟡 in-mem events | ✅ JSON | ✅ files | ✅ SQLite | ✅ (design: SQLite) |
| Event system | 🟡 watch | ✅ EventHub (mem) | ❌ | ✅ status files | ✅ CDC | ✅ (design: persisted) |
| Blocking wait | ❌ poll | ✅ hybrid | ❌ | ❌ | ❌ | ✅ (design: hybrid) |
| Polling | ✅ 500ms | ✅ 100ms | ❌ | ✅ watcher | ✅ 5s reaper | 🟡 bounded |
| Stall detection | 🟡 timeout | ✅ PromptStalled | ❌ | 🟡 stale | ✅ reaper | ✅ (design) |
| Restart | 🟡 resume | 🟡 --resume | 🟡 healState | ✅ reconcile | ✅ Reconcile | ✅ (design) |
| Reconciliation | ❌ | 🟡 | ✅ healState | ✅ inactive | ✅ Boot Reconcile | ✅ (design) |
| Worktree isolation | ❌ | ✅ panes | ✅ | ✅ | ✅ | ✅ (design: treehouse) |
| Workspace lease | ❌ | ❌ | ✅ durable | ✅ worktree | ✅ | ✅ (design: treehouse) |
| Locking | ✅ optimistic | ✅ app-loop | ✅ flock | ✅ task lock | ✅ | ✅ (design) |
| Guard | ✅ fail-closed | ❌ | ❌ | ✅ shape-deny | 🟡 prompt | ✅ (design) |
| Fail-closed | ✅ | 🟡 | ✅ | 🟡 (fail-open malformed) | 🟡 | ✅ (design) |
| Native-subagent prevention | ❌ | ❌ | ❌ | ✅ | 🟡 prompt | ✅ (design) |
| Fleet registry | ❌ | ✅ agent.list | ❌ | ✅ meta | ✅ sessions | ✅ (design) |
| Messaging | 🟡 mail | ❌ | ❌ | ✅ | ✅ | 🟡 P3 (Agent Mail clean-room) |
| Handoff | ❌ | ❌ | ❌ | ❌ | 🟡 conv | ✅ (design: artifact) |
| Hierarchy | ❌ | ❌ | ❌ | ❌ | 🟡 2-level | ✅ (design: SLP) |
| Authority model | 🟡 2-person | ❌ | ❌ | ❌ | 🟡 | ✅ (design) |
| Harness integration | ✅ 3 surfaces | ✅ | ❌ | ✅ | ✅ 23 adapters | ✅ (design) |
| Testing | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ (design) |
| Cross-platform | 🟡 unix | ✅ | ✅ win | 🟡 unix | 🟡 | ✅ (design: Rust) |

Every non-obvious cell traces to source in §5-§9.

---

## 11. Reuse/Adapt/Depend/Reimplement Decisions

| Component | Source | Decision | Rationale |
|---|---|---|---|
| Spawn-risk classifier | slb `PatternEngine` | **ADAPT** | Swap pattern set for spawn shapes; keep normalize+precedence+parse-upgrade |
| Fail-closed unmatched default | slb `request.go:198` | **COPY** | The most important safety default |
| Optimistic-lock claim | slb `db/requests.go:302` | **COPY** | Race-proof atomic claim |
| 5-gate pre-spawn check | slb `execute.go:99` | **COPY** | Merge slb's split gates into one |
| Command hash | slb `db.ComputeCommandHash` | **COPY** | Bind exact spawn command |
| State machine | slb `statemachine.go` | **COPY** (DB layer) | Port one authoritative layer |
| HMAC approval signature | slb `reviews.go:258` | **COPY** | Tamper-proof records |
| Fail-closed hook gen | slb `cli/hook.go` slb_guard.py | **COPY** | Proven PreToolUse generation |
| Config hierarchy | slb `config/loader.go` | **COPY** | defaults<user<project<env<flags |
| Hybrid wait | herdr `api/wait.rs` | **ADAPT** | Event-gated polling + two-phase prompt stall |
| State derivation precedence | herdr `state.rs:2125` | **ADAPT** | visible-blocker>hook>screen |
| Process-group probe | herdr `detect/mod.rs:222` | **ADAPT** | Subagent-spawn detection |
| ManagedAgent spawn health | herdr `state.rs:1949` | **ADAPT** | Did-it-come-up check |
| Workspace lease | treehouse | **DEPEND** | CLI subprocess, clean boundary |
| Worktree crash recovery | treehouse `state.go:101` | **DEPEND** | Fail-closed to leased |
| Guard shape-deny | firstmate `fm-subagent-pretool-check.sh` | **ADAPT** | matcher `.*`, stem scan, scope gate |
| Metadata-before-spawn | firstmate `fm-spawn.sh:2655` | **COPY** | The anti-fleet-blindness invariant |
| Substrate pipeline (derive) | agent-orchestrator | **ADAPT** | OBSERVE→UPDATE→DERIVE |
| Nudge dedup | agent-orchestrator `sendOnce` | **COPY** | `reviewMaxNudge=3` |
| Reaper circuit breaker | agent-orchestrator `reaper.go` | **ADAPT** | Mass-death breaker |
| File reservation + guard | mcp_agent_mail_rust | **CLEAN-ROOM ADOPT** | License blocks copy; hierarchy-neutral |
| Peer identity mechanics | opengoat | **ADAPT** | Per-agent workspace + ROLE.md + session keys; drop disclosure |
| Agent Mail / MCP mesh | mcp_agent_mail_rust | **DO NOT USE** | Visible mesh collapses concealment; not part of CLI-only scope |

**Uniquely anti_subagent (must be authored, nothing to copy):**
1. Independent full-agent spawning (CLI exec).
2. Durable peer identity registry (id + disposition + workspace).
3. Native-subagent escape prevention guard.
4. Lead/Peer authority model.
5. Experience-handoff artifact.
6. SLP governance (Supervisor memory notebook, instruction-patching, Lead replacement).
7. 4-arm benchmark harness.

---

## 12. License Analysis

| Repo | License | Copy permitted? | Decision |
|---|---|---|---|
| slb | MIT + OpenAI/Anthropic Rider (© Jeffrey Emanuel) | Rider excludes Anthropic/OpenAI restricted parties | **ADAPT pattern**, clean-room port |
| herdr | Apache-2.0 | Yes (with attribution) | ADAPT pattern |
| treehouse | MIT (© kunchenguid) | Yes | **DEPEND** (binary dep) |
| firstmate | MIT (© Kun Chen) | Yes | ADAPT pattern |
| gnap | MIT (© Farol Labs) | Yes | Reference |
| multipi | **No LICENSE file** | ⚠️ Unclear | DO NOT USE |
| opengoat | MIT (© Mariano Pardo) | Yes | ADAPT pattern |
| swarm-protocol | MIT | Yes | Reference concept |
| agent-orchestrator | Apache-2.0 | Yes (with attribution) | ADAPT pattern |
| mcp_agent_mail | MIT + AI-Rider | **Hard blocker** for Claude-driven | Clean-room only |
| mcp_agent_mail_rust | MIT + AI-Rider | **Hard blocker** | Clean-room only |
| pi-subagents | MIT | Yes | Reference |
| maestro-orchestrate | Apache-2.0 | Yes | Reference |

**Critical constraint [VERIFIED-FROM-SOURCE]:** the MIT+AI-Rider license (slb, mcp_agent_mail, mcp_agent_mail_rust) grants no rights to Anthropic PBC / OpenAI LLC or anyone acting under their direction. Since the POC is driven by Claude Code, **no code from these three repos may be copied, vendored, or linked.** Treat them as pattern references and implement clean-room. For slb the patterns are small and generic (regex classifier, optimistic UPDATE, hook JSON) — clean-room is low-risk. treehouse (plain MIT) remains a safe binary dependency.

---

## 13. Final Architecture

```
                    anti CLI  (Rust binary)
                         │
        ┌────────────────┼───────────────────┐
        │                │                   │
   command layer    anti daemon           (CLI only — no MCP)
        │                │
        ├── process/runtime layer  → spawns real CLI subprocesses (claude/codex/opencode)
        ├── identity/state layer   → SQLite agent registry (persist BEFORE spawn)
        ├── lifecycle layer        → state machine (slb-derived)
        ├── event layer            → persisted append-only event log (herdr-shaped)
        ├── workspace layer        → DEPENDS on treehouse (get --lease / return)
        ├── guard layer            → PreToolUse shape-deny (firstmate-shaped, slb fail-closed)
        ├── recovery layer         → reconcile PIDs, restore state on restart
        └── harness adapters       → claude / codex / opencode
```

**Design decisions from research:**
- **Language: Rust.** Matches herdr/treehouse (binary, cross-platform, PTY). [THESIS-BASELINE + VERIFIED]
- **Daemon = coordinator, not executor.** slb's "daemon is a notary" lesson: the daemon owns state, events, wait; the *peer* process executes. [VERIFIED]
- **SQLite for state** (single-file, WAL). POC filesystem-first per baseline §12; SQLite when filesystem bottlenecks (identity/events scale). [VERIFIED]
- **treehouse as external dep** — keeps the Supervisor stateless about filesystem. [VERIFIED]
- **CLI-only. No MCP layer is built or planned** (explicit scope decision). The CLI is the single control-plane interface. [THESIS-BASELINE]
- **Peer prompts are a benchmark variable, not a fixed default.** §25 and §22 define the *concealed* prompt, but the peer-prompt text and guard config must be swapped per arm via a `--peer-prompt` parameter (see §34). No prompt text is hard-coded as the sole supported mode. [REVISION]
- **Windows IPC is a P0 gate.** slb's Unix sockets don't apply; the daemon socket transport (named pipes vs TCP localhost) is chosen in P0, not deferred to §39.7. [REVISION]

---

## 14. Component Architecture

```
anti/
├── Cargo.toml            (workspace)
├── crates/
│   ├── anti-core/        identity, lifecycle, events, state machine, hashing (pure)
│   ├── anti-cli/         commands (spawn/list/status/wait/stop/kill/logs/send/handoff/guard/daemon/doctor)
│   ├── anti-daemon/      event loop, process watcher, IPC (local socket), wait engine
│   ├── anti-workspace/   treehouse adapter (shell out to `treehouse` binary)
│   ├── anti-guard/       rule engine + per-harness hook generation (claude/codex/opencode)
│   ├── anti-adapters/    HarnessAdapter trait + ClaudeCode/Codex/OpenCode impls
│   ├── anti-recovery/    reconcile on restart, stale PID/workspace sweep
│   └── anti-bench/       benchmark runner (4-arm)
```
No MCP crate. The CLI is the entire control plane; harness-agnostic integration is achieved by spawning executables and installing per-harness guard hooks, not by an MCP server. [THESIS-BASELINE]

> **[REVISION]** The `anti daemon` subcommand (start/stop/status) is now listed in the CLI spec (§26) — it was previously only implied by the crate layout. A daemon is required to host the guard's fail-closed socket and the process watcher; it is a P0 deliverable, not an afterthought.

**HarnessAdapter trait** (from baseline §3):
```rust
trait HarnessAdapter {
    fn spawn(&self, ctx: &SpawnContext) -> Result<Child, SpawnError>;
    fn stop(&self, agent: &AgentRecord) -> Result<(), StopError>;
    fn kill(&self, agent: &AgentRecord) -> Result<(), KillError>;
    fn status(&self, agent: &AgentRecord) -> Result<AgentStatus, StatusError>;
    fn send(&self, agent: &AgentRecord, msg: &str) -> Result<(), SendError>;
    fn logs(&self, agent: &AgentRecord) -> Result<String, LogsError>;
    fn install_guard(&self, home: &Path) -> Result<(), GuardError>;
}
```

**Guard rule source of truth** — one set of rules, generated per harness:
```
guard/rules.toml        ← the single source of truth (firstmate-shaped stems)
  → adapters/claude → PreToolUse hook JSON (slb claudehooks.go shape)
  → adapters/codex → config gate (spawn_agent)
  → adapters/opencode → tool.execute.before hook
```

---

## 15. Process Model

```
anti spawn --role peer --harness claude --task task.md
  → daemon validates
  → reserve agent id (SQLite)
  → persist AgentRecord(status=CREATED)
  → workspace = treehouse get --lease --lease-holder <id> --json
  → persist workspace lease id
  → adapter.spawn() → OS subprocess (claude --permission-mode ...)
  → attach PID
  → emit AGENT_STARTED
```

**The peer is an independent OS process.** It never goes through the harness's native Task/spawn_agent tool. This is the core invariant (§18).

---

## 16. Identity Model

**AgentRecord** (SQLite):
```json
{
  "id": "peer-01",              // stable, opaque, issued by anti
  "role": "peer",               // supervisor | lead | peer
  "disposition": "engineer",    // engineer|architect|reviewer|scout|proof-auditor|shadow
  "harness": "claude",          // claude | codex | opencode
  "parent": "lead-01",          // null for supervisor
  "pid": 18234,
  "workspace": {"lease_id":"...", "path":"...", "holder":"peer-01"},
  "task": "task.md",
  "status": "running",
  "created_at": "...", "updated_at": "...",
  "restart_count": 0,
  "lifecycle_meta": {"spawn_gen":1, "last_state_change_seq": 42}
}
```

**Identity is persisted BEFORE spawn** (firstmate lesson, §8). Durable identity means: **Peer-01 crash → restart → Peer-01** (same id), not Peer-02 replacement — unless governance explicitly replaces. [THESIS-REQUIREMENT]

**Identity vs concealment** (opengoat lesson, §9.3): durable identity and invisible hierarchy are **independent axes**. The POC must measure them separately (§17).

**[REVISION]** Durable identity (§16) is a *baseline* for all full-agent arms. **Concealment is a benchmark variable.** ARM C toggles concealment **on**, ARM D toggles it **off** — the only difference between the two arms is the `--peer-prompt` and guard config applied at spawn, *not* the identity/state/workspace substrate (which is identical). This keeps concealment swappable as a runtime parameter rather than a hard-coded CLI default (§34).

---

## 17. Lifecycle State Machine

Only states justified by research. States: **CREATED → STARTING → RUNNING → BLOCKED → COMPLETED**; failure states **FAILED, CRASHED, STOPPED, RECOVERING, REPLACED**.

```
                  ┌──────────┐
        (create)  │ CREATED  │
                  └────┬─────┘
                       │ persist metadata
                       ▼
                  ┌──────────┐   spawn fail ──→ FAILED
                  │ STARTING │
                  └────┬─────┘
                       │ process up + health check (herdr ManagedAgent)
                       ▼
                  ┌──────────┐
                  │ RUNNING  │
                  └──┬───┬───┘
                blocked │   │ crash
                    ▼   │   ▼
             ┌────────┐ │ ┌──────────┐
             │ BLOCKED│ │ │ CRASHED  │
             └───┬────┘ │ └────┬─────┘
                 │      │      │ (supervised restart decision)
                 │      │      ▼
                 │      │ ┌──────────┐  ┌────────────┐
                 │      │ │ RECOVERING│→│ (restart ok)→ RUNNING (same id)
                 │      │ └──────────┘  └────────────┘
                 │      │      │ (governance decides replacement)
                 │      │      ▼
                 │      │ ┌──────────┐
                 │      │ │ REPLACED │   (new id issued; old archived)
                 │      │ └──────────┘
                 ▼      ▼
              ┌──────────┐  ┌──────────┐
              │ COMPLETED│  │ STOPPED  │
              └──────────┘  └──────────┘
```

Transitions are enforced by **optimistic-lock UPDATE** (slb `db/requests.go:302` pattern): `UPDATE agents SET status=? WHERE id=? AND status=<expected>`. `Restart` increments `restart_count`, preserves id/workspace/task. [VERIFIED pattern]

**[REVISION]** In SLP arms, **Lead replacement is Supervisor-driven, not self-driven**: a degrading Lead never transitions itself to REPLACED; it signals the Supervisor (durable RECOVERING + `AGENT_DEGRADED` event), and the Supervisor orders the replacement and writes the handoff (§24). This closes the supervisor-replacement gap that agent-orchestrator leaves user-driven (§9.5). In Flat arms there is no Supervisor, so this applies only to ARM C/D.

---

## 18. Spawn Protocol

**Transaction (validate → reserve → persist → allocate → persist → spawn → attach → emit):**
1. `validate` — args valid, role in {supervisor,lead,peer}, harness known, task exists.
2. `reserve agent id` — SQLite `INSERT` with id, status=CREATED (optimistic, unique).
3. `persist metadata` — full AgentRecord written.
4. `allocate workspace` — `treehouse get --lease --lease-holder <id> --json` (blocking; on failure → FAILED, no ghost).
5. `persist workspace` — store `lease_id` on the AgentRecord.
6. `spawn process` — `adapter.spawn(ctx)` → OS subprocess.
7. `attach PID` — update record with pid.
8. `emit AGENT_STARTED` — append event.

**Failure at every step:**

| Failure | Behavior | Idempotency |
|---|---|---|
| workspace alloc fails | status=FAILED; no process; no ghost | retry fresh |
| spawn fails | status=FAILED; release lease (`treehouse return`) | retry fresh |
| process starts but CLI crashes | daemon watches; on restart reconcile PID; restore RUNNING | id preserved |
| anti daemon crashes | state on disk; process survives; reconcile on next start | id preserved |
| process dies immediately | CRASHED; supervised restart (same id) with backoff | restart_count++ |
| duplicate spawn request (same id) | optimistic `INSERT` fails → error | reject |
| concurrent spawn requests (different ids) | each serialized; lease from treehouse pool is unique | each idempotent |

**Key principle (firstmate incident):** persist BEFORE spawn. A spawn that never reaches RUNNING leaves a durable FAILED record — never invisible. [VERIFIED]

---

## 19. Workspace Protocol

**Decision: DEPEND on treehouse** (§7.3). Protocol:
```
acquire:  treehouse get --lease --lease-holder <agent-id> --json
          → {path, lease_id, lease_holder, leased_at}
release:  treehouse return --force --if-lease-id <lease_id>
          → idempotent: on ErrLeasePreconditionFailed, verify via status --json,
            treat as already-released
verify:   treehouse status --json
crash:    treehouse healState marks unknowns leased (fail-closed)
```

**Requirements satisfied** (from baseline §14): one isolated workspace per peer ✓ (lease), race-safe allocation ✓ (lock), cleanup ✓ (`return --if-lease-id`), stale workspace recovery ✓ (healState + lease inventory in anti), process crash handling ✓ (treehouse process scan), daemon restart recovery ✓ (anti carries lease inventory).

**Caveat [VERIFIED]:** leases never expire; anti's Supervisor must carry the lease inventory across restarts (bind to AgentRecord.workspace.lease_id). Unlanded (untracked-not-ignored) files wiped on return — extract deliverables before release.

---

## 20. Event Protocol

Append-only, **persisted** (unlike herdr's in-memory ring — §6.6 risk).

```
~/.anti_subagent/events/000001.jsonl  (append-only, seq monotonic)
```

**Events** (from baseline §15):
```
AGENT_REGISTERED
AGENT_STARTED
AGENT_PROGRESS
AGENT_BLOCKED
AGENT_COMPLETED
AGENT_FAILED
AGENT_CRASHED
AGENT_RESTARTED
AGENT_STOPPED
AGENT_REPLACED
HANDOFF_CREATED
```

**Schema** (baseline §15):
```json
{"seq": 123, "timestamp": "...", "agent_id": "peer-01", "type": "AGENT_COMPLETED", "payload": {}}
```

**Persistence:** append-only JSONL; `current_sequence()` = last seq; `events_after(seq)` for wait replay. Crash-safe: each event is a single-`write`-then-`sync` (recover partial tail). **Seq survives restart** (anti-subagent differs from herdr deliberately). [THESIS-REQUIREMENT + VERIFIED herdr gap]

---

## 21. Wait Protocol

**Decision: ADAPT herdr's hybrid wait** (§6.3) + two-phase prompt.

```
anti wait peer-01 --until done
  → snapshot current_sequence
  → status() — if already in `until`, return
  → loop:
      events_after(last_seq) → replay, set should_probe on state-change events
      if should_probe → status() snapshot
      if status ∈ until → return
      if deadline → error
      sleep(poll_interval=100ms)   # bounded; near-instant on event
```

**Two-phase `anti send` (herdr `prompt --wait` shape):**
- Phase 0: snapshot seq + status before send.
- Phase 1 (activity gate): after send, require ANY state_change_seq bump within a configurable window (default **60s**, not herdr's 5s — LLM agents legitimately go silent during long tool calls / extended thinking; 5s false-positives), else `AGENT_PROMPT_STALLED`.
- Phase 2: wait until target status, replaying events from pre-submit seq.

**This eliminates F3 polling:** the Lead/CLI never loops "are you done?" — it blocks on `anti wait`, which is event-gated. [VERIFIED pattern]

**[REVISION]** The activity-gate window and the poll interval are **config values**, not literals (defaults: `stall_timeout=60s`, `poll_interval=100ms`), read from `config` (§27). This prevents the wait substrate from misfiring on real LLM latency. The `AGENT_PROMPT_STALLED` event stays, but a stall is only declared when no state_change_seq bump occurs within the configurable window.

---

## 22. Guard/Security Model

**Core invariant:** a Peer MUST NOT escape anti_subagent and invoke the harness's native subagent mechanism.

```
Peer → native Task/spawn_agent → Guard → DENY
Peer → anti spawn → independent OS process → coding agent → ALLOW
```

**Guard architecture (firstmate-shaped + slb fail-closed):**
1. **Rule source of truth** — `guard/rules.toml` with delegation-shaped stems (firstmate `DELEGATION_STEMS` shape). One source, generated per harness.
2. **Per-harness installation** — `anti guard install --harness claude|codex|opencode` writes the harness-native hook:
   - Claude Code: PreToolUse hook with **matcher `.*`** (firstmate) + deny object to stderr, stdout empty.
   - Codex: config gate on `spawn_agent`.
   - OpenCode: `tool.execute.before` hook.
3. **Fail-closed:** if anti daemon unavailable → guard DENIES (slb lesson). Unlike firstmate's fail-open-on-malformed, anti_subagent's guard **fails closed** because the threat (native escape) is only detectable when the control plane is up. [THESIS-REQUIREMENT, differs from firstmate deliberately]
4. **Bypass prevention:** escape hatch is env var set at launch (firstmate `FM_ALLOW_SUBAGENT=1` shape), unforgeable in-session.
5. **Scope gate:** guard active only in genuine anti-managed peer workspaces (firstmate `fm_primary_scope_matches` shape) — never in the Supervisor/Lead session itself.
6. **Race:** guard is synchronous at PreToolUse; daemon socket query with short timeout; unknown → deny.
7. **Audit:** every deny logged to event log (`AGENT_REJECTED` with tool name + reason).

**[REVISION — blast-radius cap]** The guard intercepts **only delegation-shaped tool calls**, not every tool call. The hook's first step is a **local, in-process, deny-list check** (tool name matched against `rules.toml` stems, exactly as firstmate's step 1-5): 
- A tool name that is **clearly non-delegation** (e.g. `Read`, `Grep`, `Edit`) is allowed **locally, without a daemon round-trip** — the daemon socket is only queried for the small set of *candidate* delegation tools. If the daemon is down, only those candidate tools deny; Read/Grep/Edit keep working.
- This bounds the fail-closed blast radius to the delegation surface instead of bricking the peer's entire tool use when the daemon is flaky (was §40 "Med", now an explicit design point).

**[REVISION — concealment leak + per-arm guard config]** The guard hook config is installed into the peer's own home/workspace (§22.2); a peer with Read could in principle read `.claude/hooks.json` or the guard rules and discover the orchestration layer. Two mitigations: (a) for ARM C (concealed), the hook file lives **outside the peer's readable scope** where the harness supports it (e.g. a harness-owned config dir), otherwise the concealment leak is accepted and noted; (b) `anti guard install` takes the same `--peer-prompt`/`--arm` toggle as `anti spawn`, so the disclosed arms (B/D) can install a guard that explicitly says "an orchestrator may dispatch work" while the concealed arm (C) installs one that does not. The guard config is **arm-parameterized, not hard-coded**. [THESIS-REQUIREMENT, differs from firstmate deliberately]

**What anti_subagent does NOT copy from firstmate:** fail-open-on-malformed transport (firstmate), and prompt-only subagent ban (agent-orchestrator). Both are weaker than a hard guard. [VERIFIED + THESIS]

---

## 23. Recovery Model

**Scenarios (baseline §15):**

| Scenario | Behavior |
|---|---|
| Peer crash | detect → mark CRASHED → supervised restart (same id, backoff) |
| Lead crash | preserve peers + artifacts; new Lead reads state/handoff and resumes |
| Supervisor/daemon crash | state on disk survives; process survives; reconcile on restart (firstmate session-start sweep shape) |
| CLI restart | state survives; process survives; reconcile PID |
| Machine restart | state survives; process gone → mark CRASHED → supervised restart, or preserve handoff |
| Partially completed spawn | STARTING without PID → FAILED (no ghost) |
| Stale PID | on reconcile, `kill(pid,0)` liveness check → dead → CRASHED |
| Stale workspace | treehouse healState; anti marks leased; carry lease inventory |
| Duplicate agent | optimistic INSERT rejects |
| Lost event | append-only JSONL + seq replay; tail recover |

**Critical requirement (baseline §15):** "Agent state must not disappear merely because the control process restarts." Achieved via SQLite AgentRecord + persisted events + treehouse lease. [THESIS-REQUIREMENT]

**Recovery keyed on durable identity, not tool-call interception** — the firstmate docs' identified durable fix (§8.6). anti_subagent's supervision is state-based from the start. [VERIFIED + THESIS]

---

## 24. Handoff Model

**The thesis-specific artifact.** Placed in the lifecycle: when a Lead degrades (~5-7 compactions) or is replaced, the Supervisor orders a handoff; the outgoing Lead's lessons are written to a durable artifact the successor reads.

```
handoffs/lead-001-001.md     (or .json — format is OPEN QUESTION §39)
```

**Minimum content** (baseline §16):
```
Objective
Current state
Completed work
Unfinished work
Decisions (incl. rejected approaches)
Known failures
Tests / verification status
Risks
Next actions
```

**Where it sits in lifecycle:** Lead status → RECOVERING → (Supervisor writes handoff artifact) → REPLACED → new Lead reads artifact → RUNNING. The artifact is **persisted before** the old Lead is torn down (firstmate lesson). [THESIS-REQUIREMENT]

**Note:** opengoat has conversation continuity (not a standardized artifact); agent-orchestrator has project-scoped orchestrator narrative (partial). No repo ships a standardized lesson-transfer artifact — this is uniquely anti_subagent's to author. [VERIFIED]

---

## 25. Harness Adapter Model

**POC: Claude Code only** (Phase 0-3), then Codex (Phase 5), then OpenCode.

| Adapter | Spawn | Stop | Guard |
|---|---|---|---|
| Claude Code | `claude --permission-mode <mode> --append-system-prompt-file <peer-prompt> --session-id <uuid>` (interactive in PTY, per agent-orchestrator `BuildLaunchCommand`) | `kill` / graceful exit | PreToolUse hook (matcher `.*`) + optional per-home `permissions.deny` |
| Codex | `codex exec [--resume] --skip-git-repo-check --session <id> -- <msg>` (per opengoat `codex/provider.ts`) | `kill` | config gate on `spawn_agent` |
| OpenCode | `opencode run --format json [--session <id>]` with `OPENCODE_PERMISSION` allow (per opengoat) | `kill` | `tool.execute.before` hook |

**System-prompt override (identity concealment):** peer receives a clean session with a prompt like "You are working with a human project owner" — **no mention of being an agent/peer in an org** (opengoat's disclosure model is deliberately dropped). **[REVISION — concealment is a benchmark variable, not the default.]** The prompt above is the **ARM C (concealed)** prompt. The same `--peer-prompt` parameter that feeds `adapter.spawn()` also toggles the guard config (§22) and is what distinguishes ARM C from ARM D (disclosed: "You are a peer in an SLP hierarchy; you report to a Lead"): the substrate is identical, only the prompt/guard differ. No prompt text is hard-coded as the sole supported mode. [THESIS-REQUIREMENT — see §39/§17 for the open axis]

**Tool list stays default** — the peer gets no orchestration tools (multipi/pi-subagents mistake avoided). [VERIFIED + THESIS]

---

## 26. CLI Specification

**Commands (baseline §19 + POC-scope):**

### `anti spawn`
- Purpose: create a durable agent, allocate workspace, launch independent process.
- Args: `--role peer|lead|supervisor`, `--disposition`, `--harness claude|codex|opencode`, `--task <file>`, `--repo <path>`, `--parent <id>`, `--model <name>`, `--peer-prompt <file>` **[REVISION — the concealment toggle; distinct prompt per arm]**.
- Output (JSON): `{id, status:"starting", pid, worktree, lease_id}`.
- Exit codes: 0 success; 1 error; 2 invalid args; 3 duplicate id.
- State: CREATED→STARTING→RUNNING; on any failure → FAILED (never invisible).
- Concurrency: optimistic id reservation; treehouse lease unique.

### `anti list`
- Purpose: enumerate agents. Args: `--role`, `--status`, `--json`. Output: table/JSON of AgentRecords. State: none (read).

### `anti status <id>`
- Purpose: current state + derived status. Output: full AgentRecord + status. Exit: 0 running, 1 not-found, 2 terminal-failed.

### `anti wait <id> [--until <status>] [--timeout <s>]`
- Purpose: block until status (event-gated, no F3 polling). Output: final status. Exit: 0 reached, 1 timeout, 2 not-found.

### `anti stop <id>`
- Purpose: graceful stop (send exit, wait). State: RUNNING→STOPPED.

### `anti kill <id>`
- Purpose: force kill (SIGKILL after grace). State: RUNNING→CRASHED (or STOPPED).

### `anti logs <id>`
- Purpose: tail the peer's stdout/stderr log.

### `anti send <id> <message>`
- Purpose: send text to the peer's session (with two-phase activity gate — herdr §21).

### `anti handoff <lead-id>`
- Purpose: write the experience-handoff artifact for a degrading/replaced Lead. State: Lead→REPLACED; writes `handoffs/<id>-N.md` before teardown.

### `anti guard install|status|test`
- Purpose: install/verify per-harness guard hooks; `test` classifies a tool name (deny/allow). Fail-closed semantics.
- **[REVISION]** `install` takes `--peer-prompt`/`--arm` to parameterize the guard config per arm (§22), and `status` verifies the daemon socket reachability that the fail-closed path depends on.

### `anti doctor`
- Purpose: check daemon, state dir, treehouse dependency, guard installation.

### `anti daemon start|stop|status` **[REVISION — P0 deliverable]**
- Purpose: manage the control-plane daemon. `start` launches the daemon (owns state/events/wait and the guard's fail-closed socket); `status` reports PID + IPC transport (named pipes vs TCP localhost, chosen in P0 — §13). The daemon was previously only implied by the crate layout (§14); making it an explicit CLI command closes the gap.
- Exit: 0 running, 1 not-running, 2 transport-unavailable.

**Global flags:** `--json`, `--state-dir` (default `~/.anti_subagent/`), `--config` (defaults<user<project<env<flags, slb pattern).

---

## 27. Data Model

SQLite schema (anti_subagent):
```
agents(id PK, role, disposition, harness, parent_id, pid, workspace_lease_id,
       workspace_path, task_path, status, restart_count, spawn_gen,
       created_at, updated_at, last_state_change_seq)
events(seq INTEGER PK AUTOINCREMENT, agent_id, type, payload_json, created_at)
handoffs(id, lead_id, created_at, content_path)
config(key, value)                            -- incl. stall_timeout, poll_interval, ipc_transport
```
> **[REVISION]** The `locks` table is **removed**. Optimistic locking is enforced purely by `status`-guarded `UPDATE ... WHERE status=<expected>` (slb pattern); a separate lock table would split authority and was contradictory as written.

Filesystem:
```
~/.anti_subagent/
├── agents/<id>.json          (AgentRecord snapshot; SQLite is authoritative)
├── events/000001.jsonl        (append-only)
├── handoffs/<lead>-<N>.md
├── runs/<run-id>/run.json
├── logs/<id>.log              (peer stdout/stderr)
├── prompts/<id>.md            (the clean peer session prompt)
└── locks/
```

**Design choice:** SQLite (single file, WAL) for state — simpler than firstmate's file-fanout, more robust than herdr's in-memory events. POC filesystem-first per baseline §12; SQLite when it bottlenecks. [VERIFIED]

---

## 28. Persistence Model

- **SQLite** (authoritative state): AgentRecords, events index. WAL mode, `busy_timeout`, `synchronous=NORMAL` (slb `db.go:167-193` pattern).
- **Append-only JSONL** for events (crash-safe, seq survives restart — differs from herdr deliberately).
- **treehouse state** is external (DEPEND).
- **Git archive** for handoffs + benchmark evidence (audit trail, gnap/mcp_agent_mail concept).
- **Writes are atomic** (temp + fsync + rename, treehouse `state.go:146` pattern).

**Recovery on restart:** scan events → rebuild in-memory state → reconcile agents (PID liveness, workspace leases) → mark CRASHED where process dead, keep RUNNING where alive. [THESIS-REQUIREMENT]

---

## 29. Failure Modes

From research, the failures anti_subagent must not repeat:
1. **Fleet blindness** (firstmate incident): spawn without metadata → invisible. Fixed by persist-before-spawn.
2. **Loss on restart** (firstmate): subagent work in primary's process tree. Fixed by independent OS process.
3. **Silent supervision collapse** (firstmate): metadata-keyed guard inert. Fixed by durable-identity-keyed supervision.
4. **In-memory event loss** (herdr): events vanish on restart. Fixed by persisted event log.
5. **Trust leak in fences** (pi-subagents): session files editable by child. Fixed by identity registry owned by daemon + read-only peer prompts.
6. **Lease leak** (treehouse): leases never expire. Fixed by lease inventory in anti + reconciliation.
7. **Unsupervised recovery** (herdr): crashed agent not auto-restarted. Fixed by supervised restart with backoff.
8. **Visible hierarchy → F4** (opengoat/agent-orchestrator): workers know they're in an org. This is the benchmark axis (§17).

**[REVISION — confounded-variable failure]** The original 3-arm design would have failed to distinguish *hierarchy* from *concealment*: ARM B ("flat, peers know each other") collapsed two variables, so a result would be uninterpretable. Fixed by the 4-arm design (§34): ARM C vs D isolates concealment with the substrate held constant; ARM B vs D isolates hierarchy with disclosure held constant.

---

## 30. Threat Model

| Threat | Mitigation |
|---|---|
| Peer escapes to native subagent | Guard (PreToolUse shape-deny, fail-closed) |
| Peer forges identity / rewrites its own record | daemon owns registry; peers get no state-write access; optimistic locks |
| Peer reads orchestration layer (sees hierarchy) | Clean peer prompts; no orchestration tools; guard on MCP tools in peer sessions |
| Malicious peer sends fake completion | Verdict protocol (Lead council) + proof requirement (agent-orchestrator reviewer shape) |
| Daemon down → peers act unsupervised | Guard fails closed (deny native escape); peers continue but can't spawn |
| Workspace cross-talk between peers | treehouse lease isolation + process scan |
| Stale process holds workspace | treehouse `TerminateWorkprocesses` (parent-chain protected) |
| Duplicate execution of same task | Optimistic claim (first-executor-wins) |
| Handoff lost on Supervisor crash | Handoff artifact written before teardown; persisted |
| License contamination | Clean-room from AI-Rider repos; treehouse (plain MIT) only as binary dep |

**Threat scope (POC):** local, single-machine, trusted-user. No production auth, no network. [THESIS-BASELINE]

---

## 31. POC Scope

**In scope (Claude Code only):**
- anti CLI (spawn/list/status/wait/stop/kill/logs/send/handoff/guard/doctor)
- Process/runtime layer (spawn independent `claude` subprocess)
- Identity/state layer (SQLite AgentRecord, persist-before-spawn)
- Workspace layer (DEPEND treehouse)
- Event layer (append-only, persisted)
- Wait protocol (hybrid, two-phase send)
- Guard (Claude PreToolUse)
- Recovery (restart/reconcile)
- Benchmark harness (4-arm)

**Out of scope (explicitly):**
- ❌ Web UI, distributed cluster, Kubernetes, complex scheduler, production auth
- ❌ 10 harness adapters (one: Claude Code)
- ❌ MCP server of any kind (explicit scope decision — CLI is the only control plane)
- ❌ Automatic Supervisor intelligence (Supervisor is a documented role, not an autonomous agent in POC)
- ❌ Database infrastructure beyond SQLite
- ❌ Agent Mail mesh

---

## 32. Explicit Non-Goals

1. Prove "subagents always bad" — the claim is scoped to long-horizon work.
2. Build a full multi-agent framework.
3. Replace Claude Code / Codex / OpenCode.
4. Solve harness-native tool interop for all agents at once.
5. Production-grade security/RBAC.
6. Distributed execution.
7. Benchmark on trivial tasks.

---

## 33. Implementation Phases

| Phase | Deliverable | Gate |
|---|---|---|
| **P0** | Data model + `anti spawn/list/status` for Claude Code | Spawn mechanics work (process up, durable record, workspace leased) |
| **P1** | Persistence + events + hybrid wait | `anti wait --until done` returns without polling; state survives CLI restart |
| **P2** | Guard (Claude PreToolUse) | Native escape denied; fail-closed when daemon down |
| **P3** | Recovery + reconcile | Peer crash → supervised restart same id; Supervisor restart preserves state |
| **P4** | Benchmark harness (4-arm) | Same task, same env, 5 runs/arm, metrics auto-collected |
| **P5** | Second harness (Codex) | Adapter trait proven; guard adapts |

**The plan ends at P5. There is no MCP phase** — the CLI is the entire control plane by explicit scope decision. **Do not proceed to P3 until P0-P1 gates pass** (baseline §14: "If CLI chưa chứng minh được... thì MCP chỉ là API wrapper quanh architecture chưa chứng minh" — and here we do not add MCP at all).

**[REVISION — P0 gates, do not defer]**
- Windows IPC transport is chosen (named pipes vs TCP localhost) and implemented before the daemon socket work is considered done.
- The `--peer-prompt`/`--arm` parameter is threaded through `spawn` + `guard install` **in P0**, so concealment stays a runtime toggle and ARM C/D can be built without reworking the adapter layer later.
- `anti daemon start|status` exists before P1's wait protocol is built (the wait engine + guard both need a daemon).
- Stall-timeout and poll-interval are config values (defaults 60s / 100ms) from the start, not literals.

---

## 34. Benchmark Architecture

**4 arms (baseline §20, §19):**
```
ARM A: Native Subagent     Lead + harness-native Task/spawn_agent workers
ARM B: Flat Full-Agent     Lead + independent OS process workers (no SLP hierarchy, peers know each other)
ARM C: SLP (concealed)     Supervisor + Lead + independent peers (invisible hierarchy)
ARM D: SLP (disclosed)     Supervisor + Lead + independent peers (hierarchy visible to workers)
```

**Controlled (same across arms):** repo, commit, task, model, tools, token budget, timeout, environment, test command.
**Varied:** orchestration architecture only.

**Run protocol (baseline §19):** 5 runs per task per arm; randomize execution order; never single-run.

**4-arm design [REVISION]:** the original 3-arm design collapsed two variables into ARM B. Four arms separate them:
- **ARM A — Native Subagent:** Lead + harness-native `Task`/`spawn_agent` workers (subagent → full-agent independence axis).
- **ARM B — Flat Full-Agent, disclosed:** Lead + independent OS-process workers, **no hierarchy**, peers disclosed (full-agent independence → SLP hierarchy axis, disclosure held constant).
- **ARM C — SLP, concealed:** Supervisor → Lead → Peer, invisible hierarchy (the thesis's original claim).
- **ARM D — SLP, disclosed:** same SLP substrate, hierarchy **disclosed** to workers (represents the opengoat/agent-orchestrator configuration §9.3/§9.5).

C vs D differs **only** in `--peer-prompt` + guard config (§22); substrate is identical.

**Blinding:** the `review_score` is scored by a reviewer **blind to which arm produced the artifact** — the benchmark writer (who knows the arms) is not the scorer. Human artifacts are stripped of agent IDs / arm tags before review.

**Pre-registered comparison method:** primary outcome = task_success rate; primary comparison = two-sided exact sign test between arms (treating the 5 runs per task per arm as paired); declare "better" only when the sign test reaches p<0.05 **and** the effect exceeds a stated minimum (e.g. ≥1 success / ≥20% token reduction). Report raw data, not just summary stats.

**This is the critical design.** Only ARM B vs D isolates whether the SLP hierarchy adds value beyond full-agent independence (both disclosed); only ARM A vs B isolates whether full agents add value beyond native subagents; only ARM C vs D isolates whether concealment adds value (the §39.1 axis), with the substrate held constant. [THESIS-REQUIREMENT]

---

## 35. Benchmark Tasks

Long-horizon, multi-file, unfamiliar repo (baseline §16, §20):
1. "Add a new authentication provider to an existing TypeScript service (config, runtime integration, tests, docs, backward compatibility)."
2. "Investigate and fix a flaky integration test suite; preserve existing behavior."
3. "Implement a feature X across 5-15 files in an unfamiliar codebase; add tests; run full suite."
4. "Refactor a module with a hidden edge case; document architectural decision; update dependent code."
5. "Investigate a performance regression and fix it with tests."

**Avoid:** typo fixes, single-function additions, variable renames. Per opencode-solo's honest caveat (`README.md:89-92`): short single-bug tasks may not distinguish architectures. [VERIFIED]

---

## 36. Metrics

| Group | Metric | Collection |
|---|---|---|
| Correctness | task_success, tests_passed, regressions, review_score | test runner + reviewer agent + human review |
| Efficiency | wall_time, input_tokens, output_tokens, total_tokens, context_consumption | harness API usage + anti event log |
| Coordination | messages, polling ops, waits, handoffs, duplicate_work | anti event log (AGENT_PROGRESS/COMPLETED counts) |
| Reliability | crashes, restarts, state_loss, recovery_success, duplicate_execution | anti event log + AgentRecord restart_count |
| Thesis-specific | native_subagent_escape, ownership_loss, context_degradation, unjustified_agreement, supervision_failure | guard log + Lead handoff artifacts + post-hoc transcript analysis |

Every metric is collected from **anti's own persisted logs** (events, AgentRecord, guard denials, handoff files) + harness usage APIs — not from agent self-reports (opencode-solo lesson: "Never declare success based on self-report", `agent/solo.md:88`). [VERIFIED + THESIS]

---

## 37. Failure Injection

| # | Test | Setup → Inject → Expect → Observe → Pass/Fail |
|---|---|---|
| 1 | Kill Lead | start SLP arm → kill lead pid → restart → peers survive, work continues |
| 2 | Kill Peer | start → kill peer-02 → daemon detects → restart same id, no duplicate execution |
| 3 | Kill anti daemon | start → kill daemon → state survives; guard fails closed (native escape denied) |
| 4 | Restart anti daemon | kill + restart → reconcile PIDs, restore RUNNING |
| 5 | Native subagent escape | peer attempts Task tool → guard DENY + audit log |
| 6 | Concurrent spawn | 10 parallel `anti spawn` same id → 1 succeeds, 9 reject (optimistic) |
| 7 | Duplicate task assignment | two peers claim same task → first-executor-wins |
| 8 | Stale workspace | leave dirty worktree → treehouse healState marks leased → anti carries inventory |
| 9 | Lost event | truncate event log tail → seq replay recovers |
| 10 | Context degradation/compaction | long task → force compaction → measure ownership/state loss |

Each recorded as setup→inject→expected→observed→pass/fail in `runs/<run-id>/failure-injection.md`.

---

## 38. Definition of Done

**Runtime:**
- [ ] Independent process spawn (not native subagent)
- [ ] Durable identity (persist-before-spawn; restart → same id)
- [ ] Persistent state (survives daemon/CLI restart)
- [ ] Workspace isolation (treehouse lease per peer)
- [ ] Lifecycle (state machine correct, all transitions)
- [ ] Events (persisted, append-only, seq survives restart)
- [ ] Blocking wait (hybrid, no F3 polling)
- [ ] Restart/reconcile

**Security:**
- [ ] Native subagent blocked (guard)
- [ ] Fail closed (daemon down → deny)
- [ ] Race-safe spawn (optimistic)
- [ ] No duplicate task execution

**Harness:**
- [ ] Claude Code adapter

**Benchmark:**
- [ ] Native / Flat / SLP-concealed / SLP-disclosed arms
- [ ] 5 runs/arm
- [ ] Automatic metrics
- [ ] Reproducible (same repo/commit/model/task)

**Final output:**
```
POC RESULT
Native        PASS: X/Y  tokens: ...  recovery: ...
Flat Full     PASS: X/Y  tokens: ...  recovery: ...
SLP           PASS: X/Y  tokens: ...  recovery: ...
```

---

## 39. Open Questions

1. **[THESIS-CHALLENGE]** Is "invisible hierarchy" (identity concealment) a **causal lever** for F4, or a deployment choice? opengoat/agent-orchestrator show full-agent fleets work with visible hierarchy. **[REVISION]** The 4-arm benchmark (§34) now separates the three variables: **ARM C vs D isolates concealment** (same SLP substrate, prompt/guard swapped only); **ARM B vs D isolates hierarchy** (both disclosed). This resolves the axis the 3-arm design confounded. **Concealment is treated as a benchmark variable throughout** — §16/§22/§25 no longer hard-code it as the default; `--peer-prompt` and `--arm` thread through spawn and guard install. **This is the single most important design risk to resolve before the benchmark.**
2. Experience-handoff artifact **format** (.md vs .json vs both)? Research baseline leaves it open.
3. Control-plane events **schema** (context %, review-count > 3)? herdr's `events.subscribe/wait` is the substrate reference.
4. Is the guard's **fail-closed-on-malformed** stance (vs firstmate's fail-open) the right tradeoff? It prevents escape but can block legitimate delegation if the daemon is flaky.
5. **Peer prompt wording** for "working with a human" — exact phrasing that achieves concealment without harming autonomy.
6. Does the POC need a **verdict council** (Engineer/Reviewer/Architect) in the SLP arm, or is a single Lead verdict sufficient for P0-P4?
7. **Windows IPC** for the daemon socket (named pipes vs TCP localhost) — slb is Unix-only; herdr/treehouse are cross-platform. **[REVISION — resolved in P0, not deferred]** Windows is the primary dev environment, so the transport is chosen and implemented as a P0 gate (§33); if it blocks, it blocks P0, not P5.

---

## 40. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| Benchmark can't distinguish arms (short tasks) | High | Long-horizon tasks only (§35) |
| "Invisible hierarchy" claim untestable as designed | High | 4-arm design separates concealment (C vs D) from hierarchy (B vs D); §34.2 |
| License contamination (AI-Rider) | High | Clean-room; treehouse plain-MIT binary only |
| Guard fail-closed blocks legitimate work | Med | **Blast radius capped to delegation-shaped calls (§22)**; escape hatch at launch; audit log; `doctor` |
| Lease leak on Supervisor crash | Med | Lease inventory persisted; reconcile |
| Peer session files editable (pi-subagents leak) | Med | daemon-owned registry; peers no state write |
| herdr recovery manual (no supervised restart) | Med | anti adds supervised restart w/ backoff |
| treehouse not-a-daemon → same-machine only | Low (POC) | Documented limitation |
| Scope creep (MCP/UI/cluster) | High | Explicit non-goals (§32) |
| **Windows IPC transport blocks the daemon (P0)** | High | Transport chosen + implemented as a P0 gate (§33); do not defer to §39.7 |
| **Benchmark cost** (5 tasks × 4 arms × 5 runs) | Med | Rough estimate §43; right-size to budget before P4 |
| **Concealment leak via readable guard config** | Med | Hook file outside peer's readable scope where harness allows; else accept + document (§22) |

---

## 41. Future Extension: Protocol, Not MCP

**Decision: CLI-only, no MCP layer is built or planned** (explicit scope decision). Harness-agnostic reach is achieved through the CLI itself — any coding agent that can shell out to a command can use anti_subagent. This matches the thesis baseline's observation that "CLI để mọi agent có thể dùng" is the first-class universal interface.

**Extension path (CLI-native, in priority order):**
1. **Second harness adapter (Codex, then OpenCode)** — the `HarnessAdapter` trait + per-harness guard hooks are the extension surface (§14, §25). This is what makes "repo nào cũng dùng được", not an MCP server.
2. **Guard rule evolution** — extend `guard/rules.toml` stems and per-harness generation as harnesses ship new delegation-shaped tools (firstmate's matcher-`.*` shape makes this future-proof).
3. **Standardized handoff artifact** (open question §39.2) — once the format is settled, it becomes the durable cross-agent contract.
4. **Verdict protocol** (council Engineer/Reviewer/Architect, §39.6) — implemented as CLI commands the Lead invokes, not as a service.

**Explicitly NOT pursued:** an `anti mcp-serve` server, `spawn_peer`-as-MCP-tool, or any orchestration tool exposed into Peer sessions. A Peer must never see orchestration machinery (identity concealment — the CLI keeps it invisible because peers simply are not handed these commands). [THESIS-BASELINE + VERIFIED]

---

## 42. Final Recommendation

1. **Build the POC as a Rust CLI**, DEPENDING on treehouse for workspace lease, ADAPTING slb (guard patterns) + herdr (hybrid wait) + firstmate (metadata-before-spawn, shape-deny) + agent-orchestrator (substrate pipeline) as patterns, and **clean-room authoring the SLP-specific layer** (identity registry, guard, handoff, events, benchmark).
2. **Resolve §39.1 via the 4-arm design** — the invisible-hierarchy axis is now measured as ARM C vs ARM D, with concealment a **benchmark variable** threaded as `--peer-prompt`/`--arm` through spawn and guard install. It is a runtime toggle, not an assumed invariant. Proceed with P0–P3 first: those are implementation-agnostic to how §39.1 resolves.
3. **Follow the phases** (§33): P0 spawn → P1 persist+events+wait → P2 guard → P3 recovery → P4 benchmark → P5 second harness. Do not skip gates. **CLI-only; no MCP phase exists.**
4. **Measure everything from anti's own logs**, never agent self-reports.
5. **The thesis is not yet proven.** The plan's value is that it converts the thesis from a manifesto into a **falsifiable 4-arm experiment** whose only unknown is whether independent full agents + SLP hierarchy (+ concealment) actually outperform native subagents on long-horizon work. Blind the review, pre-register the comparison, report raw data (§34.2).

---

## 43. Cost Estimate

**[REVISION — added to make P4 a real gate, not a surprise.]** Rough budget for the benchmark (the dominant cost driver), plus dev-time for P0–P3.

**Assumptions:** long-horizon tasks ≈ 100–400k input + 30–100k output tokens per full-agent run (single agent). SLP arms (C/D) run ~3 concurrent agents; ARM A runs a Lead + subagents. Multiply by a conservative 1.2× for restarts/reconciling.

| Arm | Tokens per run | Runs (5 tasks × 5 reps = 25) | Total |
|---|---|---|---|
| A — Native Subagent | ~150k in / 60k out | 25 | 3.75M in / 1.5M out |
| B — Flat Full-Agent (disclosed) | ~250k in / 80k out | 25 | 6.25M in / 2M out |
| C — SLP (concealed) | ~3 × 250k = 750k in / 240k out | 25 | 18.75M in / 6M out |
| D — SLP (disclosed) | ~3 × 250k = 750k in / 240k out | 25 | 18.75M in / 6M out |
| **Total** | | **100 runs** | **~47M in / ~16M out** |

At ~$3/M in + $15/M out (rough Opus-class pricing), this is **~$350–400 per full sweep**; ~$500 with restarts. A single full benchmark = 100 agentic runs (many with 3 concurrent agents) — **~20–30 wall-clock hours** of agent time on a decent machine.

**Right-sizing levers (if the budget is tight):**
- Drop to 3 tasks × 5 reps = 60 runs (~$250) — still distinguishes arms on the sign test.
- Run ARM C/D at the same token *cap* as B instead of 3× (make hierarchy budget-neutral) — changes what's being tested; decide deliberately.
- Reduce reps only to 3 for ARM C/D (the two arms that cost the most) once they track each other on early runs.

**Dev-time (P0–P3):** the infra itself is modest — a Rust CLI + SQLite + subprocess spawn + hooks is **~3–5 engineer-days** to P2 (spawn/persist/guard), P3 recovery ~2–3 more days, P4 harness ~3–5 days. The benchmark runs are the real cost; budget the sweep as part of P4.

---

*Appendix: source anchors. All archaeology was done on shallow clones (commit-pinned where noted). For per-claim anchors see §5-§9. The three AI-Rider repos (slb, mcp_agent_mail, mcp_agent_mail_rust) were used as pattern references only — no code copied, per §12.*
