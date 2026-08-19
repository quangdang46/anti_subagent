# anti-workspace

Treehouse adapter for anti-subagent — library-based workspace management.

## Overview

`anti-workspace` wraps [treehouse-core](https://github.com/quangdang46/treehouse_rust) to provide worktree lifecycle management without subprocess calls. Every peer agent gets an isolated worktree via the pool; on exit, worktrees are released back for reuse.

## Quick Start

```rust
use anti_workspace::{Treehouse, AntiEnv, PoolConfig};
use std::path::Path;

// Create a treehouse adapter
let env = AntiEnv::new(PathBuf::from("~/.anti_subagent"));
let treehouse = Treehouse::new(env, PoolConfig::default());

// Acquire a worktree for an agent
let lease = treehouse.acquire(
    Path::new("/path/to/repo"),
    Some("https://github.com/user/repo.git"),
    "peer-1",
)?;

// ... agent works in lease.path ...

// Release when done
treehouse.release(&lease.path, Path::new("/path/to/repo"), None)?;
```

## Key Types

| Type | Description |
|------|-------------|
| `Treehouse` | Primary API — acquire, release, gc |
| `AntiPool` | Lower-level pool wrapper (used by Treehouse) |
| `AntiEnv` | Environment config (state directory path) |
| `PoolConfig` | Pool settings (max_trees, lock_timeout, gc_interval) |
| `Lease` | Worktree lease identity (path, lease_id, holder) |
| `WorkspaceError` | Error type for pool/io failures |

## Architecture

```
anti-daemon
    └── Treehouse (Arc<Treehouse>)
         └── AntiPool
              └── treehouse_core::pool::Pool
                   ├── SQLite state (WAL mode)
                   ├── git worktree management
                   └── gc (stale/dead-owner reclamation)
```

## Modules

- **`pool`** — `AntiPool`, `AntiEnv`, `PoolConfig` wrapping treehouse-core
- **`cas`** — Content-addressable write protection (SHA-256 baseline + lock)
- **`lib.rs`** — `Treehouse`, `Lease`, `WorkspaceError`

## Configuration

`PoolConfig` defaults:
- `max_trees: 16` — maximum worktrees per pool
- `lock_timeout_secs: 10` — SQLite lock timeout
- `gc_interval_secs: 300` — advisory GC interval

## Dependencies

- `treehouse-core` — git dependency (workspace pool/state/lease management)
- `chrono` — TTL duration conversion
- `serde`, `serde_json` — serialization
- `sha2` — CAS baseline hashing
- `thiserror` — error derivation
