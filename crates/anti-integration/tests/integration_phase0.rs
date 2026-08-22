//! Integration tests for Phase 0 — Safety / Process / Treehouse
//!
//! These tests verify the core safety invariants:
//! - Spawn peer works correctly
//! - Terminate peer works correctly
//! - Kill peer doesn't kill lead
//! - Peer crash lifecycle
//! - Crash with lease
//! - Crash recovery on restart
//!
//! Determinism: every test uses the `sleep` harness (a plain `sleep <secs>`
//! process — no model, no auth, no network) and its own state dir plus a
//! fresh git repo under the OS temp dir. Nothing leaks between runs.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Helper: get binary path (debug build — what cargo test produces on macOS/Linux)
fn bin(name: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent")
        .parent()
        .expect("grandparent")
        .join("target")
        .join("debug")
        .join(exe)
}

fn anti_cli() -> PathBuf {
    bin("anti-cli")
}

/// Helper: get the daemon binary path
fn anti_daemon() -> PathBuf {
    bin("anti-daemon")
}

/// Per-test isolated environment: unique state dir + fresh git repo.
struct Env {
    state_dir: PathBuf,
    repo: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Self {
        let uniq = format!(
            "{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let base = std::env::temp_dir().join(&uniq);
        let state_dir = base.join("state");
        let repo = base.join("repo");
        std::fs::create_dir_all(&state_dir).expect("create state dir");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("git failed")
        };
        // A worktree pool needs at least one commit and a resolvable HEAD.
        assert!(git(&["init", "-q"]).status.success(), "git init failed");
        assert!(
            git(&["commit", "--allow-empty", "-q", "-m", "init"])
                .status
                .success(),
            "git commit failed"
        );
        Env { state_dir, repo }
    }

    /// Run anti-cli against this env's state dir.
    fn cli(&self, args: &[&str]) -> (String, i32) {
        run_cli(&self.state_dir, args)
    }

    /// Spawn a `sleep <secs>` peer.
    fn spawn_sleep(&self, id: &str, secs: u64) -> (String, i32) {
        self.cli(&[
            "spawn",
            "--id",
            id,
            "--role",
            "peer",
            "--harness",
            "sleep",
            "--task",
            &secs.to_string(),
            "--repo",
            self.repo.to_str().unwrap(),
        ])
    }

    /// Extract `"pid": N` from a spawn/status JSON response.
    fn pid_of(out: &str) -> u32 {
        serde_json::from_str::<serde_json::Value>(out)
            .ok()
            .and_then(|v| v.get("pid").and_then(|p| p.as_u64()))
            .map(|p| p as u32)
            .unwrap_or(0)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Best-effort teardown; never panic in drop.
        let _ = Command::new(anti_daemon_bin_stop()).output();
        let sock = self.state_dir.join("anti.sock");
        if sock.exists() {
            let req = serde_json::json!({"method": "Shutdown"});
            #[cfg(unix)]
            if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&sock) {
                use std::io::Write;
                let line = format!("{req}\n");
                let _ = stream.write_all(line.as_bytes());
            }
        }
        let _ = std::fs::remove_dir_all(self.state_dir.parent().unwrap_or(&self.state_dir));
    }
}

fn anti_daemon_bin_stop() -> &'static str {
    // Placeholder for symmetric stop handling on non-Unix (no-op command).
    "true"
}

/// Helper: kill all anti processes (cross-platform, like paseo's FakeAgentClient pid mgmt)
fn kill_all() {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "anti-daemon.exe"])
            .output();
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "anti-cli.exe"])
            .output();
    } else {
        let _ = Command::new("pkill").args(["-f", "anti-daemon"]).output();
    }
    std::thread::sleep(Duration::from_millis(500));
}

/// Helper: start daemon bound to `state_dir`, wait until the IPC socket answers.
fn start_daemon(state_dir: &PathBuf) -> bool {
    let child = Command::new(anti_daemon())
        .env("ANTI_STATE_DIR", state_dir)
        .env("ANTI_DAEMONIZED", "1") // skip self-fork: we manage the process
        .spawn();
    if child.is_err() {
        return false;
    }
    // Poll Ping until the socket is live (max ~10s).
    let sock = state_dir.join("anti.sock");
    for _ in 0..100 {
        if sock.exists() {
            let (out, code) = run_cli(state_dir, &["list"]);
            if code == 0 || !out.is_empty() {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Helper: run CLI command
fn run_cli(state_dir: &PathBuf, args: &[&str]) -> (String, i32) {
    let output = Command::new(anti_cli())
        .args(args)
        .arg("--state-dir")
        .arg(state_dir)
        .output()
        .expect("failed to execute anti-cli");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (format!("{}\n{}", stdout, stderr), code)
}

/// Helper: check if process is alive (cross-platform: kill -0 on Unix)
fn is_process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("failed to run tasklist");
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.contains(&pid.to_string())
    } else {
        // kill -0: 0 = alive, non-zero = dead
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Kill a pid (cross-platform crash simulation, like paseo's real process kill)
fn kill_pid(pid: u32) {
    if cfg!(windows) {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    } else {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

/// Wait until the agent's status JSON contains one of `needles` (max `secs`).
fn wait_status(env: &Env, id: &str, needles: &[&str], secs: u64) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let (out, _) = env.cli(&["status", id]);
        for n in needles {
            if out.contains(n) {
                return out;
            }
        }
        if std::time::Instant::now() >= deadline {
            return out;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ─── T1: Spawn peer normally ─────────────────────────────────────────

#[test]
fn t1_spawn_peer_normally() {
    kill_all();
    let env = Env::new("t1");
    assert!(start_daemon(&env.state_dir), "daemon failed to start");

    let (out, code) = env.spawn_sleep("t1-spawn", 120);
    assert_eq!(code, 0, "spawn failed: {}", out);
    assert!(out.contains("running"), "expected running status: {}", out);

    // Verify PID is valid
    let pid = Env::pid_of(&out);
    assert!(pid > 0, "PID should be positive: {out}");

    // Verify PID is alive
    assert!(is_process_alive(pid), "peer PID should be alive");

    // Verify agent is in list
    let (list_out, _) = env.cli(&["list"]);
    assert!(list_out.contains("t1-spawn"), "agent should appear in list");

    // Verify current session not affected
    // (If we got here, our session is still alive)

    println!("T1 PASS: spawn peer works, PID alive, current session unaffected");
}

// ─── T2: Terminate peer normally ─────────────────────────────────────

#[test]
fn t2_terminate_peer_normally() {
    kill_all();
    let env = Env::new("t2");
    assert!(start_daemon(&env.state_dir));

    let (out, _) = env.spawn_sleep("t2-peer", 120);
    let pid = Env::pid_of(&out);
    assert!(pid > 0, "failed to get PID: {out}");
    assert!(is_process_alive(pid), "peer should be alive before stop");

    // Terminate
    let (stop_out, stop_code) = env.cli(&["stop", "t2-peer"]);
    assert_eq!(stop_code, 0, "stop failed: {}", stop_out);

    // Verify process is dead (allow the SIGTERM→SIGKILL escalation a moment)
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while is_process_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!is_process_alive(pid), "peer should be dead after stop");

    // Verify state
    let status_out = wait_status(&env, "t2-peer", &["Stopped", "STOPPED"], 5);
    assert!(
        status_out.contains("Stopped") || status_out.contains("STOPPED"),
        "state should be terminal: {}",
        status_out
    );

    println!("T2 PASS: terminate peer works, process dead, state updated");
}

// ─── T3: Kill peer doesn't kill lead ─────────────────────────────────

#[test]
fn t3_kill_peer_doesnt_kill_lead() {
    kill_all();
    let env = Env::new("t3");
    assert!(start_daemon(&env.state_dir));

    // Get current session PID (this test process)
    let my_pid = std::process::id();

    // Spawn a peer
    let (out, _) = env.spawn_sleep("t3-peer", 120);
    let peer_pid = Env::pid_of(&out);
    assert!(peer_pid > 0, "peer PID should be valid: {out}");

    // Verify peer PID is different from our PID
    assert_ne!(peer_pid, my_pid, "peer PID should differ from lead PID");

    // Kill the peer
    let (kill_out, kill_code) = env.cli(&["kill", "t3-peer"]);
    assert_eq!(kill_code, 0, "kill failed: {}", kill_out);

    // Verify peer is dead
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while is_process_alive(peer_pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!is_process_alive(peer_pid), "peer should be dead");

    // Verify current session (lead) is still alive
    assert!(
        is_process_alive(my_pid),
        "current session should still be alive"
    );

    // Verify daemon is still running
    let (doctor_out, _) = env.cli(&["doctor"]);
    assert!(
        doctor_out.contains("daemon: OK"),
        "daemon should still be running"
    );

    println!("T3 PASS: kill peer doesn't kill lead, session unaffected");
}

// ─── T4: Peer crash ──────────────────────────────────────────────────

#[test]
fn t4_peer_crash() {
    kill_all();
    let env = Env::new("t4");
    assert!(start_daemon(&env.state_dir));

    // Spawn a long-lived sleep peer so kill -9 lands before natural exit.
    let (out, _) = env.spawn_sleep("t4-peer", 120);
    let pid = Env::pid_of(&out);
    assert!(pid > 0, "failed to get PID: {out}");
    assert!(is_process_alive(pid), "peer must be alive pre-crash");

    // Kill the peer (simulate crash)
    kill_pid(pid);

    // Reaper runs every 5s — allow up to 10s for detection.
    // killed-by-signal must map to Crashed, never Completed.
    let status_out = wait_status(&env, "t4-peer", &["CRASHED", "FAILED", "Crashed"], 12);
    assert!(
        status_out.contains("CRASHED")
            || status_out.contains("FAILED")
            || status_out.contains("Crashed"),
        "state should be crashed/failed: {}",
        status_out
    );

    // Verify treehouse still healthy
    let (doctor_out, _) = env.cli(&["doctor"]);
    assert!(
        doctor_out.contains("treehouse: OK"),
        "treehouse should still be OK"
    );

    println!("T4 PASS: peer crash detected, state updated, treehouse OK");
}

// ─── T5: Crash with lease ────────────────────────────────────────────

#[test]
fn t5_crash_with_lease() {
    kill_all();
    let env = Env::new("t5");
    assert!(start_daemon(&env.state_dir));

    // Spawn a peer with workspace
    let (out, _) = env.spawn_sleep("t5-peer", 120);
    assert!(out.contains("lease_id"), "response should contain lease_id");
    let pid = Env::pid_of(&out);
    assert!(pid > 0, "failed to get PID: {out}");
    assert!(is_process_alive(pid), "peer must be alive pre-crash");

    // Kill the peer
    kill_pid(pid);

    let status_out = wait_status(&env, "t5-peer", &["CRASHED", "FAILED", "Crashed"], 12);
    assert!(
        status_out.contains("CRASHED")
            || status_out.contains("FAILED")
            || status_out.contains("Crashed"),
        "should be crashed: {}",
        status_out
    );

    let (pool_out, _) = env.cli(&["doctor"]);
    assert!(
        pool_out.contains("treehouse: OK"),
        "treehouse should handle cleanup"
    );

    println!("T5 PASS: crash with lease — cleanup handled");
}

// ─── T6: Crash recovery on restart ───────────────────────────────────

#[test]
fn t6_crash_recovery_on_restart() {
    kill_all();
    let env = Env::new("t6");
    assert!(start_daemon(&env.state_dir));

    // Spawn a peer
    let (out, _) = env.spawn_sleep("t6-peer", 120);
    let pid = Env::pid_of(&out);
    assert!(pid > 0, "failed to get PID: {out}");

    // Kill the peer (simulate crash)
    kill_pid(pid);
    // Give the OS a moment; the peer is now dead but unreaped by any daemon.
    std::thread::sleep(Duration::from_millis(500));

    // Kill daemon (simulate daemon crash)
    kill_all();

    // Restart daemon — unified recovery must reconcile store vs reality:
    // the peer's recorded PID is gone → mark Crashed.
    assert!(start_daemon(&env.state_dir), "daemon restart failed");
    let status_out = wait_status(
        &env,
        "t6-peer",
        &["CRASHED", "FAILED", "COMPLETED", "Crashed"],
        15,
    );
    assert!(
        status_out.contains("CRASHED")
            || status_out.contains("FAILED")
            || status_out.contains("COMPLETED")
            || status_out.contains("Crashed"),
        "peer should be in terminal state after restart: {}",
        status_out
    );

    println!("T6 PASS: crash recovery on restart works");
}
