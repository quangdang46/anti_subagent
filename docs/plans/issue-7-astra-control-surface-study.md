# Issue #7 — Study ReinaMacCredy/astra: extract control-surface patterns

**Context (verified by reading the repo, not assumption):**

- `ReinaMacCredy/astra` is a **fork of T3 Code** (Theo / pingdotgg). T3 Code is an "agent harness control surface": a WebSocket server that wraps provider CLIs (Claude Code, Codex, Cursor, Grok Build, OpenCode) and serves them over web / desktop / mobile.
- It is **NOT** an SLP engine. The "SLP" label sometimes attached to it in discussion refers to how the *owner* runs their agents, not to code in the repo.
- Relevant for anti_subagent: astra is a **UI/control surface**; anti_subagent is an **orchestration engine** (Rust daemon). They are complementary layers, not competitors.

**What to extract (document, do not code yet):**

1. **Provider abstraction parity** — astra's harness layer normalizes Claude/Codex/Cursor/Grok/OpenCode into one interface. Compare with `crates/anti-providers/src/{claude,codex,opencode}.rs`. Identify which providers astra covers that anti_subagent does not, and whether a shared normalized event schema (`AgentEvent` in `anti-core`) could be reused.
2. **Control-surface UX patterns** — how astra renders sessions, transcribes, and lets a human drive multiple agents. These are the exact surfaces a Supervisor needs (see Issue #8).
3. **Spawn/lifecycle model** — how astra launches and tears down provider processes; contrast with `PeerManager` + `EventBridge` in anti_subagent. Note any lessons about worktree/process isolation.
4. **Integration boundary** — where anti_subagent could sit *under* astra: anti_subagent owns SLP governance + native-subagent guard; astra owns the human-facing UI. Define the minimal contract (spawn peer, list hierarchy, stop, stream events).

**Deliverable:** a short `docs/astra-integration-notes.md` capturing the above, with concrete file references in both repos. No code changes in this issue.

**Acceptance criteria:**

- `docs/astra-integration-notes.md` exists with the 4 sections above and at least 3 concrete cross-references to `crates/anti-*/**` files.
- No new Rust code; no Discord/community references in the doc.
