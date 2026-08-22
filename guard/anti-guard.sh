#!/usr/bin/env bash
# guard/anti-guard.sh — PreToolUse hook script (firstmate-shaped + slb fail-closed).
#
# stdin: JSON with "tool" and optionally "command" fields.
# stdout: empty (deny) or nothing (allow).
# stderr: Claude deny JSON when denied.
#
# Blast-radius cap §22: non-delegation tools allowed locally without daemon
# round-trip. Only candidate delegation tools query the daemon.

set -uo pipefail

TOOL_JSON=$(cat -)
TOOL_NAME=$(echo "$TOOL_JSON" | jq -r '.tool // .name // ""' 2>/dev/null || echo "")
if [ -z "$TOOL_NAME" ]; then
    # Malformed input → fail-closed (deny)
    echo '{"error":"malformed: no tool name"}' >&2
    exit 2
fi

# Normalize: lowercase, strip non-alnum
NORMALIZED=$(echo "$TOOL_NAME" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]//g')

# ─── Allow-list: always allowed (blast-radius cap) ───────────────────────
case "$TOOL_NAME" in
    Read|Grep|Glob|Edit|Write|ListFiles|TodoRead|TodoWrite|mcp__*)
        exit 0
        ;;
esac

# Also check by normalized name (fallback)
case "$NORMALIZED" in
    read|grep|glob|edit|write|listfiles|todoread|todowrite|mcp_*)
        exit 0
        ;;
esac

# ─── Scope gate: only enforce in anti-managed peer worktrees ──────────────
SCOPE_GATE="${ANTI_GUARD_SCOPE_GATE:-true}"
if [ "$SCOPE_GATE" = "true" ]; then
    # Simple heuristic: cwd must be under ANTI_STATE_DIR or a treehouse pool
    # dir. If not, allow (Supervisor/Lead session).
    CWD="${PWD:-$(pwd)}"
    STATE_HOME="${ANTI_STATE_DIR:-$HOME/.anti_subagent}"
    case "$CWD" in
        "$STATE_HOME"*) ;; # in-scope
        *)
            # Check treehouse pool locations
            case "$CWD" in
                *treehouse*|*worktree*)
                    ;; # in-scope (treehouse pool)
                *)
                    # Not in a peer workspace — allow (guard doesn't apply here)
                    exit 0
                    ;;
            esac
            ;;
    esac
fi

# ─── Deny-by-stem classification (fail-closed for delegation) ────────────
DENY_STEMS="agent subagent task workflow cron schedul worktree delegate spawn dispatch handoff remote sendmessage monitor"
for STEM in $DENY_STEMS; do
    if echo "$NORMALIZED" | grep -q "$STEM"; then
        # Delegation-shaped. Ask the daemon for confirmation; ANY doubt → deny.
        STATE_DIR="${ANTI_STATE_DIR:-$HOME/.anti_subagent}"
        SOCK="$STATE_DIR/anti.sock"
        RESULT="deny"   # fail-closed default
        if [ -S "$SOCK" ]; then
            RESPONSE=$(timeout 0.05 bash -c "echo '{\"method\":\"GuardCheck\",\"params\":{\"tool\":\"$TOOL_NAME\"}}' | socat - UNIX-CLIENT:\"$SOCK\" 2>/dev/null || echo \"{}\"" 2>/dev/null || echo "{}")
            PARSED=$(echo "$RESPONSE" | jq -r '.ok.data.allowed // empty' 2>/dev/null || echo "")
            if [ "$PARSED" = "true" ]; then
                RESULT="allow"
            elif [ "$PARSED" = "false" ]; then
                RESULT="deny"
            fi
            # empty parse (malformed/unreachable) → keep deny
        fi

        if [ "$RESULT" = "deny" ]; then
            echo "{\"error\":\"delegation-shaped tool denied: $TOOL_NAME (stem: $STEM)\",\"tool\":\"$TOOL_NAME\",\"reason\":\"guard-fail-closed\"}" >&2
            exit 2
        fi
        # Daemon explicitly allowed — respect it.
        exit 0
    fi
done

# Non-delegation tool — allow
exit 0
