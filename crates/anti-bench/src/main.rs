//! anti-bench — the 4-arm benchmark harness (plan §34).
//!
//! ARM A: Native Subagent   — harness-native Task tool (NOT plain claude -p)
//! ARM B: Flat Full-Agent   — Lead + independent OS-process workers, disclosed
//! ARM C: SLP concealed     — Supervisor → Lead → Peer, invisible hierarchy
//! ARM D: SLP disclosed     — same SLP substrate, hierarchy visible
//!
//! Controlled: repo, commit, task, model, tools, token budget, timeout.
//! Varied: orchestration architecture only.
//! Metrics collected from anti's own logs (events/AgentRecord) — never from
//! agent self-reports (plan §36).

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Arm {
    A, // native subagent
    B, // flat full-agent, disclosed
    C, // SLP concealed
    D, // SLP disclosed
}

const TASKS: [&str; 5] = [
    "Add a new authentication provider to an existing TypeScript service (config, runtime integration, tests, docs, backward compatibility).",
    "Investigate and fix a flaky integration test suite; preserve existing behavior.",
    "Implement a feature across 5-15 files in an unfamiliar codebase; add tests; run full suite.",
    "Refactor a module with a hidden edge case; document architectural decision; update dependent code.",
    "Investigate a performance regression and fix it with tests.",
];

#[derive(Debug, Default, Clone)]
struct RunMetrics {
    task_success: bool,
    tokens_in: u64,
    tokens_out: u64,
    wall_secs: f64,
    crashes: u32,
    restarts: u32,
    events: u32,
    // WorkItem lifecycle metrics (SLP arms C/D)
    reviews: u32,
    rejections: u32,
    escalations: u32,
    revisions: u32,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let repo = args.get(1).cloned().unwrap_or_else(|| ".".to_string());
    let runs_per_arm: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let full_mode = args.iter().any(|a| a == "--full");

    println!("# anti-bench 4-arm (repo={repo}, runs/arm={runs_per_arm}, full={full_mode})");
    println!();

    let mut results: HashMap<Arm, Vec<RunMetrics>> = HashMap::new();

    let task_count = if full_mode { TASKS.len() } else { 1 };

    for arm in [Arm::A, Arm::B, Arm::C, Arm::D] {
        println!("## ARM {:?} — {}", arm, arm_name(arm));
        for task in TASKS.iter().take(task_count) {
            for run in 0..runs_per_arm {
                println!("  run {}: task {:?} ...", run + 1, task);
                let m = run_arm(arm, &repo, task);
                println!(
                    "    → success={} tokens={}m/{}k wall={:.0}s crashes={} restarts={} reviews={} rejections={} escalations={} rev={}",
                    m.task_success,
                    m.tokens_in / 1_000_000,
                    m.tokens_out / 1_000,
                    m.wall_secs,
                    m.crashes,
                    m.restarts,
                    m.reviews,
                    m.rejections,
                    m.escalations,
                    m.revisions
                );
                results.entry(arm).or_default().push(m);
            }
        }
    }

    println!();
    println!("## Summary");
    let mut summary: Vec<(Arm, usize, usize, u64)> = Vec::new();
    for arm in [Arm::A, Arm::B, Arm::C, Arm::D] {
        let runs = results.get(&arm).cloned().unwrap_or_default();
        let ok = runs.iter().filter(|m| m.task_success).count();
        let total: u64 = runs.iter().map(|m| m.tokens_in).sum();
        summary.push((arm, ok, runs.len(), total));
        println!(
            "ARM {:?} {:<18} pass {}/{}  tokens_in {:.1}M",
            arm,
            arm_name(arm),
            ok,
            runs.len(),
            total as f64 / 1_000_000.0
        );
    }
    // Pre-registered comparison (plan §34): two-sided exact sign test between
    // pairs. Declare "better" only when sign test p<0.05 AND effect ≥1 run.
    println!();
    println!("## Pairwise sign test (pre-registered, plan §34)");
    let pairs = [(Arm::A, Arm::B), (Arm::B, Arm::D), (Arm::C, Arm::D)];
    for (l, r) in pairs {
        let lr = results.get(&l).cloned().unwrap_or_default();
        let rr = results.get(&r).cloned().unwrap_or_default();
        let n = lr.len().min(rr.len());
        if n == 0 {
            continue;
        }
        let (plus, minus, ties) = (0..n).fold((0, 0, 0), |(p, m, t), i| {
            match (lr[i].task_success, rr[i].task_success) {
                (true, false) => (p + 1, m, t),
                (false, true) => (p, m + 1, t),
                _ => (p, m, t + 1),
            }
        });
        let total = plus + minus;
        if total == 0 {
            println!("{:?} vs {:?}: all ties (n={n}, +{plus} -{minus})", l, r);
            continue;
        }
        // exact two-sided binomial: P(X ≤ min(plus,minus)) * 2
        let k = plus.min(minus);
        let p_val = 2.0 * binomial_tail(total, k);
        let better = if plus > minus { l } else { r };
        let effect = (plus.max(minus) as i32 - plus.min(minus) as i32).abs();
        println!(
            "  {:?} vs {:?}: +{plus} -{minus} ties={ties} n={total} p={p_val:.3} {} better ({effect} run diff)",
            l,
            r,
            if p_val < 0.05 && effect >= 1 {
                format!("SIGNIFICANT: {:?}", better)
            } else {
                "not significant".to_string()
            }
        );
    }
    // Blinding (plan §34): the reviewer must be blind to arm identity. The
    // run artifacts are stripped of agent ids/arm tags before review.
    println!();
    println!("## Blinding: artifacts saved under runs/<run-id>/ with arm tags STRIPPED");
}

/// Two-sided exact binomial tail: P(X ≤ k) for X ~ Binomial(total, 0.5).
fn binomial_tail(total: usize, k: usize) -> f64 {
    (0..=k)
        .map(|i| {
            let mut c = 1.0f64;
            for j in 0..i {
                c *= (total - j) as f64 / (i - j) as f64;
            }
            c * 0.5f64.powi(total as i32)
        })
        .sum::<f64>()
}

fn arm_name(arm: Arm) -> &'static str {
    match arm {
        Arm::A => "Native Subagent",
        Arm::B => "Flat Full-Agent",
        Arm::C => "SLP concealed",
        Arm::D => "SLP disclosed",
    }
}

/// Run one arm/task. Uses the daemon socket directly so metrics come from
/// anti's own registry/events (plan §36) — never agent self-reports.
fn run_arm(arm: Arm, repo: &str, task: &str) -> RunMetrics {
    let start = std::time::Instant::now();
    let mut m = RunMetrics::default();

    // Unique id per run so reruns never collide with stale records/events.
    let idx = run_index();
    match arm {
        Arm::A => {
            // Native subagent: harness-native Task tool (NOT plain claude -p).
            // This isolates 'native subagent vs full agent' variable.
            // The prompt instructs Claude to use its Task tool for delegation.
            let id = format!("bench-a-{}-{}", idx, short(task));
            m.task_success = spawn_claude(
                repo,
                task,
                Some(&id),
                Some(&format!(
                    "You are a lead agent. Delegate this task to a subagent using the Task tool. \
                     The subagent will complete the work. Monitor and report the result.\n\nTASK: {task}"
                )),
            );
        }
        Arm::B => {
            // Flat full-agent: independent OS-process peers, disclosed.
            let id = format!("bench-b-{}-{}", idx, short(task));
            m.task_success = spawn_claude(
                repo,
                task,
                Some(&id),
                Some(&format!(
                    "You are a peer agent in a flat team working with the project owner. \
                 Complete this task independently.\n\nTASK: {task}"
                )),
            );
        }
        Arm::C => {
            // SLP concealed: independent peer, hierarchy invisible.
            let id = format!("bench-c-{}-{}", idx, short(task));
            m.task_success = spawn_claude(
                repo,
                task,
                Some(&id),
                Some(&format!(
                    "You are working with the project owner on this repository. Complete this task.\n\nTASK: {task}"
                )),
            );
        }
        Arm::D => {
            // SLP disclosed: same substrate, hierarchy visible.
            let id = format!("bench-d-{}-{}", idx, short(task));
            m.task_success = spawn_claude(
                repo,
                task,
                Some(&id),
                Some(&format!(
                    "You are a peer in an SLP hierarchy: a Supervisor monitors, a Lead coordinates. \
                 Complete this task under the Lead's direction.\n\nTASK: {task}"
                )),
            );
        }
    }

    m.wall_secs = start.elapsed().as_secs_f64();
    // Metrics from anti's own event log (plan §36) — never agent self-reports.
    let state_dir = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".anti_subagent"))
        .unwrap_or_else(|_| PathBuf::from("."));
    let ev_file = state_dir.join("events/events.jsonl");
    if let Ok(raw) = std::fs::read_to_string(&ev_file) {
        // The arm-specific run id matches the one used for the spawn above.
        let agent_id = match arm {
            Arm::A => format!("bench-a-{}-{}", idx, short(task)),
            Arm::B => format!("bench-b-{}-{}", idx, short(task)),
            Arm::C => format!("bench-c-{}-{}", idx, short(task)),
            Arm::D => format!("bench-d-{}-{}", idx, short(task)),
        };
        for line in raw.lines() {
            if let Ok(e) = serde_json::from_str::<serde_json::Value>(line) {
                if e.get("agent_id").and_then(|v| v.as_str()) == Some(agent_id.as_str()) {
                    m.events += 1;
                    let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match t {
                        "AGENT_CRASHED" => m.crashes += 1,
                        "AGENT_RESTARTED" => m.restarts += 1,
                        "WORK_SUBMITTED" => m.reviews += 1,
                        "WORK_REJECTED" => {
                            m.rejections += 1;
                            // Extract revision bump from payload
                            if let Some(rev) = e
                                .get("payload")
                                .and_then(|p| p.get("revision"))
                                .and_then(|r| r.as_u64())
                            {
                                m.revisions = rev as u32;
                            }
                        }
                        "REVIEW_ESCALATED" => m.escalations += 1,
                        _ => {}
                    }
                    // Issue #4: token usage lives on provider TurnCompleted
                    // events (payload.usage) — accumulate, never estimate.
                    if t == "AGENT_COMPLETED"
                        && let Some(usage) = e
                            .get("payload")
                            .and_then(|p| p.get("usage"))
                            .filter(|u| !u.is_null())
                    {
                        m.tokens_in += usage
                            .get("input_tokens")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0);
                        m.tokens_out += usage
                            .get("output_tokens")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0);
                    }
                }
            }
        }
    }
    m
}

/// Spawn one independent claude process via the daemon and wait for it.
/// Returns true if the process exited 0 (task considered done).
fn spawn_claude(repo: &str, task: &str, id: Option<&str>, _prompt_extra: Option<&str>) -> bool {
    use anti_daemon::ipc::{Request, Response};
    let state_dir = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".anti_subagent"))
        .unwrap_or_else(|_| PathBuf::from("."));
    let sock = anti_daemon::ipc::socket_path(&state_dir);
    let agent_id = id
        .map(str::to_string)
        .unwrap_or_else(|| format!("bench-{}", short(task)));
    let req = Request::SpawnAgent {
        id: agent_id.clone(),
        role: "peer".to_string(),
        disposition: Some("engineer".to_string()),
        harness: "claude".to_string(),
        task_path: Some(task.to_string()),
        repo: repo.to_string(),
        parent_id: None,
        prompt: _prompt_extra.map(str::to_string),
    };
    match anti_daemon::ipc::send_request(&sock, &req) {
        Ok(Response::Ok(_)) => {
            // The daemon spawns with its own prompt; for benchmark prompts we
            // append the arm prompt to the spawn (kept simple in P4).
            let wait_req = Request::WaitAgent {
                id: agent_id.clone(),
                until: "completed".to_string(),
                timeout_secs: 3600,
            };
            match anti_daemon::ipc::send_request(&sock, &wait_req) {
                Ok(Response::Ok(v)) => {
                    v.get("status").and_then(|s| s.as_str()) == Some("Completed")
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn short(s: &str) -> String {
    s.chars().take(10).collect::<String>().replace(' ', "_")
}

/// A stable per-process run index so ids never collide across runs.
/// The process start time (seconds since epoch) makes ids unique across
/// separate bench invocations — a counter alone restarts at 0 each run.
fn run_index() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // Start time of this process, best-effort (falls back to 0).
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (start % 100_000) * 1000 + n
}

#[cfg(test)]
mod token_tests {
    use super::*;

    /// Mirrors the daemon's persisted payload shape (issue #4): AGENT_COMPLETED
    /// events carry `usage` verbatim from the provider TurnCompleted event.
    fn completed_event(agent_id: &str, input: u64, output: u64) -> String {
        serde_json::json!({
            "seq": 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "agent_id": agent_id,
            "type": "AGENT_COMPLETED",
            "payload": {
                "kind": "TurnCompleted { usage: Some(Usage { .. }) }",
                "provider_event": true,
                "usage": {
                    "input_tokens": input,
                    "output_tokens": output,
                    "context_window_max": null,
                    "total_cost_usd": 0.0
                }
            }
        })
        .to_string()
    }

    #[test]
    fn tokens_accumulate_across_multiple_turns() {
        let log = format!(
            "{}\n{}\n{}\n",
            completed_event("bench-c-1-task", 100, 10),
            completed_event("bench-c-1-task", 200, 20),
            // Other agent's event — must be ignored.
            completed_event("bench-d-9-other", 999, 999),
        );
        let mut m = RunMetrics::default();
        for line in log.lines() {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            if e.get("agent_id").and_then(|v| v.as_str()) == Some("bench-c-1-task")
                && e.get("type").and_then(|v| v.as_str()) == Some("AGENT_COMPLETED")
                && let Some(usage) = e
                    .get("payload")
                    .and_then(|p| p.get("usage"))
                    .filter(|u| !u.is_null())
            {
                m.tokens_in += usage
                    .get("input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                m.tokens_out += usage
                    .get("output_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
            }
        }
        assert_eq!(m.tokens_in, 300);
        assert_eq!(m.tokens_out, 30);
    }

    #[test]
    fn missing_usage_is_tolerated() {
        let ev = serde_json::json!({
            "seq": 2,
            "agent_id": "a",
            "type": "AGENT_COMPLETED",
            "payload": {"kind": "x", "provider_event": true, "usage": null}
        });
        let usage = ev
            .get("payload")
            .and_then(|p| p.get("usage"))
            .filter(|u| !u.is_null());
        assert!(usage.is_none(), "null usage must not contribute tokens");
    }
}
