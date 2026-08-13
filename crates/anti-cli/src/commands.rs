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
            let ok_status = status
                .map_or(true, |s| a.get("status").and_then(|x| x.as_str()) == Some(s));
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
    let resp = ipc::send_request(&socket(state_dir), &Request::GetAgent { id: id.to_string() })?;
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
            let child = Command::new(daemon_bin)
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
            // P0: no control message yet — kill by pidfile would be ideal; for
            // now we require the socket to disappear after killing the process.
            let sock = socket(state_dir);
            if !sock.exists() {
                return Ok("daemon not running".into());
            }
            Err("stop not implemented in P0 — kill the anti-daemon process manually (e.g. pkill -f anti-daemon)".into())
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

pub fn doctor(state_dir: &PathBuf) -> Result<String, String> {
    let mut lines = vec![format!("state_dir: {}", state_dir.display())];
    lines.push(if daemon_running(state_dir) {
        "daemon: OK".to_string()
    } else {
        "daemon: NOT RUNNING".to_string()
    });

    let treehouse = std::process::Command::new("treehouse")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    lines.push(if treehouse {
        "treehouse: OK".to_string()
    } else {
        "treehouse: NOT FOUND (install or set config treehouse_bin)".to_string()
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
        format!("state.db: present ({} bytes)", std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0))
    } else {
        "state.db: missing (start daemon once to create)".to_string()
    });

    Ok(lines.join("\n"))
}
