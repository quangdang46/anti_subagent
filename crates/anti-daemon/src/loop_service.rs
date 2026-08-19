//! Loop service (mirrors Paseo loops.json, plan §6.6).
//!
//! Slim loop service: LoopRecord (8-char hex id), per-iteration verifyChecks
//! shell commands (64KB cap), verifyPrompt result, loop logs (seq pagination),
//! create/list/get/stop. Worker/verifier spawning deferred to daemon peer path.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const MAX_VERIFY_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopStatus {
    Running,
    Succeeded,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCheckResult {
    pub command: String,
    pub exit_code: i32,
    pub passed: bool,
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "completedAt")]
    pub completed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyPromptResult {
    pub passed: bool,
    pub reason: String,
    #[serde(rename = "verifierAgentId")]
    pub verifier_agent_id: Option<String>,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "completedAt")]
    pub completed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIteration {
    pub index: u32,
    #[serde(rename = "workerAgentId")]
    pub worker_agent_id: Option<String>,
    #[serde(rename = "verifierAgentId")]
    pub verifier_agent_id: Option<String>,
    pub status: LoopStatus,
    #[serde(rename = "verifyChecks")]
    pub verify_checks: Vec<VerifyCheckResult>,
    #[serde(rename = "verifyPrompt")]
    pub verify_prompt: Option<VerifyPromptResult>,
    pub logs: Vec<LoopLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopLogEntry {
    pub seq: u64,
    pub text: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopRecord {
    pub id: String,
    pub name: Option<String>,
    pub prompt: String,
    pub verify_checks: Vec<String>,
    #[serde(rename = "verifyPrompt")]
    pub verify_prompt: Option<String>,
    #[serde(rename = "sleepMs")]
    pub sleep_ms: u64,
    #[serde(rename = "maxIterations")]
    pub max_iterations: Option<u32>,
    #[serde(rename = "maxTimeMs")]
    pub max_time_ms: Option<u64>,
    pub status: LoopStatus,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub iterations: Vec<LoopIteration>,
    pub logs: Vec<LoopLogEntry>,
    #[serde(rename = "nextLogSeq")]
    pub next_log_seq: u64,
    #[serde(rename = "activeIteration")]
    pub active_iteration: Option<u32>,
}

pub struct LoopService {
    state_dir: PathBuf,
    inner: Arc<Mutex<LoopState>>,
}

#[derive(Debug, Default)]
struct LoopState {
    loops: Vec<LoopRecord>,
    persisted: bool,
}

fn loop_id() -> String {
    let uuid = uuid::Uuid::new_v4().to_string().replace('-', "");
    uuid[..8].to_string()
}

impl LoopService {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            state_dir,
            inner: Arc::new(Mutex::new(LoopState::default())),
        }
    }

    fn loops_path(&self) -> PathBuf {
        self.state_dir.join("loops").join("loops.json")
    }

    fn persist(&self, loops: &[LoopRecord]) -> std::io::Result<()> {
        let path = self.loops_path();
        std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
        let tmp = format!("{}.tmp.{}", path.display(), std::process::id());
        std::fs::write(&tmp, serde_json::to_string_pretty(loops).unwrap())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn create(
        &self,
        name: Option<String>,
        prompt: String,
        verify_checks: Vec<String>,
        verify_prompt: Option<String>,
        sleep_ms: u64,
        max_iterations: Option<u32>,
    ) -> LoopRecord {
        let now = Utc::now().to_rfc3339();
        let rec = LoopRecord {
            id: loop_id(),
            name,
            prompt,
            verify_checks,
            verify_prompt,
            sleep_ms,
            max_iterations,
            max_time_ms: None,
            status: LoopStatus::Running,
            created_at: now.clone(),
            updated_at: now,
            iterations: vec![],
            logs: vec![],
            next_log_seq: 1,
            active_iteration: None,
        };
        let mut s = self.inner.lock().unwrap();
        let mut all = s.loops.clone();
        all.push(rec.clone());
        let _ = self.persist(&all);
        s.loops = all;
        rec
    }

    pub fn list(&self) -> Vec<LoopRecord> {
        self.inner.lock().unwrap().loops.clone()
    }

    pub fn get(&self, id: &str) -> Option<LoopRecord> {
        self.inner
            .lock()
            .unwrap()
            .loops
            .iter()
            .find(|l| l.id == id)
            .cloned()
    }

    pub fn stop(&self, id: &str) -> Result<LoopRecord, String> {
        let mut s = self.inner.lock().unwrap();
        let pos = s
            .loops
            .iter()
            .position(|l| l.id == id)
            .ok_or_else(|| format!("loop {id} not found"))?;
        if matches!(
            s.loops[pos].status,
            LoopStatus::Succeeded | LoopStatus::Failed | LoopStatus::Stopped
        ) {
            return Err(format!("loop {id} already terminal"));
        }
        s.loops[pos].status = LoopStatus::Stopped;
        s.loops[pos].updated_at = Utc::now().to_rfc3339();
        let rec = s.loops[pos].clone();
        let all = s.loops.clone();
        let _ = self.persist(&all);
        Ok(rec)
    }

    pub fn logs(&self, id: &str, cursor: u64) -> Result<(Vec<LoopLogEntry>, u64), String> {
        let s = self.inner.lock().unwrap();
        let rec = s
            .loops
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| format!("loop {id} not found"))?;
        let entries: Vec<_> = rec
            .logs
            .iter()
            .filter(|e| e.seq >= cursor)
            .cloned()
            .collect();
        let next = rec.next_log_seq;
        Ok((entries, next))
    }

    /// Run a single verify check (shell command) — 64KB output cap per Paseo.
    pub fn run_verify_check(command: &str) -> VerifyCheckResult {
        let started_at = Utc::now().to_rfc3339();
        let out = if cfg!(windows) {
            Command::new("cmd").args(["/c", command]).output()
        } else {
            Command::new("sh").args(["-c", command]).output()
        };
        let out = match out {
            Ok(v) => v,
            Err(e) => {
                return VerifyCheckResult {
                    command: command.to_string(),
                    exit_code: 127,
                    passed: false,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    started_at,
                    completed_at: Utc::now().to_rfc3339(),
                };
            }
        };
        let completed_at = Utc::now().to_rfc3339();
        let passed = out.status.success();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let stdout = if stdout.len() > MAX_VERIFY_OUTPUT_BYTES {
            stdout[..MAX_VERIFY_OUTPUT_BYTES].to_string()
        } else {
            stdout
        };
        let stderr = if stderr.len() > MAX_VERIFY_OUTPUT_BYTES {
            stderr[..MAX_VERIFY_OUTPUT_BYTES].to_string()
        } else {
            stderr
        };
        VerifyCheckResult {
            command: command.to_string(),
            exit_code: out.status.code().unwrap_or(1),
            passed,
            stdout,
            stderr,
            started_at,
            completed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_loop() {
        let svc = LoopService::new(PathBuf::from("/tmp/anti-loop-test-1"));
        let rec = svc.create(
            Some("n".into()),
            "prompt".into(),
            vec!["echo ok".into()],
            None,
            0,
            Some(3),
        );
        assert_eq!(rec.status, LoopStatus::Running);
        assert!(rec.id.len() == 8);
    }
    #[test]
    fn stop_loop() {
        let svc = LoopService::new(PathBuf::from("/tmp/anti-loop-test-2"));
        let rec = svc.create(None, "p".into(), vec![], None, 0, None);
        let stopped = svc.stop(&rec.id).unwrap();
        assert_eq!(stopped.status, LoopStatus::Stopped);
    }
    #[test]
    fn verify_check() {
        let r = LoopService::run_verify_check("echo hello");
        assert!(r.passed);
        assert!(r.stdout.contains("hello"));
    }
}
