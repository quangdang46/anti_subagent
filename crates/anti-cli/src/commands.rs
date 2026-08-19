//! CLI command implementations — all talk to the daemon over the Unix socket.

use anti_daemon::ipc::{self, Request, Response};
use std::path::PathBuf;
use std::process::Command;

fn socket(state_dir: &PathBuf) -> PathBuf {
    ipc::socket_path(state_dir)
}

fn daemon_running(state_dir: &PathBuf) -> bool {
    let sock = socket(state_dir);
    ipc::send_request(&sock, &Request::Ping).is_ok()
}

fn check(resp: Response) -> Result<String, String> {
    match resp {
        Response::Ok(v) => Ok(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?),
        Response::Err { code, message } => Err(format!("{code}: {message}")),
    }
}

pub fn spawn(
    state_dir: &PathBuf,
    id: &str,
    role: &str,
    disposition: Option<&str>,
    harness: &str,
    task: Option<&str>,
    repo: &str,
    parent: Option<&str>,
) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(
        &socket(state_dir),
        &Request::SpawnAgent {
            id: id.to_string(),
            role: role.to_string(),
            disposition: disposition.map(str::to_string),
            harness: harness.to_string(),
            task_path: task.map(str::to_string),
            repo: repo.to_string(),
            parent_id: parent.map(str::to_string),
            prompt: None,
        },
    )?;
    check(resp)
}

pub fn list(
    state_dir: &PathBuf,
    role: Option<&str>,
    status: Option<&str>,
    json: bool,
) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(&socket(state_dir), &Request::ListAgents)?;
    let v = match resp {
        Response::Ok(v) => v,
        Response::Err { code, message } => return Err(format!("{code}: {message}")),
    };
    let agents = v.as_array().cloned().unwrap_or_default();
    let filtered: Vec<_> = agents
        .into_iter()
        .filter(|a| {
            let ok_role = role.map_or(true, |r| a.get("role").and_then(|x| x.as_str()) == Some(r));
            let ok_status = status.map_or(true, |s| {
                a.get("status").and_then(|x| x.as_str()) == Some(s)
            });
            ok_role && ok_status
        })
        .collect();
    if json {
        Ok(serde_json::to_string_pretty(&filtered).map_err(|e| e.to_string())?)
    } else {
        let mut out = String::from("ID\tROLE\tSTATUS\tPID\tTASK");
        for a in filtered {
            let id = a.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let role = a.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let status = a.get("status").and_then(|x| x.as_str()).unwrap_or("");
            let pid = a
                .get("pid")
                .and_then(|x| x.as_i64())
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into());
            let task = a.get("task_path").and_then(|x| x.as_str()).unwrap_or("");
            out.push_str(&format!("\n{id}\t{role}\t{status}\t{pid}\t{task}"));
        }
        Ok(out)
    }
}

pub fn status(state_dir: &PathBuf, id: &str) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(
        &socket(state_dir),
        &Request::GetAgent { id: id.to_string() },
    )?;
    check(resp)
}

pub fn wait(state_dir: &PathBuf, id: &str, until: &str, timeout: u64) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(
        &socket(state_dir),
        &Request::WaitAgent {
            id: id.to_string(),
            until: until.to_string(),
            timeout_secs: timeout,
        },
    )?;
    check(resp)
}

pub fn stop(state_dir: &PathBuf, id: &str, force: bool) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(
        &socket(state_dir),
        &Request::StopAgent {
            id: id.to_string(),
            force,
        },
    )?;
    check(resp)
}

pub fn restart(state_dir: &PathBuf, id: &str) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(
        &socket(state_dir),
        &Request::RestartAgent { id: id.to_string() },
    )?;
    check(resp)
}

pub fn daemon(state_dir: &PathBuf, action: crate::DaemonAction) -> Result<String, String> {
    match action {
        crate::DaemonAction::Start => {
            if daemon_running(state_dir) {
                return Ok("daemon already running".into());
            }
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let daemon_bin = exe
                .parent()
                .map(|p| p.join("anti-daemon"))
                .unwrap_or_else(|| PathBuf::from("anti-daemon"));
            // The daemon daemonizes itself (new process group, detached from
            // the parent shell), so a killed parent shell never takes it down.
            let child = Command::new(&daemon_bin)
                .env("ANTI_STATE_DIR", state_dir)
                .spawn()
                .map_err(|e| format!("cannot start daemon: {e}"))?;
            // Give the socket a moment to appear.
            for _ in 0..50 {
                if daemon_running(state_dir) {
                    return Ok(format!("daemon started (pid {})", child.id()));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err("daemon failed to come up within 5s".into())
        }
        crate::DaemonAction::Stop => {
            let sock = socket(state_dir);
            if !sock.exists() {
                return Ok("daemon not running".into());
            }
            // Send a shutdown request; the daemon's serve loop exits and the
            // process terminates, removing the socket.
            let req = serde_json::json!({"method": "Shutdown"});
            #[allow(unused_variables)]
            let line = format!("{req}\n");
            #[cfg(unix)]
            {
                if let Ok(stream) = std::os::unix::net::UnixStream::connect(&sock) {
                    let mut stream = stream;
                    use std::io::Write;
                    let _ = stream.write_all(line.as_bytes());
                }
            }
            // Wait for the socket to disappear.
            for _ in 0..50 {
                if !sock.exists() {
                    return Ok("daemon stopped".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err("daemon did not stop within 5s".into())
        }
        crate::DaemonAction::Status => {
            let sock = socket(state_dir);
            if sock.exists() && daemon_running(state_dir) {
                Ok(format!("daemon running (socket {})", sock.display()))
            } else {
                Err("daemon not running".into())
            }
        }
    }
}

pub fn guard(state_dir: &PathBuf, action: crate::GuardAction) -> Result<String, String> {
    match action {
        crate::GuardAction::Test { tool } => {
            // Local classification — no daemon needed (mirrors the guard script's stem scan).
            // Local classification (mirrors the guard script's stem scan).
            let normalized: String = tool
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect();
            let stems = [
                "agent",
                "subagent",
                "task",
                "workflow",
                "cron",
                "schedul",
                "worktree",
                "delegate",
                "spawn",
                "dispatch",
                "handoff",
                "remote",
                "sendmessage",
                "monitor",
            ];
            let matched = stems.iter().find(|s| normalized.contains(**s));
            Ok(match matched {
                Some(s) => format!("deny (delegation-shaped, stem '{s}')"),
                None => "allow".to_string(),
            })
        }
        crate::GuardAction::Install { workspace } => {
            let ws = std::path::Path::new(&workspace);
            if !ws.is_dir() {
                return Err(format!("workspace does not exist: {workspace}"));
            }
            let claude_dir = ws.join(".claude");
            std::fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;
            let hooks_path = claude_dir.join("hooks.json");
            let guard_script = std::env::var("HOME")
                .map(|h| format!("{h}/.anti_subagent/guard/anti-guard.sh"))
                .unwrap_or_else(|_| "anti-guard.sh".to_string());
            let existing = if hooks_path.exists() {
                std::fs::read_to_string(&hooks_path).unwrap_or_else(|_| "{}".to_string())
            } else {
                "{}".to_string()
            };
            let mut v: serde_json::Value =
                serde_json::from_str(&existing).map_err(|e| format!("invalid hooks.json: {e}"))?;
            let hooks = v.as_object_mut().ok_or("hooks.json must be an object")?;
            hooks.insert(
                "PreToolUse".to_string(),
                serde_json::json!([
                    {
                        "matcher": ".*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": format!("{guard_script} --claude")
                            }
                        ]
                    }
                ]),
            );
            std::fs::write(
                &hooks_path,
                serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            Ok(format!("guard installed at {}", hooks_path.display()))
        }
        crate::GuardAction::Status => {
            let sock = socket(state_dir);
            if sock.exists() && daemon_running(state_dir) {
                Ok("guard: daemon up — fail-closed active (delegation tools denied)".to_string())
            } else {
                Ok(
                    "guard: daemon DOWN — guard fails closed (delegation tools denied locally)"
                        .to_string(),
                )
            }
        }
    }
}

pub fn doctor(state_dir: &PathBuf) -> Result<String, String> {
    let mut lines = vec![format!("state_dir: {}", state_dir.display())];
    lines.push(if daemon_running(state_dir) {
        "daemon: OK".to_string()
    } else {
        "daemon: NOT RUNNING".to_string()
    });

    // IPC transport info
    let transport = anti_core::config::IpcTransport::auto();
    let socket = socket(state_dir);
    let transport_ok = if daemon_running(state_dir) {
        ipc::send_request(&socket, &Request::Ping).is_ok()
    } else {
        false
    };
    lines.push(format!(
        "ipc_transport: {} ({})",
        transport.name(),
        if transport_ok {
            "reachable"
        } else if daemon_running(state_dir) {
            "configured but unreachable"
        } else {
            "daemon not running"
        }
    ));

    let treehouse = std::process::Command::new("treehouse")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    lines.push(if treehouse {
        "treehouse: OK".to_string()
    } else {
        "treehouse: NOT FOUND (install treehouse-core)".to_string()
    });

    let claude = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    lines.push(if claude {
        "claude: OK".to_string()
    } else {
        "claude: NOT FOUND".to_string()
    });

    let db = state_dir.join("state.db");
    lines.push(if db.exists() {
        format!(
            "state.db: present ({} bytes)",
            std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0)
        )
    } else {
        "state.db: missing (start daemon once to create)".to_string()
    });

    Ok(lines.join("\n"))
}

pub fn work(state_dir: &PathBuf, action: crate::WorkAction) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    match action {
        crate::WorkAction::Submit {
            id,
            sha,
            path,
            timeout,
        } => {
            let resp = ipc::send_request(
                &socket(state_dir),
                &Request::SubmitWork {
                    id,
                    sha256: sha,
                    artifact_path: path,
                    review_timeout_secs: timeout,
                },
            )?;
            check(resp)
        }
        crate::WorkAction::Review { id, verdict, note } => {
            let resp = ipc::send_request(
                &socket(state_dir),
                &Request::ReviewWork { id, verdict, note },
            )?;
            check(resp)
        }
        crate::WorkAction::List => {
            let resp = ipc::send_request(&socket(state_dir), &Request::ListWorkItems)?;
            let v = match resp {
                Response::Ok(v) => v,
                Response::Err { code, message } => return Err(format!("{code}: {message}")),
            };
            let items = v.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                return Ok("No work items.".into());
            }
            let mut out = String::from("ID\tSTATE\tREV\tPEER\tDEADLINE");
            for item in &items {
                let id = item.get("id").and_then(|x| x.as_str()).unwrap_or("");
                let state = item.get("state").and_then(|x| x.as_str()).unwrap_or("");
                let rev = item.get("revision").and_then(|x| x.as_i64()).unwrap_or(0);
                let peer = item.get("peer_id").and_then(|x| x.as_str()).unwrap_or("");
                let deadline = item
                    .get("review_deadline")
                    .and_then(|x| x.as_str())
                    .unwrap_or("-");
                out.push_str(&format!("\n{id}\t{state}\t{rev}\t{peer}\t{deadline}"));
            }
            Ok(out)
        }
    }
}

pub fn report(
    state_dir: &PathBuf,
    task_id: &str,
    status: &str,
    commit: Option<&str>,
    error: Option<&str>,
    message: Option<&str>,
) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    let resp = ipc::send_request(
        &socket(state_dir),
        &Request::ReportTask {
            task_id: task_id.to_string(),
            status: status.to_string(),
            commit: commit.map(str::to_string),
            error: error.map(str::to_string),
            message: message.map(str::to_string),
        },
    )?;
    check(resp)
}

pub fn escalations(state_dir: &PathBuf) -> Result<String, String> {
    if !daemon_running(state_dir) {
        return Err("daemon not running — start it first with `anti daemon start`".into());
    }
    // List all work items to find any with recent ReviewEscalated events.
    // For the MVP, we list Submitted items past their deadline.
    let resp = ipc::send_request(&socket(state_dir), &Request::ListWorkItems)?;
    let v = match resp {
        Response::Ok(v) => v,
        Response::Err { code, message } => return Err(format!("{code}: {message}")),
    };
    let items = v.as_array().cloned().unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    let overdue: Vec<_> = items
        .iter()
        .filter(|item| {
            item.get("state").and_then(|x| x.as_str()) == Some("Submitted")
                && item
                    .get("review_deadline")
                    .and_then(|x| x.as_str())
                    .map(|d| d < now.as_str())
                    .unwrap_or(false)
        })
        .collect();
    if overdue.is_empty() {
        return Ok("No overdue reviews (no escalations).".into());
    }
    let mut out = String::from("OVERDUE REVIEWS (escalation candidates):");
    for item in &overdue {
        let id = item.get("id").and_then(|x| x.as_str()).unwrap_or("");
        let peer = item.get("peer_id").and_then(|x| x.as_str()).unwrap_or("");
        let deadline = item
            .get("review_deadline")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let rev = item.get("revision").and_then(|x| x.as_i64()).unwrap_or(0);
        out.push_str(&format!(
            "\n  {id} peer={peer} rev={rev} deadline={deadline}"
        ));
    }
    Ok(out)
}
