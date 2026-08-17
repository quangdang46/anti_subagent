use anti_adapters::{ClaudeCodeAdapter, CodexAdapter, HarnessAdapter, OpenCodeAdapter, SpawnContext};
use anti_core::events::EventType;
use anti_core::model::{AgentRecord, AgentStatus, Harness, Role};
use anti_daemon::ipc::{self, Request, Response};
use anti_daemon::store::Store;
use anti_daemon::wait;
use anti_workspace::Treehouse;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

/// Resolve treehouse binary: check TREEHOUSE_BIN env, then PATH, then fallback.
fn resolve_treehouse() -> PathBuf {
    // 1. Explicit env var (highest priority)
    if let Ok(p) = std::env::var("TREEHOUSE_BIN") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return path;
        }
        // On Windows, try adding .exe if not present
        #[cfg(windows)]
        if !p.ends_with(".exe") {
            let with_exe = PathBuf::from(format!("{p}.exe"));
            if with_exe.exists() {
                return with_exe;
            }
        }
    }
    // 2. Search PATH
    if let Ok(output) = std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg("treehouse")
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                let p = PathBuf::from(first_line.trim());
                if p.exists() {
                    return p;
                }
            }
        }
    }
    // 3. Fallback
    PathBuf::from("treehouse")
}

/// Daemonize: detach from the parent's process group/session so a killed
/// parent shell (e.g. a timed-out Bash tool call) never takes the daemon
/// down with it. macOS has no `setsid` binary, so we do it in-process.
#[cfg(unix)]
fn daemonize() {
    use std::os::unix::process::CommandExt;
    if std::env::var("ANTI_DAEMONIZED").is_ok() {
        return; // already detached
    }
    // Fork via spawning ourselves detached with the flag set.
    let exe = std::env::current_exe().unwrap_or_default();
    let mut cmd = std::process::Command::new(&exe);
    cmd.env("ANTI_DAEMONIZED", "1");
    for (k, v) in std::env::vars() {
        if k != "ANTI_DAEMONIZED" {
            cmd.env(k, v);
        }
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd.process_group(0); // new process group
    let _ = cmd.spawn();
    std::process::exit(0);
}

fn main() {
    #[cfg(unix)]
    daemonize();
    let state_dir = std::env::var("ANTI_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".anti_subagent"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    let socket = ipc::socket_path(&state_dir);

    let mut store = match Store::open(&state_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("anti-daemon: failed to open store: {e}");
            std::process::exit(1);
        }
    };

    // Reconcile on restart (plan §23): peers whose processes died while the
    // daemon was down become CRASHED/COMPLETED.
    reconcile_on_start(&mut store);

    // Track live children so a peer's exit becomes AGENT_COMPLETED/CRASHED.
    let children: HashMap<String, Child> = HashMap::new();

    let handle = |store: &mut Store, children: &mut HashMap<String, Child>, req: Request| -> Response {
        match req {
            Request::Shutdown => Response::ok(json!({"shutdown": true})),
            Request::Ping => Response::ok(json!({"pong": true})),
            // Guard policy: peers are never allowed to delegate (plan §22).
            Request::GuardCheck { tool } => {
                // Check if tool is delegation-shaped (existing logic)
                let is_delegation = tool.contains("agent")
                    || tool.contains("subagent")
                    || tool.contains("spawn")
                    || tool.contains("dispatch")
                    || tool.contains("delegate");

                if is_delegation {
                    Response::ok(json!({"tool": tool, "allowed": false, "reason": "delegation-shaped tool denied"}))
                } else {
                    Response::ok(json!({"tool": tool, "allowed": true}))
                }
            }
            Request::SpawnAgent {
                id,
                role,
                disposition,
                harness,
                task_path,
                repo,
                parent_id,
                prompt,
            } => spawn(store, children, &id, &role, disposition.as_deref(), &harness, task_path.as_deref(), &repo, parent_id.as_deref(), prompt.as_deref()),
            Request::ListAgents => match store.list_agents() {
                Ok(agents) => Response::ok(agents),
                Err(e) => Response::err("store", e.to_string()),
            },
            Request::GetAgent { id } => match store.get_agent(&id) {
                Ok(Some(rec)) => Response::ok(rec),
                Ok(None) => Response::err("not_found", format!("agent {id} not found")),
                Err(e) => Response::err("store", e.to_string()),
            },
            Request::WaitAgent {
                id,
                until,
                timeout_secs,
            } => {
                let until_status = parse_status(&until).unwrap_or(AgentStatus::Completed);
                let timeout = Duration::from_secs(timeout_secs.max(1));
                match wait::wait_for_status(
                    store,
                    &id,
                    until_status,
                    timeout,
                    Duration::from_millis(100),
                ) {
                    Ok(status) => Response::ok(json!({"id": id, "status": format!("{:?}", status)})),
                    Err(e) => Response::err("wait", e),
                }
            }
            Request::StopAgent { id, force } => {
                let mut cmd = std::process::Command::new("kill");
                if force {
                    cmd.arg("-9");
                } else {
                    cmd.arg("-TERM");
                }
                cmd.arg(&id);
                // find the pid from the registry
                let pid = store.get_agent(&id).ok().flatten().and_then(|r| r.pid);
                match pid {
                    Some(pid) => {
                        let st = std::process::Command::new("kill")
                            .args([if force { "-9" } else { "-TERM" }, &pid.to_string()])
                            .status();
                        match st {
                            Ok(s) if s.success() => {
                                let _ = store.update_status(&id, AgentStatus::Stopped);
                                let _ = store.append_event(&id, EventType::AgentStopped, json!({"force": force}));
                                Response::ok(json!({"id": id, "status": "stopped"}))
                            }
                            Ok(_) => Response::err("stop", format!("kill returned failure for {id}")),
                            Err(e) => Response::err("stop", e.to_string()),
                        }
                    }
                    None => Response::err("not_found", format!("no pid for {id}")),
                }
            }
            Request::RestartAgent { id } => {
                match restart_agent(store, children, &id) {
                    Ok(pid) => Response::ok(json!({"id": id, "status": "restarting", "pid": pid})),
                    Err(e) => Response::err("restart", e),
                }
            }
            Request::SubmitWork { id, sha256, artifact_path, review_timeout_secs } => {
                handle_submit_work(store, &id, &sha256, &artifact_path, review_timeout_secs)
            }
            Request::ReviewWork { id, verdict, note } => {
                handle_review_work(store, &id, &verdict, &note)
            }
            Request::VerifyWork { id, profile } => {
                handle_verify_work(store, &id, &profile)
            }
            Request::CheckDisposition { disposition, tool } => {
                handle_check_disposition(&disposition, &tool)
            }
            Request::ListWorkItems => {
                match store.list_work_items(None) {
                    Ok(items) => Response::ok(items),
                    Err(e) => Response::err("store", e.to_string()),
                }
            }
        }
    };

    eprintln!(
        "anti-daemon: listening on {} (seq={})",
        socket.display(),
        store.current_sequence()
    );
    // Periodic reaper: a peer's exit must become COMPLETED/CRASHED even when
    // no IPC request ever arrives (e.g. a long `anti wait`). Without this the
    // benchmark would hang forever on a dead agent.
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let children = std::sync::Arc::new(std::sync::Mutex::new(children));
    // Reaper uses try_lock so it can never block or deadlock the IPC loop.
    let (rs, rc) = (store.clone(), children.clone());
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        if let (Ok(mut s), Ok(mut c)) = (rs.try_lock(), rc.try_lock()) {
            reap_children(&mut s, &mut c);
        }
    });
    // Lease sweeper: releases treehouse leases of agents that reached a
    // terminal state. Runs OUTSIDE the state lock (treehouse subprocess can
    // block), so it never stalls IPC. Treehouse acquire skips leased
    // worktrees, but without this the pool fills up over many runs.
    let sweeper_store = store.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(15));
        let terminal: Vec<(String, String, String)> = {
            let s = match sweeper_store.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            s.list_agents()
                .unwrap_or_default()
                .into_iter()
                .filter(|a| a.status.is_terminal())
                .filter_map(|a| {
                    a.workspace
                        .map(|w| (a.id.clone(), w.lease_id.clone(), w.path.clone()))
                })
                .collect()
        };
        for (id, lease_id, path) in terminal {
            let _ = Treehouse::new(resolve_treehouse()).release_if_lease(
                &lease_id,
                std::path::Path::new(&path),
                std::path::Path::new(&path),
            );
            if let Ok(s) = sweeper_store.lock() {
                let _ = s.clear_workspace(&id);
            }
        }
    });
    // Review watchdog: mỗi 15s, quét overdue reviews.
    // Bài học veylen: lead im lặng = kẹt vô thời hạn. Escalate, không auto-accept.
    let watchdog_store = store.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(15));
        let s = match watchdog_store.lock() {
            Ok(g) => g,
            Err(_) => continue,
        };
        let now = chrono::Utc::now().to_rfc3339();
        let overdue = match s.overdue_reviews(&now) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for w in overdue {
            let mut s = match watchdog_store.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            let _ = s.append_event(
                &w.id,
                EventType::ReviewEscalated,
                json!({
                    "peer_id": w.peer_id,
                    "lead_id": w.lead_id,
                    "revision": w.revision,
                    "deadline": w.review_deadline,
                    "action": "supervisor intervention required",
                }),
            );
        }
    });
    let (s2, c2) = (store.clone(), children.clone());
    let dispatch = move |req: Request| -> Response {
        // WaitAgent must NOT hold the state locks while it loops for minutes —
        // that would starve every other request. It polls with short, discrete
        // lock acquisitions instead.
        if let Request::WaitAgent {
            id,
            until,
            timeout_secs,
        } = &req
        {
            let until_status = parse_status(until).unwrap_or(AgentStatus::Completed);
            let timeout = Duration::from_secs((*timeout_secs).max(1));
            let deadline = std::time::Instant::now() + timeout;
            let mut last_seq = 0u64;
            loop {
                let status = {
                    let s = match s2.lock() {
                        Ok(g) => g,
                        Err(_) => return Response::err("internal", "state lock poisoned"),
                    };
                    let cur = s.current_sequence();
                    let rec = s.get_agent(id).ok().flatten();
                    (cur, rec.map(|r| r.status))
                };
                let (seq, status) = status;
                if let Some(st) = status {
                    if st == until_status {
                        return Response::ok(json!({"id": id, "status": format!("{:?}", st)}));
                    }
                    if st.is_terminal() && st != until_status {
                        return Response::ok(json!({"id": id, "status": format!("{:?}", st)}));
                    }
                }
                if seq != last_seq {
                    last_seq = seq;
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    return Response::err("wait", format!("timeout after {timeout:?} waiting for {id}"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let (mut s, mut c) = match (s2.lock(), c2.lock()) {
            (Ok(s), Ok(c)) => (s, c),
            _ => return Response::err("internal", "state lock poisoned"),
        };
        reap_children(&mut s, &mut c);
        handle(&mut s, &mut c, req)
    };
    // serve returns Ok(()) on graceful Shutdown or Err on failure — either
    // way the daemon process must exit so the socket is cleaned up.
    if let Err(e) = ipc::serve(&socket, dispatch) {
        eprintln!("anti-daemon: server error: {e}");
    }
    std::process::exit(0);
}

/// Mark agents whose process died while the daemon was down (plan §23).
fn handle_submit_work(
    store: &mut Store,
    id: &str,
    sha256: &str,
    artifact_path: &str,
    review_timeout_secs: u64,
) -> Response {
    let mut w = match store.get_work_item(id) {
        Ok(Some(w)) => w,
        Ok(None) => {
            // Auto-create: Pending → InProgress
            let mut w = anti_core::work::WorkItem::new(id.to_string(), "cli".into());
            if let Err(e) = w.transition(anti_core::work::WorkItemState::InProgress) {
                return Response::err("transition", e.to_string());
            }
            w
        }
        Err(e) => return Response::err("store", e.to_string()),
    };

    let evidence = anti_core::work::EvidenceRef {
        sha256: sha256.to_string(),
        artifact_path: artifact_path.to_string(),
        produced_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = w.submit(evidence, review_timeout_secs) {
        return Response::err("transition", e.to_string());
    }

    // Save: INSERT OR REPLACE handles both new items and re-submits after reject
    if let Err(e) = store.insert_work_item(&w) {
        return Response::err("store", e.to_string());
    }
    let _ = store.append_event(
        id,
        EventType::WorkSubmitted,
        json!({
            "sha256": sha256,
            "artifact_path": artifact_path,
            "review_deadline": w.review_deadline,
        }),
    );
    Response::ok(json!({"id": id, "state": "Submitted", "review_deadline": w.review_deadline}))
}

fn handle_review_work(
    store: &mut Store,
    id: &str,
    verdict: &str,
    note: &str,
) -> Response {
    let mut w = match store.get_work_item(id) {
        Ok(Some(w)) => w,
        Ok(None) => return Response::err("not_found", format!("work item {id} not found")),
        Err(e) => return Response::err("store", e.to_string()),
    };
    match verdict {
        "accept" => {
            if w.state != anti_core::work::WorkItemState::Verified {
                return Response::err("precondition", "accept requires Verified state — run verify first");
            }
            if let Err(e) = w.transition(anti_core::work::WorkItemState::Accepted) {
                return Response::err("transition", e.to_string());
            }
            let _ = store.insert_work_item(&w);
            let _ = store.append_event(id, EventType::WorkAccepted, json!({"note": note}));
            Response::ok(json!({"id": id, "state": "Accepted"}))
        }
        "reject" => {
            if let Err(e) = w.reject("lead", note) {
                return Response::err("transition", e.to_string());
            }
            let _ = store.insert_work_item(&w);
            let _ = store.append_event(id, EventType::WorkRejected, json!({
                "note": note,
                "revision": w.revision,
            }));
            Response::ok(json!({"id": id, "state": format!("{:?}", w.state), "revision": w.revision}))
        }
        other => Response::err("invalid", format!("unknown verdict '{other}' — use 'accept' or 'reject'")),
    }
}

fn handle_verify_work(store: &mut Store, id: &str, profile_str: &str) -> Response {
    use anti_core::work::{VerifyProfile, VerificationResult, VerifyStatus};

    let mut w = match store.get_work_item(id) {
        Ok(Some(w)) => w,
        Ok(None) => return Response::err("not_found", format!("work item {id} not found")),
        Err(e) => return Response::err("store", e.to_string()),
    };

    if w.state != anti_core::work::WorkItemState::Submitted {
        return Response::err("precondition", "verify requires Submitted state");
    }

    let profile = match profile_str {
        "full" => VerifyProfile::Full,
        "check" => VerifyProfile::Check,
        "test" => VerifyProfile::Test,
        "build" => VerifyProfile::Build,
        other if other.starts_with("named:") => {
            VerifyProfile::Named(other.strip_prefix("named:").unwrap_or("").to_string())
        }
        _ => return Response::err("invalid", format!("unknown profile '{profile_str}' — use full/check/test/build/named:<name>")),
    };

    let mut result = VerificationResult::new(profile.clone());
    let mut all_pass = true;

    // Run each command in the profile
    for cmd in profile.commands() {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let program = parts[0];
        let args = &parts[1..];

        let output = std::process::Command::new(program)
            .args(args)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = format!("{stdout}\n{stderr}");
                let code = out.status.code().unwrap_or(-1);

                if cmd.contains("test") {
                    result.test_output = Some(combined.clone());
                    result.test_exit_code = Some(code);
                } else if cmd.contains("build") {
                    result.build_output = Some(combined.clone());
                    result.build_exit_code = Some(code);
                }

                if !out.status.success() {
                    all_pass = false;
                    result.diagnostics.push(format!("[{cmd}] exit {code}: {stderr}"));
                }
            }
            Err(e) => {
                all_pass = false;
                result.diagnostics.push(format!("[{cmd}] failed to execute: {e}"));
            }
        }
    }

    // Capture git state
    if let Ok(out) = std::process::Command::new("git").args(["rev-parse", "HEAD"]).output() {
        result.git_sha = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    if let Ok(out) = std::process::Command::new("git").args(["diff", "--stat"]).output() {
        let diff = String::from_utf8_lossy(&out.stdout).to_string();
        if !diff.trim().is_empty() {
            result.git_diff = Some(diff);
        }
    }

    result.status = if all_pass { VerifyStatus::Pass } else { VerifyStatus::Fail };

    // Transition state based on verification result
    let new_state = if all_pass {
        anti_core::work::WorkItemState::Verified
    } else {
        // Stay at Submitted on failure — peer must fix and resubmit
        anti_core::work::WorkItemState::Submitted
    };

    if let Err(e) = w.transition(new_state) {
        return Response::err("transition", e.to_string());
    }

    let _ = store.insert_work_item(&w);
    let _ = store.append_event(id, EventType::WorkVerified, json!({
        "status": format!("{:?}", result.status),
        "profile": format!("{:?}", result.profile),
        "diagnostics_count": result.diagnostics.len(),
    }));

    Response::ok(json!({
        "id": id,
        "status": format!("{:?}", result.status),
        "state": format!("{:?}", w.state),
        "profile": profile_str,
        "diagnostics": result.diagnostics,
    }))
}

fn handle_check_disposition(disposition_str: &str, tool: &str) -> Response {
    use anti_core::disposition::{contract_for, DispositionError};
    use anti_core::model::Disposition;

    let disposition = match disposition_str {
        "engineer" => Disposition::Engineer,
        "architect" => Disposition::Architect,
        "reviewer" => Disposition::Reviewer,
        "scout" => Disposition::Scout,
        "proof_auditor" | "proofauditor" => Disposition::ProofAuditor,
        "shadow" => Disposition::Shadow,
        _ => return Response::err("invalid", format!("unknown disposition '{disposition_str}'")),
    };

    let contract = contract_for(disposition);
    match contract.check_tool(tool) {
        Ok(()) => Response::ok(json!({
            "disposition": disposition_str,
            "tool": tool,
            "allowed": true,
        })),
        Err(DispositionError::ToolDenied { .. }) => Response::ok(json!({
            "disposition": disposition_str,
            "tool": tool,
            "allowed": false,
            "reason": "tool denied for this disposition",
        })),
        Err(e) => Response::err("disposition", e.to_string()),
    }
}

fn reconcile_on_start(store: &mut Store) {
    let agents = match store.list_agents() {
        Ok(a) => a,
        Err(_) => return,
    };
    for rec in agents {
        if !matches!(
            rec.status,
            AgentStatus::Running | AgentStatus::Blocked | AgentStatus::Starting
        ) {
            continue;
        }
        let alive = rec
            .pid
            .map(|pid| {
                std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !alive {
            let _ = store.mark_exit(&rec.id, false);
        }
    }
}

/// Poll children with try_wait; on exit, mark the agent Completed/Crashed.
fn reap_children(store: &mut Store, children: &mut HashMap<String, Child>) {
    let dead: Vec<(String, bool, Option<i32>)> = children
        .iter_mut()
        .filter_map(|(id, child)| {
            child
                .try_wait()
                .ok()
                .flatten()
                // claude -p can exit non-zero (1-2) with warnings even when
                // the task succeeded (is_error=false in the JSON output).
                // Treat exit code ≤ 2 as success.
                .map(|status| {
                    let code = status.code();
                    let ok = code.unwrap_or(1) <= 2;
                    (id.clone(), ok, code)
                })
        })
        .collect();
    for (id, ok, exit_code) in dead {
        children.remove(&id);

        // Capture workspace info before mark_exit (which may clear it)
        let workspace_lease = store.get_agent(&id).ok().flatten().and_then(|a| a.workspace);

        let _ = store.mark_exit(&id, ok);

        // Emit structured PeerCrashed event with crash evidence
        if !ok {
            let payload = json!({
                "exit_code": exit_code,
                "workspace_lease_id": workspace_lease.as_ref().map(|w| &w.lease_id),
                "workspace_path": workspace_lease.as_ref().map(|w| &w.path),
                "crash_evidence": {
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "process_exit_code": exit_code,
                    "reason": if exit_code == Some(137) { "killed" } else { "crashed" },
                },
            });
            let _ = store.append_event(&id, EventType::PeerCrashed, payload);

            // Cleanup workspace via Treehouse (release lease + clean worktree)
            if let Some(lease) = workspace_lease {
                let treehouse = Treehouse::new(resolve_treehouse());
                let _ = treehouse.release_if_lease(
                    &lease.lease_id,
                    std::path::Path::new(&lease.path),
                    std::path::Path::new("."),
                );
            }
        }
    }
}

/// Supervised restart (plan §17, §23): CRASHED → RECOVERING → RUNNING with
/// the SAME id, workspace, and task. Replacement is a governance decision —
/// a supervised restart never issues a new id.
fn restart_agent(
    store: &mut Store,
    children: &mut HashMap<String, Child>,
    id: &str,
) -> Result<u32, String> {
    let rec = store
        .get_agent(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("agent {id} not found"))?;
    if !matches!(rec.status, AgentStatus::Crashed | AgentStatus::Recovering) {
        return Err(format!("agent {id} is {:?}, cannot restart", rec.status));
    }

    let _ = store.begin_recovery(id);
    let _ = store.append_event(id, EventType::AgentRestarted, json!({"restart_count": rec.restart_count + 1}));

    // Backoff: 1s * 2^restart_count (cap 30s) so a crash-loop doesn't spin.
    let backoff = std::time::Duration::from_secs((1u64 << rec.restart_count.min(5)).min(30));
    std::thread::sleep(backoff);

    let repo = rec
        .workspace
        .as_ref()
        .map(|ws| ws.path.clone())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });

    let mut cmd = std::process::Command::new("claude");
    let log_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".anti_subagent/logs"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/anti_logs"));
    std::fs::create_dir_all(&log_path).ok();
    let log_file = log_path.join(format!("{id}.log"));
    // Truncate the previous session's log so stale output (e.g. an old trust
    // dialog) can never be misread as this session's state.
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_file);
    cmd.args([
        "-p",
        "--output-format",
        "json",
        "--permission-mode",
        "acceptEdits",
        "--dangerously-skip-permissions",
        "--append-system-prompt",
        "You are a peer working on this repository with the project owner. Work independently.",
    ]);
    cmd.current_dir(&repo);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::from(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .unwrap_or_else(|_| std::fs::OpenOptions::new().create(true).append(true).open("/dev/null").unwrap()),
    ));
    cmd.stderr(std::process::Stdio::inherit());
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    let _ = store.inc_restart(id);
    let _ = store.set_running(id, pid);
    let _ = store.append_event(id, EventType::AgentStarted, json!({"pid": pid, "restart": true}));
    children.insert(id.to_string(), child);
    Ok(pid)
}

/// Spawn an agent with full plan §18 transaction: validate → reserve →
/// persist → treehouse lease → spawn PTY → attach PID → emit events.
fn spawn(
    store: &mut Store,
    children: &mut HashMap<String, Child>,
    id: &str,
    role: &str,
    disposition: Option<&str>,
    harness: &str,
    task_path: Option<&str>,
    repo: &str,
    parent_id: Option<&str>,
    prompt: Option<&str>,
) -> Response {
    // 1. validate
    if id.trim().is_empty() {
        return Response::err("invalid", "id cannot be empty");
    }
    let role_parsed = match role {
        "supervisor" => Role::Supervisor,
        "lead" => Role::Lead,
        "peer" => Role::Peer,
        other => return Response::err("invalid", format!("unknown role {other}")),
    };
    let harness_parsed = match harness {
        "claude" => Harness::Claude,
        "codex" => Harness::Codex,
        "opencode" => Harness::OpenCode,
        other => return Response::err("invalid", format!("unknown harness {other}")),
    };
    if !std::path::Path::new(repo).is_dir() {
        return Response::err("invalid", format!("repo path does not exist: {repo}"));
    }

    // 2-3. reserve id + persist metadata BEFORE spawn (firstmate lesson)
    let now = chrono::Utc::now().to_rfc3339();
    let rec = AgentRecord {
        id: id.to_string(),
        role: role_parsed,
        disposition: disposition.map(parse_disposition),
        harness: harness_parsed,
        parent_id: parent_id.map(str::to_string),
        pid: None,
        workspace: None,
        task_path: task_path.map(str::to_string),
        status: AgentStatus::Created,
        restart_count: 0,
        spawn_gen: 1,
        last_state_change_seq: 0,
        created_at: now.clone(),
        updated_at: now,
    };
    if let Err(e) = store.insert_agent(&rec) {
        return Response::err("duplicate", format!("cannot reserve id {id}: {e}"));
    }
    let _ = store.append_event(
        id,
        EventType::AgentRegistered,
        json!({"role": role, "harness": harness}),
    );
    if let Err(e) = store.update_status(id, AgentStatus::Starting) {
        return Response::err("store", format!("{e}"));
    }

    // 4. allocate workspace lease (plan §19) — failure → FAILED, no ghost
    let treehouse = Treehouse::new(resolve_treehouse());
    let worktree = match treehouse.acquire(id, std::path::Path::new(repo)) {
        Ok(l) => l,
        Err(e) => {
            let _ = store.update_status(id, AgentStatus::Failed);
            let _ = store.append_event(
                id,
                EventType::AgentFailed,
                json!({"error": format!("workspace: {e}")}),
            );
            return Response::err("workspace", e.to_string());
        }
    };
    // Kill any stale process still running inside the freshly-leased worktree.
    // SAFETY: Use PID-based termination from the store, not pattern matching.
    // Pattern-based pkill -f is dangerous — it can match and kill unrelated processes.
    // We only kill processes whose PIDs are tracked in the agent store.
    if let Ok(agents) = store.list_agents() {
        for agent in &agents {
            if let Some(pid) = agent.pid {
                // Check if this agent's workspace matches our new worktree
                if let Some(ws) = &agent.workspace {
                    if ws.path == worktree.path.display().to_string() && agent.id != id {
                        // Found an orphaned process in this worktree — terminate by PID
                        #[cfg(unix)]
                        {
                            let _ = std::process::Command::new("kill")
                                .args(["-TERM", &pid.to_string()])
                                .status();
                            // Give it a moment to exit gracefully
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                        #[cfg(windows)]
                        {
                            // On Windows, use taskkill by PID
                            let _ = std::process::Command::new("taskkill")
                                .args(["/F", "/PID", &pid.to_string()])
                                .status();
                        }
                    }
                }
            }
        }
    }
    let _ = store.set_workspace(id, &worktree.lease_id, &worktree.path.display().to_string());

    // 5-6. spawn the harness non-interactively inside the leased worktree
    let log_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".anti_subagent/logs"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/anti_logs"));
    std::fs::create_dir_all(&log_path).ok();
    let log_file = log_path.join(format!("{id}.log"));
    // Truncate previous session log (see restart_agent).
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_file);
    let peer_prompt = prompt.unwrap_or(
        "You are a peer working on this repository with the project owner. Work independently.",
    );
    // Harness adapter dispatch (plan §25).
    let ctx = SpawnContext {
        worktree: worktree.path.clone(),
        task: task_path.map(str::to_string),
        peer_prompt: Some(peer_prompt.to_string()),
    };
    let adapter: Box<dyn HarnessAdapter> = match harness {
        "codex" => Box::new(CodexAdapter),
        "opencode" => Box::new(OpenCodeAdapter),
        _ => Box::new(ClaudeCodeAdapter),
    };
    let mut cmd = match adapter.spawn_command(&ctx) {
        Ok(c) => c,
        Err(e) => {
            let _ = store.update_status(id, AgentStatus::Failed);
            return Response::err("spawn", e.to_string());
        }
    };
    cmd.stdout(std::process::Stdio::from(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .unwrap_or_else(|_| std::fs::OpenOptions::new().create(true).append(true).open("/dev/null").unwrap()),
    ));
    match cmd.spawn() {
        Ok(mut child) => {
            // Feed the task prompt via stdin for pipe-fed CLIs (claude -p).
            if let Some(task) = task_path {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(task.as_bytes());
                    drop(stdin);
                }
            }
            let pid = child.id();
            let _ = store.attach_pid(id, pid);
            let _ = store.update_status(id, AgentStatus::Running);
            let _ = store.append_event(
                id,
                EventType::AgentStarted,
                json!({"pid": pid, "worktree": worktree.path.display().to_string()}),
            );
            children.insert(id.to_string(), child);
            Response::ok(json!({
                "id": id,
                "status": "running",
                "pid": pid,
                "workspace": {"lease_id": worktree.lease_id, "path": worktree.path.display().to_string()}
            }))
        }
        Err(e) => {
            let _ = store.update_status(id, AgentStatus::Failed);
            let _ = store.append_event(
                id,
                EventType::AgentFailed,
                json!({"error": e.to_string()}),
            );
            let _ = Treehouse::new(resolve_treehouse()).release_if_lease(
                &worktree.lease_id,
                &worktree.path,
                std::path::Path::new(repo),
            );
            Response::err("spawn", format!("{e}"))
        }
    }
}

fn parse_disposition(s: &str) -> anti_core::model::Disposition {
    match s {
        "architect" => anti_core::model::Disposition::Architect,
        "reviewer" => anti_core::model::Disposition::Reviewer,
        "scout" => anti_core::model::Disposition::Scout,
        "proof-auditor" => anti_core::model::Disposition::ProofAuditor,
        "shadow" => anti_core::model::Disposition::Shadow,
        _ => anti_core::model::Disposition::Engineer,
    }
}

fn parse_status(s: &str) -> Option<AgentStatus> {
    Some(match s {
        "completed" | "done" => AgentStatus::Completed,
        "running" => AgentStatus::Running,
        "blocked" => AgentStatus::Blocked,
        "failed" => AgentStatus::Failed,
        "crashed" => AgentStatus::Crashed,
        "stopped" => AgentStatus::Stopped,
        _ => return None,
    })
}
