# Issue #3 — Model tier routing (lightweight Peer, strong Lead/Supervisor)

**Status of code today (read before implementing):**

- `crates/anti-core/src/routing.rs` already has the routing machinery:
  - `CapabilityTier { Lightweight, Standard, Heavyweight }`
  - `Complexity { Low, Medium, High }`
  - `ProviderConfig` (loaded from `providers.toml`) with `ProviderTier { lightweight, standard, heavyweight }`
  - `resolve_route(disposition, complexity, config, default_provider) -> ModelRoute`
  - Default config: `claude` → haiku/sonnet/opus, `codex` → gpt-4o-mini/gpt-4o/o3
- `crates/anti-core/src/governance.rs` does **NOT** use it — `SlpOrchestrator::spawn_supervisor` (line ~149) and `spawn_lead` (line ~163) hard-code `model: Some("claude-sonnet")`. `spawn_peer` uses `build_agent_context(...)` which may carry a model but the hierarchy role is never a routing input.
- `crates/anti-core/src/model.rs` defines `Role { Supervisor, Lead, Peer }`.
- `crates/anti-cli/src/commands.rs` `spawn(...)` takes `role / disposition / harness / task / repo / parent / peer_prompt / arm` — **no `--model` flag**.
- `crates/anti-core/src/config.rs` `Config` has a `claude_bin` but no `providers.toml` loader yet.

**Goal:** Model selection is automatic and role-aware — Supervisor & Lead always get a strong model; Peers get the tier implied by disposition × complexity. The resolved model is visible and overridable from the CLI.

**Files to touch:**

1. `crates/anti-core/src/routing.rs`
   - Add `Role` (from `crate::model`) as a first-class routing input.
   - New signature: `resolve_route(role: Role, disposition: Disposition, complexity: Complexity, config: &ProviderConfig, default_provider: &str) -> ModelRoute`.
   - Rule: `Role::Supervisor | Role::Lead` → force `CapabilityTier::Heavyweight` (strong model) regardless of disposition. `Role::Peer` → keep the existing disposition × complexity mapping.
   - Keep the old `resolve_route` 4-arg form as a thin wrapper (Peer default) for non-governance callers, or update call sites.

2. `crates/anti-core/src/config.rs`
   - Add `providers: ProviderConfig` to `Config`, loaded from `.anti_subagent/providers.toml` (project) → `~/.anti_subagent/providers.toml` (user) → compiled default. Layer it into the existing config precedence (defaults < user < project < env < flags).
   - Add `ANTI_PROVIDERS_TOML` env override + `--providers` flag plumbing if cheap.

3. `crates/anti-core/src/governance.rs`
   - In `spawn_supervisor`, `spawn_lead`, `spawn_peer`: replace the hard-coded `claude-sonnet` with `let route = resolve_route(role, disposition, complexity, &config.providers, "claude"); config.model = Some(route.model);`
   - For `spawn_peer`, `complexity` is not yet known at spawn — default to `Complexity::Medium` (or accept it as a param from the peer task). Document the choice.

4. `crates/anti-cli/src/commands.rs` + `crates/anti-cli/src/main.rs`
   - Add `model: Option<&str>` param to `spawn(...)`. When set, it overrides the resolved route (explicit pin).
   - Pass `model` into `Request::SpawnAgent` (add `model: Option<String>` to the IPC `Request` variant in `crates/anti-daemon/src/ipc.rs`).

5. `crates/anti-daemon/src/main.rs`
   - In the `SpawnAgent` handler, set `config.model` from `req.model` **or** the governance-resolved route before spawning `Command::new("claude")` (line ~1154). Ensure `build_agent_context` honors `config.model` (see `info_filter.rs`).

**Tests:**

- Unit (`routing.rs`): `supervisor_always_heavyweight` (any disposition → opus for claude), `lead_always_heavyweight`, `peer_engineer_high_still_heavyweight`, `peer_scout_lightweight` (unchanged).
- Unit (`governance.rs`): assert `spawn_supervisor` record has `config.model == Some("opus")`, `spawn_lead` too.
- Integration (`crates/anti-cli` or `tests/`): `anti spawn --role supervisor` → daemon record `model == opus`; `anti spawn --role peer --disposition scout` → `haiku`; `anti spawn --role peer --model gpt-4o-mini` → pinned value wins.

**Acceptance criteria:**

- No hard-coded model strings remain in `governance.rs`.
- `resolve_route` takes `Role`.
- `anti spawn` supports `--model <m>` to override.
- All existing `routing.rs` / `governance.rs` unit tests still pass; new tier tests pass.
- `cargo test -p anti-core -p anti-cli` is green.
