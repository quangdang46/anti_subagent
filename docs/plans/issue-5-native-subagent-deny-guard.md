# Issue #5 — Active DENY guard for native-subagent / delegation-shaped tool calls

**Status of code today (read before implementing):**

- A guard **already exists** as an external harness hook:
  - `guard/rules.toml` — `deny_stems` (agent, subagent, task, workflow, cron, schedul, worktree, delegate, spawn, dispatch, handoff, remote, sendmessage, monitor), `allow_exact` (Read/Grep/Glob/Edit/Write/...), `allow_prefixes = ["mcp__"]`, `fail_closed = true`, `scope_gate = true`.
  - `guard/anti-guard.sh` — the script the hook invokes (read it; it should classify the tool name against `rules.toml` and exit non-zero to deny).
  - `crates/anti-cli/src/commands.rs` has `guard_install` (writes `.claude/hooks.json` PreToolUse → `anti-guard.sh`), `guard_test` (local stem scan), `guard_status`.
  - `guard_arm_spills_orchestrator(arm)` encodes the a/b/c/d arm concept (native/flat/concealed/disclosed) controlling whether a peer may know an orchestrator exists.
- The **internal** path does NOT deny:
  - `crates/anti-core/src/subagent_tracker.rs` `SidechainTracker::handle_event` only *detects and tracks* Task/Agent/subAgent tool calls (emits `SubagentStarted`/`SubagentCompleted`); it never blocks them.
  - Spawning a peer does not currently auto-install the guard hook into the treehouse worktree.

**Goal:** Every peer spawned by anti_subagent is fail-closed against native-subagent / delegation-shaped tool calls — by default, without a manual `guard install` step — and a violation is surfaced to the control plane.

**Files to touch:**

1. `guard/anti-guard.sh` (verify / strengthen)
   - Confirm it (a) reads `guard/rules.toml`, (b) classifies the tool from `CLAUDE_TOOL_NAME`/`$1`, (c) denies on any `deny_stem` match unless in `allow_exact`/`allow_prefixes`, (d) respects `fail_closed` (deny if rules unreadable), (e) respects `scope_gate` (only active inside an anti-managed worktree — detect via `.anti_subagent` marker or env `ANTI_MANAGED=1`).
   - If any of these are missing, implement them.

2. `crates/anti-daemon/src/main.rs` (spawn handler, ~line 1154)
   - After creating the peer's treehouse worktree and **before** `Command::new("claude")`, run `guard_install` for that worktree (or copy `guard/rules.toml` + `anti-guard.sh` and write `.claude/hooks.json`). Set `ANTI_MANAGED=1` in the spawned process env so the guard's `scope_gate` engages.
   - This makes the guard automatic for every peer — no separate CLI step.

3. `crates/anti-core/src/subagent_tracker.rs`
   - When `SidechainTracker::handle_event` detects a delegation-shaped tool (Task/Agent/subAgent/...), in addition to tracking, emit a control-plane `GuardViolation { agent_id, tool_name }` event type (add to `events.rs` `AgentEvent`).
   - Optionally, route the violation to the daemon reaper so the peer can be auto-stopped or flagged `attention` (ties into `attention.rs` `AttentionState`). Start with "flag + log"; auto-kill is a follow-up behind a config flag.

4. `crates/anti-cli/src/commands.rs`
   - `guard_test` already does a local stem scan — extend it to also exercise `anti-guard.sh` end-to-end (invoke the script with a tool name, assert exit code) so the test reflects real enforcement.
   - `doctor` should report whether the guard hook is present in the active worktree.

**Tests:**

- `guard/anti-guard.sh` self-test: `./anti-guard.sh Task` → non-zero; `./anti-guard.sh Read` → 0; missing rules.toml → non-zero (fail-closed).
- `subagent_tracker` test: a `Task` tool call now also produces a `GuardViolation` event in the returned vec.
- Integration: spawn a peer, confirm `.claude/hooks.json` exists in its worktree and `ANTI_MANAGED=1` is set in its env (inspect via a process-list probe or a hook that records env).

**Acceptance criteria:**

- A freshly spawned peer is guarded with no manual step.
- Delegation-shaped tools are denied in peer sessions (fail-closed).
- Violations surface as a control-plane event and are visible in `anti list --attention`.
- `cargo test -p anti-core -p anti-cli` green; `guard/anti-guard.sh` self-tests pass.
