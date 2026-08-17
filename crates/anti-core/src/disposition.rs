//! Disposition contracts — formal behavioral boundaries for each role.
//!
//! Each disposition defines what tools are allowed/denied, whether self-approval
//! is permitted, and whether evidence is required. This enforces the SLP
//! principle that peers must not approve their own work.

use crate::model::Disposition;
use serde::{Deserialize, Serialize};

/// Formal contract for a disposition — defines behavioral boundaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispositionContract {
    pub name: Disposition,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub can_approve_own_work: bool,
    pub requires_evidence: bool,
    pub max_concurrent: usize,
}

/// Error when a tool or action violates a disposition contract.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DispositionError {
    #[error("tool '{tool}' is denied for disposition {disposition:?}")]
    ToolDenied {
        tool: String,
        disposition: Disposition,
    },
    #[error("self-approval not allowed for disposition {disposition:?}")]
    SelfApprovalDenied { disposition: Disposition },
    #[error("evidence required for disposition {disposition:?}")]
    EvidenceRequired { disposition: Disposition },
    #[error("max concurrent peers ({max}) reached for disposition {disposition:?}")]
    MaxConcurrentReached {
        max: usize,
        disposition: Disposition,
    },
    #[error("unknown disposition: {0}")]
    Unknown(String),
}

impl DispositionContract {
    /// Check if a tool is allowed for this disposition.
    pub fn check_tool(&self, tool: &str) -> Result<(), DispositionError> {
        if self.denied_tools.iter().any(|d| tool.contains(d.as_str())) {
            return Err(DispositionError::ToolDenied {
                tool: tool.to_string(),
                disposition: self.name,
            });
        }
        Ok(())
    }

    /// Check if self-approval is allowed.
    pub fn check_self_approve(&self) -> Result<(), DispositionError> {
        if !self.can_approve_own_work {
            return Err(DispositionError::SelfApprovalDenied {
                disposition: self.name,
            });
        }
        Ok(())
    }

    /// Check if evidence is required.
    pub fn check_evidence(&self) -> Result<(), DispositionError> {
        if self.requires_evidence {
            return Err(DispositionError::EvidenceRequired {
                disposition: self.name,
            });
        }
        Ok(())
    }
}

/// Get the contract for a disposition.
pub fn contract_for(disposition: Disposition) -> DispositionContract {
    match disposition {
        Disposition::Engineer => DispositionContract {
            name: Disposition::Engineer,
            allowed_tools: vec![
                "read".into(),
                "write".into(),
                "edit".into(),
                "bash".into(),
                "grep".into(),
                "glob".into(),
            ],
            denied_tools: vec!["approve_own_work".into()],
            can_approve_own_work: false,
            requires_evidence: false,
            max_concurrent: 5,
        },
        Disposition::Architect => DispositionContract {
            name: Disposition::Architect,
            allowed_tools: vec![
                "read".into(),
                "grep".into(),
                "glob".into(),
            ],
            denied_tools: vec!["write".into(), "edit".into(), "bash".into()],
            can_approve_own_work: false,
            requires_evidence: false,
            max_concurrent: 2,
        },
        Disposition::Reviewer => DispositionContract {
            name: Disposition::Reviewer,
            allowed_tools: vec![
                "read".into(),
                "grep".into(),
                "glob".into(),
                "bash".into(),
            ],
            denied_tools: vec!["write".into(), "edit".into()],
            can_approve_own_work: false,
            requires_evidence: true,
            max_concurrent: 3,
        },
        Disposition::Scout => DispositionContract {
            name: Disposition::Scout,
            allowed_tools: vec![
                "read".into(),
                "grep".into(),
                "glob".into(),
            ],
            denied_tools: vec!["write".into(), "edit".into(), "bash".into()],
            can_approve_own_work: false,
            requires_evidence: false,
            max_concurrent: 3,
        },
        Disposition::ProofAuditor => DispositionContract {
            name: Disposition::ProofAuditor,
            allowed_tools: vec![
                "read".into(),
                "grep".into(),
                "glob".into(),
                "bash".into(),
            ],
            denied_tools: vec!["write".into(), "edit".into()],
            can_approve_own_work: false,
            requires_evidence: true,
            max_concurrent: 2,
        },
        Disposition::Shadow => DispositionContract {
            name: Disposition::Shadow,
            allowed_tools: vec!["read".into()],
            denied_tools: vec![
                "write".into(),
                "edit".into(),
                "bash".into(),
                "grep".into(),
                "glob".into(),
            ],
            can_approve_own_work: false,
            requires_evidence: false,
            max_concurrent: 1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engineer_cannot_self_approve() {
        let contract = contract_for(Disposition::Engineer);
        assert!(!contract.can_approve_own_work);
        assert!(contract.check_self_approve().is_err());
    }

    #[test]
    fn scout_cannot_modify() {
        let contract = contract_for(Disposition::Scout);
        assert!(contract.check_tool("write").is_err());
        assert!(contract.check_tool("edit").is_err());
        assert!(contract.check_tool("bash").is_err());
    }

    #[test]
    fn reviewer_requires_evidence() {
        let contract = contract_for(Disposition::Reviewer);
        assert!(contract.requires_evidence);
    }

    #[test]
    fn proof_auditor_requires_evidence() {
        let contract = contract_for(Disposition::ProofAuditor);
        assert!(contract.requires_evidence);
    }

    #[test]
    fn engineer_can_write() {
        let contract = contract_for(Disposition::Engineer);
        assert!(contract.check_tool("write").is_ok());
        assert!(contract.check_tool("edit").is_ok());
        assert!(contract.check_tool("bash").is_ok());
    }

    #[test]
    fn shadow_can_only_read() {
        let contract = contract_for(Disposition::Shadow);
        assert!(contract.check_tool("read").is_ok());
        assert!(contract.check_tool("write").is_err());
        assert!(contract.check_tool("edit").is_err());
        assert!(contract.check_tool("bash").is_err());
    }
}
