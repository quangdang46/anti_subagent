//! Daemon IPC — Unix domain socket with newline-delimited JSON (plan §13, §26).
//! P0 decision: Unix socket on macOS; transport is behind a thin protocol so
//! named-pipes/TCP can be swapped in later without touching callers.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub const SOCKET_NAME: &str = "anti.sock";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    Ping,
    /// Guard policy query (fail-closed: only reachable while daemon is up).
    GuardCheck { tool: String },
    SpawnAgent {
        id: String,
        role: String,
        disposition: Option<String>,
        harness: String,
        task_path: Option<String>,
        repo: String,
        parent_id: Option<String>,
        /// Per-arm peer prompt (plan §34: concealment is a benchmark variable).
        prompt: Option<String>,
    },
    ListAgents,
    GetAgent { id: String },
    WaitAgent { id: String, until: String, timeout_secs: u64 },
    StopAgent { id: String, force: bool },
    RestartAgent { id: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "ok", content = "data")]
pub enum Response {
    #[serde(rename = "true")]
    Ok(serde_json::Value),
    #[serde(rename = "false")]
    Err { code: String, message: String },
}

impl Response {
    pub fn ok(v: impl Serialize) -> Self {
        Response::Ok(serde_json::to_value(v).unwrap_or_default())
    }
    pub fn err(code: &str, message: impl Into<String>) -> Self {
        Response::Err {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SOCKET_NAME)
}

pub fn send_request(socket: &Path, req: &Request) -> Result<Response, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("cannot connect to daemon at {}: {e}", socket.display()))?;
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut resp = String::new();
    reader.read_line(&mut resp).map_err(|e| e.to_string())?;
    serde_json::from_str(&resp).map_err(|e| e.to_string())
}

/// Serve requests on a Unix socket, one thread per connection so a slow
/// request (e.g. `anti wait` blocking for minutes) never stalls the accept
/// loop — other clients (status/list/daemon) keep getting responses.
pub fn serve<F>(socket: &Path, handle: F) -> std::io::Result<()>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    if socket.exists() {
        // Stale socket from a previous daemon; remove and rebind.
        std::fs::remove_file(socket)?;
    }
    let handle = std::sync::Arc::new(handle);
    let listener = UnixListener::bind(socket)?;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let handle = handle.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let resp = match serde_json::from_str::<Request>(&line) {
                        Ok(req) => handle(req),
                        Err(e) => Response::err("bad_request", format!("{e}")),
                    };
                    let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
                        serde_json::to_string(&Response::err("internal", "serialize")).unwrap()
                    });
                    out.push('\n');
                    let mut stream = reader.into_inner();
                    let _ = stream.write_all(out.as_bytes());
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

