# Paseo & Astra harness integration — so sánh với anti (P3 study)

Nguồn: `.tmp/paseo/packages/server/src/server/agent/providers/`,
`.tmp/astra/packages/contracts/src/providerInstance.ts` + `providerRuntime.ts`.

## Cách paseo tích hợp 3 harness

### Claude — Agent SDK, không tự build CLI flags
- Dùng `@anthropic-ai/claude-agent-sdk`; paseo chỉ override
  `spawnClaudeCodeProcess` để inject binary/env/runtime settings
  (`claude-agent.ts:176 resolveClaudeSpawnCommand`, mode default/append/argv).
- Session resume, permission flow, sidechain tracking đều qua SDK events.
→ Anti dùng CLI trực tiếp (`claude -p --verbose --output-format stream-json`,
stdin text) — khác cách đi nhưng đúng đắn cho mô hình one-shot peer của anti;
đã verify e2e thật. Flags khớp yêu cầu của CLI (verbose bắt buộc với
stream-json — chính là bug PR #9 đã fix).

### Codex — app-server JSON-RPC, KHÔNG phải exec
- Spawn `codex app-server` làm daemon con, nói JSON-RPC qua stdio
  (`codex-app-server-agent.ts:4011`: `spawnProcess(cmd, [...args, "app-server"])`),
  methods initialize/newConversation/sendUserTurn; event stream theo session.
  Có rollout timeline parsing để resume (`codex-rollout-timeline.ts`).
→ Anti dùng `codex exec --json --skip-git-repo-check -C <wt> "<prompt>"` —
one-shot chính chủ của codex CLI, đủ cho peer=1-task. app-server chỉ cần khi
muốn multi-turn/session persistence. Lưu ý: probe capability của anti đã nhận
diện được `app-server` support nếu sau này muốn nâng cấp.

### OpenCode — serve + HTTP/SSE client, KHÔNG phải run
- Tìm port rảnh → spawn `opencode serve --port N` → chờ "listening on" trên
stdout (timeout 30s) → HTTP client SDK: `session.create` → prompt →
`client.event.subscribe` (SSE) → `session.abort`/`session.messages`
(`opencode-agent.ts:631-720`, `:742-780`).
→ Anti dùng `opencode run --format json --model M --dir <wt>` one-shot. Hạn chế
đã biết: `--model` BẮT BUỘC (bug treo đã fix a1715d8). Serve-mode sẽ bỏ được
dependency --model và cold-start, nhưng thêm port management + client code.

## Astra — driver registry (khác lớp bài toán)
- `ProviderDriverKind` là open branded slug; driver lạ trong persisted state →
mark "unavailable", không crash (`providerInstance.ts` forward-compat invariant).
- Bài học cho anti: `Harness` enum hiện là closed enum; nếu muốn cho phép fork
thêm harness (như astra), chuyển sang slug-based registry + availability probe.
Chưa cần ngay — anti kiểm soát đầy đủ các variant.

## Kết luận: anti implement đúng không?
Đúng cho use-case anti (peer = 1 task độc lập, KISS, không relay/session):
1. claude CLI one-shot ✓ hoạt động thật (mimo-v2.5 $1.79 / deepseek-flash
   $1.07, task 4 phần hoàn thành)
2. codex exec ✓ hoạt động thật ($0.39, DONE.txt)
3. opencode run ✓ hoạt động thật sau fix --model

Gap có chủ đích (chưa cần): multi-turn session (codex app-server /
opencode serve+resume), sidechain tracker sâu hơn (paseo parse từng tool call
trong subagent). Khi cần: ưu tiên opencode serve-mode vì nó giải luôn bug
--model và cold-start.

Probe capability (`capabilities.rs`) khớp thực tế: claude --help expose
stream-json (probe ✓); codex/opencode help không liệt kê → fallback defaults,
vẫn an toàn vì adapter không phụ thuộc flag nào ngoài exec/run vốn luôn có.
