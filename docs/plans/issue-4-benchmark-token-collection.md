# Issue #4 — Benchmark harness: collect real token-cost metrics (SLP vs native subagent)

**Status of code today (read before implementing):**

- `crates/anti-bench/src/main.rs` is already a complete **4-arm** harness (plan §34):
  - Arm A: Native Subagent (harness Task tool)
  - Arm B: Flat Full-Agent (disclosed)
  - Arm C: SLP concealed
  - Arm D: SLP disclosed
  - Controlled vars: repo/commit/task/model/tools/token-budget/timeout.
  - Pre-registered pairwise exact sign test (`binomial_tail`) between (A,B),(B,D),(C,D).
  - Blinding: artifacts saved under `runs/<run-id>/` with arm tags STRIPPED.
- `RunMetrics` struct has `tokens_in: u64, tokens_out: u64` but they are **never populated** — only `events`, `crashes`, `restarts`, `reviews`, `rejections`, `escalations`, `revisions` are read from `events/events.jsonl`.
- `spawn_claude(...)` goes through the daemon socket (`Request::SpawnAgent`, wait until `completed`). The daemon emits `AgentEvent::TurnCompleted { usage }` (see `crates/anti-daemon/src/event_bridge.rs`) but the bench never reads `usage`.
- The harness uses a **fixed** model per arm (whatever the daemon default is) — it cannot yet vary model per arm from a config.

**Goal:** The benchmark produces defensible, anti's-own token-cost numbers (never agent self-reports) so the SLP thesis can be stated with evidence, and per-arm models can be configured.

**Files to touch:**

1. `crates/anti-bench/src/main.rs`
   - In `run_arm`, after reading `events/events.jsonl`, also capture token usage:
     - Match `TurnCompleted` events for the run's `agent_id` that carry `usage` (shape `{ input_tokens, output_tokens }` or `{ prompt_tokens, completion_tokens }` — confirm against `events.rs` `AgentEvent::TurnCompleted`).
     - Accumulate into `m.tokens_in += usage.input_tokens; m.tokens_out += usage.output_tokens;`
   - Add a per-arm model selector: read an optional `bench.models.toml` (or reuse `.anti_subagent/providers.toml`) mapping `arm → model`, and pass it through `Request::SpawnAgent { model: Some(...) }` (requires Issue #3's `--model` plumbing to be merged first, or add `model` to `spawn_claude`).
   - Emit a structured report: write `runs/<run-id>/metrics.json` (full `RunMetrics`) AND a final `summary.csv` with columns `arm,pass,n,tokens_in_m,tokens_out_k,wall_s,crashes,restarts,significance`.

2. `crates/anti-core/src/events.rs` (verify, do not invent)
   - Confirm `AgentEvent::TurnCompleted` actually carries `usage` and the field names. If the daemon currently drops usage when logging to `events.jsonl`, fix the logger so `usage` is persisted (this is the real data source for the metric).

3. `crates/anti-daemon/src/main.rs` (logger)
   - Ensure the event-log writer serializes `TurnCompleted.usage` verbatim to `events/events.jsonl` (do not strip it for "privacy" — usage is not hierarchy metadata).

**Tests:**

- Add `anti-bench` unit test on the token parser: given a fake `events.jsonl` with 3 `TurnCompleted` lines for an agent_id, assert `tokens_in/out` sum correctly.
- Add a small `--dry` / `--self-test` mode to `anti-bench` that runs one arm against a stub agent (set `ANTI_CLAUDE_BIN` to a script that emits fake `TurnCompleted` JSON) and asserts the metrics file is produced.

**Acceptance criteria:**

- `anti-bench --full` (or a single run) populates `tokens_in`/`tokens_out` from the daemon event log, not from agent output.
- A `summary.csv` + `metrics.json` are emitted under `runs/`.
- Each arm can run with a configured model (proves "SLP with cheap peers" cost claim).
- Sign-test logic unchanged and still green.
- `cargo test -p anti-bench` passes.
