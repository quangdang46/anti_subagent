# Issue #8 — Expose Supervisor/Lead/Peer state via a control surface

**Status of code today (read before implementing):**

- `crates/anti-core/src/governance.rs` `SlpOrchestrator` holds `agents / supervisor / leads / peer_groups` **in memory** only.
- Visibility today is text-only: `crates/anti-cli/src/commands.rs` `list(...)` prints `ID ROLE STATUS PID TASK` from `Request::ListAgents`; `status(...)` prints one agent; `list --attention` shows triage queue.
- There is **no** programmatic HTTP/WS API a UI (e.g. astra / T3 Code, see Issue #7) could consume to render the hierarchy.
- IPC is already Unix-socket/TCP (`crates/anti-daemon/src/ipc.rs` with `Request`/`Response`); a control surface can reuse the same transport or add a thin HTTP listener.

**Goal:** Publish the live SLP hierarchy (Supervisor → Leads → Peers, with status, pid, task, attention flag, last event) over a small, stable endpoint so a control-surface UI can render and drive it.

**Files to touch:**

1. `crates/anti-daemon/src/` — add a control-surface module (e.g. `control_surface.rs`)
   - Serialize the `SlpOrchestrator` state into a stable JSON schema:
     ```json
     {
       "supervisor": {"id": "...", "status": "running"},
       "leads": [{"id": "...", "workspace": "...", "compactions": 3, "peers": ["p1","p2"]}],
       "peers": [{"id":"p1","role":"peer","disposition":"engineer","status":"running","pid":12345,"task":"...","attention":false}],
       "attention_queue": ["p3"]
     }
     ```
   - Expose `GET /v1/hierarchy` (and optionally a WS `/v1/stream` that pushes `AgentEvent`s). Reuse the existing IPC transport where possible; if adding HTTP, bind to loopback only (`127.0.0.1`) and gate behind `config.control_surface` (default off).
   - Add IPC `Request::GetHierarchy` / `Response` variant so the CLI `anti status --json` and the HTTP surface share one serializer.

2. `crates/anti-cli/src/commands.rs` + `main.rs`
   - `anti status --json` → emits the hierarchy JSON (no human PII beyond agent ids/tasks).
   - Keep text output as default.

3. `crates/anti-core/src/config.rs`
   - Add `control_surface: ControlSurfaceConfig { enabled: bool, bind: String, transport: http|ws }`.

**Tests:**

- Unit: serialize a known `SlpOrchestrator` state → assert JSON shape (supervisor/leads/peers/attention_queue keys present).
- Integration (reuse Issue #6 harness): boot daemon with control surface on, spawn a peer, `GET /v1/hierarchy` returns the peer; stop it, hierarchy updates.

**Acceptance criteria:**

- `anti status --json` returns the full hierarchy.
- With `control_surface.enabled = true`, `GET /v1/hierarchy` (loopback) reflects live state; updates after spawn/stop.
- Off by default; never binds to a non-loopback address.
- No Discord/community references in code or docs.
- `cargo test -p anti-daemon -p anti-cli` green.
