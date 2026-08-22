//! `opencode serve` HTTP client — paseo-style opencode control.
//!
//! Verified against opencode 1.18.15:
//! 1. spawn `opencode serve --port N`, wait for stdout "listening on"
//! 2. `POST /session?directory=<wt>` → {id}
//! 3. `POST /session/{id}/message?directory=<wt>` body
//!    {"parts":[{"type":"text","text":P}],"model":{providerID,modelID}}
//!    — synchronous: returns the final assistant message.
//!
//! Model comes from ANTI_OPENCODE_MODEL ("provider/model" split on first '/'),
//! falling back to the global opencode config's "model" key (same resolution
//! as the exec adapter).

use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("spawn failed: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("server did not announce listening within 30s")]
    StartupTimeout,
    #[error("http error for {url}: {message}")]
    Http { url: String, message: String },
}

/// A running `opencode serve` child plus its base URL.
pub struct OpenCodeServe {
    pub child: Child,
    pub base_url: String,
}

impl OpenCodeServe {
    /// Spawn the server bound to a free-ish fixed port and wait for readiness.
    pub fn connect(port: u16) -> Result<Self, ServeError> {
        let mut child = Command::new("opencode")
            .args(["serve", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        // Readiness: paseo waits for "listening on" on stdout; fall back to
        // polling the HTTP port in case the banner format changes.
        let started = Instant::now();
        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            while started.elapsed() < Duration::from_secs(30) {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) if line.contains("listening on") => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            // Give the HTTP listener a beat after the banner.
            std::thread::sleep(Duration::from_millis(300));
        }
        // Port probe regardless of banner.
        let url = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                return Ok(Self {
                    child,
                    base_url: url,
                });
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(ServeError::StartupTimeout);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ServeError> {
        let url = format!("{}{}", self.base_url, path);
        let send = || -> std::result::Result<serde_json::Value, String> {
            let resp = match (method, body.clone()) {
                ("POST", Some(b)) => ureq::post(url.as_str()).send_json(b),
                ("GET", _) => ureq::get(url.as_str()).call(),
                (m, Some(b)) => ureq::request(m, url.as_str()).send_json(b),
                (m, None) => ureq::request(m, url.as_str()).call(),
            };
            match resp {
                Ok(r) => r
                    .into_json::<serde_json::Value>()
                    .map_err(|e| format!("decode: {e}")),
                Err(ureq::Error::Status(code, _)) => Err(format!("status {code}")),
                Err(e) => Err(format!("transport: {e}")),
            }
        };
        send().map_err(|message| ServeError::Http { url, message })
    }

    /// Create a session scoped to the worktree directory.
    pub fn create_session(&self, directory: &str) -> Result<String, ServeError> {
        let v = self.http(
            "POST",
            &format!("/session?directory={}", urlencode(directory)),
            Some(json!({})),
        )?;
        Ok(v.get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string())
    }

    /// Drive one task to completion (synchronous message endpoint).
    pub fn run_prompt(
        &self,
        session_id: &str,
        directory: &str,
        prompt: &str,
    ) -> Result<(), ServeError> {
        let (provider, model) = resolve_model();
        let body = json!({
            "parts": [{"type": "text", "text": prompt}],
            "model": {"providerID": provider, "modelID": model},
        });
        self.http(
            "POST",
            &format!(
                "/session/{}/message?directory={}",
                session_id,
                urlencode(directory)
            ),
            Some(body),
        )?;
        Ok(())
    }
}

fn ureq_request(url: &str, method: &str) -> ureq::Request {
    match method {
        "POST" => ureq::post(url),
        "GET" => ureq::get(url),
        _ => ureq::request(method, url),
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Same resolution order as the exec adapter: ANTI_OPENCODE_MODEL env →
/// "model" key in ~/.config/opencode/opencode.json|.jsonc. Split on first '/'.
fn resolve_model() -> (String, String) {
    let raw = std::env::var("ANTI_OPENCODE_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| {
            let home = std::env::var("HOME").ok()?;
            let base = std::path::PathBuf::from(home)
                .join(".config")
                .join("opencode");
            for name in ["opencode.json", "opencode.jsonc"] {
                if let Ok(content) = std::fs::read_to_string(base.join(name))
                    && let Some(model) = anti_adapters::extract_json_string_field(&content, "model")
                {
                    return Some(model);
                }
            }
            None
        });
    match raw {
        Some(m) if m.contains('/') => {
            let (p, r) = m.split_once('/').unwrap();
            (p.to_string(), r.to_string())
        }
        Some(m) => {
            let both = m.clone();
            (both, m)
        }
        None => ("9router".into(), "xxx".into()),
    }
}
