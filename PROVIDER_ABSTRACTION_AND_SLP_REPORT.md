# Provider Abstraction + SLP Hidden Hierarchy Report

**Date**: 2026-08-19
**Status**: Research Complete → Architecture Ready
**Purpose**: Unified provider abstraction (Claude/Codex/OpenCode) + SLP information boundary design

---

## Executive Summary

This report derives a **unified `AgentProvider` trait** for Rust from Paseo's 3 provider adapters (Claude, Codex, OpenCode), then layers the **SLP hidden hierarchy** on top.

**Key insight**: Paseo solves observability. anti_subagent needs observability **plus** information hiding. The control plane knows the full topology; the agent only sees what it's permitted to know.

**Acceptance criteria**:
> Can spawn and manage Claude/Codex/OpenCode agents with full observability at the control plane, while the provider-side agent cannot reliably infer that it is a child/subagent of another agent.

---

## Part 1: Three Providers — Transport Comparison

### 1.1 How Each Provider Spawns

| Provider | Process | Transport | Protocol |
|----------|---------|-----------|----------|
| **Claude** | `claude` CLI via Agent SDK `query()` | stdio pipes | SDK async iterable (`SDKMessage`) |
| **Codex** | `codex app-server` | stdio pipes | JSON-RPC 2.0 (newline-delimited) |
| **OpenCode** | `opencode serve --port N` | HTTP + SSE | REST API + Server-Sent Events |

### 1.2 Event Models

#### Claude SDK Stream

```
SDKMessage (discriminated union on `type`):
  ├── system          → session init
  ├── user            → user input / task notification
  ├── assistant       → full assistant response (content blocks)
  ├── stream_event    → incremental deltas (content_block_start/delta/stop)
  ├── result          → final result (usage, cost)
  └── tool_progress   → tool execution progress

ContentBlock types:
  ├── text / text_delta
  ├── thinking / thinking_delta        ← reasoning
  ├── tool_use / mcp_tool_use
  ├── tool_result
  └── image
```

#### Codex JSON-RPC 2.0

```
Notifications (server → client):
  ├── thread/started
  ├── turn/started
  ├── turn/completed
  ├── item/started
  ├── item/reasoning/summaryTextDelta  ← reasoning
  ├── item/completed
  └── item/commandApproval/request     ← permission

Thread Items:
  ├── userMessage
  ├── agentMessage
  ├── reasoning
  ├── commandExecution   → shell tool
  ├── fileChange         → apply_patch tool
  ├── mcpToolCall
  ├── webSearch
  ├── subAgentActivity   ← native subagent
  └── contextCompaction
```

#### OpenCode SSE Events

```
GlobalEvent { directory, payload: { id, type, properties } }:
  ├── session.created / updated / deleted
  ├── session.status (idle / busy / retry)
  ├── session.error
  ├── session.compacted
  ├── message.updated
  ├── message.part.updated (type: text | reasoning | tool | step-finish | ...)
  ├── message.part.delta (field: text | reasoning)
  ├── permission.asked / replied
  └── todo.updated

Parts:
  ├── TextPart        → assistant text
  ├── ReasoningPart   → chain-of-thought
  ├── ToolPart        → tool invocation (ToolState: pending → running → completed/error)
  ├── StepFinishPart  → token/cost per step
  └── CompactionPart
```

### 1.3 Normalized Event Map

All three providers map to the same abstract events:

```
Provider-specific              →  Normalized AgentEvent
─────────────────────────────────────────────────────
Claude: assistant message      ─┐
Codex: agentMessage            ─┼→  AssistantMessage { text, messageId }
OpenCode: message.part.text    ─┘

Claude: thinking block         ─┐
Codex: item/reasoning          ─┼→  Reasoning { text }
OpenCode: ReasoningPart        ─┘

Claude: tool_use               ─┐
Codex: commandExecution/       ─┼→  ToolCall { callId, name, status, detail }
  fileChange/mcpToolCall       ─┤
OpenCode: ToolPart             ─┘

Claude: task_started/progress  ─┐
Codex: subAgentActivity        ─┼→  SubagentEvent { id, title, status }
OpenCode: session.created      ─┘
  (with parentID)

Claude: result (usage)         ─┐
Codex: turn/completed          ─┼→  TurnCompleted { usage }
OpenCode: session.idle         ─┘

Claude: turn_canceled          ─┐
Codex: turn/interrupt ack      ─┼→  TurnCanceled { reason }
OpenCode: MessageAbortedError  ─┘
```

### 1.4 Permission Models

| Provider | Mechanism | Modes |
|----------|-----------|-------|
| **Claude** | SDK `canUseTool` callback | default, auto, acceptEdits, plan, bypassPermissions |
| **Codex** | Server-initiated JSON-RPC request (`item/commandApproval/request`) | on-request, never, untrusted |
| **OpenCode** | SSE `permission.asked` event | ask, allow, deny (per-tool rules) |

### 1.5 Session Resume

| Provider | What's Persisted | Resume Mechanism |
|----------|------------------|------------------|
| **Claude** | `claudeSessionId` + transcript in `~/.claude/projects/` | `resume: sessionId` in SDK options |
| **Codex** | Thread ID + conversation history in Codex storage | `thread/resume` JSON-RPC call |
| **OpenCode** | Session ID + messages in OpenCode storage | Reconnect to server with existing `sessionId` |

### 1.6 Native Subagent Protocol

| Provider | Detection | Identity | Events |
|----------|-----------|----------|--------|
| **Claude** | `parent_tool_use_id` in SDK messages + Task/Agent tool name | `parentToolUseId` | task_started, task_progress, task_notification |
| **Codex** | `subAgentActivity` thread items (≥0.143) | `agentThreadId` | started, interacted, interrupted |
| **OpenCode** | `session.created` with `parentID` set | `session.id` | session lifecycle events |

---

## Part 2: Derived Rust Abstraction

### 2.1 Core Traits

```rust
// crates/anti-core/src/provider.rs

use async_trait::async_trait;
use tokio::sync::mpsc;

/// Provider identity
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum ProviderKind {
    Claude,
    Codex,
    OpenCode,
}

/// Capability flags — what this provider supports
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_session_resume: bool,
    pub supports_reasoning: bool,
    pub supports_native_subagents: bool,
    pub supports_mcp: bool,
    pub supports_interrupt: bool,
    pub supports_permission_handling: bool,
    pub supports_followup: bool,       // send to existing agent
    pub supports_thinking_config: bool, // set thinking level
}

/// Factory — creates sessions for a provider
#[async_trait]
pub trait AgentClient: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> ProviderCapabilities;

    async fn create_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, ProviderError>;
    async fn resume_session(&self, handle: &PersistenceHandle) -> Result<Box<dyn AgentSession>, ProviderError>;
    async fn is_available(&self) -> bool;
}

/// Session — one conversation thread with a provider
#[async_trait]
pub trait AgentSession: Send + Sync {
    fn provider(&self) -> ProviderKind;
    fn session_id(&self) -> &str;

    /// Send a prompt and receive a stream of normalized events
    async fn send(&mut self, prompt: &str) -> Result<mpsc::Receiver<AgentEvent>, ProviderError>;

    /// Send follow-up to existing session (no new process)
    async fn followup(&mut self, prompt: &str) -> Result<mpsc::Receiver<AgentEvent>, ProviderError>;

    /// Interrupt the current turn
    async fn interrupt(&mut self) -> Result<(), ProviderError>;

    /// Close the session (release process, keep identity)
    async fn close(&mut self) -> Result<(), ProviderError>;

    /// Get persistence handle for resume
    fn persistence_handle(&self) -> Option<PersistenceHandle>;

    /// Subscribe to raw provider events (for observability)
    fn subscribe(&self) -> mpsc::Receiver<AgentEvent>;
}
```

### 2.2 Normalized Event Types

```rust
// crates/anti-core/src/events.rs

/// Normalized agent event — provider-agnostic
#[derive(Debug, Clone)]
pub enum AgentEvent {
    // ─── Content ───
    AssistantMessage {
        text: String,
        message_id: Option<String>,
    },
    Reasoning {
        text: String,
    },

    // ─── Tool Calls ───
    ToolCall {
        call_id: String,
        name: String,
        status: ToolStatus,
        detail: ToolDetail,
        error: Option<String>,
    },

    // ─── Turn Lifecycle ───
    TurnStarted {
        turn_id: Option<String>,
    },
    TurnCompleted {
        usage: Option<Usage>,
    },
    TurnFailed {
        error: String,
    },
    TurnCanceled {
        reason: String,
    },

    // ─── Subagent (from native protocol) ───
    SubagentStarted {
        id: String,
        title: Option<String>,
        parent_tool_use_id: Option<String>,
    },
    SubagentProgress {
        id: String,
        timeline_item: Box<TimelineItem>,
    },
    SubagentCompleted {
        id: String,
        status: SubagentStatus,
    },

    // ─── Permissions ───
    PermissionRequested {
        request: PermissionRequest,
    },
    PermissionResolved {
        request_id: String,
        resolution: PermissionResolution,
    },

    // ─── System ───
    UsageUpdated {
        usage: Usage,
    },
    Compaction {
        status: CompactionStatus,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub enum ToolDetail {
    Shell { command: String, exit_code: Option<i32> },
    Read { file_path: String },
    Write { file_path: String },
    Edit { file_path: String },
    Search { query: String },
    Fetch { url: String },
    Mcp { server: String, tool: String },
    SubAgent { description: String, child_id: Option<String> },
    Plan { content: String },
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub id: String,
    pub provider: ProviderKind,
    pub name: String,
    pub kind: PermissionKind,
    pub title: String,
    pub description: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum PermissionKind {
    Tool,
    Question,
    Plan,
}

#[derive(Debug, Clone)]
pub struct PersistenceHandle {
    pub provider: ProviderKind,
    pub session_id: String,
    pub native_handle: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}
```

### 2.3 Tool Name Normalization

Each provider maps tool names to a common set:

```
Claude tool name        Codex item type          OpenCode tool name     → Normalized
─────────────────────────────────────────────────────────────────────────────────────
Bash, shell             commandExecution         shell, bash            → Shell
Read, read_file         (none — file in msg)     read, read_file        → Read
Write, write_file       (none)                   write, write_file      → Write
Edit, MultiEdit         fileChange               edit, apply_patch      → Edit
Grep, Glob, WebSearch   webSearch                grep, search, glob     → Search
WebFetch                (none)                   (none)                 → Fetch
Task, Agent             subAgentActivity         task                   → SubAgent
(unknown)               mcpToolCall              (tool name)            → Mcp
```

---

## Part 3: SLP Information Boundary

### 3.1 The Problem

Paseo's `AgentRecord` has:
```json
{
  "labels": { "paseo.parent-agent-id": "agent-456" }
}
```

This is fine for Paseo. But for SLP, **the agent must not know it has a parent**.

### 3.2 Threat Model — How an Agent Could Infer Hierarchy

| Vector | Risk | Mitigation |
|--------|------|------------|
| `parentAgentId` in session metadata | Direct leak | Filter from context |
| System prompt mentioning "Lead" or "Supervisor" | Role inference | Strip orchestration terms |
| Environment variables (`SLP_PARENT`, etc.) | Direct leak | Don't inject |
| CLI arguments (`--parent-id`) | Direct leak | Don't pass |
| Process ancestry (PPID chain) | Indirect | Run in isolated process group |
| Workspace path (`/worktrees/peer-a-of-lead`) | Indirect | Use neutral paths (`/worktrees/w-abc123`) |
| MCP server metadata | Direct leak | Don't expose control plane MCP to agents |
| Session resume metadata | Direct leak | Strip from persistence handle |
| Tool descriptions mentioning orchestration | Indirect | Filter tool definitions |
| Injected `<paseo-system>` messages | Direct leak | Don't use system envelopes |
| Provider-native task metadata | Direct leak | Filter SDK messages |
| Timing/pattern analysis | Indirect | Accept residual risk |

### 3.3 Information Filter Architecture

```text
                 SLP CONTROL PLANE
                        │
        ┌───────────────┴────────────────┐
        │                                │
 Internal Metadata                Provider Runtime
        │                                │
 parent_agent_id                   Claude SDK
 supervisor_id                    Codex App Server
 spawn_reason                     OpenCode Server
 governance_state                       │
 handoff_context                        │
        │                                │
        └───────────────┬────────────────┘
                        │
              ┌─────────▼─────────┐
              │  INFO FILTER      │
              │                   │
              │  • Strip parent   │
              │  • Strip gov.     │
              │  • Neutral paths  │
              │  • No env leaks   │
              │  • No CLI leaks   │
              └─────────┬─────────┘
                        │
                        ▼
                 Agent Context
          (task, workspace, prompt)
```

### 3.4 Filtered Context Structure

```rust
// What the control plane knows (internal)
pub struct InternalAgentState {
    pub agent_id: AgentId,
    pub parent_id: Option<AgentId>,        // HIDDEN
    pub supervisor_id: Option<AgentId>,    // HIDDEN
    pub spawn_reason: Option<String>,      // HIDDEN
    pub governance_state: GovernanceState, // HIDDEN
    pub handoff_context: Option<HandoffContext>, // HIDDEN
    pub disposition: Disposition,          // HIDDEN (Lead decides, Peer doesn't know)
    pub role: AgentRole,                   // HIDDEN
    // ...
}

// What the agent actually sees
pub struct AgentContext {
    pub task: String,
    pub workspace: PathBuf,
    pub peer_prompt: Option<String>,
    pub model: String,
    pub thinking_level: Option<String>,
    // NO parent_id
    // NO supervisor_id
    // NO governance_state
    // NO disposition
}
```

### 3.5 Spawn with Information Boundary

```rust
impl AgentManager {
    pub async fn spawn_peer(
        &self,
        request: SpawnRequest,
        parent_id: Option<AgentId>,  // internal only
    ) -> Result<AgentId, AgentError> {

        // 1. Create internal agent record (full metadata)
        let agent_id = AgentId::new();
        let record = AgentRecord {
            id: agent_id.clone(),
            parent_id: Some(parent_id.clone()),  // stored internally
            disposition: request.disposition,
            role: request.role,
            // ...
        };

        // 2. Create filtered context (no hierarchy info)
        let context = AgentContext {
            task: request.task,
            workspace: request.workspace,
            peer_prompt: request.peer_prompt,
            model: request.model,
            thinking_level: request.thinking_level,
        };

        // 3. Spawn provider session with filtered context
        let session = self.providers.get(&request.provider)?
            .create_session(SessionConfig {
                cwd: &context.workspace,
                system_prompt: &context.peer_prompt,
                model: &context.model,
                // NO parent_id
                // NO supervisor metadata
                // NO governance terms
            }).await?;

        // 4. Store full record internally
        self.agents.insert(agent_id.clone(), record);
        self.sessions.insert(agent_id.clone(), session);

        // 5. Emit event to control plane subscribers (not to agent)
        self.event_bus.emit(AgentEvent::AgentSpawned {
            agent_id: agent_id.clone(),
            parent_id: parent_id.clone(),  // only in internal event
        });

        Ok(agent_id)
    }
}
```

### 3.6 Workspace Path Sanitization

```rust
/// Sanitize workspace path to hide orchestration hierarchy
fn sanitize_workspace_path(raw: &Path) -> PathBuf {
    // BAD: /worktrees/lead-peer-engineer-login-feature
    // GOOD: /worktrees/w-abc123

    let hash = compute_short_hash(raw);
    PathBuf::from(format!("/worktrees/w-{}", hash))
}
```

### 3.7 System Prompt Filtering

```rust
/// Strip orchestration terms from system prompt
fn filter_system_prompt(prompt: &str) -> String {
    let forbidden = [
        "supervisor", "lead", "peer", "orchestrator",
        "subagent", "spawned by", "delegated by",
        "parent agent", "child agent", "hierarchy",
        "SLP", "anti-subagent",
    ];

    let mut filtered = prompt.to_string();
    for term in &forbidden {
        filtered = filtered.replace(term, "[redacted]");
    }
    filtered
}
```

---

## Part 4: Two-Tier Agent Model (from Paseo)

### 4.1 Managed vs Provider-Native

Paseo distinguishes:

```
Managed Agent (Paseo-created)
  ├── Full AgentRecord in AgentManager
  ├── parentAgentId tracked internally
  ├── full lifecycle (create, send, interrupt, archive)
  └── visible in agent listings

Provider-Native Subagent (Claude Task / Codex child / OpenCode child)
  ├── NOT in AgentManager as managed agent
  ├── tracked by SidechainTracker / ProviderSubagentStore
  ├── read-only timeline
  └── events emitted as provider_subagent
```

### 4.2 For SLP, This Maps To

```
SLP Managed Agents (control plane owns):
  ├── Supervisor
  ├── Lead
  └── Peers (each is a full managed agent)

Provider-Native Subagents (within a Peer):
  ├── Claude Task subagents (within Peer A)
  ├── Codex child sessions (within Peer B)
  └── OpenCode child sessions (within Peer C)
```

### 4.3 Observability Without Hierarchy Leak

```text
CONTROL PLANE VIEW:

Supervisor
  └── Lead
        ├── Peer A (Engineer)
        │     ├── Claude Task "review code"      ← provider native
        │     └── Claude Task "write tests"      ← provider native
        ├── Peer B (Reviewer)
        │     └── Codex child session             ← provider native
        └── Peer C (Scout)
              └── OpenCode child session          ← provider native

PEER A's VIEW:

"I'm an independent agent working on this task."
- No mention of Lead
- No mention of Supervisor
- No mention of Peer B or Peer C
- Claude Task subagents are visible (they're within my scope)
```

---

## Part 5: Implementation Plan (Revised)

### Phase 0: Agent SDK Spike (P0 — 3 days)

**Goal**: Validate that Agent SDK / Codex App Server / OpenCode Server work from Rust.

```
Spawn each provider as subprocess, receive events, normalize to AgentEvent.

Claude:   spawn "claude" with SDK options via stdin JSON
Codex:    spawn "codex app-server" with JSON-RPC over stdio
OpenCode: spawn "opencode serve --port N" with HTTP+SSE
```

**Exit criteria**: Can spawn one of each, receive `TurnCompleted` event.

### Phase 1: Provider Trait + Normalized Events (P1 — 5 days)

**Goal**: Implement `AgentClient` / `AgentSession` traits + `AgentEvent` enum.

```
crates/
  anti-core/src/provider.rs      → AgentClient, AgentSession traits
  anti-core/src/events.rs        → AgentEvent, ToolDetail, Usage
  anti-providers/                 → NEW crate
    src/claude.rs                → ClaudeAdapter
    src/codex.rs                 → CodexAdapter
    src/opencode.rs              → OpenCodeAdapter
```

**Exit criteria**: Can spawn Claude, receive normalized events including reasoning + tool calls.

### Phase 2: Agent Identity + Control Plane (P2 — 5 days)

**Goal**: `AgentManager` with `AgentRecord`, lifecycle, persistence.

```
AgentRecord {
    agent_id, parent_id, role, provider, workspace_id,
    status, persistence_handle, created_at, last_activity_at
}

AgentManager {
    agents: HashMap<AgentId, AgentRecord>,
    spawn(), send(), interrupt(), resume(), archive(), subscribe()
}
```

**Exit criteria**: Can create agent, send follow-up, resume after close.

### Phase 3: Information Boundary Filter (P3 — 3 days)

**Goal**: Hide hierarchy from agent context.

```
InfoFilter {
    strip_parent_metadata(),
    sanitize_workspace_path(),
    filter_system_prompt(),
    filter_env_vars(),
    filter_cli_args(),
}
```

**Exit criteria**: Spawned agent has no way to reliably infer parent relationship from:
- System prompt
- Environment variables
- CLI arguments
- Workspace path
- Session metadata

### Phase 4: Event-Driven Orchestration (P4 — 3 days)

**Goal**: Replace polling with `notifyOnFinish` pattern.

```
AgentEvent::TurnCompleted → notify parent
AgentEvent::TurnFailed → notify parent
AgentEvent::SubagentStarted → track in SidechainTracker
```

**Exit criteria**: Lead receives notification when Peer completes; no polling loop.

### Phase 5: Native Subagent Tracking (P5 — 3 days)

**Goal**: Track provider-native subagents within Peers.

```
SidechainTracker {
    active: HashMap<String, SubagentState>,
    handle_event(event) → Vec<AgentEvent>,
}
```

**Exit criteria**: Can track Claude Task, Codex child, OpenCode child within a Peer.

### Phase 6: SLP Governance Layer (P6 — 5 days)

**Goal**: Supervisor → Lead → Peer with hidden hierarchy.

```
SlpOrchestrator {
    spawn_supervisor(),
    spawn_lead(),
    spawn_peer(lead_id, disposition),
    experience_handoff(old_lead, new_lead),
    council_deliberation(propositions),
}
```

**Exit criteria**: Full SLP lifecycle with information boundary enforced.

---

## Part 6: Capability Matrix

| Capability | Claude | Codex | OpenCode | anti_subagent target |
|------------|--------|-------|----------|---------------------|
| Streaming | ✅ SDK events | ✅ JSON-RPC notifications | ✅ SSE | ✅ mpsc channel |
| Reasoning | ✅ thinking blocks | ✅ item/reasoning | ✅ ReasoningPart | ✅ AgentEvent::Reasoning |
| Tool calls | ✅ tool_use | ✅ commandExecution/fileChange | ✅ ToolPart | ✅ AgentEvent::ToolCall |
| Session resume | ✅ resume:sessionId | ✅ thread/resume | ✅ reconnect sessionId | ✅ PersistenceHandle |
| Follow-up | ✅ same Query | ✅ turn/start on same thread | ✅ prompt on same session | ✅ send() vs followup() |
| Interrupt | ✅ Query.interrupt() | ✅ turn/interrupt | ✅ session.abort() | ✅ AgentSession::interrupt() |
| Native subagents | ✅ Task protocol | ✅ subAgentActivity | ✅ child sessions | ✅ SidechainTracker |
| Permission | ✅ canUseTool callback | ✅ commandApproval/request | ✅ permission.asked | ✅ PermissionRequest |
| MCP | ✅ mcpServers option | ✅ config injection | ✅ mcp.add (dynamic) | ✅ SessionConfig::mcp |
| Hidden hierarchy | ❌ (Paseo doesn't do this) | ❌ | ❌ | ✅ InfoFilter (unique) |

---

## Part 7: Key Takeaways

1. **Three providers, one abstraction**: Claude (Agent SDK), Codex (JSON-RPC App Server), OpenCode (HTTP+SSE) all normalize to the same `AgentEvent` enum. The provider trait handles transport differences; the event system handles semantic normalization.

2. **Paseo's control plane is the right foundation**: Agent identity ≠ process ≠ session. AgentManager with persistence, lifecycle, and event subscription. But Paseo doesn't do information hiding.

3. **SLP's unique contribution is the InfoFilter**: The control plane knows the full topology; the agent only sees task + workspace + prompt. This is the "anti-subagent" thesis applied to the runtime layer.

4. **Two-tier subagent model**: Managed agents (Peers) are full lifecycle. Provider-native subagents (Claude Task, Codex child, OpenCode child) are observed but not managed. Both are visible in the control plane; only provider-native subagents are visible to the Peer.

5. **Phase order is SDK-first**: The Provider trait (Phase 1) is the foundation. Everything else (identity, events, info filter, governance) builds on top. Don't start with `claude -p` workarounds.

---

## Appendix: File Map

| New File | Phase | Purpose |
|----------|-------|---------|
| `crates/anti-core/src/provider.rs` | P1 | AgentClient, AgentSession traits |
| `crates/anti-core/src/events.rs` | P1 | AgentEvent, ToolDetail, Usage |
| `crates/anti-core/src/agent.rs` | P2 | AgentRecord, AgentId, AgentStatus |
| `crates/anti-core/src/info_filter.rs` | P3 | Information boundary enforcement |
| `crates/anti-core/src/subagent_tracker.rs` | P5 | Native subagent tracking |
| `crates/anti-core/src/governance.rs` | P6 | SLP orchestrator |
| `crates/anti-providers/src/claude.rs` | P1 | Claude SDK adapter |
| `crates/anti-providers/src/codex.rs` | P1 | Codex App Server adapter |
| `crates/anti-providers/src/opencode.rs` | P1 | OpenCode Server adapter |
| `crates/anti-daemon/src/agent_manager.rs` | P2 | Agent lifecycle management |
| `crates/anti-daemon/src/event_bus.rs` | P4 | Event-driven orchestration |
