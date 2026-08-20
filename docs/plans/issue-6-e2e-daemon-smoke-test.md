# Issue #6 — End-to-end daemon smoke test (verify a real Peer actually spawns)

**Status of code today (read before implementing):**

- The real process spawn lives in `crates/anti-daemon/src/main.rs`:
  - `~line 1154`: `let mut cmd = std::process::Command::new("claude");` (binary path from `Config::claude_bin`).
  - `~line 760`: `cmd.stdout(std::process::Stdio::piped());` so the event bridge can read it.
- `crates/anti-daemon/src/event_bridge.rs` `EventBridge::spawn(child, agent_id)` reads the child's stdout NDJSON → `AgentEvent` → channel → reaper persists to `Store`/`events.jsonl`.
- `crates/anti-daemon/src/peer_manager.rs` `PeerManager` tracks/terminates/kills/reaps `Child` handles — but its tests are **mocked** (no real process is ever spawned in tests).
- `crates/anti-cli/src/commands.rs` `spawn(...)` → `Request::SpawnAgent` → daemon. `doctor` checks for `claude`/`treehouse` binaries.
- `crates/anti-core/src/config.rs` has `claude_bin: PathBuf` — i.e. the spawn binary is overridable (key for testing with a stub).
- No integration test currently boots the daemon and asserts a real OS process comes up.

**Goal:** A repeatable E2E test that boots the daemon in a temp state dir, spawns a peer, asserts a real agent process is tracked and emits events, then stops it and confirms the process is reaped and the worktree is released.

**Files to touch / add:**

1. `tests/integration_e2e.rs` (new, at workspace root or `crates/anti-daemon/tests/`)
   - Use `std::env::temp_dir()` + uuid for `ANTI_STATE_DIR`.
   - Build a **stub agent binary** once (a tiny script, e.g. `#!/bin/sh; echo '{"type":"turn_completed","usage":{...}}'; sleep 2`) and point `Config::claude_bin` at it via `ANTI_CLAUDE_BIN` env (the daemon must honor this env — confirm in `config.rs`; if not, add the env read).
   - Start the daemon as a child process (`anti-daemon` binary) with `ANTI_STATE_DIR` set; wait for the socket (`ipc::socket_path`) to appear (mirror `daemon_running` in `commands.rs`).
   - Send `Request::SpawnAgent { role: "peer", disposition: "engineer", harness: "claude", repo, task, .. }` via `ipc::send_request`.
   - **Assert** a real process is tracked: poll `Request::ListAgents` / `Request::GetAgent` until `pid` is set and `status` reaches `Running`; cross-check `PeerManager::is_alive` via a debug IPC call or by scanning `events.jsonl` for `AgentStarted`.
   - **Assert** events flow: `events/events.jsonl` contains a `TurnCompleted` (or stub-equivalent) line for the agent id.
   - Send `Request::StopAgent { force: true }`; assert the process is reaped (`pid` cleared / status terminal) and the treehouse worktree is removed (`treehouse destroy` or the worktree dir is gone).
   - Kill the daemon (shutdown request) and assert socket removed.

2. `crates/anti-daemon/src/main.rs` / `config.rs`
   - Ensure `ANTI_CLAUDE_BIN` (and a treehouse override) are honored so the test does not require a real `claude` install. If `claude_bin` only reads from config file, add the env read.

3. CI
   - Add `cargo test --test integration_e2e` to the workflow; mark it `#[ignore]` by default if it needs a built `anti-daemon` binary, with a CI step that builds first.

**Tests:**

- The integration test above is the primary deliverable.
- Keep `peer_manager` unit tests; add one that uses a **real** short-lived stub child (e.g. `sleep 1`) to exercise `track`/`is_alive`/`reap` against a genuine process (not just the API surface).

**Acceptance criteria:**

- `cargo test --test integration_e2e` boots the daemon, spawns a peer, observes a live process + at least one event, stops it, and confirms reaping + worktree cleanup — all without a real `claude` binary.
- The "MVP verified: spawn autonomous Claude Code peers" claim is now backed by an automated test, not just unit tests.
- `cargo test` across the workspace is green.
