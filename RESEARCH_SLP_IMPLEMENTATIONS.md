# SLP Implementation Patterns - Research Report

**Date**: 2026-08-19
**Status**: Research Complete
**Purpose**: Document how other repos implement SLP (Supervisor → Lead → Peer) orchestration, focusing on `claude -p` vs interactive approaches and session tracking challenges

---

## Executive Summary

The research reveals a fundamental tension: **`claude -p` was designed as a one-shot tool** (prompt in, result out, exit), but production SLP orchestration needs **long-lived sessions** with proper tracking. Four main approaches exist in the ecosystem, each with distinct trade-offs.

**Key Finding**: The `claude -p` mode has documented session tracking issues that make it unsuitable for robust multi-turn orchestration without workarounds.

---

## 1. Current Implementation Analysis

### anti_subagent Architecture

```rust
// crates/anti-adapters/src/lib.rs:46-64
fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
    let mut cmd = Command::new("claude");
    cmd.args([
        "-p",                          // ← Headless print mode
        "--output-format", "json",
        "--permission-mode", "acceptEdits",
        "--dangerously-skip-permissions",
        "--append-system-prompt", &peer_prompt,
    ]);
    cmd.current_dir(&ctx.worktree);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::inherit());
    Ok(cmd)
}
```

**Current Limitations**:
- Each peer invocation is a separate `claude -p` process
- No session continuity between invocations
- Session ID not accessible until process exits
- Cannot track peer activity in real-time

---

## 2. Four Main Approaches in the Ecosystem

### Approach A: `claude -p` (Headless/Print Mode)

**Description**: Official mechanism for programmatic Claude Code usage.

```bash
# Basic usage
claude -p "prompt" --output-format json

# With session tracking (workaround)
SESSION_ID=$(uuidgen)
claude -p "task" --session-id "$SESSION_ID" --output-format json

# Resume later
claude -p "follow-up" --resume "$SESSION_ID" --output-format json
```

**Pros**:
- Official, stable, debuggable
- JSON output format for programmatic parsing
- Supports `--continue` and `--resume` for conversation continuation

**Cons**:
- Sessions not indexed in `sessions-index.json` (GitHub #61058)
- Session ID not accessible at runtime (GitHub #44607)
- No `CLAUDE_SESSION_ID` environment variable (GitHub #17188)
- One process per turn, no stdin loop
- Billing moved to "Agent SDK monthly credit" (June 2026)

**Used By**: CI/CD pipelines, pre-commit hooks, simple automation scripts

---

### Approach B: PTY Wrapper Libraries

**Description**: Drive the **interactive TUI** through a pseudo-terminal instead of using `claude -p`.

| Project | Language | Notes |
|---------|----------|-------|
| Equality-Machine/claude-p | Python | Assigns session ID, reads canonical JSONL |
| PeterSR/claude-p | Go | Daemon mode for persistent multi-turn |
| smithersai/claude-p | Zig | Drop-in replacement using zmux PTY |
| empty-user77/open-claude-p | Node.js | Background daemon for session persistence |
| ybouane/dash-p | TypeScript | Headless terminal emulator for screen reading |
| kcosr/claude-pty-wrapper | - | Streams assistant text from session JSONL |
| umputun/fya | - | PTY wrapper for one-shot compatibility |
| quietforgelabs/AgentPTY | Python | FastAPI server exposing PTY over HTTP |
| Finndersen/claude-interactive-sdk | Python | Drives Claude through tmux |

**Pros**:
- Uses subscription billing (not credit-based)
- True multi-turn without process restart
- Persistent daemon mode possible

**Cons**:
- **Brittle**: Dependent on TUI rendering, terminal probes, Ink runtime behavior
- Large prompts (>1KB) confuse TUI paste detection
- Hook payload schema changes break wrappers
- No true token-level streaming (only turn-level)
- Tied to undocumented TUI internals

**Why PTY Wrappers Exist**: Anthropic announced `claude -p`/SDK usage moves to "Agent SDK monthly credit" billing, while interactive Claude Code uses normal subscription allocation. PTY wrappers let users drive interactive TUI programmatically to stay on subscription billing.

---

### Approach C: Official Agent SDK

**Description**: Official Python/TypeScript SDK for programmatic multi-turn conversations.

**Python** (`claude-agent-sdk`):
```python
from claude_agent_sdk import ClaudeSDKClient, ClaudeOptions

async with ClaudeSDKClient(options=ClaudeOptions()) as client:
    # First turn
    await client.query("Implement the login feature")
    async for msg in client.receive_response():
        print(msg)

    # Second turn (same context)
    await client.query("Now add password validation")
    async for msg in client.receive_response():
        print(msg)
```

**TypeScript** (`@anthropic-ai/claude-agent-sdk`):
```typescript
import { query, ClaudeSDKClient } from '@anthropic-ai/claude-agent-sdk';

const client = new ClaudeSDKClient(options);
await client.query("First task");
await client.query("Follow-up in same context");
```

**Pros**:
- **Proper session management** - sessions tracked automatically
- Streaming support (token-level)
- Interrupt support
- In-process MCP tools
- Custom hooks
- Conversation persistence

**Cons**:
- Python/TypeScript only (no Rust SDK yet)
- Requires subprocess per session
- Async-only API
- Separate billing model

**Used By**: Production multi-agent systems, IDE integrations

---

### Approach D: Direct Subprocess Wrappers

**Description**: Custom wrappers that invoke Claude CLI with specific configurations.

| Project | Approach |
|---------|----------|
| elizaOS/plugin-sub-agent-claude-code | Drives `claude --print` in sandboxed Bun subprocess with RPC |
| kolodny/claude-call | Invokes Claude tools directly (no LLM reasoning) |
| zhusq20/CLI2API | Wraps `claude` CLI as OpenAI-compatible HTTP API |

**Pros**:
- Full control over process lifecycle
- Can implement custom session tracking
- Language-agnostic (any language can spawn subprocess)

**Cons**:
- Must build and maintain custom infrastructure
- Still faces same `claude -p` limitations
- No official support

---

## 3. Session Tracking Issues (Documented GitHub Issues)

### Issue #61058: Sessions Not Indexed

**Problem**: `claude -p` writes valid `.jsonl` transcript files but **never registers them in `sessions-index.json`**.

**Impact**:
- Sessions resumable by UUID (`claude --resume <uuid>`) but invisible to interactive `--resume` picker
- Title-based resume fails completely for `-p` sessions
- Affects every third-party `-p` consumer

**Reproducibility**: Still reproducible as of v2.1.176 (18 releases after initial report)

**Workaround**: Track session UUIDs externally, always use `--resume <uuid>` instead of title-based resume.

---

### Issue #44607: Session ID Not Accessible at Runtime

**Problem**: No way for a running session to know its own session ID. The session ID is only revealed **after** the session ends.

**Workaround**: Use a `SessionStart` hook to capture the session_id:
```bash
# In hooks config
SessionStart:
  command: "echo $CLAUDE_SESSION_ID > /tmp/session.env"
```

**Impact**: Cannot correlate real-time activity with specific sessions.

---

### Issue #17188: `--session-id` Not Propagated as Environment Variable

**Problem**: No `CLAUDE_SESSION_ID` or `CLAUDE_SESSION_NAME` environment variables.

**Impact**: Multi-agent orchestration systems cannot correlate sessions with their own tracking.

**Workaround**: `SessionStart` hook that reads `session_id` from stdin JSON and writes to `$CLAUDE_ENV_FILE`.

---

### Issue #14859: Subagent Event Attribution

**Problem**: All hook events from parent and sub-agents share the same `session_id`. No `agent_id`, `parent_agent_id`, or `agent_slug` fields in hook payloads.

**Impact**:
- Cannot determine which agent produced which event
- Cannot build observability dashboards
- Cannot implement cost attribution

**Missing**: `SubagentStart` hook (only `SubagentStop` exists)

---

### Issue #42458: SessionStart Hooks Fire with `--no-session-persistence`

**Problem**: `claude -p --no-session-persistence` still loads and executes all `SessionStart` hooks.

**Impact**: Slow or blocking hooks cause `claude -p` to hang for 30-60 seconds. Multi-agent dispatch systems see throughput drop.

---

### Issue #81937: Auth Divergence Between `-p` and Interactive Mode

**Problem**: `claude -p` can fail with "OAuth session expired" while interactive mode works fine.

**Impact**: Unreliable automation in some environments.

---

## 4. Alternative Approaches for SLP Implementation

### Option 1: Pre-assigned Session IDs (Quick Fix)

```bash
#!/bin/bash
# spawn_peer.sh

PEER_ID=$1
TASK=$2
WORKTREE=$3

# Pre-assign UUID for live tracking
SESSION_ID=$(uuidgen)
echo "$PEER_ID:$SESSION_ID" >> /tmp/peer_sessions.log

# Spawn peer with tracked session
claude -p "$TASK" \
    --session-id "$SESSION_ID" \
    --output-format json \
    --permission-mode acceptEdits \
    --dangerously-skip-permissions \
    --append-system-prompt "You are a peer working independently." \
    --cwd "$WORKTREE"
```

**Pros**: Simple, uses official `--session-id` flag
**Cons**: Still one process per turn, sessions not indexed

---

### Option 2: Agent SDK with Rust FFI (Recommended)

Since anti_subagent is in Rust, options:
1. **Spawn Python/TS subprocess** running Agent SDK
2. **Wait for official Rust SDK** (not yet available)
3. **Use `claude-agent-sdk` via FFI** (experimental)

```rust
// Conceptual Rust wrapper
pub struct AgentSdkPeer {
    session_id: String,
    python_process: Child,
}

impl AgentSdkPeer {
    pub fn spawn(worktree: &Path, task: &str) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let python_script = format!(r#"
import asyncio
from claude_agent_sdk import ClaudeSDKClient, ClaudeOptions

async def main():
    async with ClaudeSDKClient(
        options=ClaudeOptions(
            session_id="{session_id}",
            cwd="{worktree}"
        )
    ) as client:
        await client.query("{task}")
        async for msg in client.receive_response():
            print(msg)

asyncio.run(main())
"#);

        // Write script to temp file, execute with python
        // ...
    }
}
```

**Pros**: Proper session management, streaming, multi-turn
**Cons**: Requires Python runtime, subprocess overhead

---

### Option 3: PTY Daemon Mode

```rust
// Keep Claude TUI alive between invocations
pub struct PtyDaemon {
    pty_master: File,
    session_id: String,
}

impl PtyDaemon {
    pub fn start(worktree: &Path) -> Result<Self> {
        // 1. Create PTY pair
        // 2. Spawn `claude` in interactive mode
        // 3. Wait for TUI initialization
        // 4. Capture session_id from first output
    }

    pub fn send_task(&mut self, task: &str) -> Result<String> {
        // 1. Write task to PTY stdin
        // 2. Read response from PTY stdout
        // 3. Parse ANSI output
        // 4. Return response
    }

    pub fn resume_session(&mut self, session_id: &str) -> Result<()> {
        // Use `--resume` flag on initial spawn
    }
}
```

**Pros**: Subscription billing, true multi-turn, no process restart
**Cons**: Brittle, tied to TUI internals, requires PTY handling

---

### Option 4: `stream-json` Format for Multi-Turn

```bash
# Pass conversation history to new Claude instance
cat conversation.jsonl | claude -p \
    --input-format stream-json \
    --output-format stream-json \
    --append-system-prompt "Continue the conversation"
```

**Pros**: Can pass existing conversations, good for multi-phase pipelines
**Cons**: One process per turn, conversation history must be serialized

---

### Option 5: Background Agents (`--bg`)

```bash
# Start background agent
claude --bg "investigate the flaky test"  # Returns session ID immediately

# List active sessions
claude agents --json

# Resume later
claude --resume <session-id>
```

**Pros**: Parallel sessions, managed from one place, proper lifecycle
**Cons**: Cannot combine with `-p`, designed for interactive terminal

---

## 5. Recommendations for anti_subagent

### Short-term (Immediate)

1. **Implement Pre-assigned Session IDs**
   - Add `uuid` crate dependency
   - Generate session ID before spawning peer
   - Pass `--session-id` flag to `claude -p`
   - Store mapping in `PeerManager`

```rust
// In anti-adapters/src/lib.rs
pub struct SpawnContext {
    pub worktree: PathBuf,
    pub task: Option<String>,
    pub peer_prompt: Option<String>,
    pub session_id: Option<String>,  // ← Add this
}

fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
    let mut cmd = Command::new("claude");
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        // ...
    ];

    if let Some(session_id) = &ctx.session_id {
        args.push("--session-id".to_string());
        args.push(session_id.clone());
    }

    cmd.args(args);
    // ...
}
```

2. **Add Session Tracking to PeerManager**
```rust
pub struct PeerManager {
    children: HashMap<String, Child>,
    session_ids: HashMap<String, String>,  // peer_id → session_id
}

impl PeerManager {
    pub fn track_with_session(&mut self, id: &str, child: Child, session_id: String) {
        self.children.insert(id.to_string(), child);
        self.session_ids.insert(id.to_string(), session_id);
    }

    pub fn session_id_of(&self, id: &str) -> Option<&str> {
        self.session_ids.get(id).map(|s| s.as_str())
    }
}
```

### Medium-term (Next Sprint)

3. **Implement SessionStart Hook**
   - Capture session ID at runtime
   - Write to env file for orchestration layer

```bash
#!/bin/bash
# ~/.claude/hooks/session_start.sh
echo "$CLAUDE_SESSION_ID" >> /tmp/active_sessions.log
```

4. **Add `--resume` Support for Multi-Turn**
   - Store session IDs in database
   - Allow resuming peer sessions for follow-up tasks

### Long-term (Future)

5. **Evaluate Agent SDK Integration**
   - Monitor Rust SDK availability
   - Consider Python subprocess wrapper if needed
   - Evaluate billing implications

6. **Consider PTY Daemon for Subscription Billing**
   - If credit-based billing becomes expensive
   - Build robust PTY wrapper with ANSI parsing

---

## 6. Comparison Matrix

| Feature | `claude -p` | PTY Wrapper | Agent SDK | Background Agents |
|---------|-------------|-------------|-----------|-------------------|
| Session tracking | ❌ (workaround) | ✅ | ✅ | ✅ |
| Multi-turn | ❌ | ✅ | ✅ | ✅ |
| Streaming | ❌ | ⚠️ (turn-level) | ✅ (token-level) | ❌ |
| Rust support | ✅ | ⚠️ (custom) | ❌ (Python/TS) | ❌ |
| Subscription billing | ❌ (credit) | ✅ | ❌ (credit) | ❌ |
| Stability | ✅ | ❌ (brittle) | ✅ | ✅ |
| Official support | ✅ | ❌ | ✅ | ✅ |
| Process per turn | Yes | No | Yes | Yes |
| Real-time tracking | ❌ | ⚠️ | ✅ | ✅ |

---

## 7. Key Takeaways

1. **`claude -p` is not designed for multi-turn orchestration** - it's a one-shot tool with documented session tracking issues.

2. **PTY wrappers are a temporary bridge** - they work today but are inherently fragile and depend on undocumented TUI behavior.

3. **Agent SDK is the official path forward** - but requires Python/TypeScript, not Rust.

4. **For anti_subagent**, the most pragmatic approach is:
   - **Short-term**: Pre-assigned session IDs with `--session-id` flag
   - **Medium-term**: SessionStart hooks for runtime tracking
   - **Long-term**: Evaluate Agent SDK integration or wait for Rust SDK

5. **The billing landscape is shifting** - `claude -p` and SDK usage moved to credit-based billing (June 2026), making PTY wrappers attractive for cost optimization.

---

## Appendix A: GitHub Issues Referenced

| Issue | Title | Status |
|-------|-------|--------|
| #61058 | `claude -p` sessions not indexed | Open (v2.1.176) |
| #44607 | Session ID not accessible at runtime | Open |
| #17188 | `--session-id` not propagated as env var | Open |
| #14859 | Subagent event attribution | Open |
| #42458 | SessionStart hooks fire with `--no-session-persistence` | Open |
| #81937 | Auth divergence between `-p` and interactive | Open |

## Appendix B: Repos Researched

- Equality-Machine/claude-p (Python)
- PeterSR/claude-p (Go)
- smithersai/claude-p (Zig)
- empty-user77/open-claude-p (Node.js)
- ybouane/dash-p (TypeScript)
- kcosr/claude-pty-wrapper
- umputun/fya
- quietforgelabs/AgentPTY (Python)
- Finndersen/claude-interactive-sdk (Python)
- elizaOS/plugin-sub-agent-claude-code
- kolodny/claude-call
- zhusq20/CLI2API

---

**Next Steps**:
1. Review this report with team
2. Prioritize short-term fixes (pre-assigned session IDs)
3. Create implementation plan for medium-term changes
4. Schedule Agent SDK evaluation
