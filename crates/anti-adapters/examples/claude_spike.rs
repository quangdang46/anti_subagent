//! Spike: prove Claude stream-json → AgentEvent → TurnCompleted works end-to-end.
//!
//! Usage: cargo run -p anti-adapters --example claude_spike
//!
//! This spawns a real Claude session with stream-json piping, sends a trivial
//! task, and verifies we receive AssistantDelta/AssistantMessage + TurnCompleted
//! events through the NDJSON → parse_claude_stream_line pipeline.

use anti_adapters::{AgentEvent, AgentSession, ClaudeSession};
use std::time::{Duration, Instant};

fn main() {
    let worktree = std::env::current_dir().expect("need a working directory");
    let task = "echo 'hello from spike test'";
    let peer_prompt = "You are a test peer. Complete the task and nothing else.";

    eprintln!("[spike] spawning Claude session in {}", worktree.display());
    let start = Instant::now();

    let (mut session, result) = match ClaudeSession::spawn(&worktree, task, peer_prompt) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[spike] FAIL: spawn error: {e}");
            eprintln!("[spike] Is `claude` installed and authenticated?");
            std::process::exit(1);
        }
    };

    eprintln!(
        "[spike] session spawned: id={}, caps={:?}",
        result.session_id, result.capabilities
    );

    // Poll for events until TurnCompleted or timeout.
    let timeout = Duration::from_secs(120);
    let deadline = Instant::now() + timeout;
    let mut got_assistant = false;
    let mut got_turn_completed = false;
    let mut event_count = 0u32;

    loop {
        let events = session.drain_events();
        for event in &events {
            event_count += 1;
            match event {
                AgentEvent::AssistantDelta { text, .. } => {
                    eprint!("[delta] {text}");
                    got_assistant = true;
                }
                AgentEvent::AssistantMessage { text, .. } => {
                    eprintln!("[message] {text}");
                    got_assistant = true;
                }
                AgentEvent::ToolCallStart {
                    tool_name, call_id, ..
                } => {
                    eprintln!("[tool] {tool_name} ({call_id})");
                }
                AgentEvent::ToolCallComplete { call_id, .. } => {
                    eprintln!("[tool-done] {call_id}");
                }
                AgentEvent::TurnCompleted { usage } => {
                    eprintln!("[turn-completed] usage={usage:?}");
                    got_turn_completed = true;
                }
                AgentEvent::TurnFailed { error } => {
                    eprintln!("[turn-failed] {error}");
                }
                AgentEvent::PermissionRequested {
                    tool_name,
                    request_id,
                    ..
                } => {
                    eprintln!("[permission] {tool_name} ({request_id}) — auto-allowing");
                    // In spike mode, we can't do bidirectional permission yet.
                    // This proves the event surfaces correctly.
                }
                other => {
                    eprintln!("[event] {other:?}");
                }
            }
        }

        if got_turn_completed {
            break;
        }
        if Instant::now() >= deadline {
            eprintln!("[spike] TIMEOUT after {timeout:?} — no TurnCompleted received");
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let elapsed = start.elapsed();
    session.kill();

    eprintln!();
    eprintln!("[spike] === RESULTS ===");
    eprintln!("[spike] elapsed:     {elapsed:.1?}");
    eprintln!("[spike] events:      {event_count}");
    eprintln!("[spike] assistant:   {got_assistant}");
    eprintln!("[spike] completed:   {got_turn_completed}");

    if got_turn_completed && got_assistant {
        eprintln!("[spike] PASS — stream-json → AgentEvent pipeline works end-to-end");
        std::process::exit(0);
    } else if got_turn_completed {
        eprintln!("[spike] PARTIAL — got TurnCompleted but no assistant text");
        std::process::exit(0);
    } else {
        eprintln!("[spike] FAIL — did not receive TurnCompleted");
        std::process::exit(1);
    }
}
