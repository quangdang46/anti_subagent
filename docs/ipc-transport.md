# IPC Transport

anti_subagent daemon communicates with clients (CLI, guard, other daemons) over
a local IPC transport. The protocol is identical on all platforms — newline-delimited
JSON (one request line, one response line). Only the transport layer differs.

## Supported Transports

| Transport | Platform | Address format | Default? |
|---|---|---|---|
| Unix domain socket | Linux/macOS | `<state_dir>/anti.sock` | ✅ auto on Unix |
| TCP loopback | Windows (also Linux/macOS) | `127.0.0.1:<port>` | ✅ auto on Windows |
| Named pipe | Windows | `\\.\pipe\anti-subagent-<id>` | ❌ opt-in |

## Selection

Transport is resolved at daemon startup in this order:

1. **`ANTI_IPC_TRANSPORT` env var** — `unix`, `tcp`, or `named_pipe` (Windows only)
2. **`config.toml` key** — `ipc_transport = "unix"` (in `~/.anti_subagent/config.toml`)
3. **Auto-detect** — Unix socket on Linux/macOS, TCP loopback on Windows

Once selected, the transport is immutable for the daemon's lifetime.

## Security

All three transports bind only to the local machine:

- **Unix socket**: file permissions on `<state_dir>/anti.sock`
- **TCP loopback**: bound to `127.0.0.1` only (no firewall rules needed)
- **Named pipe**: Windows ACL restricts to local users

No remote access is possible without explicit port forwarding or tunneling.

## Guard Fail-Closed

The guard (PreToolUse hook) queries the daemon over this transport with a 50ms
timeout. If the daemon is unreachable, the guard denies delegation-shaped tools
(fail-closed). This behavior is transport-agnostic.

## TCP Port Selection

When TCP is used (Windows auto or explicit override), the port is derived
deterministically from the state directory hash: `49152 + hash(state_dir) % 16383`.
This maps to the unprivileged port range (49152–65535) and is stable across
restarts for the same state directory.

## Named Pipe (Windows opt-in)

Named pipes use the path `\\.\pipe\anti-subagent-<id>` where `<id>` is derived
from the state directory hash. Set `ipc_transport = "named_pipe"` in config.toml
or `ANTI_IPC_TRANSPORT=named_pipe` to use this transport instead of TCP.

## Configuration Example

```toml
# ~/.anti_subagent/config.toml
ipc_transport = "tcp"  # or "unix" or "named_pipe" (Windows)
```

## Diagnostics

`anti doctor` reports the active transport and whether it is reachable:

```
state_dir: /home/user/.anti_subagent
daemon: OK
ipc_transport: unix_socket (reachable)
treehouse: OK
claude: OK
state.db: present (12288 bytes)
```
