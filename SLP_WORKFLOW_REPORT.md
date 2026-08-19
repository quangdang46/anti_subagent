# Anti-Subagent SLP Workflow Report

> Báo cáo chi tiết workflow hiện tại khi anti_subagent spawn ra Claude Code peer

---

## 1. Architecture Overview

```
HUMAN
 │
 └─ SUPERVISOR   governance · memory notebook · optimization (planned)
    │
    └─ LEAD      planning · coordination · integration · acceptance
       │         (anti-daemon — TCP IPC on 127.0.0.1:PORT)
       │
       └─ PEER   Engineer · Architect · Reviewer · Scout · Proof Auditor · Shadow
                  (Claude Code — independent OS process in treehouse worktree)
```

**Key principle:** Every peer is a full, autonomous agent. It believes it's working with a human. The hierarchy is invisible to peers.

---

## 2. Spawn Workflow — Step by Step

### 2.1. CLI Command

```bash
anti-cli spawn \
  --id my-peer-1 \
  --role peer \
  --harness claude \
  --repo /path/to/repo \
  --task "Build a landing page with login"
```

### 2.2. Daemon Processing (main.rs → spawn())

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Validate inputs (id, role, harness, repo path exists)    │
│ 2. Reserve identity (SQLite INSERT → status: Created)       │
│ 3. Acquire workspace lease via treehouse-core pool          │
│ 4. Spawn Claude Code subprocess in the leased worktree      │
│ 5. Attach PID to store                                      │
│ 6. Return response: {id, pid, status, workspace}            │
└─────────────────────────────────────────────────────────────┘
```

### 2.3. Workspace Lease (treehouse-core)

```
AntiPool::acquire()
  └→ Pool::open_with_env(repo_root, AntiEnv, config)
       └→ resolve_pool_dir() → ~/.anti_subagent/worktrees/.treehouse/<repo>-<hash>/
       └→ pool.get(AcquireOptions { lease: Some(holder) })
            └→ Find available worktree or create new one
            └→ Git worktree add + reset to default branch
            └→ Stamp lease (id, holder, acquired_at)
            └→ Return Acquired { path, lease_id }
```

**Pool location:** `~/.anti_subagent/worktrees/.treehouse/<repo-name>-<sha256[:6]>/`

### 2.4. Peer Process Spawn

```rust
// anti-adapters/src/claude.rs
let mut cmd = Command::new("claude");
cmd.args(["-p", "--output-format", "json",
          "--permission-mode", "acceptEdits",
          "--dangerously-skip-permissions",
          "--append-system-prompt", "You are a peer..."]);
cmd.current_dir(&worktree_path);  // cd into leased worktree
cmd.stdin(Stdio::null());
cmd.stdout(Stdio::from(log_file));  // → ~/.anti_subagent/logs/<id>.log
cmd.stderr(Stdio::inherit());
let child = cmd.spawn()?;
```

### 2.5. Response

```json
{
  "id": "my-peer-1",
  "pid": 16420,
  "status": "running",
  "workspace": {
    "lease_id": "6b1afb2e3362...",
    "path": "C:\\Users\\ADMIN\\.anti_subagent\\worktrees\\.treehouse\\repo-hash\\1\\repo"
  }
}
```

---

## 3. Lifecycle Monitoring

### 3.1. Reaper Thread (every 5s)

```rust
// main.rs — reaper thread
loop {
    sleep(5s);
    reap_children(store, children);  // try_wait on all child processes
    // On exit: mark Completed/Crashed in store, emit event
}
```

### 3.2. Watchdog Thread (every 15s)

```rust
// main.rs — watchdog thread
loop {
    sleep(15s);
    overdue = store.overdue_reviews(now);
    for w in overdue {
        emit(ReviewEscalated, { peer_id, lead_id, deadline });
    }
}
```

### 3.3. Unified Recovery (on daemon restart)

```
Phase 1: treehouse.gc() → reclaim orphaned worktrees
Phase 2: find_dead_agents() → PID + start time check
Phase 3: recover_work_items() → InProgress/Submitted → NeedsRevision
Phase 4: mark dead agents as Crashed + emit PeerCrashed events
```

---

## 4. Data Flow

```
anti-cli (IPC) ←→ anti-daemon (TCP) ←→ SQLite (state.db)
                      │
                      ├→ treehouse-core (pool state)
                      ├→ Claude Code (subprocess)
                      └→ ~/.anti_subagent/logs/<id>.log
```

### 4.1. State Management (Store — SQLite WAL)

| Table | Purpose |
|-------|---------|
| `agents` | Agent records: id, role, status, pid, workspace, spawn_gen |
| `events` | Append-only event log (PeerCrashed, WorkSubmitted, etc.) |
| `work_items` | Task lifecycle: Pending → InProgress → Submitted → Verified → Accepted |
| `dispatch_events` | Looper dispatch tracking |

### 4.2. PID-Reuse Safety

```rust
// store.rs
store.attach_pid_with_timestamp(id, pid, process_start_time);
store.is_agent_alive(id)  // checks PID alive + start time matches
```

### 4.3. Daemon Lock

```rust
// main.rs — fd-lock
let lock_file = OpenOptions::new().create(true).write(true).open("daemon.lock");
let mut lock = fd_lock::RwLock::new(lock_file);
let _guard = lock.write()?;  // held for entire daemon lifetime
```

---

## 5. File System Layout

```
~/.anti_subagent/
├── state.db                    # SQLite agent registry + events
├── daemon.lock                 # fd-lock single instance guard
├── events/
│   └── events.jsonl            # Append-only event log
├── logs/
│   ├── <peer-id>.log           # Claude Code JSON output per peer
│   └── ...
└── worktrees/
    └── .treehouse/
        └── <repo>-<hash>/
            ├── 1/<repo>/       # Worktree 1 (git worktree add)
            ├── 2/<repo>/       # Worktree 2
            ├── treehouse-state.json
            └── treehouse-state.lock
```

---

## 6. Verified End-to-End Test

### Test 1: Simple Task

```bash
anti-cli spawn --id test-slp-peer-1 --role peer --harness claude \
  --repo <repo> --task "Create slp_test.txt with 'Hello from SLP peer!'"
```

| Metric | Result |
|--------|--------|
| Spawn | ✅ PID 13536, lease acquired |
| Execution | ✅ 2 turns, 12.2s, $0.43 |
| Status | ✅ COMPLETED |
| Workspace | ✅ Released back to pool |

### Test 2: Landing Page Build

```bash
anti-cli spawn --id landing-page-builder --role peer --harness claude \
  --repo <repo> --task "Build modern landing page with login feature"
```

| Metric | Result |
|--------|--------|
| Spawn | ✅ PID 7484, worktree leased |
| Output | ✅ index.html (14KB) + REPORT.md (2.5KB) |
| Status | ✅ COMPLETED |
| Features | ✅ Hero, 3 feature cards, login modal, dashboard, responsive |

### Test 3: Pool Path Verification

```bash
# After fix: AntiEnv implements TreehouseEnv
anti-cli spawn --id pool-fix-verify --role peer --harness claude \
  --repo <repo> --task "Write 'POOL PATH FIXED' to pool_fix_verify.txt"
```

| Metric | Before Fix | After Fix |
|--------|-----------|-----------|
| Worktree path | `~/.treehouse/<repo>-<hash>/` | `~/.anti_subagent/worktrees/.treehouse/<repo>-<hash>/` |
| File location | Wrong directory | ✅ Correct directory |

---

## 7. Safety Features

| Feature | Implementation | Status |
|---------|---------------|--------|
| **Daemon lock** | fd-lock on daemon.lock | ✅ Prevents concurrent daemons |
| **PID-reuse safety** | attach_pid_with_timestamp + is_agent_alive | ✅ Checks PID + process start time |
| **CAS write protection** | SHA-256 baseline + .anti.lock | ✅ Prevents last-writer-wins |
| **Guard (fail-closed)** | Deny delegation-shaped tool calls in peers | ✅ Peers can't spawn sub-agents |
| **Lease isolation** | Each peer gets its own git worktree | ✅ No cross-peer file conflicts |
| **Recovery on restart** | gc + dead-agent detection + work-item reconciliation | ✅ Orphans reclaimed automatically |
| **Bounded context** | Capsule ≤ 64KB | ✅ Prevents context overflow |

---

## 8. Configuration

### AntiEnv (pool path)

```rust
AntiEnv::new(PathBuf::from("~/.anti_subagent"))
// pool_root = ~/.anti_subagent/worktrees
// Pool stores: ~/.anti_subagent/worktrees/.treehouse/<repo>-<hash>/
```

### PoolConfig

```rust
PoolConfig {
    max_trees: 16,           // max worktrees per pool
    lock_timeout_secs: 10,   // SQLite lock timeout
    gc_interval_secs: 300,   // advisory GC interval
}
```

### Harness Adapters

| Harness | Command | Notes |
|---------|---------|-------|
| `claude` | `claude -p --output-format json --permission-mode acceptEdits` | Default |
| `codex` | `codex exec --quiet` | OpenAI |
| `opencode` | `opencode exec` | Open source |

---

## 9. Test Results

```
anti-core:   64 tests passed
anti-daemon: 38 tests passed
anti-workspace: 15 tests passed
Total:       117 tests passed ✅
```

---

## 10. Bugs Found & Fixed

### Bug: Treehouse pool root ignored

**Root cause:** `DefaultEnv::pool_root()` hardcodes `$HOME/.treehouse`, ignoring `TreehouseConfig.root`.

**Fix:** Implemented `TreehouseEnv` trait for `AntiEnv` with correct `pool_root()`. Changed `Pool::open()` to `Pool::open_with_env()` with `AntiEnv`.

**Impact:** Worktrees now created at `~/.anti_subagent/worktrees/.treehouse/` as intended.

---

## 11. What's Next (from beads)

| Bead | Status | Description |
|------|--------|-------------|
| `anti_subagent-38i` | ✅ Closed | Phase 0: Safety & Correctness |
| `anti_subagent-88h` | ✅ Closed | Phase 1: Execution Architecture |
| `anti_subagent-d26` | ✅ Closed | Phase 2: SLP-Native Orchestration |
| `anti_subagent-2zl` | ✅ Closed | Phase 3: Scheduling & Routing |
| `anti_subagent-hcx` | ✅ Closed | Phase 4: Reliability |

**All 42 beads closed — backlog fully cleared.**

---

*Report generated: 2026-08-19*
*anti_subagent version: dev (post Phase 0-4)*
*117 tests passing, SLP workflow verified end-to-end*
