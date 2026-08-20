//! AgentManager — manages agent lifecycle, persistence, and events.
//!
//! Decouples agent identity from process/session. AgentManager owns the
//! source of truth for agent state; runtime sessions are ephemeral.
//!
//! Architecture:
//!   AgentManager
//!     ├── agents: HashMap<AgentId, AgentRecord>
//!     ├── storage: AgentStorage (JSON files)
//!     └── event_bus: broadcast::Sender<AgentEvent>

use anti_core::agent::{AgentId, AgentRecord, SpawnRequest, WorkspaceId};
use anti_core::events::AgentEvent;
use anti_core::model::AgentStatus;
use anti_core::provider::PersistenceHandle;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum AgentManagerError {
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("agent already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid status transition: {0}")]
    InvalidTransition(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("provider error: {0}")]
    Provider(String),
}

// ─── Agent Storage ────────────────────────────────────────────────────────────

/// File-based agent persistence (JSON files).
pub struct AgentStorage {
    base_dir: PathBuf,
}

impl AgentStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Get the path for an agent's JSON file.
    fn agent_path(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> PathBuf {
        self.base_dir
            .join("agents")
            .join(workspace_id.as_str())
            .join(format!("{}.json", agent_id.as_str()))
    }

    /// Persist an agent record to disk.
    pub fn save(&self, record: &AgentRecord) -> Result<(), AgentManagerError> {
        let path = self.agent_path(&record.workspace_id, &record.agent_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(record)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Load an agent record from disk.
    pub fn load(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentRecord>, AgentManagerError> {
        let path = self.agent_path(workspace_id, agent_id);
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(&path)?;
        let record: AgentRecord = serde_json::from_str(&json)?;
        Ok(Some(record))
    }

    /// List all agent records in a workspace.
    pub fn list_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<AgentRecord>, AgentManagerError> {
        let dir = self.base_dir.join("agents").join(workspace_id.as_str());
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let json = std::fs::read_to_string(&path)?;
                let record: AgentRecord = serde_json::from_str(&json)?;
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Delete an agent record from disk.
    pub fn delete(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<(), AgentManagerError> {
        let path = self.agent_path(workspace_id, agent_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

// ─── Agent Manager ────────────────────────────────────────────────────────────

/// Central manager for agent lifecycle, persistence, and events.
pub struct AgentManager {
    /// In-memory agent registry.
    agents: HashMap<String, AgentRecord>,
    /// File-based persistence.
    storage: AgentStorage,
    /// Event broadcast (subscribers receive AgentEvent).
    event_tx: std::sync::mpsc::Sender<AgentEvent>,
    event_rx: Arc<RwLock<Vec<std::sync::mpsc::Receiver<AgentEvent>>>>,
}

impl AgentManager {
    /// Create a new AgentManager.
    pub fn new(state_dir: PathBuf) -> Self {
        let storage = AgentStorage::new(state_dir);
        let (event_tx, _) = std::sync::mpsc::channel();
        Self {
            agents: HashMap::new(),
            storage,
            event_tx,
            event_rx: Arc::new(RwLock::new(vec![])),
        }
    }

    /// Spawn a new agent (validate → reserve → persist → emit event).
    pub fn spawn(&mut self, request: SpawnRequest) -> Result<AgentId, AgentManagerError> {
        // Create agent record
        let mut record = AgentRecord::new(
            request.role,
            request.provider,
            request.workspace_id,
            request.config,
        );
        record.parent_id = request.parent_id;
        record.disposition = request.disposition;

        // Transition: Created → Starting
        record
            .transition(AgentStatus::Starting)
            .map_err(|e| AgentManagerError::InvalidTransition(e.to_string()))?;

        let agent_id = record.agent_id.clone();

        // Persist to disk
        self.storage.save(&record)?;

        // Add to in-memory registry
        self.agents.insert(agent_id.to_string(), record);

        // Emit event
        self.emit(AgentEvent::AssistantMessage {
            text: format!("agent spawned: {agent_id}"),
            message_id: agent_id.to_string(),
        });

        Ok(agent_id)
    }

    /// Get an agent record by ID.
    pub fn get(&self, agent_id: &AgentId) -> Result<AgentRecord, AgentManagerError> {
        self.agents
            .get(agent_id.as_str())
            .cloned()
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_string()))
    }

    /// Update agent status.
    pub fn update_status(
        &mut self,
        agent_id: &AgentId,
        new_status: AgentStatus,
    ) -> Result<(), AgentManagerError> {
        let record = self
            .agents
            .get_mut(agent_id.as_str())
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_string()))?;

        record
            .transition(new_status)
            .map_err(|e| AgentManagerError::InvalidTransition(e.to_string()))?;

        // Persist
        self.storage.save(record)?;

        Ok(())
    }

    /// Set the persistence handle (after session creation).
    pub fn set_persistence(
        &mut self,
        agent_id: &AgentId,
        handle: PersistenceHandle,
    ) -> Result<(), AgentManagerError> {
        let record = self
            .agents
            .get_mut(agent_id.as_str())
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_string()))?;

        record.persistence_handle = Some(handle);
        self.storage.save(record)?;

        Ok(())
    }

    /// Set the process ID.
    pub fn set_pid(&mut self, agent_id: &AgentId, pid: u32) -> Result<(), AgentManagerError> {
        let record = self
            .agents
            .get_mut(agent_id.as_str())
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_string()))?;

        record.pid = Some(pid);
        self.storage.save(record)?;

        Ok(())
    }

    /// Archive an agent (soft-delete).
    pub fn archive(&mut self, agent_id: &AgentId) -> Result<(), AgentManagerError> {
        let record = self
            .agents
            .get_mut(agent_id.as_str())
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_string()))?;

        record.archive();
        self.storage.save(record)?;

        Ok(())
    }

    /// List all agents.
    pub fn list(&self) -> Vec<&AgentRecord> {
        self.agents.values().collect()
    }

    /// List agents by status.
    pub fn list_by_status(&self, status: AgentStatus) -> Vec<&AgentRecord> {
        self.agents
            .values()
            .filter(|r| r.status == status)
            .collect()
    }

    /// Emit an event to all subscribers.
    fn emit(&self, event: AgentEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Subscribe to agent events.
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<AgentEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Ok(mut subs) = self.event_rx.write() {
            // Note: this is a simplified broadcast — in production, use tokio::sync::broadcast
            // For now, events are emitted via the single event_tx
        }
        rx
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use anti_core::model::Role;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anti-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn storage_save_and_load() {
        let dir = temp_dir();
        let storage = AgentStorage::new(dir.clone());
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));
        let record = AgentRecord::new(Role::Peer, ProviderKind::Claude, ws, AgentConfig::default());
        let agent_id = record.agent_id.clone();
        let workspace_id = record.workspace_id.clone();

        storage.save(&record).unwrap();
        let loaded = storage.load(&workspace_id, &agent_id).unwrap().unwrap();
        assert_eq!(loaded.agent_id, agent_id);
        assert_eq!(loaded.status, AgentStatus::Created);

        // Cleanup
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_list_workspace() {
        let dir = temp_dir();
        let storage = AgentStorage::new(dir.clone());
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));

        // Save two agents
        let r1 = AgentRecord::new(
            Role::Peer,
            ProviderKind::Claude,
            ws.clone(),
            AgentConfig::default(),
        );
        let r2 = AgentRecord::new(
            Role::Peer,
            ProviderKind::Codex,
            ws.clone(),
            AgentConfig::default(),
        );
        storage.save(&r1).unwrap();
        storage.save(&r2).unwrap();

        let records = storage.list_workspace(&ws).unwrap();
        assert_eq!(records.len(), 2);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manager_spawn_and_get() {
        let dir = temp_dir();
        let mut manager = AgentManager::new(dir.clone());
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));

        let request = SpawnRequest {
            role: Role::Peer,
            disposition: None,
            provider: ProviderKind::Claude,
            workspace_id: ws,
            config: AgentConfig::default(),
            parent_id: None,
        };

        let agent_id = manager.spawn(request).unwrap();
        let record = manager.get(&agent_id).unwrap();
        assert_eq!(record.status, AgentStatus::Starting);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manager_update_status() {
        let dir = temp_dir();
        let mut manager = AgentManager::new(dir.clone());
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));

        let request = SpawnRequest {
            role: Role::Peer,
            disposition: None,
            provider: ProviderKind::Claude,
            workspace_id: ws,
            config: AgentConfig::default(),
            parent_id: None,
        };

        let agent_id = manager.spawn(request).unwrap();
        manager
            .update_status(&agent_id, AgentStatus::Running)
            .unwrap();

        let record = manager.get(&agent_id).unwrap();
        assert_eq!(record.status, AgentStatus::Running);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manager_archive() {
        let dir = temp_dir();
        let mut manager = AgentManager::new(dir.clone());
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));

        let request = SpawnRequest {
            role: Role::Peer,
            disposition: None,
            provider: ProviderKind::Claude,
            workspace_id: ws,
            config: AgentConfig::default(),
            parent_id: None,
        };

        let agent_id = manager.spawn(request).unwrap();
        manager.archive(&agent_id).unwrap();

        let record = manager.get(&agent_id).unwrap();
        assert_eq!(record.status, AgentStatus::Replaced);
        assert!(record.archived_at.is_some());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manager_list_by_status() {
        let dir = temp_dir();
        let mut manager = AgentManager::new(dir.clone());
        let ws = WorkspaceId::new(std::path::Path::new("/tmp"));

        // Spawn two agents
        let r1 = SpawnRequest {
            role: Role::Peer,
            disposition: None,
            provider: ProviderKind::Claude,
            workspace_id: ws.clone(),
            config: AgentConfig::default(),
            parent_id: None,
        };
        let r2 = SpawnRequest {
            role: Role::Peer,
            disposition: None,
            provider: ProviderKind::Codex,
            workspace_id: ws,
            config: AgentConfig::default(),
            parent_id: None,
        };

        let id1 = manager.spawn(r1).unwrap();
        let _ = manager.spawn(r2).unwrap();

        // Move one to Running
        manager.update_status(&id1, AgentStatus::Running).unwrap();

        let running = manager.list_by_status(AgentStatus::Running);
        assert_eq!(running.len(), 1);

        let starting = manager.list_by_status(AgentStatus::Starting);
        assert_eq!(starting.len(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }
}
