# Paseo Architecture Analysis & anti_subagent Integration Plan

**Date**: 2026-08-19
**Status**: Research Complete → Implementation Plan Ready
**Purpose**: Deep-dive into Paseo's control plane architecture and map it to anti_subagent's SLP governance model

---

## Executive Summary

Paseo solves the **control plane problem** that anti_subagent currently lacks. While anti_subagent has the correct SLP governance thesis (Supervisor → Lead → Peer), its runtime layer is thin — peers are spawned via `claude -p` one-shot processes with no lifecycle management, event streaming, or session continuity.

**Key Discovery**: Paseo does NOT use `claude -p`. It uses `@anthropic-ai/claude-agent-sdk`'s `query()` function, which spawns Claude in **stream-json mode over stdio pipes**. This gives Paseo access to Claude's **native structured event stream** — including `task_started`, `task_progress`, `thinking` events, tool calls, and subagent lifecycle announcements — without any PTY hacking.

**Recommendation**: anti_subagent should adopt Paseo's control plane pattern (agent identity ≠ process ≠ session) while keeping its SLP governance model. The current `claude -p` adapter should be replaced with an Agent SDK-based adapter that provides structured event streaming.

---

## Part 1: Paseo Architecture Deep-Dive

### 1.1 Control Plane Overview

```
┌─────────────────────────────────────────────────────────┐
│                    PASEO DAEMON                          │
│                                                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐    │
│  │ AgentManager│  │ Workspace   │  │ Schedule    │    │
│  │             │  │ Manager     │  │ Service     │    │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘    │
│         │                │                │            │
│  ┌──────▼────────────────▼────────────────▼──────┐    │
│  │              Provider Registry                 │    │
│  │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ │    │
│  │  │ Claude │ │ Codex  │ │ Cursor │ │  ACP   │ │    │
│  │  │Adapter │ │Adapter │ │Adapter │ │Generic │ │    │
│  │  └────────┘ └────────┘ └────────┘ └────────┘ │    │
│  └───────────────────────────────────────────────┘    │
│                                                         │
│  ┌───────────────────────────────────────────────┐    │
│  │              Event Bus (WebSocket)             │    │
│  │  agent_state │ agent_stream │ provider_subagent│    │
│  └───────────────────────────────────────────────┘    │
│                                                         │
│  ┌───────────────────────────────────────────────┐    │
│  │              MCP Server (Control Plane)        │    │
│  │  create_agent │ send_agent_prompt │ archive    │    │
│  └───────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

### 1.2 Agent Identity Model

Paseo separates **four concerns** that anti_subagent currently conflates:

```
Agent Identity
    ≠
Process (PID)
    ≠
Session (Claude session ID)
    ≠
Workspace (cwd / worktree)
```

**AgentRecord** (persisted as JSON):
```typescript
{
  id: "agent-abc-123",
  provider: "claude",
  model: "sonnet",
  cwd: "/path/to/worktree",
  workspaceId: "ws-xyz-789",
  parentAgentId: "agent-parent-456",  // null for top-level
  title: "Implement login feature",
  status: "idle",                     // initializing|idle|running|error|closed|archived
  config: {
    modeId: "code",
    thinkingOptionId: "medium",
    systemPrompt: "...",
    mcpServers: [...]
  },
  persistence: {
    provider: "claude",
    sessionId: "claude-uuid-here",
    nativeHandle: { ... }            // provider-specific resume data
  },
  createdAt: "2026-08-19T10:00:00Z",
  lastActivityAt: "2026-08-19T10:15:00Z",
  archivedAt: null
}
```

**Key insight**: The `session_id` is **runtime metadata** inside `persistence`, not the primary identity. This means:
- Process can die → agent identity survives → daemon knows it died → can restart
- Session can be resumed by UUID without knowing the old process
- Agent can be transferred to a different provider without losing identity

### 1.3 Agent Lifecycle State Machine

```
                    ┌──────────────┐
                    │ initializing │
                    └──────┬───────┘
                           │
                           ▼
                    ┌──────────────┐
              ┌────▶│     idle     │◀────┐
              │     └──────┬───────┘     │
              │            │             │
              │            ▼             │
              │     ┌──────────────┐     │
              │     │   running    │     │
              │     └──────┬───────┘     │
              │            │             │
              │     ┌──────┴───────┐     │
              │     │              │     │
              │     ▼              ▼     │
              │ ┌────────┐  ┌────────┐   │
              │ │ error  │  │completed│   │
              │ └───┬────┘  └───┬────┘   │
              │     │           │        │
              │     └───────────┘        │
              │                          │
              │     ┌──────────────┐     │
              └─────│   closed     │─────┘
                    │ (resumable)  │
                    └──────┬───────┘
                           │
                           ▼
                    ┌──────────────┐
                    │  archived    │
                    │ (soft-delete)│
                    └──────────────┘
```

**State transitions**:
- `idle → running`: `send_agent_prompt()` triggers a turn
- `running → idle`: Turn completes successfully
- `running → error`: Turn fails
- `error → idle`: Retry succeeds
- `idle → closed`: Agent session released (provider process killed)
- `closed → idle`: `resumeAgent()` re-establishes provider session
- `closed → archived`: Soft-delete, cascades to children

### 1.4 Event System

Paseo uses a **subscriber-based event broadcasting** system:

```typescript
// Three event channels
type AgentManagerEvent =
  | { type: "agent_state"; agentId: string; status: AgentStatus }
  | { type: "agent_stream"; agentId: string; event: AgentStreamEvent; seq: number }
  | { type: "provider_subagent"; agentId: string; event: ProviderSubagentInputEvent };
```

**AgentStreamEvent** (discriminated union):
```typescript
type AgentStreamEvent =
  | { type: "thread_started"; sessionId: string; provider: AgentProvider }
  | { type: "turn_started"; provider: AgentProvider; turnId?: string }
  | { type: "turn_completed"; provider: AgentProvider; usage?: AgentUsage }
  | { type: "turn_failed"; provider: AgentProvider; error: string }
  | { type: "turn_canceled"; provider: AgentProvider; reason: string }
  | { type: "timeline"; item: AgentTimelineItem; provider: AgentProvider }
  | { type: "permission_requested"; provider: AgentProvider; request: AgentPermissionRequest }
  | { type: "permission_resolved"; provider: AgentProvider; requestId: string; resolution: AgentPermissionResponse }
  | { type: "attention_required"; provider: AgentProvider; reason: "finished" | "error" | "permission" }
  | { type: "provider_subagent"; provider: AgentProvider; event: ProviderSubagentInputEvent };
```

**AgentTimelineItem** (content types):
```typescript
type AgentTimelineItem =
  | { type: "user_message"; content: string }
  | { type: "assistant_message"; content: string }
  | { type: "reasoning"; content: string }           // ← thinking/reasoning
  | { type: "tool_call"; name: string; status: "running"|"completed"|"failed"; detail: ToolCallDetail }
  | { type: "todo"; items: TodoItem[] }
  | { type: "error"; message: string }
  | { type: "compaction"; ... };
```

**Tool call detail types** (normalized across providers):
```typescript
type ToolCallDetail =
  | { type: "shell"; command: string; exitCode?: number }
  | { type: "read"; filePath: string }
  | { type: "write"; filePath: string }
  | { type: "edit"; filePath: string }
  | { type: "search"; query: string }
  | { type: "sub_agent"; log: string; actions: string[] }  // ← native subagent
  | { type: "mcp"; serverName: string; toolName: string }
  | { type: "plan"; ... }
  | { type: "unknown" };
```

### 1.5 How Paseo Spawns Claude (The Key Discovery)

**Paseo does NOT use `claude -p`.** It uses the Agent SDK:

```typescript
// packages/server/src/server/agent/providers/claude/query.ts
import { query, type Options, type Query } from "@anthropic-ai/claude-agent-sdk";

export function claudeQuery(input: ClaudeQueryInput): Query {
  return query({
    options: {
      cwd: input.worktree,
      permissionMode: "acceptEdits",
      model: input.model,
      systemPrompt: input.peerPrompt,
      resume: input.sessionId,           // ← resume existing session
      thinking: input.thinkingOption,
      includePartialMessages: true,      // ← streaming!
      mcpServers: input.mcpServers,
      // ...
    },
  });
}
```

The SDK spawns Claude with **stdio pipes** (not PTY):
```typescript
spawnClaudeCodeProcess: (spawnOptions) => {
  const child = spawn(command, args, {
    cwd: spawnOptions.cwd,
    stdio: ["pipe", "pipe", "pipe"],  // ← structured stream
    shell: false,
  });
  return child;
},
```

**The `Query` object is an async iterable of `SDKMessage`**:
```typescript
for await (const message of query) {
  // message is typed SDKMessage
  // types: "system" | "user" | "assistant" | "stream_event" | "result" | "tool_progress"
  const events = translateMessageToEvents(message);
  agentManager.dispatch(agentId, events);
}
```

### 1.6 Native Claude Subagent Tracking

Claude Code announces subagent lifecycle on the SDK stream:
```
task_started    → subagent created
task_progress   → subagent working
task_updated    → subagent state changed
task_notification → subagent completed/failed
```

Paseo reads these via `ClaudeTaskProtocolSource` + `ClaudeSidechainTracker`:

```typescript
class ClaudeSidechainTracker {
  private activeSidechains = new Map<string, SubAgentActivityState>();

  handleMessage(message: SDKMessage, parentToolUseId: string): AgentStreamEvent[] {
    // 1. Extract action candidates from assistant messages
    // 2. Update sub-agent context from Task tool input
    // 3. Emit provider_subagent events

    return [
      {
        type: "provider_subagent",
        provider: "claude",
        event: {
          type: "upsert",
          id: parentToolUseId,
          title: state.name ?? "Claude subagent",
          status: "running",
          toolCallId: parentToolUseId,
        },
      },
      {
        type: "provider_subagent",
        provider: "claude",
        event: {
          type: "timeline",
          id: parentToolUseId,
          item: { type: "tool_call", detail: { type: "sub_agent", ... } },
        },
      },
    ];
  }
}
```

**Two-tier subagent model**:
- **Managed Paseo subagents**: Created via `create_agent`, tracked by `AgentManager`, full lifecycle
- **Native Claude subagents**: Created by Claude's Task tool, tracked by `ClaudeSidechainTracker`, read-only timeline

### 1.7 Workspace Isolation

```
Project (codebase identity)
    │
    ├── Workspace A (cwd: /worktrees/feat-login)
    │   ├── Agent 1 (Engineer)
    │   └── Agent 2 (Reviewer)
    │
    └── Workspace B (cwd: /worktrees/feat-auth)
        └── Agent 3 (Architect)
```

**Key rule**: Workspace placement ≠ parentage. Passing `workspaceId` only changes where the agent works, not its hierarchical relationship.

**Worktree modes**:
- `branch-off`: New branch from HEAD
- `checkout-branch`: Existing branch
- `checkout-pr`: PR branch from forge

**Auto-reconciliation**: `WorkspaceReconciliationService` auto-archives workspaces whose directories no longer exist on disk.

### 1.8 Notification Patterns

| Pattern | Behavior | Use Case |
|---------|----------|----------|
| `notifyOnFinish` | Parent notified when child completes | Lead monitoring Peers |
| `attention_required` | UI alert (finished/error/permission) | Human oversight |
| Heartbeat | Cron prompt into same conversation | Long-running monitoring |
| Schedule | Create new agent per cron tick | Periodic tasks |
| Loop | Worker-Verifier iteration | Retry-until-success |

**Anti-polling principle**: Agents use `notifyOnFinish` instead of polling status. The calling agent receives a notification when the delegated agent completes, errors, or needs permission.

---

## Part 2: Gap Analysis — Paseo vs anti_subagent

### 2.1 What Paseo Has That anti_subagent Lacks

| Capability | Paseo | anti_subagent | Gap |
|------------|-------|---------------|-----|
| Agent identity ≠ process | ✅ AgentRecord with persistence | ❌ PeerManager tracks Child process | **Critical** |
| Structured event stream | ✅ AgentStreamEvent from SDK | ❌ `claude -p` one-shot JSON | **Critical** |
| Session resume | ✅ `resume` via AgentPersistenceHandle | ❌ No resume support | **Critical** |
| Native subagent tracking | ✅ ClaudeSidechainTracker | ❌ Not implemented | **High** |
| Thinking/reasoning capture | ✅ `reasoning` timeline items | ❌ Not captured | **High** |
| Event-driven orchestration | ✅ `notifyOnFinish` + subscriber | ⚠️ Reaper thread polling | **High** |
| Workspace as entity | ✅ Workspace with auto-reconciliation | ⚠️ CAS + pool, no entity model | **Medium** |
| Provider abstraction | ✅ AgentClient/AgentSession + ACP | ⚠️ HarnessAdapter trait | **Medium** |
| Follow-up to existing agent | ✅ `send_agent_prompt(agentId, ...)` | ❌ Must spawn new process | **High** |
| Agent archival | ✅ Soft-delete with cascade | ❌ Not implemented | **Medium** |
| Permission handling | ✅ Interactive permission flow | ⚠️ `--dangerously-skip-permissions` | **Low** |
| MCP as control plane | ✅ AgentMcpServer tool catalog | ⚠️ TCP IPC | **Medium** |

### 2.2 What anti_subagent Has That Paseo Lacks

| Capability | anti_subagent | Paseo | Gap |
|------------|---------------|-------|-----|
| SLP governance model | ✅ Supervisor → Lead → Peer | ❌ Flat agent hierarchy | **Unique** |
| Hidden hierarchy | ✅ Peer doesn't know it's a peer | ❌ Parent/child visible | **Unique** |
| Adversarial council | ✅ Lead doesn't implement, only delegates | ❌ No such pattern | **Unique** |
| Experience handoff | ✅ Lead degradation → new Lead | ⚠️ Basic resume only | **Unique** |
| On-demand Supervisor | ✅ Pull up when needed, no heartbeat | ❌ No supervisor concept | **Unique** |
| Provider-agnostic governance | ✅ Governance layer separate from runtime | ❌ Tightly coupled | **Unique** |
| Disposition system | ✅ Engineer/Architect/Reviewer/Scout | ⚠️ Basic role labels | **Unique** |

### 2.3 The Architecture Gap

```
anti_subagent currently:

PeerManager
    │
    ├── children: HashMap<PeerId, Child>    ← process-centric
    ├── session_ids: HashMap<PeerId, String> ← if added
    └── spawn/terminate/reap

anti_subagent should be:

AgentManager
    │
    ├── agents: HashMap<AgentId, AgentRecord>  ← identity-centric
    │
    ├── AgentRecord
    │   ├── agent_id
    │   ├── parent_id (SLP hierarchy)
    │   ├── role (disposition)
    │   ├── provider + model
    │   ├── workspace_id
    │   ├── runtime: RuntimeState (pid, process)
    │   ├── conversation: ConversationState (session_id, resume_handle)
    │   ├── status: AgentStatus
    │   └── timeline: Vec<TimelineEvent>
    │
    ├── Event Bus
    │   ├── AgentStarted
    │   ├── AgentCompleted
    │   ├── AgentFailed
    │   ├── AgentBlocked
    │   ├── AgentHandoffRequested
    │   └── SubagentDiscovered
    │
    └── Operations
        ├── spawn()
        ├── send()
        ├── interrupt()
        ├── resume()
        ├── archive()
        └── subscribe()
```

---

## Part 3: Implementation Plan

### Phase 0: Pre-assigned Session IDs (P0 Workaround)

**Goal**: Immediate improvement without architectural changes.

**Changes**:
1. Add `uuid` crate to `anti-adapters`
2. Generate session ID before spawning peer
3. Pass `--session-id` flag to `claude -p`
4. Store mapping in `PeerManager`

```rust
// crates/anti-adapters/src/lib.rs
pub struct SpawnContext {
    pub worktree: PathBuf,
    pub task: Option<String>,
    pub peer_prompt: Option<String>,
    pub session_id: Option<String>,  // ← ADD
}

fn spawn_command(&self, ctx: &SpawnContext) -> Result<Command, AdapterError> {
    let mut cmd = Command::new("claude");
    let mut args = vec!["-p".to_string(), "--output-format".to_string(), "json".to_string()];

    if let Some(session_id) = &ctx.session_id {
        args.push("--session-id".to_string());
        args.push(session_id.clone());
    }

    cmd.args(args);
    // ...
}
```

```rust
// crates/anti-daemon/src/peer_manager.rs
pub struct PeerManager {
    children: HashMap<String, Child>,
    session_ids: HashMap<String, String>,  // ← ADD: peer_id → session_id
}
```

**Effort**: 1-2 hours
**Risk**: Low
**Value**: Enables session resume via `--resume <uuid>`

---

### Phase 1: Agent Identity Layer (P1 Critical)

**Goal**: Decouple agent identity from process.

**New crate**: `anti-agent` (or extend `anti-core`)

```rust
// crates/anti-core/src/agent.rs (NEW)

/// Unique agent identifier — survives process death
pub struct AgentId(pub String);

/// Agent record — the source of truth for agent state
pub struct AgentRecord {
    pub id: AgentId,
    pub parent_id: Option<AgentId>,
    pub role: AgentRole,           // Engineer, Architect, Reviewer, etc.
    pub provider: Provider,
    pub model: Model,
    pub workspace_id: WorkspaceId,

    // Runtime (ephemeral)
    pub runtime: Option<RuntimeState>,

    // Conversation (persisted)
    pub conversation: ConversationState,

    // State
    pub status: AgentStatus,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

pub struct RuntimeState {
    pub pid: u32,
    pub process: Option<Child>,
}

pub struct ConversationState {
    pub session_id: Option<String>,
    pub resume_handle: Option<serde_json::Value>,
}

pub enum AgentStatus {
    Initializing,
    Idle,
    Running,
    Error { message: String },
    Closed,        // process dead, but resumable
    Archived,      // soft-deleted
}
```

**AgentManager**:
```rust
// crates/anti-daemon/src/agent_manager.rs (NEW)

pub struct AgentManager {
    agents: HashMap<AgentId, AgentRecord>,
    storage: AgentStorage,
    event_bus: broadcast::Sender<AgentEvent>,
}

impl AgentManager {
    pub async fn spawn(&mut self, request: SpawnRequest) -> Result<AgentId, AgentError>;
    pub async fn send(&mut self, agent_id: &AgentId, prompt: &str) -> Result<(), AgentError>;
    pub async fn interrupt(&mut self, agent_id: &AgentId) -> Result<(), AgentError>;
    pub async fn resume(&mut self, agent_id: &AgentId) -> Result<(), AgentError>;
    pub async fn archive(&mut self, agent_id: &AgentId) -> Result<(), AgentError>;
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent>;
}
```

**Persistence** (JSON files like Paseo):
```
~/.anti-subagent/
  agents/
    {workspace-id}/
      {agent-id}.json
```

**Effort**: 3-5 days
**Risk**: Medium
**Value**: Foundation for all subsequent phases

---

### Phase 2: Event-Driven Orchestration (P2 High)

**Goal**: Replace polling with event notifications.

```rust
// crates/anti-core/src/events.rs (EXTEND)

pub enum AgentEvent {
    // Lifecycle
    AgentSpawned { agent_id: AgentId, parent_id: Option<AgentId> },
    AgentCompleted { agent_id: AgentId, exit_code: Option<i32> },
    AgentFailed { agent_id: AgentId, error: String },
    AgentArchived { agent_id: AgentId },

    // Content (from structured stream)
    AgentThinking { agent_id: AgentId, content: String },
    AgentToolCall { agent_id: AgentId, tool: String, status: ToolStatus },
    AgentMessage { agent_id: AgentId, content: String },

    // Subagent (from Claude's native protocol)
    SubagentStarted { parent_id: AgentId, child_id: String, name: String },
    SubagentProgress { parent_id: AgentId, child_id: String, progress: String },
    SubagentCompleted { parent_id: AgentId, child_id: String },

    // SLP-specific
    HandoffRequested { from: AgentId, to: AgentId, reason: String },
    CouncilVerdict { agent_id: AgentId, verdict: Verdict },
}
```

**`notifyOnFinish` pattern**:
```rust
impl AgentManager {
    pub async fn spawn_with_notification(
        &mut self,
        request: SpawnRequest,
        notify_parent: Option<AgentId>,
    ) -> Result<AgentId, AgentError> {
        let agent_id = self.spawn(request).await?;

        if let Some(parent_id) = notify_parent {
            // When this agent completes, send event to parent
            self.register_notification(&agent_id, &parent_id);
        }

        Ok(agent_id)
    }
}
```

**Lead subscription**:
```rust
// Lead subscribes to peer events
let mut events = agent_manager.subscribe();
while let Ok(event) = events.recv().await {
    match event {
        AgentEvent::AgentCompleted { agent_id, .. } => {
            // Handle peer completion
        }
        AgentEvent::AgentFailed { agent_id, error } => {
            // Handle peer failure
        }
        AgentEvent::SubagentStarted { parent_id, child_id, .. } => {
            // Track native Claude subagents
        }
        _ => {}
    }
}
```

**Effort**: 3-5 days
**Risk**: Medium
**Value**: Eliminates context-wasting polling, enables real-time monitoring

---

### Phase 3: Agent SDK Adapter (P3 High)

**Goal**: Replace `claude -p` with Agent SDK for structured event streaming.

**Option A: Rust FFI to Agent SDK** (experimental)
```rust
// crates/anti-adapters/src/claude_sdk.rs (NEW)

use std::process::Stdio;
use tokio::process::Command;

pub struct ClaudeSdkAdapter {
    python_process: Child,
    event_rx: mpsc::Receiver<ClaudeEvent>,
}

impl ClaudeSdkAdapter {
    pub async fn spawn(config: ClaudeConfig) -> Result<Self, AdapterError> {
        // Spawn Python process running Agent SDK
        let mut cmd = Command::new("python3");
        cmd.arg("-c");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());

        // Send spawn request via stdin JSON
        // Receive events via stdout JSONL
    }
}
```

**Option B: Direct SDK binary** (if Rust SDK becomes available)
```rust
// When Anthropic releases Rust SDK
use claude_agent_sdk::{query, Options};

let mut session = query(Options {
    cwd: worktree,
    resume: Some(session_id),
    include_partial_messages: true,
    // ...
}).await?;

while let Some(message) = session.next().await {
    let events = translate_sdk_message(message);
    event_bus.dispatch(agent_id, events);
}
```

**Option C: WebSocket proxy** (Paseo-style daemon)
```
anti_subagent daemon
    │
    ├── Paseo daemon (or custom WebSocket server)
    │   └── Claude Agent SDK adapter
    │
    └── anti_subagent subscribes to events
```

**Effort**: 5-10 days (Option A/B), 2-3 days (Option C)
**Risk**: High (Option A/B), Medium (Option C)
**Value**: Full structured event stream, native subagent tracking, thinking capture

---

### Phase 4: Follow-up & Long-lived Agents (P4 High)

**Goal**: Send follow-ups to existing agents instead of spawning new ones.

```rust
impl AgentManager {
    /// Send follow-up to an existing agent (no new process)
    pub async fn send_followup(
        &mut self,
        agent_id: &AgentId,
        prompt: &str,
        background: bool,
    ) -> Result<(), AgentError> {
        let agent = self.agents.get_mut(agent_id)?;

        // Ensure agent is alive
        if agent.status == AgentStatus::Closed {
            self.resume(agent_id).await?;
        }

        // Send prompt to existing Claude session
        let runtime = agent.runtime.as_mut()?;
        runtime.send_prompt(prompt)?;

        if !background {
            // Wait for completion
            self.wait_for_completion(agent_id).await?;
        }

        Ok(())
    }
}
```

**SLP integration**:
```
Lead
 │
 └── Peer A (AgentId: peer-a)
       │
       ├── task 1 (spawn)
       │   └── AgentCompleted
       │
       ├── task 2 (follow-up, same agent)
       │   └── AgentCompleted
       │
       ├── task 3 (follow-up, same agent)
       │   └── AgentCompleted
       │
       └── archive
```

**Effort**: 3-5 days
**Risk**: Medium
**Value**: No process restart overhead, conversation continuity

---

### Phase 5: Native Claude Subagent Tracking (P5 Medium)

**Goal**: Track Claude's native Task tool subagents without Paseo.

```rust
// crates/anti-core/src/subagent_tracker.rs (NEW)

/// Tracks native Claude subagents (Task tool children)
pub struct SubagentTracker {
    active: HashMap<String, SubagentState>,
}

pub struct SubagentState {
    pub parent_tool_use_id: String,
    pub name: Option<String>,
    pub sub_agent_type: Option<String>,
    pub status: SubagentStatus,
    pub timeline: Vec<TimelineEvent>,
}

pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

impl SubagentTracker {
    /// Parse Claude's task_* events from SDK stream
    pub fn handle_sdk_message(&mut self, message: &SdkMessage) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        match message {
            SdkMessage::TaskStarted { id, name, .. } => {
                self.active.insert(id.clone(), SubagentState {
                    parent_tool_use_id: id.clone(),
                    name: name.clone(),
                    status: SubagentStatus::Running,
                    timeline: Vec::new(),
                });
                events.push(AgentEvent::SubagentStarted {
                    parent_id: /* parent */,
                    child_id: id.clone(),
                    name: name.unwrap_or_default(),
                });
            }
            SdkMessage::TaskNotification { id, status, .. } => {
                if let Some(state) = self.active.get_mut(id) {
                    state.status = match status.as_str() {
                        "completed" => SubagentStatus::Completed,
                        "failed" => SubagentStatus::Failed,
                        _ => SubagentStatus::Canceled,
                    };
                }
                events.push(AgentEvent::SubagentCompleted {
                    parent_id: /* parent */,
                    child_id: id.clone(),
                });
            }
            _ => {}
        }

        events
    }
}
```

**Effort**: 2-3 days
**Risk**: Low
**Value**: Visibility into Claude's native subagent activity

---

### Phase 6: SLP Governance Layer (P6 Unique)

**Goal**: Implement Supervisor → Lead → Peer governance on top of the new control plane.

```rust
// crates/anti-core/src/governance.rs (NEW)

/// SLP hierarchy — invisible to Peers
pub enum SlpRole {
    Supervisor {
        memory_notebook: Notebook,
        optimization_rules: Vec<OptimizationRule>,
    },
    Lead {
        workspace: WorkspaceId,
        council: CouncilProtocol,
        max_compactions: usize,
    },
    Peer {
        disposition: Disposition,  // Engineer, Architect, Reviewer, Scout, etc.
        // Peer doesn't know it's a Peer
    },
}

/// Council protocol for Lead decisions
pub struct CouncilProtocol {
    pub engineer: AgentId,
    pub reviewer: AgentId,
    pub architect: Option<AgentId>,  // only on hard problems
}

impl CouncilProtocol {
    pub async fn deliberate(&self, propositions: Vec<Proposition>) -> Verdict {
        // 1. Extract 3-5 material propositions
        // 2. Verify only decision-changing claims
        // 3. Allow at most one challenge per proposition
        // 4. Issue binding verdict
        // Provider count creates no authority
    }
}

/// Experience handoff when Lead degrades
pub struct ExperienceHandoff {
    pub from: AgentId,
    pub to: AgentId,
    pub lessons: Vec<Lesson>,
    pub timeline_snapshot: Vec<TimelineEvent>,
}

impl ExperienceHandoff {
    pub async fn execute(self, agent_manager: &mut AgentManager) -> Result<(), HandoffError> {
        // 1. Archive old Lead
        agent_manager.archive(&self.from).await?;

        // 2. Create new Lead
        let new_id = agent_manager.spawn(SpawnRequest {
            role: SlpRole::Lead { .. },
            // ...
        }).await?;

        // 3. Transfer lessons as initial context
        agent_manager.send_followup(&new_id, &self.lessons_to_prompt(), false).await?;

        Ok(())
    }
}
```

**Effort**: 5-8 days
**Risk**: Medium
**Value**: Unique differentiator — Paseo doesn't have this

---

## Part 4: Migration Strategy

### 4.1 Incremental Migration (Not Big Bang)

```
Phase 0 (Day 1)     → Pre-assigned session IDs
Phase 1 (Week 1)    → Agent identity layer
Phase 2 (Week 2)    → Event-driven orchestration
Phase 3 (Week 3-4)  → Agent SDK adapter
Phase 4 (Week 4-5)  → Follow-up support
Phase 5 (Week 5-6)  → Native subagent tracking
Phase 6 (Week 6-8)  → SLP governance layer
```

### 4.2 Backward Compatibility

During migration, maintain both old and new paths:

```rust
pub enum PeerSpawnMode {
    Legacy(LegacyPeerManager),       // current claude -p approach
    Modern(AgentManager),            // new Agent SDK approach
}

impl PeerSpawnMode {
    pub async fn spawn(&self, request: SpawnRequest) -> Result<AgentId, AgentError> {
        match self {
            Self::Legacy(mgr) => mgr.spawn_legacy(request),  // claude -p
            Self::Modern(mgr) => mgr.spawn(request),         // Agent SDK
        }
    }
}
```

### 4.3 Feature Flags

```toml
# crates/anti-daemon/Cargo.toml
[features]
default = ["legacy-spawn"]
legacy-spawn = []           # Keep claude -p path
agent-identity = []         # Phase 1: AgentRecord
event-driven = []           # Phase 2: Event bus
agent-sdk = []              # Phase 3: Agent SDK adapter
follow-up = []              # Phase 4: send_followup
subagent-tracking = []      # Phase 5: Native subagent events
slp-governance = []         # Phase 6: Supervisor/Lead/Peer
```

---

## Part 5: Comparison Matrix — Final

| Feature | anti_subagent (Current) | anti_subagent (Target) | Paseo |
|---------|------------------------|------------------------|-------|
| Agent identity | ❌ Process-based | ✅ AgentRecord | ✅ AgentRecord |
| Event stream | ❌ One-shot JSON | ✅ Structured SDK | ✅ Structured SDK |
| Session resume | ❌ No | ✅ Yes | ✅ Yes |
| Follow-up | ❌ Spawn new | ✅ Send to existing | ✅ Send to existing |
| Native subagent tracking | ❌ No | ✅ Yes | ✅ Yes |
| Thinking/reasoning | ❌ Not captured | ✅ Captured | ✅ Captured |
| Event-driven orchestration | ⚠️ Polling | ✅ notifyOnFinish | ✅ notifyOnFinish |
| SLP governance | ✅ (concept) | ✅ (implemented) | ❌ |
| Hidden hierarchy | ✅ (concept) | ✅ (implemented) | ❌ |
| Adversarial council | ✅ (concept) | ✅ (implemented) | ❌ |
| Experience handoff | ✅ (concept) | ✅ (implemented) | ❌ |
| On-demand Supervisor | ✅ (concept) | ✅ (implemented) | ❌ |
| Multi-provider | ⚠️ Basic adapter | ✅ AgentClient trait | ✅ ACP + Direct |

---

## Part 6: Key Takeaways

1. **`claude -p` is a dead end for SLP**. It's a one-shot tool with no session tracking, no structured events, and no follow-up support. The Agent SDK is the correct abstraction.

2. **Agent identity ≠ process ≠ session**. This is the fundamental insight from Paseo. anti_subagent should adopt this separation immediately.

3. **Event-driven > polling**. The `notifyOnFinish` pattern eliminates context-wasting polling and enables real-time monitoring.

4. **Paseo's control plane + anti_subagent's governance = best of both worlds**. Paseo has the runtime infrastructure; anti_subagent has the governance thesis. Combine them.

5. **Native Claude subagents are trackable**. Claude's SDK stream includes `task_started`, `task_progress`, `task_notification` events. anti_subagent should leverage these instead of trying to reconstruct subagent state from transcripts.

6. **The PTY wrapper approach is a temporary bridge**. It works today but is inherently fragile. The Agent SDK is the stable, official path forward.

---

## Appendix A: Paseo Source Files Reference

| File | Purpose |
|------|---------|
| `packages/server/src/server/agent/agent-manager.ts` | Central lifecycle management |
| `packages/server/src/server/agent/agent-session.ts` | Provider-agnostic session interface |
| `packages/server/src/server/agent/providers/claude/agent.ts` | Claude adapter implementation |
| `packages/server/src/server/agent/providers/claude/query.ts` | Claude SDK query wrapper |
| `packages/server/src/server/agent/providers/claude/sidechain-tracker.ts` | Native subagent tracking |
| `packages/server/src/server/agent/providers/claude/task-notification-tool-call.ts` | Task notification parsing |
| `packages/server/src/server/agent/provider-subagents/store.ts` | Provider subagent persistence |
| `packages/server/src/server/agent/agent-storage.ts` | JSON file persistence |
| `packages/server/src/server/agent/workspace-manager.ts` | Workspace lifecycle |
| `packages/server/src/server/mcp/tools/` | MCP tool catalog (control plane) |

## Appendix B: Anti_subagent Files to Modify

| File | Phase | Change |
|------|-------|--------|
| `crates/anti-core/src/lib.rs` | P1 | Add `agent.rs` module |
| `crates/anti-core/src/agent.rs` | P1 | NEW: AgentRecord, AgentStatus, AgentId |
| `crates/anti-core/src/events.rs` | P2 | Extend with AgentEvent enum |
| `crates/anti-daemon/src/agent_manager.rs` | P1 | NEW: AgentManager |
| `crates/anti-daemon/src/peer_manager.rs` | P0 | Add session_ids HashMap |
| `crates/anti-daemon/src/recovery.rs` | P1 | Use AgentRecord for recovery |
| `crates/anti-adapters/src/lib.rs` | P0 | Add session_id to SpawnContext |
| `crates/anti-adapters/src/claude_sdk.rs` | P3 | NEW: Agent SDK adapter |
| `crates/anti-core/src/subagent_tracker.rs` | P5 | NEW: Native subagent tracking |
| `crates/anti-core/src/governance.rs` | P6 | NEW: SLP governance layer |

---

**Next Steps**:
1. Review this report with team
2. Approve Phase 0 (pre-assigned session IDs) — can ship today
3. Begin Phase 1 (Agent Identity Layer) design review
4. Start Phase 3 (Agent SDK Adapter) spike — highest risk, highest value
