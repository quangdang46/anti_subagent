//! Normalized AgentEvent — produced by Claude CLI stream-json (or fallback).
//!
//! Every runtime adapter normalizes its stdout into these events. Downstream
//! stages (timeline projection, wait engine, attention) feed only from this
//! enum — not from raw CLI output.

use serde::{Deserialize, Serialize};

/// Tool call lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

/// Token/cost usage reported on turn completion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub context_window_max: Option<u64>,
    pub total_cost_usd: Option<f64>,
}

/// Normalized events from any provider runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    /// Assistant text delta (streaming chunk).
    AssistantDelta {
        text: String,
        message_id: String,
    },
    AssistantMessage {
        text: String,
        message_id: String,
    },
    SystemMessage {
        text: String,
    },

    /// Tool call lifecycle (normalized across providers).
    ToolCallStart {
        call_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    ToolCallUpdate {
        call_id: String,
        status: ToolCallStatus,
        detail: Option<serde_json::Value>,
    },
    ToolCallComplete {
        call_id: String,
        output: Option<String>,
    },
    ToolCallFailed {
        call_id: String,
        error: String,
    },

    /// Turn lifecycle.
    TurnCompleted {
        usage: Option<Usage>,
    },
    TurnFailed {
        error: String,
    },

    /// Permission (only when capability.permission == Some(true)).
    PermissionRequested {
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    PermissionResolved {
        request_id: String,
    },
}

/// Parse a single NDJSON line from `claude -p --output-format stream-json`
/// into an `AgentEvent`. Returns None for unrecognized lines (skipped).
pub fn parse_claude_stream_line(line: &str) -> Option<AgentEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    parse_claude_value(&v)
}

/// Parse a JSON value (one-shot or stream-json) into AgentEvent.
pub fn parse_claude_value(v: &serde_json::Value) -> Option<AgentEvent> {
    // Stream-json variants: check `type` field first.
    if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
        match t {
            "assistant" => {
                let text = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .and_then(|arr| {
                        arr.iter()
                            .find(|x| x.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .and_then(|x| x.get("text"))
                            .and_then(|x| x.as_str())
                    })
                    .unwrap_or("")
                    .to_string();
                let message_id = v
                    .get("message")
                    .and_then(|m| m.get("id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return None;
                }
                return Some(AgentEvent::AssistantMessage { text, message_id });
            }
            "content_block_delta" | "stream_event" => {
                let text = v
                    .get("delta")
                    .and_then(|d| d.get("text"))
                    .and_then(|x| x.as_str())
                    .or_else(|| {
                        v.get("content_block")
                            .and_then(|c| c.get("text"))
                            .and_then(|x| x.as_str())
                    })
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return None;
                }
                return Some(AgentEvent::AssistantDelta {
                    text,
                    message_id: String::new(),
                });
            }
            "tool_use" | "tool_update" | "tool_call" => {
                let call_id = v
                    .get("id")
                    .or_else(|| v.get("tool_use_id"))
                    .or_else(|| v.get("call_id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_name = v
                    .get("name")
                    .or_else(|| v.get("tool_name"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if call_id.is_empty() && tool_name.is_empty() {
                    return None;
                }
                return Some(AgentEvent::ToolCallStart {
                    call_id: if call_id.is_empty() {
                        uuid_fallback()
                    } else {
                        call_id
                    },
                    tool_name,
                    input: v
                        .get("input")
                        .or_else(|| v.get("input_json"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                });
            }
            // System/permission variants: best-effort.
            "permission_request" | "permission_requested" => {
                let request_id = v
                    .get("request_id")
                    .or_else(|| v.get("id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_name = v
                    .get("tool_name")
                    .or_else(|| v.get("tool"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                return Some(AgentEvent::PermissionRequested {
                    request_id: if request_id.is_empty() {
                        uuid_fallback()
                    } else {
                        request_id
                    },
                    tool_name,
                    input: v.get("input").cloned().unwrap_or(serde_json::Value::Null),
                });
            }
            "permission_resolved" => {
                let request_id = v
                    .get("request_id")
                    .or_else(|| v.get("id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                return Some(AgentEvent::PermissionResolved { request_id });
            }
            "result" => {
                let is_error = v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
                if is_error {
                    let error = v
                        .get("result")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("error").and_then(|x| x.as_str()))
                        .unwrap_or("unknown error")
                        .to_string();
                    return Some(AgentEvent::TurnFailed { error });
                }
                let usage = parse_usage(v);
                return Some(AgentEvent::TurnCompleted { usage });
            }
            "system" => {
                let text = v
                    .get("subtype")
                    .or_else(|| v.get("content"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    return None;
                }
                return Some(AgentEvent::SystemMessage { text });
            }
            _ => {}
        }
    }

    // One-shot JSON result: top-level `result` or `is_error`.
    if v.get("result").is_some() || v.get("is_error").is_some() {
        let is_error = v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
        if is_error {
            let error = v
                .get("result")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("error").and_then(|x| x.as_str()))
                .unwrap_or("unknown error")
                .to_string();
            return Some(AgentEvent::TurnFailed { error });
        }
        let usage = parse_usage(v);
        return Some(AgentEvent::TurnCompleted { usage });
    }

    None
}

fn parse_usage(v: &serde_json::Value) -> Option<Usage> {
    let usage = v.get("usage")?;
    Some(Usage {
        input_tokens: usage.get("input_tokens").and_then(|x| x.as_u64()),
        output_tokens: usage.get("output_tokens").and_then(|x| x.as_u64()),
        context_window_max: usage.get("context_window_max").and_then(|x| x.as_u64()),
        total_cost_usd: usage
            .get("total_cost_usd")
            .and_then(|x| x.as_f64())
            .or_else(|| v.get("total_cost_usd").and_then(|x| x.as_f64())),
    })
}

fn uuid_fallback() -> String {
    format!(
        "evt-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_message_parsed() {
        let v = serde_json::json!({
            "type": "assistant",
            "message": {
                "id": "msg-1",
                "content": [{ "type": "text", "text": "hello" }]
            }
        });
        let evt = parse_claude_value(&v).unwrap();
        assert!(matches!(evt, AgentEvent::AssistantMessage { text, .. } if text == "hello"));
    }

    #[test]
    fn stream_delta_parsed() {
        let line = r#"{"type":"content_block_delta","delta":{"text":"hi "}}"#;
        let evt = parse_claude_stream_line(line).unwrap();
        assert!(matches!(evt, AgentEvent::AssistantDelta { text, .. } if text == "hi "));
    }

    #[test]
    fn result_success_parsed() {
        let v = serde_json::json!({ "type": "result", "is_error": false, "result": "done" });
        let evt = parse_claude_value(&v).unwrap();
        assert!(matches!(evt, AgentEvent::TurnCompleted { .. }));
    }

    #[test]
    fn result_error_parsed() {
        let v = serde_json::json!({ "type": "result", "is_error": true, "result": "failed" });
        let evt = parse_claude_value(&v).unwrap();
        assert!(matches!(evt, AgentEvent::TurnFailed { .. }));
    }

    #[test]
    fn one_shot_fallback_parsed() {
        let v = serde_json::json!({ "result": "done", "is_error": false });
        let evt = parse_claude_value(&v).unwrap();
        assert!(matches!(evt, AgentEvent::TurnCompleted { .. }));
    }

    #[test]
    fn permission_request_parsed() {
        let v = serde_json::json!({ "type": "permission_request", "request_id": "r1", "tool_name": "bash" });
        let evt = parse_claude_value(&v).unwrap();
        assert!(matches!(evt, AgentEvent::PermissionRequested { .. }));
    }

    #[test]
    fn tool_call_parsed() {
        let v = serde_json::json!({ "type": "tool_use", "id": "t1", "name": "read", "input": { "path": "/tmp/x" } });
        let evt = parse_claude_value(&v).unwrap();
        assert!(matches!(evt, AgentEvent::ToolCallStart { .. }));
    }

    #[test]
    fn unknown_type_returns_none() {
        let v = serde_json::json!({ "type": "something_unknown_xyz" });
        assert!(parse_claude_value(&v).is_none());
    }
}
