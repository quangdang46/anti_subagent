//! `codex app-server` JSON-RPC client (stdio) — paseo-style codex control.
//!
//! Protocol verified against codex-cli 0.149.0:
//! 1. request  `initialize` {clientInfo, capabilities{experimentalApi:true}}
//! 2. notify   `initialized` {}
//! 3. request  `thread/start` {cwd, approvalPolicy, sandbox} → thread.id
//! 4. request  `turn/start` {threadId, input:[{type:"text",text}], …}
//!    → notifications `item/completed`* then `turn/completed`
//!
//! The child process is long-lived; the daemon owns it like any other peer
//! child so stop/kill/reap keep working.

use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum AppServerError {
    #[error("spawn failed: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("serialize failed: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("rpc error for {method}: {message}")]
    Rpc { method: String, message: String },
    #[error("thread/start returned no thread id")]
    NoThread,
}

/// A live connection to one `codex app-server` child.
pub struct CodexAppServer {
    pub child: Child,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl CodexAppServer {
    /// Spawn and handshake the app-server child.
    pub fn connect(cwd: &std::path::Path) -> Result<Self, AppServerError> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut client = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };
        client.request(
            "initialize",
            json!({
                "clientInfo": {"name": "anti", "title": "Anti", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }),
        )?;
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    fn send(&mut self, value: serde_json::Value) -> Result<(), AppServerError> {
        use std::io::Write;
        let mut line = serde_json::to_string(&value)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Send a request and wait for the matching-id response line.
    pub fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppServerError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                return Err(AppServerError::Rpc {
                    method: method.to_string(),
                    message: "app-server closed the stream".into(),
                });
            }
            let v: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue, // not JSON — skip
            };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(AppServerError::Rpc {
                        method: method.to_string(),
                        message: err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                    });
                }
                return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
            }
            // Notifications while waiting — ignore here; turn loop reads them.
        }
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), AppServerError> {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Start a thread in `cwd` with full-access sandbox (peers run guarded by
    /// the anti deny-guard hook, not by codex sandboxing).
    pub fn start_thread(&mut self, cwd: &str) -> Result<String, AppServerError> {
        let result = self.request(
            "thread/start",
            json!({"cwd": cwd, "approvalPolicy": "never", "sandbox": "danger-full-access"}),
        )?;
        let id = result
            .pointer("/thread/id")
            .and_then(|v| v.as_str())
            .ok_or(AppServerError::NoThread)?
            .to_string();
        Ok(id)
    }

    /// Run one blocking turn. Returns when the `turn/completed` notification
    /// arrives (or an RPC error surfaces). Intermediate `item/completed`
    /// notifications are consumed silently — callers observe progress via
    /// the anti event log written after completion.
    pub fn run_turn(
        &mut self,
        thread_id: &str,
        prompt: &str,
        timeout_secs: u64,
    ) -> Result<(), AppServerError> {
        self.send(json!({
            "jsonrpc": "2.0", "id": self.next_id, "method": "turn/start",
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt}],
                "approvalPolicy": "never",
                "sandboxPolicy": {"mode": "danger-full-access"},
            }
        }))?;
        self.next_id += 1;

        // Blocking read loop on our own reader thread-side: set a coarse
        // deadline by counting time between lines (a silent server is not an
        // error while the model works; only total budget matters).
        let started = std::time::Instant::now();
        loop {
            // Poll without blocking forever: app-server sends nothing until an
            // event fires, but readline blocks. Use the child's exit as the
            // abort signal and rely on the daemon reaper for hard timeouts.
            if started.elapsed() > std::time::Duration::from_secs(timeout_secs) {
                return Err(AppServerError::Rpc {
                    method: "turn/start".into(),
                    message: format!("turn exceeded {timeout_secs}s budget"),
                });
            }
            let mut line = String::new();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                return Err(AppServerError::Rpc {
                    method: "turn/start".into(),
                    message: "app-server closed mid-turn".into(),
                });
            }
            let v: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                if method == "turn/completed" {
                    return Ok(());
                }
            }
            // Responses to turn/start itself carry id — treat an error there
            // as fatal for this turn.
            if v.get("id").and_then(|i| i.as_u64()) == Some(self.next_id - 1)
                && v.get("error").is_some()
            {
                return Err(AppServerError::Rpc {
                    method: "turn/start".into(),
                    message: v["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                });
            }
        }
    }
}
