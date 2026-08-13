# Research Report — Related Repositories vs. the SLP Thesis

> **Status:** as-of 2026-08-13, shallow clones (1 commit) read in full.
> **Purpose:** evidence corpus for the `anti_subagent` thesis — the SLP (Supervisor → Lead → Peer) architecture as the fix for harness-native-subagent failure modes.
> **Method:** each repo was read end-to-end (source, AGENTS.md, config, agent definitions, skills, docs). Findings cite `file_path:line_number` where a claim is anchored to code.

---

## 0. The thesis, in one paragraph

Harness-native subagents make multi-agent work *worse*. Four failure modes recur in production:

| # | Failure mode | What happens |
|---|---|---|
| F1 | **Subagents become livestock** | Workers read/edit files to order; nothing beyond the call — "how is that different from calling a function?" |
| F2 | **Reflexive agreement** | Subagent over-agrees with the orchestrator (or resists everything) to please the requester; neither is correct work |
| F3 | **Context burned on polling** | Orchestrator polls "are you done yet" instead of reasoning; wastes context, misses state that changed underneath |
| F4 | **Identity deception** | Worker knows it is subordinate, stops acting like an owner of the outcome |

**The fix is not "no agents" — it is full agents:** every worker is a full, autonomous agent (spawned plainly, addressed as a human, free to disagree). Hierarchy is invisible to workers. Peers come in *dispositions*, not ranks. Architecture: **Supervisor → Lead → Peer (SLP)**.

This report maps 12 real repositories against that thesis — which pieces of SLP already exist, which counterexamples argue against it, and which claimed gaps are confirmed.

---

## 1. Repository map

| Repo | ⭐ | Stack | Category | Relation to thesis |
|---|---|---|---|---|
| `herdrdev/herdr` | 28.4k | Rust | Terminal substrate | "Runtime coding agents live on" — substrate SLP runs on |
| `kunchenguid/treehouse` | 1.4k | Go | Worktree pool | Peer-isolation at filesystem layer |
| `josstei/maestro-orchestrate` | 455 | JS/MCP | Counterexample | 39-specialist orchestrator → native subagents |
| `edxeth/pi-subagents` | 107 | TS/Pi ext | Counterexample | Harness-native subagent machinery; self-documents the thesis |
| `simota/agent-skills` | 71 | Skills | Skill ecosystem | 123 SKILL.md + Nexus hub-spoke, 4-tier governance |
| `Dqz00116/opencode-solo` | 32 | MD config | Close-to-SLP | Closed-loop orchestrator, adversarial verify, SWE-bench data |
| `tempont/small-opencode-orchestrator` | 31 | MD config | Close-to-SLP | "Delegation is a permission boundary" |
| `AndyShaman/senior-fable` | 23 | Plugin | Close-to-SLP | "Expensive model decides, cheap models type" |
| `Dicklesworthstone/slb` | — | Go | Safety gate | Execution-safety gate (not supervision) |
| `kunchenguid/firstmate` | — | Bash distro | Crew distro | Fleet supervision *by watching*; ships subagent guard |
| `ReinaMacCredy/maestro` | — | Rust | Work tracker | Gated card store + proof/QA |
| `quangdang46/discord-digests` | — | MD | Evidence corpus | The working research corpus behind the thesis |

---

## 2. Group A — Substrate: what SLP runs on

### 2.1 herdr — the runtime your coding agents live on

**Architecture.** Rust daemon terminal-workspace. TUI is only *one client* of a background server (`src/server/headless.rs:1-15`): the server runs headless (no raw mode, no stdin), owns two sockets — `herdr.sock` (JSON-RPC, used by agents) and `herdr-client.sock` (binary render protocol). When you run `herdr` bare, `auto_detect_launch` checks for a live server, spawns the daemon if absent, waits up to 15s for the socket, then attaches as a thin client (`src/server/autodetect.rs:189-306`). `--no-session` escapes to monolithic mode.

**Two transports.**
1. **JSON-RPC over NDJSON** on a Unix socket (named pipe on Windows) — the agent-facing API (`src/api/mod.rs`).
2. **Binary render protocol** with `PROTOCOL_VERSION` handshake; version mismatch refuses attach with restart instructions (`src/server/autodetect.rs:120-173`).

**Data model.** `Workspace → Tab → Layout (pane tree) → Pane → PTY`. Each `TerminalState` holds `id, cwd, detected_agent, fallback_state, hook_authority, agent_metadata, state, revision` (`src/terminal/state.rs:120-150`). AppState is "pure data, no channels or async runtime. Testable without PTYs" (`src/app/state.rs:1318-1319`).

**Core state machine — two-tier "status authority".** Each pane has exactly one authority (`docs/agents.mdx:41-49`):
1. **Lifecycle hooks** (pi, omp, mastracode, opencode, kilo, kimi) report state over the socket → hook-authoritative (`src/detect/mod.rs:295-305`).
2. **Screen manifests** — for hook-less agents (Claude Code, Codex), state is *inferred* from live bottom-buffer snapshots via TOML detection rules. Deliberately strict: *"Herdr only marks blocked when the live bottom-buffer snapshot matches known visible approval, question, or permission UI"* (`docs/agents.mdx:58-61`).

Arbitration lives in `recompute_effective_state` (`src/terminal/state.rs:2125-2166`): hook authority if effective (full-lifecycle + process alive), else fallback from screen detection; `visible_blocker_overrides_hook` lets an unmistakable blocker screen override a non-blocked hook report (`state.rs:1837-1848`). `AgentState` = `Idle | Working | Blocked | Unknown`; API adds `Done` (same as Idle but for unseen background work). A sequence number on reports prevents stale state from resurrecting (`state.rs:859-989`).

**Agent-native control surface** (JSON-RPC methods, `src/api/schema.rs:40-243`):
- **Agent control:** `agent.list/get/read/explain/send_keys/rename/view.set/view.clear/focus/start/prompt/wait`
- **Pane control:** `pane.split/swap/zoom/send_text/send_keys/read/process_info/wait_for_output`
- **Self-report:** `pane.report_agent`, `pane.report_agent_session`, `pane.report_metadata`, `pane.release_agent`
- **Events:** `events.subscribe`, `events.wait` (event hub, 512-entry buffer, monotonically increasing sequence numbers)

**Waiting between agents.** `agent.prompt --wait` is two-phase (`src/api/wait.rs:177-306`):
- **Phase 1 (activity gate):** snapshot sequence before submit; after sending text, require *any* lifecycle change within `AGENT_PROMPT_EFFECT_TIMEOUT_MS = 5_000` ms, else error `agent_prompt_stalled` (`wait.rs:20, 620-631`). This catches an agent that is not actually receiving input.
- **Phase 2:** wait until target state (`until`), replaying events from the pre-submit marker (`wait.rs:279-300`).
`agent.wait --until blocked` (`wait.rs:348-498`) is the canonical "this agent needs a human" signal — server-owned, event-driven, no polling.

**Persistence matrix** (strongest → weakest): detach (server alive) > live handoff (PTY FDs transferred between old/new server processes via Unix socket + `SCM_RIGHTS`, token-validated, `src/server/handoff.rs:41-174`) > native agent session restore (`claude --resume`) > snapshot restore > pane screen history. Remote: SSH attach bridges local render to remote server, socket permission 0600 (`src/remote/attach.rs:1-34`).

**Plugins.** Manifest `herdr-plugin.toml` (`build/startup/actions/events/panes/link_handlers`). Marketplace indexes the GitHub `herdr-plugin` topic **with no review** (refresh 30 min). Explicit security stance: *"no sandbox — Herdr validates the manifest but does not review or sandbox plugin code"* (`docs/plugins.mdx:36-52`). Plugins run with user permissions and full socket access.

**Verdict for the thesis.** Herdr is a **pure substrate** — no supervisor/orchestrator at the daemon level. No `Method` or event models one agent being authorized to control another. `agent.start` refuses a busy pane as a *safety* boundary, not an *authority* one (`src/app/agents.rs:183-185`). But it ships **every primitive** an on-demand supervisor needs:

| Primitive | Method |
|---|---|
| Observability | `agent.list/get/read` (pane snapshot), `agent.explain` (why this state) |
| Event-driven blocking detection | `events.subscribe/wait`, `agent.wait --until blocked` |
| Intervention | `agent.prompt`, `agent.send_keys`, `agent.view.set`, `agent.focus` |
| Agent self-report | `pane.report_agent/report_agent_session` |
| On-demand lifecycle | plugin `startup` commands run at server start; plugin `events/actions` react to state changes |

**Gaps to become a supervisor:** no RBAC between agents (any socket client has full power over every pane), no quota/rate-limit on `agent.prompt`/`send_keys`, no daemon-level audit log of "who prompted whom". Supervision therefore lives in an *outer* layer calling the socket API — exactly the SLP claim that the orchestrator does not ship.

---

## 3. Group B — Counterexamples: the orchestrator→subagent pattern the thesis argues against

### 3.1 maestro-orchestrate (josstei) — the strongest counterexample

**Architecture.** `src/`-first, generator-driven: content written once in `src/`, `scripts/generate.js` emits 4 runtime surfaces (Gemini root, Claude `claude/`, Codex `plugins/maestro/`, Qwen `qwen/`). The orchestrator wears a **TechLead persona**: *"does not implement code directly. It designs, plans, delegates to specialized agents, validates results, and reports outcomes"* (`src/references/architecture.md:12`).

**39 specialists**, capability-tiered:
- `read_only` (7): architect, api-designer, code-reviewer, cloud-architect, solutions-architect, content-strategist, compliance-reviewer
- `read_shell` (9): debugger, performance-engineer, security-engineer, seo-specialist, accessibility-specialist, database-administrator, site-reliability-engineer, db2-dba, zos-sysprog
- `read_write` (6): technical-writer, product-manager, ux-designer, copywriter, release-manager, prompt-engineer
- `full` (17): coder, data-engineer, devops-engineer, tester, refactor, design-system-engineer, i18n-specialist, analytics-engineer, ml-engineer, mlops-engineer, mobile-engineer, cobol-engineer, observability-engineer, platform-engineer, integration-engineer, hlasm-assembler-specialist, ibm-i-specialist

Notable: a rare **legacy-mainframe cluster** (cobol, db2, ibm-i, hlasm, zos-sysprog). Each agent declares `Downstream Consumers` — who consumes its output (e.g. `src/agents/coder.md:106-109`: coder feeds `tester` and `code-reviewer`).

**Workflow — 41 steps, two modes, hard gates.** `docs/flow.md:7-64`, `src/references/orchestration-steps.md`. Express (simple: 1 phase, 1 agent) vs Standard (Design/Plan/Execute/Complete). Representative hard gates:
- **Pre-load skills before Plan Mode** — Gemini CLI deregisters MCP tools in Plan Mode, so later fetches fail "tool not found" (`orchestration-steps.md:11-21`).
- **Design gate** blocks `create_session` until approval; each section approved individually.
- **`validate_plan` gate BEFORE showing the user** — enforces phase count, dependency cycles, unknown agents, file-ownership conflicts, read-only agents not assigned file-creating phases (`orchestration-steps.md:77-85`).
- **Blockers gate:** non-empty `## Blockers` in a task report → no `transition_phase`; ask user / re-delegate (`orchestration-steps.md:113`).
- **Completion gate:** Critical/Major findings block completion; the orchestrator is **forbidden from writing code itself** (`orchestration-steps.md:129-132`).

**Native subagent delegation** (the crux):
- Claude: `Agent(subagent_type: "maestro:{{agent}}")` (`src/platforms/claude/runtime-config.js:42`)
- Codex: `spawn_agent` (`:39`), deferred result (`:42`), `child_cannot_prompt_user: true` (`:43`), fork-full-context incompatible with `agent_type/model/reasoning_effort` (`:41`)
- Gemini/Qwen: `{{agent}}(query: "...")`
- Hooks inject context "Active session: current_phase=3, status=in_progress" into the worker (`src/hooks/logic/before-agent-logic.js:23-55`); `after-agent-logic.js:18-52` denies responses missing `## Task Report` + `## Downstream Context`.
- **Hard-gate dispatch:** must call the agent's *registered* tool (with its methodology, tool restrictions, temperature, turn limits) — never a generic tool or a bare agent name (`orchestration-steps.md:107-112`).

**Session state:** `MAESTRO_STATE_DIR` (default `docs/maestro/`), YAML frontmatter + log, atomic temp-file+rename writes, dir mode 0o700, symlink/path-traversal resistant (`src/state/session-state.js:9-29`). Session-id invariance is a hard gate (slug pinned at step 9a, drift orphans the design gate).

**Verdict for the thesis.**
- ✅ It already applies "Lead owns outcome, not typing" (TechLead never implements; orchestrator forbidden from writing code).
- ✅ Hard gates approximate "verdict protocol" (validate_plan, blockers, review findings block completion).
- ❌ **No Supervisor above the Lead** — a single orchestrator layer makes every delegation/review/blocker decision itself.
- ❌ **Workers are native subagents** — ephemeral, no durable identity/state; grep for "you are a subagent" returns 0 matches, but workers are told "You run autonomously without user input" and "Do NOT call a user-prompt tool yourself" (`agent-base-protocol.md:25, 143`) — they know they cannot ask the user, without being told outright they are subordinate.

### 3.2 pi-subagents (edxeth) — harness-native subagent machinery, self-documented thesis

**Architecture.** A Pi harness extension (fork of `pi-interactive-subagents`) turning Pi into a subagent framework: children are separate `pi` processes (background workers or interactive panes in herdr/cmux/tmux/zellij/wezterm), each with its own session JSONL, env contract, and result delivery. Entry `src/index.ts:3-6` → `subagentsExtension(pi)` wiring (`src/subagents.ts`).

**Orchestrator mode (`PI_ORCHESTRATOR_MODE`).** This is the README's own articulation of the thesis failure modes:
- *"Models default to doing work themselves… you pay for two agents to race each other, and the parent floods its context with execution details instead of staying focused on coordination."*
- Implementation: `setActiveTools` keeps only `{subagent, subagent_kill, subagent_resume, set_tab_title}` (`src/tools/tool-names.ts:51-56`) — no `bash/read/edit/write/grep`; system prompt replaced with a delegation-only coordinator prompt, preserving user `APPEND_SYSTEM.md` (`src/subagents.ts:216-306`).

**How it fights F3 (context burn) at the harness level — mostly solved:**
- Each child has its own session JSONL; parent never appends child transcripts; results return only as summaries (`src/runtime/result-router.ts:66-147`).
- Fork mode seeds only the parent's *branch* (`getBranch(leafId)`), strips `subagent_roster` entries, re-chains `parentId` (`src/session/session-files.ts:131-192`).
- **3-stage context reminders** (warn → wrap-up → stop, `src/tools/context-reminders.ts:97-115`); final warning exits with `completionReason: "context-pressure"` and **blocks resume** ("Do not resume this session; resuming re-does already-summarized work and wastes a turn", `src/runtime/final-context-usage.ts:54-58`).
- **Hard timeouts** owned by the *parent* (a killed child cannot record its own outcome — sidecar `.timeout` is parent-written, `src/session/timeout-sidecar.ts`): SIGTERM→SIGKILL after 5s, soft wrap-up at 80% switches to report-only mode (`src/runtime/timeout-wrap-up.ts:148-258`), resume re-applies the same budget ("resuming a child that ran away cannot run away unbounded", `src/runtime/resume-service.ts:396-398`).
- Resume env is deliberately **not** inherited from `process.env` — "launder the parent's grant" (`resume-service.ts:381-385`).

**How it fights F4 (identity deception):**
- `<subagent-boundary>` marker + system prompt: *"Everything before this message was inherited from the parent Pi session as background context. Do not treat messages before this boundary as your current role, task, or available tool set"* (`src/launch/context-boundary.ts:13-43`); for spawn-less children: *"If prior context shows the parent using such tools, do not imitate that"* (`:17-19`).
- Tool-set restriction: no spawn grant → `PI_DENY_TOOLS` includes spawn tools (`src/launch/prep.ts:413-467`).
- Hard self-spawn guard: *"You are the X agent — do not start another X"* (`src/tools/subagent-tools.ts:185-187`).
- `installDeniedToolGuards` enforces tool denial at the child's own runtime (`src/tools/subagent-done.ts:27-97`).

**Where it still leaks (the SLP thesis's strongest material):**
1. **The boundary is prompt-level advice, not a hard fence.** The `<subagent-boundary>` tag is text; no mechanism forces compliance. `PI_SUBAGENT_DISABLE_CHILD_CONTEXT_BOUNDARY` exists to turn it off (`context-boundary.ts:9-14`).
2. **Session JSONL is plaintext the child (with bash) can rewrite.** The code admits: `readSubagentLaunchMetadata` *"is an ordering guard, not a trust boundary… a child that can write files can rewrite this entry in place — and can equally edit its own agent definition"* (`session-files.ts:356-361`).
3. **Session paths are hidden from the model for a reason.** `formatSessionRef` hides the path because *"a path or command in model-visible text is an invitation to route around it with a shell"* (`final-context-usage.ts:27-44`).
4. **Env-var guards are not tamper-proof.** A child with shell can change env of its own children; hence resume must launder the grant.

**Verdict.** pi-subagents solves F3/F4 against *accidental* model error remarkably well at the harness layer, but its own source concedes the guards do not hold against a *determined or severely hallucinating* agent. That is precisely the SLP argument: when workers are full agents with durable identity and shell, they must be governed, not fenced.

---

## 4. Group C — closest to SLP

### 4.1 senior-fable (AndyShaman) — "the expensive model decides, the cheap models type"

**Shape.** Tiny Claude Code plugin, **9 files, zero JS/TS** — all orchestration logic lives in `skills/senior-fable/SKILL.md`; agents are markdown with frontmatter. No hooks, no runtime.

**Roster** (lead is the session model; roles are native subagents with model overrides):
| Role | Model | Work |
|---|---|---|
| lead | session model | decomposition, architecture, contested decisions, synthesis |
| implementer | opus | code where decisions live inside the task |
| worker | sonnet | tests-to-spec, boilerplate, renames |
| investigator | opus, read-only, maxTurns 50 | long digs returning a conclusion |
| reviewer | opus, read-only, maxTurns 30 | independent review of finished work |

**Writer-verifier split enforced by permission, not prose:**
- `agents/reviewer.md:4-5` → `disallowedTools: Write, Edit, NotebookEdit`; same for `deep-reasoner.md:5-6`.
- `SKILL.md:58`: *"Review crosses a role boundary… routine re-checking of your own work is not review — the model already verifies itself."*
- Reviewer is given an explicit scope (diff/commit range) because a fresh context in a dirty worktree cannot infer the change; reviewer returns *everything* it finds and the lead filters (`SKILL.md:60-62`); "Do not modify anything… describe it in a sentence and leave it to the implementer" (`reviewer.md:16`).
- Optional **cross-family reviewer** (e.g. a Codex CLI) for a genuinely independent model opinion.

**Roster override.** `## Senior Fable roster` block in CLAUDE.md outranks defaults; one `role: model` line per role, plus an `effort:` knob (`SKILL.md:28-35`). Two model-resolution traps documented: agents without `model:` inherit the session model (not cheap), and `CLAUDE_CODE_SUBAGENT_MODEL` collapses the whole roster onto one model (`SKILL.md:30-33`).

**Delegation restraint.** Do not delegate what fits in a handful of tool calls; no fleets where one agent suffices; **never delegate to double-check yourself**; *"the cheapest delegation is the work that isn't needed"* (`SKILL.md:37-41`). On the cheapest model the skill self-negates ("nothing below it to route to").

**Verdict.** The closest philosophical match to "Lead owns outcome, not typing" — but workers remain **native subagents**: ephemeral, same worktree, one-way communication ("final message is the only thing that comes back", `deep-reasoner.md:12`), and orchestration state does not survive compaction (the lead must re-invoke the skill, `SKILL.md:68-70`).

### 4.2 small-opencode-orchestrator (tempont) — delegation as a permission boundary

- Orchestrator: `edit: deny`, `bash: deny`, `glob: deny`, `grep: deny`, `list: deny`, `read: deny` — **only** `question`, `todowrite`, and a `task` allow-list of 10 subagents (`agents/orchestrator.md:3-41`).
- *"Delegation is a permission boundary, not just a workflow preference"* (`AGENTS.md:42`). Needs a fact → Task `code-explorer`; simple edit → hand to `build` rather than inspect itself.
- `plan-runner` writes a plan file (self-contained, survives compaction) and is **forbidden from invoking PlanApprove** — only the orchestrator calls `question` (`agents/plan-runner.md:5, 65`).
- Approval flow: `plan-runner` → orchestrator `question` with header literal `PlanApprove` → revise loop → slices via `code-executor` (serialized unless provably independent) → `test-verifier` → cumulative diff → `code-reviewer` + `docs-reviewer` (+ optional `security-reviewer`).
- A TS plugin `plan-post-approval.ts` catches the `PlanApprove` question and automates the handoff text, skipping duplicate bursts (`plugin-src/plan-post-approval.ts:322-360`).
- **Token-consciousness:** *minimal child prompt* — Goal (1-2 sentences) + Scope + return shape; *"Do not paste the entire parent conversation into the subagent unless necessary"* (`skills/agent-delegation/SKILL.md:34-41`); no heavy parallel chaining; no `code-reviewer` before a stable diff; no `test-verifier` without repo root/expected commands/acceptance.
- Model tiering: orchestrator/build/plan on `deepseek-v4-pro`, every subagent on `v4-flash` (`opencode.jsonc:38-103`).

### 4.3 opencode-solo (Dqz00116) — closed loop, with benchmark data

**Shape.** 7 markdown agents + 1 config example; no plugin, no MCP server. "Solo" is the primary orchestrator.

**Closed-loop feedback (the distinguishing design).** Solo denies `read/write/edit/apply_patch/glob/grep` but allows `bash` with a long deny-list of file-reading commands (`cat/head/tail/sed/awk/rg/grep...`). It therefore cannot read files, but it **runs the target tests itself via bash and reads raw output**:
- *"Never declare success based on `@editor`'s text report. Success is defined ONLY by raw test output"* (`agent/solo.md:88`).
- Loop is capped at 5 rounds (tracked via `todowrite`); editor edits; Solo runs tests; all pass + no regression → exit immediately; never re-explore after a pass (`solo.md:71-96`).
- Conditional `@verify` only when the change is large/risky.

**Context isolation.** Heavy file reads/tool outputs stay in subagent sessions; Solo's context holds only summaries and decisions (~5-10K tokens) versus 100K+ for a monolith (`README.md:44-47`). 52% of total tokens run on the cheap `v4-flash` tier (explore 31.7M + editor 1.5M vs solo 29.2M on `v4-pro`).

**Adversarial verification.** `verify.md` is a deliberate skeptic: *"Your job is not to confirm… it is to try to break it"* (`verify.md:19`); *"Reading code is not verification — every check must run a real command and observe real output"* (`:19`); every check requires `**Command:**` + `**Output:**` or it is rejected (`:133`); ≥1 adversarial probe (concurrency, boundaries, idempotency) before PASS; test suite results are "context, not evidence" (`:59`); counters two failure modes — verification avoidance and "seduced by the first 80%" (`:23-26`).

**Benchmark — SWE-bench Verified (50 instances, DeepSeek v4-pro/flash):**
| Metric | Solo | Build agent |
|---|---|---|
| Resolution | 35/50 (70%) | 34/50 (68%) |
| Total prompt tokens | 63.9M | 63.2M |
| Total output tokens | 653K | 432K |
| Cache hit rate | 95.4% | 97.1% |

**Honest caveat:** *"SWE-bench is not where Solo's multi-agent architecture shines"* — on short single-bug-fix tasks it "matches the monolith" on resolution and tokens; the real advantage is long-horizon, multi-file tasks (`README.md:89-92`). This is an important honesty point for the thesis: subagent-vs-full-agent differences may not show up on short-horizon benchmarks.

---

## 5. Group D — skill ecosystems and worktree isolation

### 5.1 agent-skills (simota) — 123 SKILL.md, Nexus hub-spoke, 4-tier governance

**Shape.** A pure prompt-engineering corpus: 123 agents, each a directory with `SKILL.md` + `reference/` (~13 reference files each; 1,621 `.md` references total). Frontmatter is strictly the Anthropic Agent Skills allowlist `{name, description, model, tools}` (`lint-frontmatter.py:59`); capability declarations live in HTML comment blocks (`SKILL_TEMPLATE.md:6-19`). CI lints structure on every PR.

**Nexus = hub-spoke orchestrator:**
- *"Keep hub-spoke routing. All delegation and aggregation flows through Nexus; no direct agent-to-agent handoffs"* (`nexus/SKILL.md:57`).
- "Minimum viable chain" — with quantitative backing: *"17.2× uncoordinated, 4.4× centrally orchestrated"* (`:56`).
- Routing matrix = **93 task types** → default chains (`nexus/reference/routing-matrix.md`); a `LADDER` path spawns `compass` → `architect` to propose a new agent when nothing matches (`routing-matrix.md:21`).
- 5 execution modes (AUTORUN_FULL default, AUTORUN, GUIDED, INTERACTIVE, HANDOFF); phase contract `PLAN → PREPARE → CHAIN_SELECT → EXECUTE → AGGREGATE → VERIFY → DELIVER`.

**Key divergence from SLP:** roles are **separate files**, not *dispositions* of one profile. Boundaries enforced three ways: (1) per-SKILL Trigger Guidance + negative triggers; (2) `_common/BOUNDARIES.md` central role-ownership table; (3) the Nexus routing matrix. The repo has no concept of "disposition"/"one profile" anywhere. Overlap between sibling roles is treated as "ecosystem debt" (`architect/SKILL.md:81`), with a ≥50% overlap reject threshold.

**Lore = institutional memory.** "Cross-agent knowledge curator and institutional memory guardian" (`lore/SKILL.md:40`): sources = all agent journals + postmortems; output = `METAPATTERNS.md` (4-axis taxonomy, pattern IDs); confidence 1=Anecdote→11+=Foundational; **knowledge decay** (freshness score, STALE >180 days, per-domain half-lives); organizational forgetting/unlearning. Conceptually the closest thing to the SLP "Supervisor memory notebook", but far more systematized.

**Titan = meta-orchestrator "above the hub":** *"Titan operates above the hub. It issues chains to Nexus and does not bypass the hub"* (`titan/SKILL.md:289`). 9 phases (`DISCOVER→DEFINE→ARCHITECT→BUILD→HARDEN→VALIDATE→LAUNCH→GROW→EVOLVE`), scaled by scope (S/M/L/XL), Agent Justification Gate before deploy, Anti-Stall ladder L1-L5 with hard budgets.

**Four governance tiers (not flat):**
1. **Hierarchy:** Titan above Nexus; Sherpa decomposes, Rally parallelizes.
2. **Quality:** Judge evaluates every agent's output with the "generator ≠ evaluator" rule (`judge/SKILL.md:15`).
3. **Evolution:** Darwin audits ecosystem fitness of *everything including Nexus* — its detected anti-patterns include "passive supervisor" and "micromanaging supervisor" (`darwin/SKILL.md:83`); Gauge audits SKILL.md format (19-item checklist, P0-P3, Health Score, drift detection, Safety Levels A-D); Architect designs new agents; Lore curates; Chain handles supply-chain (manifest sha256, Unicode-tag scan, MCP pinning).
4. **Safety:** Nexus L1-L4 guardrails + circuit breaker + checkpoint-resume; Titan Anti-Stall; human approval gates everywhere (Nexus L4, Architect self-modify Level C, Darwin sunset, Titan cumulative risk ≥100).

**Distinction from SLP:** agent-skills disciplines *by prompt contract + CI lint*, not by a single supervisor agent. Governance is **distributed** across Darwin/Gauge/Judge/Lore/Architect/Chain. This is both a strength (quantified fitness scores, explicit safety levels) and a cost: 123 SKILL.md ≈ 35,600 lines + 1,621 references is enormous context, and the repo itself concedes multi-agent runs at "4.4× is a floor, not zero".

### 5.2 treehouse (kunchenguid) — worktree pool: filesystem peer isolation

**Shape.** A single Go binary (cobra), **no daemon**; git ops shell out to the `git` binary (go-git worktree support judged incomplete, `AGENTS.md:58`).

**Pool model.** Per-repository pool under `~/.treehouse/<repo>-<shortHash>/`; each worktree at `<pool>/<number>/<repoName>/`. State is `treehouse-state.json`, written **atomically** (temp file + fsync + rename; `ReplaceFileW`/`MoveFileEx` write-through on Windows). All lifecycle ops run under a file lock (`flock`/`LockFileEx`).

**Cache preservation — the core value prop.** On `return`, the worktree is **not deleted** — only reset: `git checkout --detach --force <ref> && git reset --hard <ref> && git clean -fd` (`git.go:164-178`). The key is **`clean -fd` without `-x`**: ignored files (`node_modules/`, `target/`, `.venv/`, build cache, vendored deps) **survive every reuse cycle** → *"dependencies and build cache intact, ready for the next agent"* (`README.md:21-24`). Detached HEAD avoids branch-name collisions. `post_create` hooks re-prepare environment after acquire (user-level only — repo-level hooks deliberately ignored for safety).

**Conflict detection — three layers:**
1. **Process scan:** gopsutil lists every process and compares cwd (symlink-resolved) to the worktree path → catches "someone is inside" including agent servers that ignore SIGHUP (`internal/process/detect.go:44-85`).
2. **Owner reservation:** short-lived PID + start time, auto-healed when the process dies.
3. **Durable lease:** `LeaseID` (128-bit), *not* derived from process state; survives no-process situations; never pruned by `prune`/`destroy --all`; released only by the matching holder via `return --if-lease-id` (`state.go:20-42`, `AGENTS.md:40-41`).

**Process cleanup on exit.** `killLingeringProcesses` terminates every process with cwd in the worktree, excluding treehouse itself and its ancestors (`internal/process/terminate.go:19-66`); Unix SIGTERM → 2s grace → SIGKILL; Windows `TerminateProcess` (no SIGTERM equivalent). This clears detached tools (e.g. opencode servers) that would otherwise hold the worktree forever.

**Windows support.** Build-tagged files per platform (lock, state commit, terminate, hook command, updater); CI matrix ubuntu/macos/windows; `Makefile` builds 6 OS/arch variants.

**Verdict for SLP.** Treehouse supplies the **filesystem isolation layer** that native-subagent systems lack — the exact gap senior-fable and maestro-orchestrate hit when parallel workers share one worktree. `treehouse get --lease` is a clean, machine-readable way for an SLP Lead to give each Peer an isolated, cache-warm working directory. It explicitly scopes itself as substrate, not policy: *"treehouse owns the worktree lifecycle rather than agent orchestration"* (`VISION.md:3`); *"not a security sandbox"* (`VISION.md:13`).

---

## 6. Group E — adjacent: safety gates, crew distros, work trackers

### 6.1 slb (Dicklesworthstone) — execution-safety gate
A "two-person rule" CLI: dangerous commands (`rm -rf /`, `git push --force`, `terraform destroy`, `DROP TABLE`) require peer review + approval before execution. Risk tiers SAFE → CAUTION (auto 30s) → DANGEROUS (1 approval) → CRITICAL (2 approvals). Key mechanics: shell-aware tokenization with compound-command splitting (highest-risk segment wins), SHA-256 command-hash binding, SQLite state in `.slb/state.db`, five execution gates (status, TTL, hash, tier consistency, first-executor-wins), fail-closed behavior (parse error upgrades tier; daemon down blocks). **It is a safety gate, not supervision** — it guards *commands*, never governs *agents*. Confirms the README's "execution-safety gates exist" without overlapping the supervisor gap.

### 6.2 firstmate (kunchenguid) — crew distro that supervises by watching
A bash distro turning a terminal agent into a fleet supervisor ("first mate"); you talk to it, it spawns crewmates in tmux/herdr/zellij windows + git worktrees, and returns PRs/merges/reports. **Direct evidence for the thesis:** its `docs/subagent-guard.md` documents a 2026-07-22 incident where a primary delegated four workers through Claude Code's *built-in subagent tool* instead of its own spawn path — with exactly the thesis's failure modes: (1) fleet blindness (no `state/<id>.meta` written, view showed zero work), (2) loss on restart (two workers died mid-flight), (3) silent supervision collapse (watch cycle down 73 min). The shipped fix was `bin/fm-subagent-pretool-check.sh`, a PreToolUse guard denying delegation-shaped tool names. Supervision here is **by watching** (bash watcher + turn-end guard), not by governing — again confirming the README's taxonomy.

### 6.3 maestro (ReinaMacCredy) — gated card store
A single Rust binary giving an agent a durable, repo-local workspace under `.maestro/` (no daemon, no cloud). Feature cards own contracts (spec.md/qa.md facets); work cards dock via `parent`; proof + QA gates keep "done" evidence-backed (`task complete --claim --proof` → `verify`; `feature close` refuses until every `[bl-NNN]` baseline scenario has a covering QA slice). Rule-based (no-LLM) `harness` detectors spot recurring friction → `idea` card → `harness apply` spawns a fix task → `harness measure`. **It is a work tracker with instruction-patching via installed skills** — the README's "work trackers exist" box, and one that already versions agent instructions.

---

## 7. Cross-cutting: what the 12 repos confirm about the SLP thesis

### 7.1 Confirmed gaps (no repo ships these)

| SLP piece | Status across the corpus | Strongest near-miss |
|---|---|---|
| **On-demand Supervisor** (read-only governance agent, memory notebook, instruction-patching, *above* a Lead it can replace) | ❌ **Nobody.** herdr has the primitives but no supervisor; agent-skills governance is distributed across 6 prompt-contract agents; maestro-orchestrate is one flat orchestrator layer | herdr plugin `startup` + socket API (substrate only) |
| **Peers that believe they work for a human** (identity as a control variable) | ❌ **Nobody.** pi-subagents ships `<subagent-boundary>` but as prompt advice the author himself treats as non-binding; opencode-solo's solo is an orchestrator that *knows* it delegates | pi-subagents (closest, still leaks) |
| **Experience-handoff artifact** (Lead lessons surviving retirement, format-standard) | ❌ **Nobody.** pi-subagents has context-pressure + resume-blocking and timeouts, but no "lesson transfer"; none of the others address Lead degradation | pi-subagents (context-pressure exit → block resume) |
| **Lead that never presolves + verdict protocol** | 🟡 **Partially.** maestro-orchestrate TechLead never writes code; opencode-solo's solo never trusts implementer self-reports; small-orchestrator is a pure delegator. Verdict *council* (multi-agent falsification, "provider count creates no authority") does not exist anywhere | opencode-solo adversarial verify, maestro-orchestrate gates |

### 7.2 Confirmed failure modes (production evidence)

| Failure mode | Evidence found |
|---|---|
| F1 livestock | firstmate subagent-guard incident (subagent calls invisible to orchestration, no metadata); pi-subagents "Models default to doing work themselves"; maestro-orchestrate must hard-gate dispatch to prevent generic-agent use |
| F3 context burn | pi-subagents self-documentation + its whole context-reminder/timeout machinery; opencode-solo built around keeping orchestrator context at ~5-10K; maestro-orchestrate per-runtime `child_cannot_prompt_user` + deferred results; firstmate "polls waste context" note |
| F4 identity deception | pi-subagents boundary marker + `formatSessionRef` path-hiding + self-spawn guard; opencode-solo "never on `@editor`'s self-report"; senior-fable "routine re-checking is not review" |

### 7.3 The strongest single rebuttal and how the thesis must answer it

**maestro-orchestrate** (and, to a lesser degree, pi-subagents) shows an orchestrator → native-subagent system can be *engineered* to avoid most of F1/F3/F4 in practice — via hard gates, worker isolation, and never letting the orchestrator write. A skeptic can argue the failure modes are *implementation failures*, not inherent to subagents.

**The SLP answer, supported by this corpus:**
1. pi-subagents' own source concedes the fences are **prompt/trust-level, not hard** — they stop accidental model error, not a determined or severely-hallucinating agent with shell access to its own session files.
2. No native-subagent system gives workers **durable identity/state that survives supervisor restart** — the exact failure the firstmate incident recorded (work lost on restart because subagent calls wrote no metadata).
3. None of them ships the **supervision-above-lead** authority model (instruction patching, Lead replacement) — the thing that keeps a degraded Lead from compounding errors.
4. opencode-solo's honest SWE-bench note cuts both ways: short-horizon benchmarks will not show the difference; the SLP advantage is long-horizon, multi-file, multi-worker work.

---

## 8. Recommended next steps for the repo

1. **Version the SLP role instructions** — the corpus shows the missing primitive is the Supervisor; the closest analog (agent-skills Lore + Darwin, herdr plugin startup) should be studied as reference designs, then turned into checked-in Supervisor/Lead/Peer instructions.
2. **Design the experience-handoff artifact** — study pi-subagents' context-pressure exit + resume-blocking and firstmate's session reconciliation; a format that transfers *lessons* (not just state) is the open question.
3. **Spec the control-plane events** — herdr's `events.subscribe/wait` and context reminders are the substrate; subscribe to a Lead's context %, alarm on review-count-per-task > 3.
4. **Build a reference implementation on substrate** — herdr (runtime primitives) + treehouse (peer isolation) + slb (execution gates) is a complete substrate stack; no native subagents, full agents per peer.
5. **Run the controlled experiment** — same task, native-subagent tree vs SLP, measured on correctness and context cost, long-horizon tasks only (per opencode-solo's caveat).

---

## 9. Source anchor index

| Claim | Anchor |
|---|---|
| Herdr: substrate, no supervisor | `herdr/src/api/schema.rs:40-243`; `herdr/docs/agents.mdx:41-49` |
| Herdr: blocked only on real UI match | `herdr/src/detect/manifests/claude.toml:96-111` |
| Herdr: prompt two-phase activity gate | `herdr/src/api/wait.rs:177-306` |
| Herdr: no RBAC / no audit | `herdr/src/api/mod.rs` (no scope/principal) |
| maestro: TechLead never implements | `maestro-orchestrate/src/references/architecture.md:12` |
| maestro: native subagent dispatch | `maestro-orchestrate/src/platforms/claude/runtime-config.js:42`; `codex/runtime-config.js:39-43` |
| maestro: validate_plan / blockers / no-code gates | `maestro-orchestrate/src/references/orchestration-steps.md:77-132` |
| pi-subagents: orchestrator mode cuts tools | `pi-subagents/src/subagents.ts:167-306`; `src/tools/tool-names.ts:51-56` |
| pi-subagents: boundary is prompt-level, trust leak admitted | `pi-subagents/src/launch/context-boundary.ts:9-14`; `src/session/session-files.ts:356-361` |
| pi-subagents: context-pressure blocks resume | `pi-subagents/src/runtime/completion-reason.ts:13-43`; `src/tools/resume-tool.ts:124-129` |
| senior-fable: writer-verifier by disallowedTools | `senior-fable/agents/reviewer.md:4-5`; `skills/senior-fable/SKILL.md:58` |
| opencode-solo: success only by raw test output | `opencode-solo/agent/solo.md:88` |
| opencode-solo: adversarial verify, evidence required | `opencode-solo/agent/verify.md:19,133` |
| opencode-solo: SWE-bench caveat | `opencode-solo/README.md:89-92` |
| small-orchestrator: delegation is permission boundary | `small-opencode-orchestrator/AGENTS.md:42` |
| agent-skills: hub-spoke, no direct agent-agent | `agent-skills/nexus/SKILL.md:57` |
| agent-skills: distributed governance (Darwin supervises Nexus) | `agent-skills/darwin/SKILL.md:83` |
| treehouse: clean -fd keeps deps alive | `treehouse/internal/git.go:164-178` (via `git.go`) |
| treehouse: durable lease never pruned | `treehouse/internal/pool/destroy.go:232-257`; `AGENTS.md:40-41` |
| firstmate: subagent-guard incident + fix | `firstmate/docs/subagent-guard.md` |
| slb: risk tiers + hash binding + gates | `slb/README.md` (tiers, §Pattern Matching, §Execution Verification) |
| maestro: proof/QA gates + harness detectors | `maestro/README.md` (§Task/QA, §Harness self-improvement) |
