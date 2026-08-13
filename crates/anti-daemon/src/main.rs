use anti_core::events::EventType;
use anti_core::model::{AgentRecord, AgentStatus, Harness, Role};
use anti_daemon::ipc::{self, Request, Response};
use anti_daemon::store::Store;
use anti_daemon::wait;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let state_dir = std::env::var("ANTI_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".anti_subagent"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    let socket = ipc::socket_path(&state_dir);

    let store = match Store::open(&state_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("anti-daemon: failed to open store: {e}");
            std::process::exit(1);
        }
    };

    let handle = |store: &mut Store, req: Request| -> Response {
        match req {
            Request::Ping => Response::ok(json!({"pong": true})),
            Request::SpawnAgent {
                id,
                role,
                disposition,
                harness,
                task_path,
                repo,
                parent_id,
            } => spawn(store, &id, &role, disposition.as_deref(), &harness, task_path.as_deref(), &repo, parent_id.as_deref()),
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
                    &store,
                    &id,
                    until_status,
                    timeout,
                    Duration::from_millis(100),
                ) {
                    Ok(status) => Response::ok(json!({"id": id, "status": format!("{:?}", status)})),
                    Err(e) => Response::err("wait", e),
                }
            }
        }
    };

    let mut closure_store = store;
    eprintln!(
        "anti-daemon: listening on {} (seq={})",
        socket.display(),
        closure_store.current_sequence()
    );
    if let Err(e) = ipc::serve(&socket, |req| handle(&mut closure_store, req)) {
        eprintln!("anti-daemon: server error: {e}");
        std::process::exit(1);
    }
}

/// Spawn an agent: persist BEFORE spawn (plan §15, §18) — reserve id, write
/// record, then launch. P0 launches the harness in a PTY via `script`.
fn spawn(
    store: &mut Store,
    id: &str,
    role: &str,
    disposition: Option<&str>,
    harness: &str,
    task_path: Option<&str>,
    repo: &str,
    parent_id: Option<&str>,
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
    let _ = store.append_event(id, EventType::AgentRegistered, json!({"role": role, "harness": harness}));
    if let Err(e) = store.update_status(id, AgentStatus::Starting) {
        return Response::err("store", format!("{e}"));
    }
    let _ = store.append_event(id, EventType::AgentStarted, json!({"phase": "spawning"}));

    // 4. spawn the harness in a PTY (independent OS process, not a subagent)
    let mut cmd = std::process::Command::new("script");
    let log_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".anti_subagent/logs"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/anti_logs"));
    std::fs::create_dir_all(&log_path).ok();
    let log_file = log_path.join(format!("{id}.log"));
    cmd.args(["-q", log_file.to_str().unwrap_or("/dev/null"), "claude", "--permission-mode", "acceptEdits", "--append-system-prompt", "You are a peer working on this repository with the project owner. Work independently."]);
    cmd.current_dir(repo);
    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            let _ = store.attach_pid(id, pid);
            let _ = store.update_status(id, AgentStatus::Running);
            let _ = store.append_event(id, EventType::AgentStarted, json!({"pid": pid}));
            Response::ok(json!({"id": id, "status": "running", "pid": pid, "workspace": null}))
        }
        Err(e) => {
            let _ = store.update_status(id, AgentStatus::Failed);
            let _ = store.append_event(id, EventType::AgentFailed, json!({"error": e.to_string()}));
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
