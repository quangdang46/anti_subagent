//! Paseo-style harness runtimes — controlled, session-oriented integrations.
//!
//! Two paseo-proven transports beyond one-shot CLI:
//!
//! - [`codex_app_server`]: spawn `codex app-server`, speak JSON-RPC over
//!   stdio (`initialize` → `initialized` → `thread/start` → `turn/start`),
//!   completion = `turn/completed` notification. Verified against codex-cli
//!   0.149.0.
//! - [`opencode_serve`]: spawn `opencode serve --port N`, create a session
//!   scoped to the worktree via `POST /session?directory=...`, drive the task
//!   with `POST /session/{id}/message?directory=...` (parts + model), poll
//!   session status until idle.
//!
//! Both are opt-in per harness via env:
//!   ANTI_CODEX_MODE=app-server   (default: exec)
//!   ANTI_OPENCODE_MODE=serve     (default: exec)

pub mod codex_app_server;
pub mod opencode_serve;
