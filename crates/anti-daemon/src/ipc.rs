//! Daemon IPC — newline-delimited JSON over platform-native transport.
//!
//! Supported transports (plan §13, §33):
//! - Unix:   Unix domain socket (default on Linux/macOS)
//! - TCP:    TCP loopback on 127.0.0.1 (default on Windows; local-only)
//! - Named Pipe: Windows named pipe (opt-in via config)
//!
//! Transport selection is auto-resolved at startup based on platform, with an
//! optional config override (ANTI_IPC_TRANSPORT env or config.toml key).
//!
//! The protocol is identical on all platforms: one JSON line request,
//! one JSON line response. Only the transport layer differs.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SOCKET_NAME: &str = "anti.sock";
pub const PIPE_PREFIX: &str = "anti-subagent";
pub const TCP_LOOPBACK: &str = "127.0.0.1";

/// IPC transport type — selected at startup, immutable for the daemon's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcTransport {
    /// Unix domain socket (Linux/macOS default)
    Unix,
    /// TCP loopback on 127.0.0.1 (Windows default; stable port from state_dir hash)
    Tcp,
    /// Windows named pipe (alternative on Windows)
    #[cfg(windows)]
    NamedPipe,
}

impl IpcTransport {
    /// Auto-select transport based on platform.
    pub fn auto() -> Self {
        #[cfg(unix)]
        {
            IpcTransport::Unix
        }
        #[cfg(windows)]
        {
            IpcTransport::Tcp
        }
    }

    /// Resolve from config value string.
    pub fn from_config(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "unix" | "socket" => Ok(IpcTransport::Unix),
            "tcp" | "loopback" => Ok(IpcTransport::Tcp),
            #[cfg(windows)]
            "named_pipe" | "pipe" => Ok(IpcTransport::NamedPipe),
            _ => Err(format!(
                "unknown IPC transport '{s}'. Valid: unix, tcp{}",
                if cfg!(windows) { ", named_pipe" } else { "" }
            )),
        }
    }

    /// Display name for diagnostics.
    pub fn display_name(&self) -> &'static str {
        match self {
            IpcTransport::Unix => "unix_socket",
            IpcTransport::Tcp => "tcp_loopback",
            #[cfg(windows)]
            IpcTransport::NamedPipe => "named_pipe",
        }
    }
}

// ─── Protocol types ───────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    Shutdown,
    Ping,
    GuardCheck {
        tool: String,
    },
    SpawnAgent {
        id: String,
        role: String,
        disposition: Option<String>,
        harness: String,
        task_path: Option<String>,
        repo: String,
        parent_id: Option<String>,
        prompt: Option<String>,
    },
    /// Full SLP hierarchy snapshot (issue #8): supervisor/leads/peers +
    /// attention queue. One serializer shared by CLI --json and HTTP surface.
    GetHierarchy,
    ListAgents,
    /// List only agents requiring attention (supervisor/human triage queue).
    ListAttention,
    /// Clear attention flag on one agent (anti ack).
    AckAttention {
        id: String,
    },
    GetAgent {
        id: String,
    },
    WaitAgent {
        id: String,
        until: String,
        timeout_secs: u64,
    },
    StopAgent {
        id: String,
        force: bool,
    },
    RestartAgent {
        id: String,
    },
    SubmitWork {
        id: String,
        sha256: String,
        artifact_path: String,
        review_timeout_secs: u64,
    },
    ReviewWork {
        id: String,
        verdict: String,
        note: String,
    },
    ListWorkItems,
    VerifyWork {
        id: String,
        profile: String, // "full", "check", "test", "build", or "named:<name>"
    },
    CheckDisposition {
        disposition: String,
        tool: String,
    },
    // Permission: peer asks permission for a legitimate tool
    PermissionRequest {
        peer_id: String,
        tool: String,
        input: String,
    },
    PermissionResolve {
        request_id: String,
        decision: String,
    },
    PermissionList,
    /// Peer reports task status. No peer_id — daemon resolves from task ownership.
    ReportTask {
        task_id: String,
        status: String, // "completed" | "failed" | "progress" | "question"
        commit: Option<String>,
        error: Option<String>,
        message: Option<String>,
    },
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

// ─── Platform endpoint path ───────────────────────────────────────────

/// Resolve the socket/address path for the default (auto) transport.
pub fn socket_path(state_dir: &Path) -> PathBuf {
    socket_path_for(IpcTransport::auto(), state_dir)
}

pub fn socket_path_for(transport: IpcTransport, state_dir: &Path) -> PathBuf {
    match transport {
        IpcTransport::Unix => state_dir.join(SOCKET_NAME),
        IpcTransport::Tcp => {
            #[cfg(unix)]
            {
                state_dir.join(SOCKET_NAME)
            }
            #[cfg(windows)]
            {
                PathBuf::from(format!("{TCP_LOOPBACK}:{}", tcp_port(state_dir)))
            }
        }
        #[cfg(windows)]
        IpcTransport::NamedPipe => PathBuf::from(named_pipe_path(state_dir)),
    }
}

/// Derive a stable TCP port from the state directory hash.
#[cfg(windows)]
fn tcp_port(state_dir: &Path) -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    state_dir.hash(&mut hasher);
    49152 + (hasher.finish() % 16383) as u16
}

/// Windows named pipe path derived from state_dir.
#[cfg(windows)]
fn named_pipe_path(state_dir: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    state_dir.hash(&mut hasher);
    let id = hasher.finish() % 100_000;
    format!("\\\\.\\pipe\\{PIPE_PREFIX}-{id}")
}

// ─── send_request ─────────────────────────────────────────────────────

#[cfg(unix)]
pub fn send_request(socket: &Path, req: &Request) -> Result<Response, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| format!("cannot connect to daemon at {}: {e}", socket.display()))?;
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    stream
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut resp = String::new();
    reader.read_line(&mut resp).map_err(|e| e.to_string())?;
    serde_json::from_str(&resp).map_err(|e| e.to_string())
}

#[cfg(windows)]
pub fn send_request(addr: &Path, req: &Request) -> Result<Response, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr_str = addr.to_str().ok_or("address not valid UTF-8")?;
    let stream = TcpStream::connect(addr_str)
        .map_err(|e| format!("cannot connect to daemon at {addr_str}: {e}"))?;
    // Set read/write timeouts to prevent hanging
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut writer = &stream;
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    writer
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut resp = String::new();
    reader.read_line(&mut resp).map_err(|e| e.to_string())?;
    serde_json::from_str(&resp).map_err(|e| e.to_string())
}

// ─── serve ────────────────────────────────────────────────────────────

#[cfg(unix)]
pub fn serve<F>(socket: &Path, handle: F) -> std::io::Result<()>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    let handle = std::sync::Arc::new(handle);
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let listener = UnixListener::bind(socket)?;
    listener.set_nonblocking(true)?;
    loop {
        if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = std::fs::remove_file(socket);
            return Ok(());
        }
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            Err(_) => continue,
        };
        let handle = handle.clone();
        let shutdown = shutdown.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let resp = match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    if matches!(req, Request::Shutdown) {
                        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    handle(req)
                }
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
    #[allow(unreachable_code)]
    Ok(())
}

/// TCP loopback server for Windows.
/// Local-only on 127.0.0.1 — no firewall, no port exposure, no WSL needed.
#[cfg(windows)]
pub fn serve<F>(addr: &Path, handle: F) -> std::io::Result<()>
where
    F: Fn(Request) -> Response + Send + Sync + 'static,
{
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    let addr_str = addr.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "address not valid UTF-8")
    })?;

    // If already bound (stale), wait briefly then retry.
    let listener = match TcpListener::bind(addr_str) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            std::thread::sleep(Duration::from_millis(200));
            TcpListener::bind(addr_str)?
        }
        Err(e) => return Err(e),
    };
    listener.set_nonblocking(true)?;

    let handle = std::sync::Arc::new(handle);
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    eprintln!("anti-daemon: listening on {addr_str}");

    loop {
        if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let (mut stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(_) => continue,
        };
        // Set read/write timeouts to prevent stuck threads
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let handle = handle.clone();
        let shutdown = shutdown.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let resp = match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    if matches!(req, Request::Shutdown) {
                        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    handle(req)
                }
                Err(e) => Response::err("bad_request", format!("{e}")),
            };
            let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
                serde_json::to_string(&Response::err("internal", "serialize")).unwrap()
            });
            out.push('\n');
            let _ = stream.write_all(out.as_bytes());
        });
    }
    #[allow(unreachable_code)]
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_returns_valid_path() {
        let p = socket_path(Path::new("/tmp/anti-test"));
        let s = p.to_string_lossy();
        // Unix: contains "anti.sock", Windows: "127.0.0.1:PORT"
        assert!(
            s.contains("anti") || s.starts_with("127.0.0.1"),
            "unexpected path: {s}"
        );
    }

    #[test]
    fn response_roundtrip() {
        let r = Response::ok(serde_json::json!({"ping": true}));
        let s = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, Response::Ok(_)));
    }
}
