//! Issue #8 — minimal loopback-only HTTP control surface.
//!
//! `GET /v1/hierarchy` returns the same JSON the IPC `GetHierarchy` request
//! returns (one serializer, two transports). Off by default; enabled with
//! `ANTI_HTTP=1`. Binds 127.0.0.1 only — never a wildcard address.
//!
//! Zero new dependencies: hand-rolled request-line parsing is enough for a
//! single read-only GET endpoint (astra pattern: small typed surface over
//! one port; here the "port" is opt-in TCP).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;

/// What the endpoint serves: a snapshot builder invoked per request.
pub type HierarchyFn = Arc<dyn Fn() -> serde_json::Value + Send + Sync>;

/// Start the HTTP surface if `ANTI_HTTP=1`. Returns the bound address.
///
/// The listener runs on a detached thread for the daemon's lifetime.
/// Failure to bind is logged but non-fatal — the Unix socket stays the
/// primary transport.
#[cfg(unix)]
pub fn maybe_start(hierarchy: HierarchyFn) -> Option<String> {
    let enabled = std::env::var("ANTI_HTTP")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    // Loopback only — plan §8 acceptance: never a non-loopback address.
    let addr = format!("127.0.0.1:{}", http_port());
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[http] cannot bind {addr}: {e} (control surface disabled)");
            return None;
        }
    };
    eprintln!("[http] control surface listening on http://{addr}/v1/hierarchy");
    let addr_clone = addr.clone();
    std::thread::spawn(move || serve(listener, hierarchy));
    Some(addr_clone)
}

fn serve(listener: TcpListener, hierarchy: HierarchyFn) {
    for stream in listener.incoming().flatten() {
        let hierarchy = Arc::clone(&hierarchy);
        std::thread::spawn(move || {
            let peer = stream
                .peer_addr()
                .map(|a| a.ip().to_string())
                .unwrap_or_default();
            // Defense in depth: even if misconfigured to a wider interface,
            // refuse anything that is not 127.0.0.1.
            if peer != "127.0.0.1" {
                return;
            }
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                return;
            }
            // Drain headers (single-shot responses need none of them).
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.trim().is_empty() => break,
                    Ok(_) => {}
                }
            }
            let method = request_line.split_whitespace().next().unwrap_or("");
            let path_q = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("");
            let (status, body) = match (method, path_q) {
                ("GET", "/v1/hierarchy") => {
                    let json = serde_json::to_string(&hierarchy()).unwrap_or_else(|_| "{}".into());
                    ("200 OK", json)
                }
                ("GET", _) => (
                    "404 NOT FOUND",
                    r#"{"error":{"code":"not_found","reason":"unknown path"}}"#.to_string(),
                ),
                _ => (
                    "405 METHOD NOT ALLOWED",
                    r#"{"error":{"code":"invalid_request","reason":"GET only"}}"#.to_string(),
                ),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let mut stream = stream;
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
    }
}

/// Stable default port derived from nothing environmental — fixed so a UI
/// can find it. Override with ANTI_HTTP_PORT.
fn http_port() -> u16 {
    std::env::var("ANTI_HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4620)
}
