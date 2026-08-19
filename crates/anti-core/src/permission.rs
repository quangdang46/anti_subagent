//! Permission request/approval flow (capability-driven, plan §22).
//!
//! Guard (delegation-shaped tools) is a SEPARATE layer — those never reach
//! the permission queue. This module covers legitimate tool use that the
//! provider asks permission for (stream-json permission_requested event).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionKind {
    ToolUse,
    Bash,
    Edit,
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub peer_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub kind: PermissionKind,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResponse {
    pub request_id: String,
    pub decision: PermissionDecision,
    pub updated_input: Option<serde_json::Value>,
}
