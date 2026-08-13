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

fn main() {
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
    let mut children: HashMap<String, Child> = HashMap::new();

    let handle = |store: &mut Store, children: &mut HashMap<String, Child>, req: Request| -> Response {
        match req {
            Request::Ping => Response::ok(json!({"pong": true})),
            // Guard policy: peers are never allowed to delegate (plan §22).
            Request::GuardCheck { tool } => {
                Response::ok(json!({"tool": tool, "allowed": false}))
            }
            Request::SpawnAgent {
                id,
                role,
                disposition,
                harness,
                task_path,
                repo,
                parent_id,
            } => spawn(store, children, &id, &role, disposition.as_deref(), &harness, task_path.as_deref(), &repo, parent_id.as_deref()),
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
        }
    };

    eprintln!(
        "anti-daemon: listening on {} (seq={})",
        socket.display(),
        store.current_sequence()
    );
    if let Err(e) = ipc::serve(&socket, |req| {
        // reap children first so completion events fire promptly
        reap_children(&mut store, &mut children);
        handle(&mut store, &mut children, req)
    }) {
        eprintln!("anti-daemon: server error: {e}");
        std::process::exit(1);
    }
}

/// Mark agents whose process died while the daemon was down (plan §23).
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

/// Poll children with try_wait; on exit, mark the agent Completed/Crashed and
/// release its treehouse lease (plan §19 idempotent return).
fn reap_children(store: &mut Store, children: &mut HashMap<String, Child>) {
    let dead: Vec<(String, bool)> = children
        .iter_mut()
        .filter_map(|(id, child)| {
            child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| (id.clone(), status.success()))
        })
        .collect();
    for (id, ok) in dead {
        children.remove(&id);
        let _ = store.mark_exit(&id, ok);
        if let Ok(Some(rec)) = store.get_agent(&id) {
            if let Some(ws) = &rec.workspace {
                let _ = Treehouse::new(PathBuf::from("treehouse")).release_if_lease(
                    &ws.lease_id,
                    std::path::Path::new(&ws.path),
                    std::path::Path::new(&ws.path),
                );
            }
        }
    }
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
    let treehouse = Treehouse::new(
        std::env::var("TREEHOUSE_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("treehouse")),
    );
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
    let _ = store.set_workspace(id, &worktree.lease_id, &worktree.path.display().to_string());

    // 5-6. spawn the harness in a PTY inside the leased worktree
    let mut cmd = std::process::Command::new("script");
    let log_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".anti_subagent/logs"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/anti_logs"));
    std::fs::create_dir_all(&log_path).ok();
    let log_file = log_path.join(format!("{id}.log"));
    cmd.args([
        "-q",
        log_file.to_str().unwrap_or("/dev/null"),
        "claude",
        "--permission-mode",
        "acceptEdits",
        "--append-system-prompt",
        "You are a peer working on this repository with the project owner. Work independently.",
    ]);
    cmd.current_dir(&worktree.path);
    match cmd.spawn() {
        Ok(child) => {
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
            let _ = Treehouse::new(PathBuf::from("treehouse")).release_if_lease(
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
