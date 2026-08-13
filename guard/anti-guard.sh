#!/bin/bash
# anti_subagent PreToolUse guard (firstmate-shaped + slb fail-closed, plan §22).
#
# Denies delegation-shaped tool calls inside an anti-managed peer workspace so
# a peer can never escape to the harness's native subagent mechanism.
#
# Fail-closed: if the anti daemon is unreachable, delegation-shaped tools are
# DENIED (the threat is only detectable while the control plane is up).
# Blast radius is capped: only delegation-shaped tools hit the daemon;
# Read/Grep/Edit etc. pass locally without a round-trip.

GUARD_RULES="${ANTI_GUARD_RULES:-$HOME/.anti_subagent/guard/rules.toml}"
ANTI_SOCKET="${ANTI_SOCKET:-$HOME/.anti_subagent/anti.sock}"

# --- 1. read the tool name from stdin (PreToolUse JSON) or --tool ---
if [ "$1" = "--tool" ]; then
  TOOL="$2"
else
  PAYLOAD=$(cat 2>/dev/null || true)
  [ -n "$PAYLOAD" ] || exit 0
  command -v jq >/dev/null 2>&1 || exit 0
  TOOL=$(printf '%s' "$PAYLOAD" | jq -r '.tool_name // .toolName // empty' 2>/dev/null) || exit 0
  [ -n "$TOOL" ] || exit 0
fi

# --- 2. normalize ---
LC_ALL=C NORMALIZED=$(printf '%s' "$TOOL" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9')

# --- 3. local stem scan (no daemon round-trip for non-delegation tools) ---
# BSD-compatible: extract quoted stems after `delegation_stems = [...]`.
STEMS=$(sed -n 's/.*delegation_stems.*= *\[//p' "$GUARD_RULES" 2>/dev/null | tr -d '[]' | grep -o '"[a-z]*"' | tr -d '"')
[ -n "$STEMS" ] || exit 0
MATCHED=""
for stem in $STEMS; do
  case "$NORMALIZED" in
    *"$stem"*) MATCHED="$stem"; break ;;
  esac
done

# --- 4. not delegation-shaped → allow locally ---
[ -n "$MATCHED" ] || exit 0

# --- 5. delegation-shaped → ask the daemon (fail-closed on unreachable) ---
ALLOWED=0
if command -v python3 >/dev/null 2>&1 && [ -S "$ANTI_SOCKET" ]; then
  RESP=$(python3 - "$ANTI_SOCKET" "$TOOL" <<'PYEOF' 2>/dev/null
import json, socket, sys
sock_path, tool = sys.argv[1], sys.argv[2]
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(0.5)
    s.connect(sock_path)
    s.sendall(json.dumps({"method":"GuardCheck","params":{"tool":tool}}).encode()+b"\n")
    data = s.recv(4096).decode()
    resp = json.loads(data)
    print(resp.get("ok") and resp.get("data",{}).get("allowed","false"))
except Exception:
    print("false")
PYEOF
  )
  [ "$RESP" = "True" ] && ALLOWED=1
fi

if [ "$ALLOWED" = "1" ]; then
  exit 0
fi

# --- 6. deny: stderr only, empty stdout (Claude ignores deny when stdout nonempty) ---
printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"anti_subagent: delegation-shaped tool %s is blocked for peers (use anti spawn for real work)"},"systemMessage":"anti_subagent: delegation-shaped tool blocked"}\n' "$TOOL" >&2
exit 2
