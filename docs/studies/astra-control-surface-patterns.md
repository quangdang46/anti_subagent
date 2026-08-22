# Astra control-surface patterns — study notes (issue #7 → feeds issue #8)

Source: `.tmp/astra/` (T3 Code fork). Read-only investigation, 2026-08-22.

## 1. Two surfaces over one port
WS RPC for interactive calls + subscriptions; REST-ish HTTP for snapshots and
large payloads (`apps/server/src/server.ts`, `packages/contracts/src/rpc.ts:973`,
`packages/contracts/src/environmentHttp.ts:610`). Method names are
dot-namespaced verbs in a const table (`projects.list`,
`subscribeServerLifecycle`). Rule of thumb in-tree: big/compressible payloads
and CLI consumers → HTTP; interactive/live traffic → WS.

State is event-sourced: commands → decider → persisted events (monotonic
global sequence) → projector read model. The surface only reads projections
and subscribes to the event stream — it never owns state.

## 2. Envelopes & errors
Every RPC declares payload/success/error schemas. Errors are tagged structs;
fixed status mapping invalid_request→400, auth→401, scope→403 (+requiredScope),
not_found→404, internal→500 with reason enum (`environmentHttp.ts:100-230`).
Command dispatch returns just `{sequence}`; events carry
`{sequence, eventId, aggregateKind, aggregateId, occurredAt, commandId}`.

## 3. Streaming = snapshot + cursor resume, never polling
- Subscribe forks the live stream into a buffer BEFORE loading the snapshot —
  no lost-update window (`apps/server/src/ws.ts:1185`, `:1304`).
- Stream items are a closed union `{synchronized} | {snapshot} | {event}` so
  the client knows when it is consistent (`contracts/src/orchestration.ts:1478`).
- Client passes `afterSequence`; server replays the gap, with a bounded gap cap
  that falls back to a fresh snapshot instead of unbounded replay.
- Simple state uses subscribe-before-snapshot returning `{latest, changes}`
  (`apps/server/src/utils/subscribeBeforeSnapshot.ts`).

## 4. Auth/security
Pairing-token bootstrap then scoped sessions. Coarse verb scopes checked via an
exhaustive `method → scope` map enforced at compile time
(`apps/server/src/auth/RpcAuthorization.ts:23` — `satisfies Record<Method, Scope>`,
adding a method without a scope fails to compile). WS avoids URL-borne tokens:
short-lived ticket minted over authenticated POST, validated at upgrade.
Binding defaults to loopback; wildcard binds surface a pairing URL.

## 5. Patterns worth transplanting into anti (Rust daemon)
1. **Typed method table + exhaustive authorization map** — one `Request` enum,
   one match to required capability; adding a variant without a decision must
   not compile.
2. **Snapshot+resume subscriptions on our JSONL sequence** — `GetHierarchy`
   already gives the snapshot; a `/v1/stream?after_seq=N` would replay from
   `events/events.jsonl` (we have the seq column) then tail live. Items:
   `{Synchronized|Snapshot|Event}`; gap too large → fresh snapshot.
3. **Versioned revision envelopes** `{version:1, revision:N, type, payload}` on
   deltas — old clients skip unknown kinds, revision jumps expose gaps.
4. **Loopback default + ticket-gated exposure** — our Unix socket is implicit
   auth; any future TCP bind stays 127.0.0.1 with explicit opt-in.
5. **Forward-compatible decode** — skip unknown serde enum variants rather than
   rejecting the payload, so a newer daemon never breaks an older `--json`
   consumer.

## Mapping to what anti already has (post-a46081a)
- `Request::GetHierarchy` ≙ `GET /api/orchestration/snapshot`.
- `events.jsonl` monotonic `seq` ≙ astra's global sequence — cursor resume is
  directly implementable.
- Unix-socket transport ≙ loopback default; nothing to change unless TCP is
  added (then gate behind config, default off per plan §8).
