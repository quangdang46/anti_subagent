//! Daemon IPC — Unix domain socket with newline-delimited JSON (plan §13, §26).
//! P0 decision: Unix socket on macOS; transport is behind a thin protocol so
//! named-pipes/TCP can be swapped in later without touching callers.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
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

/// Serve requests on a Unix socket until the listener errors or we are asked
/// to shut down. `handle` returns Ok(true) to continue or Ok(false) to stop.
pub fn serve(
    socket: &Path,
    mut handle: impl FnMut(Request) -> Response,
) -> std::io::Result<()> {
    if socket.exists() {
        // Stale socket from a previous daemon; remove and rebind.
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => continue,
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
                // Read-side is dropped above; write to the original stream
                // via a raw dup of the file descriptor.
                let _ = write_to_stream(reader.get_ref(), &out);
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn write_to_stream(stream: &UnixStream, data: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    let dup = unsafe { libc::dup(fd) };
    if dup < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut s = unsafe { UnixStream::from_raw_fd(dup) };
    s.write_all(data.as_bytes())?;
    Ok(())
}
