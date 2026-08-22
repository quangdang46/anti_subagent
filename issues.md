# ISSUES — Found during verification pass (2026-08-22)

Verified against `main @ b8788a4`. Full verification context: build ✅, unit tests **222 passed** ✅,
integration T1–T6 **6/6 passed** ✅, real e2e spawn (`claude` peer, treehouse worktree) COMPLETED ✅.

| # | Severity | Title | Status |
|---|----------|-------|--------|
| B1 | 🔴 **P0 — Critical** | Guard hook installed to wrong file — Claude Code never reads it, deny-guard is inert | Open |
| B2 | 🟡 P1 | Fresh-state-dir bootstrap fails silently (`create_dir_all` missing + stderr nulled by daemonize) | Open |
| B3 | 🟡 P2 | Event bridge maps every unmatched provider event to `AgentStarted` → 31× duplicate per run | Open |

---

## B1 (P0) — Guard hook written to `.claude/hooks.json`, which Claude Code does not read

**Severity:** Critical — the repo's headline feature (fail-closed deny-guard on peer sessions,
Issue #5) silently does nothing in real sessions.

### Evidence

After spawning a real peer (`spawn --id e2e2 --harness claude`), the worktree contained:

```
<wt>/.claude/
├── anti-guard.sh   (0755, copied correctly)
└── hooks.json      ← daemon wrote the PreToolUse registration HERE
```

`hooks.json` content (settings-style format, no wrapper):

```json
{
  "PreToolUse": [
    {
      "hooks": [{ "command": "<abs path>/.claude/anti-guard.sh", "type": "command" }],
      "matcher": ".*"
    }
  ],
  "_anti_guard": { "fail_closed": true, "installed_by": "daemon-spawn" }
}
```

Per official Claude Code docs (<https://code.claude.com/docs/en/hooks>), project-scope hook
configuration is read **only** from:

| Location | Notes |
|---|---|
| `.claude/settings.json` | project scope — `"hooks"` key inside |
| `.claude/settings.local.json` | local override |
| `~/.claude/settings.json` | user scope |
| Managed policy settings | org-wide |
| Plugin `hooks/hooks.json` | plugin bundles only, requires `{"hooks": {...}}` wrapper |

A bare `<project>/.claude/hooks.json` is **not** in the resolution list. The script itself works
when invoked manually (verified: `{"tool":"Task"}` → denied, exit 2; `Bash` → allowed, exit 0),
but nothing ever invokes it during a session because the registration file is never loaded.

The script works; the *registration* doesn't exist as far as Claude Code is concerned.

### Root cause

- `crates/anti-daemon/src/main.rs` → `install_guard_into_worktree()` writes
  `claude_dir.join("hooks.json")`.
- Same assumption baked into `crates/anti-cli/src/commands.rs` → `guard_install`
  (manual CLI path has the identical bug).
- Origin: `docs/plans/issue-5-native-subagent-deny-guard.md` specifies
  "write `.claude/hooks.json`" — the plan itself encoded the wrong filename, and the
  implementation followed the plan faithfully.

### Impact

- Peers are **not** fail-closed: delegation-shaped tools (`Task`, `Agent`, `spawn`, …) run freely
  inside peer sessions today.
- `GuardViolated` control-plane events can never fire from real sessions (only the adapter-side
  bridge path could produce them, which no live session exercises).
- Any e2e claim of "deny works" that did not assert a *denial inside a live session* was not
  actually exercising the hook path.

### Proposed fix

In both `install_guard_into_worktree()` (daemon) and `guard_install` (CLI):

1. Target `<wt>/.claude/settings.json` instead of `hooks.json`.
2. Merge, don't overwrite: if `settings.json` exists, parse it, set
   `obj["hooks"]["PreToolUse"] = [...]` (append our entry if a `PreToolUse` array already
   exists), preserving all other keys. If absent, create minimal document.
3. Drop the `_anti_guard` metadata key from `settings.json` — unknown top-level keys may trip
   schema validation. Move provenance markers to a separate inert sidecar
   (e.g. `.claude/anti-guard.meta.json`) if wanted.
4. Consider using `$CLAUDE_PROJECT_DIR/.claude/anti-guard.sh` in the hook command instead of an
   absolute path — survives worktree relocation and matches upstream hook conventions.

### Acceptance criteria

```bash
# 1. Unit: install into a temp dir containing a pre-existing settings.json with other keys
#    → keys preserved, hooks.PreToolUse appended exactly once (idempotent re-run).

# 2. E2E (the test that was missing): spawn a real claude peer with a task that tempts a
#    delegation-shaped tool call ("delegate X to a Task subagent"), then assert:
#    - the tool call was DENIED (peer log / stderr shows guard JSON error), AND
#    - events.jsonl contains GuardViolated for that agent id.
```

---

## B2 (P1) — First-run bootstrap on a fresh state dir fails with zero diagnostics

### Evidence

```bash
$ rm -rf /tmp/fresh && anti-cli --state-dir /tmp/fresh daemon start
error: daemon failed to come up within 5s          # ← only message, ever

$ ANTI_DAEMONIZED=1 ANTI_STATE_DIR=/tmp/fresh ./target/debug/anti-daemon
anti-daemon: cannot open daemon lock: No such file or directory (os error 2)
```

### Root cause

Two compounding problems in `crates/anti-daemon/src/main.rs`:

1. `main()` opens `state_dir.join("daemon.lock")` (line ~63) **before any
   `fs::create_dir_all(&state_dir)`**. A fresh state dir → open fails → exit(1).
   Note `Store::open` / treehouse would have created directories later, but the lock comes first.
2. `daemonize()` (line ~28) re-spawns self with
   `stdout/stderr → Stdio::null()`, so the child's `eprintln!` diagnostics go to `/dev/null`.
   The parent exits 0 after spawning, the wait loop in `anti-cli commands.rs::daemon(Start)`
   times out at 5 s, and the operator gets no signal that (or why) the child died.

Works today on dev machines only because `~/.anti_subagent` already exists from earlier runs.

### Proposed fix

1. In `main()`, before the lock: `std::fs::create_dir_all(&state_dir)?` with a clear
   error message on failure.
2. Stop discarding child stderr: when daemonizing, redirect stderr to
   `$state_dir/logs/daemon.err` (create dirs first) instead of `Stdio::null()`.
3. Improve the CLI failure message to surface the log location:
   `daemon failed to come up within 5s — see {state_dir}/logs/daemon.err`.

### Acceptance criteria

- `rm -rf /tmp/x && anti-cli --state-dir /tmp/x daemon start` succeeds end-to-end on a clean path.
- Deliberately breaking the lock (chmod 000 parent) yields a readable reason in `daemon.err`,
  echoed by the CLI hint.

---

## B3 (P2) — Unmatched provider events collapse into `AgentStarted` (event spam)

### Evidence

One healthy e2e peer run (`e2e2`, single claude turn, ~40 s):

```
AGENT_REGISTERED ×1
AGENT_STARTED    ×31   ← one per unmatched stream-json chunk
AGENT_COMPLETED  ×2
```

Claude Code `stream-json` emits many event kinds (system init, stream deltas, tool_use /
tool_result notifications, …). The adapter bridge maps known kinds to lifecycle events and
catches everything else here (`crates/anti-daemon/src/main.rs` ~line 385):

```rust
_ => EventType::AgentStarted,
```

So every non-matching provider chunk persists another `AgentStarted` row.

### Impact

- Timeline / audit views become noise (`list --json` consumers, future UI).
- Directly pollutes Issue #4 benchmark ground truth: RunMetrics aggregation reads this event
  log; duplicated lifecycle rows distort counts and make cost attribution harder to audit.
- Masks real restart signals: a genuine second `AgentStarted` (recovery path, lines 712/861/1318)
  is indistinguishable from chunk spam.

### Proposed fix

1. Replace the catch-all with a neutral mapping:
   - Known-but-uninteresting kinds → skip persistence (or persist behind a
     `debug_events = true` config flag).
   - Truly unknown kinds → a dedicated `ProviderEvent { kind }` variant (already half-modeled —
     payloads carry `"provider_event": true` and the raw kind string).
2. Keep `AgentStarted` emission exclusively in the lifecycle paths (spawn / recovery), never in
   the provider-event bridge.
3. Add a regression test: feed a recorded stream-json transcript through the bridge, assert
   `count(AgentStarted) == expected_lifecycle_count`.

---

## Minor notes (non-blocking)

- `guard/anti-guard.sh`: daemon round-trip budget is `timeout 0.05` (50 ms) for the socat query —
  extremely tight; fail-closed default makes slow machines deny legitimate delegation. Consider
  250–500 ms with the same fail-closed fallback.
- Scope gate heuristic matches paths containing `*treehouse*|*worktree*` — loose; a marker file
  check (`.anti_subagent` marker) would be stricter.
- `README.md` badge still says `tests-117 passing`; workspace is at 222 now.
- Integration tests build their fixture repo with plain `git init` + empty commit and **no
  `origin` remote**, so the pool takes the `refs/heads/<branch>` resolution path. Real-world
  repos carry an `origin` and take the `refs/remotes/origin/<branch>` path — worth adding one
  T-case whose fixture has a proper bare origin to cover the remote-tracking resolution branch
  (this reviewer hit exactly that divergence while reproducing e2e manually).

---

*Found 2026-08-22 · verified on macOS (arm64) · gh rev b8788a4 · all three reproducible from a
clean state dir.*
