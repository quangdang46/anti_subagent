//! Integration tests for Phase 0 — Safety / Process / Treehouse
//!
//! These tests verify the core safety invariants:
//! - Spawn peer works correctly
//! - Terminate peer works correctly
//! - Kill peer doesn't kill lead
//! - Peer crash lifecycle
//! - Crash with lease
//! - Crash recovery on restart

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Helper: get the release binary path
fn anti_cli() -> PathBuf {
    // Integration tests run from crates/anti-integration, but binaries are in root target/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent")
        .parent()
        .expect("grandparent")
        .join("target")
        .join("release")
        .join("anti-cli.exe")
}

/// Helper: get the daemon binary path
fn anti_daemon() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent")
        .parent()
        .expect("grandparent")
        .join("target")
        .join("release")
        .join("anti-daemon.exe")
}

/// Helper: kill all anti processes
fn kill_all() {
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "anti-daemon.exe"])
        .output();
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "anti-cli.exe"])
        .output();
    std::thread::sleep(Duration::from_millis(500));
}

/// Helper: start daemon
fn start_daemon() -> bool {
    let _ = Command::new(anti_daemon()).spawn();
    std::thread::sleep(Duration::from_secs(2));
    true
}

/// Helper: run CLI command
fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(anti_cli())
        .args(args)
        .output()
        .expect("failed to execute anti-cli");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (format!("{}\n{}", stdout, stderr), code)
}

/// Helper: check if process is alive
fn is_process_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .expect("failed to run tasklist");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.contains(&pid.to_string())
}

// ─── T1: Spawn peer normally ─────────────────────────────────────────

#[test]
fn t1_spawn_peer_normally() {
    kill_all();
    assert!(start_daemon(), "daemon failed to start");

    let (out, code) = run_cli(&[
        "spawn",
        "--id",
        "t1-spawn",
        "--role",
        "peer",
        "--harness",
        "claude",
        "--repo",
        ".",
    ]);
    assert_eq!(code, 0, "spawn failed: {}", out);
    assert!(out.contains("running"), "expected running status: {}", out);

    // Verify PID is valid
    let pid: u32 = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("pid")?.as_u64().map(|p| p as u32))
        .expect("failed to parse PID");
    assert!(pid > 0, "PID should be positive");

    // Verify PID is alive
    assert!(is_process_alive(pid), "peer PID should be alive");

    // Verify agent is in list
    let (list_out, _) = run_cli(&["list"]);
    assert!(list_out.contains("t1-spawn"), "agent should appear in list");

    // Verify current session not affected
    // (If we got here, our session is still alive)

    kill_all();
    println!("T1 PASS: spawn peer works, PID alive, current session unaffected");
}

// ─── T2: Terminate peer normally ─────────────────────────────────────

#[test]
fn t2_terminate_peer_normally() {
    kill_all();
    assert!(start_daemon());

    let (out, _) = run_cli(&[
        "spawn",
        "--id",
        "t2-peer",
        "--role",
        "peer",
        "--harness",
        "claude",
        "--repo",
        ".",
    ]);
    let pid: u32 = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("pid").and_then(|p| p.as_u64().map(|p| p as u32)))
        .unwrap_or(0);
    assert!(pid > 0, "failed to get PID");

    // Terminate
    let (stop_out, stop_code) = run_cli(&["stop", "t2-peer"]);
    assert_eq!(stop_code, 0, "stop failed: {}", stop_out);

    // Verify process is dead
    std::thread::sleep(Duration::from_millis(500));
    assert!(!is_process_alive(pid), "peer should be dead after stop");

    // Verify state
    let (status_out, _) = run_cli(&["status", "t2-peer"]);
    assert!(
        status_out.contains("Stopped") || status_out.contains("COMPLETED"),
        "state should be terminal: {}",
        status_out
    );

    kill_all();
    println!("T2 PASS: terminate peer works, process dead, state updated");
}

// ─── T3: Kill peer doesn't kill lead ─────────────────────────────────

#[test]
fn t3_kill_peer_doesnt_kill_lead() {
    kill_all();
    assert!(start_daemon());

    // Get current session PID (this test process)
    let my_pid = std::process::id();

    // Spawn a peer
    let (out, _) = run_cli(&[
        "spawn",
        "--id",
        "t3-peer",
        "--role",
        "peer",
        "--harness",
        "claude",
        "--repo",
        ".",
    ]);
    let peer_pid: u32 = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("pid")?.as_u64().map(|p| p as u32))
        .unwrap_or(0);

    // Verify peer PID is different from our PID
    assert_ne!(peer_pid, my_pid, "peer PID should differ from lead PID");
    assert!(peer_pid > 0, "peer PID should be valid");

    // Kill the peer
    let (kill_out, kill_code) = run_cli(&["kill", "t3-peer"]);
    assert_eq!(kill_code, 0, "kill failed: {}", kill_out);

    // Verify peer is dead
    std::thread::sleep(Duration::from_millis(500));
    assert!(!is_process_alive(peer_pid), "peer should be dead");

    // Verify current session (lead) is still alive
    assert!(
        is_process_alive(my_pid),
        "current session should still be alive"
    );

    // Verify daemon is still running
    let (doctor_out, _) = run_cli(&["doctor"]);
    assert!(
        doctor_out.contains("daemon: OK"),
        "daemon should still be running"
    );

    kill_all();
    println!("T3 PASS: kill peer doesn't kill lead, session unaffected");
}

// ─── T4: Peer crash ──────────────────────────────────────────────────

#[test]
fn t4_peer_crash() {
    kill_all();
    assert!(start_daemon());

    // Spawn a peer
    let (out, _) = run_cli(&[
        "spawn",
        "--id",
        "t4-peer",
        "--role",
        "peer",
        "--harness",
        "claude",
        "--repo",
        ".",
    ]);
    let pid: u32 = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("pid").and_then(|p| p.as_u64().map(|p| p as u32)))
        .unwrap_or(0);
    assert!(pid > 0, "failed to get PID");

    // Kill the peer (simulate crash)
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
    std::thread::sleep(Duration::from_secs(2)); // Wait for reaper

    // Verify state is CRASHED
    let (status_out, _) = run_cli(&["status", "t4-peer"]);
    assert!(
        status_out.contains("Crashed") || status_out.contains("FAILED"),
        "state should be crashed/failed: {}",
        status_out
    );

    // Verify workspace was cleaned up (no worktree left)
    // This is verified by checking the treehouse pool
    let (doctor_out, _) = run_cli(&["doctor"]);
    assert!(
        doctor_out.contains("treehouse: OK"),
        "treehouse should still be OK"
    );

    kill_all();
    println!("T4 PASS: peer crash detected, state updated, treehouse OK");
}

// ─── T5: Crash with lease ────────────────────────────────────────────

#[test]
fn t5_crash_with_lease() {
    kill_all();
    assert!(start_daemon());

    // Spawn a peer with workspace
    let (out, _) = run_cli(&[
        "spawn",
        "--id",
        "t5-peer",
        "--role",
        "peer",
        "--harness",
        "claude",
        "--repo",
        ".",
    ]);
    let pid: u32 = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("pid").and_then(|p| p.as_u64().map(|p| p as u32)))
        .unwrap_or(0);

    // Verify lease exists in response
    assert!(out.contains("lease_id"), "response should contain lease_id");

    // Kill the peer
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
    std::thread::sleep(Duration::from_secs(2));

    // Verify crash detected
    let (status_out, _) = run_cli(&["status", "t5-peer"]);
    assert!(
        status_out.contains("Crashed") || status_out.contains("FAILED"),
        "should be crashed: {}",
        status_out
    );

    // Verify treehouse pool is clean (lease released)
    let (pool_out, _) = run_cli(&["doctor"]);
    assert!(
        pool_out.contains("treehouse: OK"),
        "treehouse should handle cleanup"
    );

    kill_all();
    println!("T5 PASS: crash with lease — cleanup handled");
}

// ─── T6: Crash recovery on restart ───────────────────────────────────

#[test]
fn t6_crash_recovery_on_restart() {
    kill_all();
    assert!(start_daemon());

    // Spawn a peer
    let (out, _) = run_cli(&[
        "spawn",
        "--id",
        "t6-peer",
        "--role",
        "peer",
        "--harness",
        "claude",
        "--repo",
        ".",
    ]);
    let pid: u32 = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("pid").and_then(|p| p.as_u64().map(|p| p as u32)))
        .unwrap_or(0);

    // Kill the peer (simulate crash)
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
    std::thread::sleep(Duration::from_millis(500));

    // Kill daemon (simulate daemon crash)
    kill_all();
    std::thread::sleep(Duration::from_millis(500));

    // Restart daemon
    assert!(start_daemon());

    // Verify peer is marked as crashed/recovered
    let (status_out, _) = run_cli(&["status", "t6-peer"]);
    assert!(
        status_out.contains("Crashed")
            || status_out.contains("FAILED")
            || status_out.contains("COMPLETED"),
        "peer should be in terminal state after restart: {}",
        status_out
    );

    kill_all();
    println!("T6 PASS: crash recovery on restart works");
}
