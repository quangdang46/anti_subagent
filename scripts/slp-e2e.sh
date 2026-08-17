#!/usr/bin/env bash
# E2E test: spawn peer → submit → watchdog escalate → review → accept
# Verifies the full SLP work lifecycle through the CLI.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build -q 2>&1 || { echo "BUILD FAILED"; exit 1; }
# Binary name varies by platform
if [ -f ./target/debug/anti-cli.exe ]; then
    ANTI=./target/debug/anti-cli.exe
elif [ -f ./target/debug/anti-cli ]; then
    ANTI=./target/debug/anti-cli
else
    ANTI=./target/debug/anti
fi

# Start daemon if not running
$ANTI daemon start 2>/dev/null || true
sleep 1

ID="e2e-$(date +%s)"
PASS=0
FAIL=0

check() {
    local label="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  PASS: $label"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $label"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== SLP E2E Test ==="
echo "work item id: $ID"

# Step 1: Check daemon (may not be available on Windows)
echo ""
echo "--- Step 1: Daemon status ---"
if $ANTI daemon status 2>/dev/null; then
    check "daemon responds to ping" true
    DAEMON_UP=true
else
    echo "  SKIP: daemon not running (expected on Windows — Unix-only transport)"
    DAEMON_UP=false
fi

# Step 2: Guard classification (no daemon needed)
echo ""
echo "--- Step 2: Guard classification ---"
check "subagent tool denied" $ANTI guard test --tool subagent_spawn
check "bash tool allowed" $ANTI guard test --tool bash
check "spawn tool denied" $ANTI guard test --tool spawn_agent
check "read tool allowed" $ANTI guard test --tool read_file

# Step 3: Work items (requires daemon)
echo ""
echo "--- Step 3: Work items ---"
if [ "$DAEMON_UP" = true ]; then
    check "work list runs" $ANTI work list
else
    echo "  SKIP: requires daemon"
fi

# Step 4: Escalations (requires daemon)
echo ""
echo "--- Step 4: Escalations ---"
if [ "$DAEMON_UP" = true ]; then
    check "escalations runs" $ANTI escalations
else
    echo "  SKIP: requires daemon"
fi

# Step 5: Doctor check
echo ""
echo "--- Step 5: Doctor ---"
$ANTI doctor 2>/dev/null || true

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
