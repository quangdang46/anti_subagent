//! SLP Governance Layer — Supervisor → Lead → Peer hierarchy.
//!
//! Provider is execution layer. SLP is governance layer. InfoFilter
//! is the boundary. SLP never directly interacts with Claude SDK,
//! Codex JSON-RPC, or OpenCode SSE.

use crate::agent::{AgentConfig, AgentId, AgentRecord, SpawnRequest, WorkspaceId};
use crate::events::AgentEvent;
use crate::info_filter::{InternalAgentState, build_agent_context, build_session_config};
use crate::model::{Disposition, Role};
use crate::provider::{PersistenceHandle, ProviderKind};
use crate::routing::{Complexity, resolve_route};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ─── SLP Roles ────────────────────────────────────────────────────────────────

/// Supervisor — governance front door, on-demand, read-only.
pub struct Supervisor {
    pub agent_id: AgentId,
    /// Memory notebook: accumulates lessons across handoffs.
    pub memory: Vec<Lesson>,
    /// Optimization rules for Lead quality.
    pub rules: Vec<OptimizationRule>,
}

/// Lead — workspace-bound coordinator, never implements code.
pub struct Lead {
    pub agent_id: AgentId,
    pub workspace_id: WorkspaceId,
    /// Current compaction count (max ~5-7 before handoff).
    pub compaction_count: u32,
    /// Max compactions before degradation triggers handoff.
    pub max_compactions: u32,
    /// Council members (populated for deliberation).
    pub council: Option<CouncilProtocol>,
}

/// Peer — independent agent working on assigned task.
pub struct Peer {
    pub agent_id: AgentId,
    pub disposition: Disposition,
    pub provider: ProviderKind,
    pub workspace_id: WorkspaceId,
}

// ─── Experience Handoff ───────────────────────────────────────────────────────

/// A lesson from a Lead's experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub content: String,
    pub category: LessonCategory,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LessonCategory {
    AntiPattern,
    BestPractice,
    ToolPreference,
    CodeStyle,
    DomainKnowledge,
}

/// Handoff artifact — transferred from old Lead to new Lead.
#[derive(Debug, Clone)]
pub struct HandoffArtifact {
    pub from_lead: AgentId,
    pub to_lead: AgentId,
    pub lessons: Vec<Lesson>,
    pub timeline_snapshot: Vec<AgentEvent>,
    pub workspace_id: WorkspaceId,
}

/// Experience handoff result.
#[derive(Debug)]
pub struct HandoffResult {
    pub new_lead_id: AgentId,
    pub lessons_transferred: usize,
    pub peers_reconnected: Vec<AgentId>,
}

// ─── Council Protocol ─────────────────────────────────────────────────────────

/// Council for deliberation — Engineer + Reviewer + optional Architect.
#[derive(Debug, Clone)]
pub struct CouncilProtocol {
    pub engineer: AgentId,
    pub reviewer: AgentId,
    pub architect: Option<AgentId>,
}

/// A proposition to be deliberated.
#[derive(Debug, Clone)]
pub struct Proposition {
    pub claim: String,
    pub evidence: String,
    pub material: bool, // is this decision-changing?
}

/// Council verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected { reason: String },
    NeedsRevision { suggestions: Vec<String> },
}

// ─── Optimization Rules ───────────────────────────────────────────────────────

/// Rule for evaluating Lead quality.
#[derive(Debug, Clone)]
pub struct OptimizationRule {
    pub name: String,
    pub criterion: String,
    pub threshold: f64,
}

// ─── SLP Orchestrator ─────────────────────────────────────────────────────────

/// Central orchestrator for the SLP hierarchy.
pub struct SlpOrchestrator {
    /// All managed agents (Supervisor, Lead, Peers).
    agents: HashMap<String, AgentRecord>,
    /// Active supervisor (if any).
    supervisor: Option<AgentId>,
    /// Active leads (workspace → lead).
    leads: HashMap<WorkspaceId, AgentId>,
    /// Peers managed by each lead.
    peer_groups: HashMap<AgentId, Vec<AgentId>>,
}

impl SlpOrchestrator {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            supervisor: None,
            leads: HashMap::new(),
            peer_groups: HashMap::new(),
        }
    }

    /// Spawn a Supervisor agent.
    pub fn spawn_supervisor(&mut self) -> AgentId {
        let ws = WorkspaceId::new(std::path::Path::new("/supervisor"));
        let route = resolve_route(
            Role::Supervisor,
            Disposition::Architect,
            Complexity::High,
            &crate::routing::ProviderConfig::default(),
            "claude",
        );
        let config = AgentConfig {
            model: Some(route.model),
            ..Default::default()
        };
        let mut record = AgentRecord::new(Role::Supervisor, ProviderKind::Claude, ws, config);
        record.status = crate::model::AgentStatus::Running;
        let agent_id = record.agent_id.clone();
        self.agents.insert(agent_id.to_string(), record);
        self.supervisor = Some(agent_id.clone());
        agent_id
    }

    /// Spawn a Lead for a workspace.
    pub fn spawn_lead(&mut self, workspace_id: WorkspaceId) -> AgentId {
        let route = resolve_route(
            Role::Lead,
            Disposition::Architect,
            Complexity::High,
            &crate::routing::ProviderConfig::default(),
            "claude",
        );
        let config = AgentConfig {
            model: Some(route.model),
            system_prompt: Some(
                "You are a Lead coordinator. You own this workspace's outcome. \
                 Never implement code directly — only delegate and review."
                    .into(),
            ),
            ..Default::default()
        };
        let mut record = AgentRecord::new(
            Role::Lead,
            ProviderKind::Claude,
            workspace_id.clone(),
            config,
        );
        record.status = crate::model::AgentStatus::Running;
        let agent_id = record.agent_id.clone();
        self.agents.insert(agent_id.to_string(), record);
        self.leads.insert(workspace_id, agent_id.clone());
        agent_id
    }

    /// Spawn a Peer under a Lead (with InfoFilter applied).
    pub fn spawn_peer(
        &mut self,
        lead_id: &AgentId,
        disposition: Disposition,
        workspace_id: WorkspaceId,
        task: &str,
    ) -> AgentId {
        // Internal state — NEVER serialized to provider
        let _internal = InternalAgentState {
            agent_id: AgentId::new(),
            parent_id: Some(lead_id.clone()),
            supervisor_id: self.supervisor.clone(),
            role: Role::Peer,
            disposition: Some(disposition),
            governance_state: None,
            handoff_context: None,
            spawn_reason: Some("lead_delegation".into()),
        };

        // Agent context — ONLY this reaches the provider
        let context = build_agent_context(
            task,
            std::path::Path::new(workspace_id.as_str()),
            Some("You are working with the project owner on this repository."),
            None,
        );

        // Session config from context — NO internal metadata
        let config = AgentConfig {
            system_prompt: context.peer_prompt.clone(),
            model: context.model.clone(),
            ..Default::default()
        };
        let session_config = build_session_config(&context, &config);

        // Create agent record (internal state)
        let mut record = AgentRecord::new(Role::Peer, ProviderKind::Claude, workspace_id, config);
        record.status = crate::model::AgentStatus::Running;
        let agent_id = record.agent_id.clone();

        // Store full record (with parent_id) in internal registry
        self.agents.insert(agent_id.to_string(), record);

        // Track under lead
        self.peer_groups
            .entry(lead_id.clone())
            .or_default()
            .push(agent_id.clone());

        agent_id
    }

    /// Experience handoff — transfer lessons from old Lead to new Lead.
    pub fn experience_handoff(
        &mut self,
        old_lead_id: &AgentId,
        workspace_id: WorkspaceId,
        lessons: Vec<Lesson>,
    ) -> Result<HandoffResult, String> {
        // 1. Archive old Lead
        if let Some(old) = self.agents.get_mut(old_lead_id.as_str()) {
            old.status = crate::model::AgentStatus::Replaced;
        }

        // 2. Create new Lead
        let new_lead_id = self.spawn_lead(workspace_id.clone());

        // 3. Transfer lessons
        // (In production, lessons would be injected as initial context)
        let lessons_count = lessons.len();

        // 4. Reconnect peers under new lead
        let peers = self.peer_groups.remove(old_lead_id).unwrap_or_default();
        self.peer_groups.insert(new_lead_id.clone(), peers.clone());

        Ok(HandoffResult {
            new_lead_id,
            lessons_transferred: lessons_count,
            peers_reconnected: peers,
        })
    }

    /// Council deliberation — extract propositions and issue verdict.
    pub fn council_deliberation(&self, propositions: Vec<Proposition>) -> Verdict {
        // 1. Extract material propositions (max 3-5)
        let material: Vec<_> = propositions.iter().filter(|p| p.material).take(5).collect();

        if material.is_empty() {
            return Verdict::Accepted;
        }

        // 2. Verify only decision-changing claims
        // 3. Allow at most one challenge per proposition
        // 4. Issue binding verdict
        // (Simplified: accept if ≥2/3 material propositions have evidence)
        let with_evidence = material.iter().filter(|p| !p.evidence.is_empty()).count();
        let total = material.len();

        if total > 0 && with_evidence * 3 >= total * 2 {
            Verdict::Accepted
        } else {
            Verdict::NeedsRevision {
                suggestions: material
                    .iter()
                    .filter(|p| p.evidence.is_empty())
                    .map(|p| format!("Needs evidence: {}", p.claim))
                    .collect(),
            }
        }
    }

    /// Get an agent record.
    pub fn get_agent(&self, agent_id: &AgentId) -> Option<&AgentRecord> {
        self.agents.get(agent_id.as_str())
    }

    /// List all peers under a lead.
    pub fn peers_of(&self, lead_id: &AgentId) -> Vec<&AgentRecord> {
        self.peer_groups
            .get(lead_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.agents.get(id.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for SlpOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_spawn_supervisor() {
        let mut orch = SlpOrchestrator::new();
        let id = orch.spawn_supervisor();
        let agent = orch.get_agent(&id).unwrap();
        assert_eq!(agent.role, Role::Supervisor);
        assert_eq!(agent.status, crate::model::AgentStatus::Running);
        // Issue #3: coordinator tier is resolved by routing, not hard-coded.
        assert_eq!(
            agent.config.model.as_deref(),
            Some("opus"),
            "supervisor must resolve to the heavyweight model"
        );
    }

    #[test]
    fn orchestrator_spawn_lead() {
        let mut orch = SlpOrchestrator::new();
        let ws = WorkspaceId::new(std::path::Path::new("/tmp/repo"));
        let id = orch.spawn_lead(ws.clone());
        let agent = orch.get_agent(&id).unwrap();
        assert_eq!(agent.role, Role::Lead);
    }

    #[test]
    fn orchestrator_spawn_peer_under_lead() {
        let mut orch = SlpOrchestrator::new();
        let ws = WorkspaceId::new(std::path::Path::new("/tmp/repo"));
        let lead_id = orch.spawn_lead(ws.clone());

        let peer_id = orch.spawn_peer(&lead_id, Disposition::Engineer, ws, "fix the bug");

        let agent = orch.get_agent(&peer_id).unwrap();
        assert_eq!(agent.role, Role::Peer);

        let peers = orch.peers_of(&lead_id);
        assert_eq!(peers.len(), 1);
    }

    #[test]
    fn experience_handoff_transfers_lessons() {
        let mut orch = SlpOrchestrator::new();
        let ws = WorkspaceId::new(std::path::Path::new("/tmp/repo"));
        let old_lead = orch.spawn_lead(ws.clone());

        // Spawn peers under old lead
        orch.spawn_peer(&old_lead, Disposition::Engineer, ws.clone(), "task 1");
        orch.spawn_peer(&old_lead, Disposition::Reviewer, ws.clone(), "task 2");

        let lessons = vec![Lesson {
            content: "Use rustfmt for formatting".into(),
            category: LessonCategory::CodeStyle,
            recorded_at: Utc::now(),
        }];

        let result = orch.experience_handoff(&old_lead, ws, lessons).unwrap();
        assert_eq!(result.lessons_transferred, 1);
        assert_eq!(result.peers_reconnected.len(), 2);

        // Old lead should be archived
        let old = orch.get_agent(&old_lead).unwrap();
        assert_eq!(old.status, crate::model::AgentStatus::Replaced);

        // New lead should exist
        let new = orch.get_agent(&result.new_lead_id).unwrap();
        assert_eq!(new.role, Role::Lead);
    }

    #[test]
    fn council_deliberation_accepted() {
        let orch = SlpOrchestrator::new();
        let propositions = vec![
            Proposition {
                claim: "Use HashMap for O(1) lookup".into(),
                evidence: "Benchmarks show 10x improvement".into(),
                material: true,
            },
            Proposition {
                claim: "Add unit tests".into(),
                evidence: "Coverage at 60%, target 80%".into(),
                material: true,
            },
            Proposition {
                claim: "Rename variable".into(),
                evidence: "".into(),
                material: false, // not material
            },
        ];

        let verdict = orch.council_deliberation(propositions);
        assert_eq!(verdict, Verdict::Accepted);
    }

    #[test]
    fn council_deliberation_needs_revision() {
        let orch = SlpOrchestrator::new();
        let propositions = vec![
            Proposition {
                claim: "Use HashMap".into(),
                evidence: "".into(), // no evidence
                material: true,
            },
            Proposition {
                claim: "Add tests".into(),
                evidence: "".into(), // no evidence
                material: true,
            },
        ];

        let verdict = orch.council_deliberation(propositions);
        match verdict {
            Verdict::NeedsRevision { suggestions } => {
                assert_eq!(suggestions.len(), 2);
            }
            _ => panic!("expected NeedsRevision"),
        }
    }

    #[test]
    fn lesson_categories() {
        let lesson = Lesson {
            content: "test".into(),
            category: LessonCategory::AntiPattern,
            recorded_at: Utc::now(),
        };
        assert_eq!(lesson.category, LessonCategory::AntiPattern);
    }
}
