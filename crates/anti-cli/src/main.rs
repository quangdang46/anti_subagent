//! anti CLI — the control plane (plan §26). CLI-only; no MCP.
//! P0 commands: spawn, list, status, wait, daemon (start/stop/status), doctor.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

#[derive(Parser)]
#[command(
    name = "anti",
    version,
    about = "Deploy peers, not subagents. SLP orchestration for coding agents."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Override state dir (default ~/.anti_subagent)
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Spawn a durable peer agent (independent OS process, not a subagent)
    Spawn {
        /// Agent id (stable, opaque)
        #[arg(long)]
        id: String,
        /// Role: supervisor | lead | peer
        #[arg(long, default_value = "peer")]
        role: String,
        /// Disposition: engineer | architect | reviewer | scout | proof-auditor | shadow
        #[arg(long)]
        disposition: Option<String>,
        /// Harness: claude | codex | opencode
        #[arg(long, default_value = "claude")]
        harness: String,
        /// Task file path
        #[arg(long)]
        task: Option<String>,
        /// Repo root to work in
        #[arg(long)]
        repo: String,
        /// Parent agent id (lead of this peer)
        #[arg(long)]
        parent: Option<String>,
        /// Peer prompt file (concealment toggle: plan §34)
        #[arg(long)]
        peer_prompt: Option<PathBuf>,
        /// Benchmark arm: a|b|c|d (concealment is a runtime variable)
        #[arg(long)]
        arm: Option<String>,
    },
    /// List agents
    List {
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show status of one agent
    Status { id: String },
    /// Block until an agent reaches a status (event-gated, no polling)
    Wait {
        id: String,
        #[arg(long, default_value = "completed")]
        until: String,
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
    },
    /// Graceful stop (SIGTERM)
    Stop { id: String },
    /// Force kill (SIGKILL)
    Kill { id: String },
    /// Supervised restart of a crashed agent (same id)
    Restart { id: String },
    /// Manage the control-plane daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Manage the peer guard (deny delegation-shaped tools in peers)
    Guard {
        #[command(subcommand)]
        action: GuardAction,
    },
    /// Check daemon, state dir, treehouse, claude
    Doctor,
    /// Manage work items (SLP task lifecycle)
    Work {
        #[command(subcommand)]
        action: WorkAction,
    },
    /// Show recent review escalations (watchdog events)
    Escalations,
    /// Report task status back to the daemon (peer → anti channel)
    Report {
        /// Task/work item ID
        #[arg(long)]
        task: String,
        /// Status: completed, failed, progress, question
        #[arg(long)]
        status: String,
        /// Git commit SHA (for completed status)
        #[arg(long)]
        commit: Option<String>,
        /// Error message (for failed status)
        #[arg(long)]
        error: Option<String>,
        /// Progress/question message text
        #[arg(long)]
        message: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkAction {
    /// Submit work item with evidence (InProgress|NeedsRevision → Submitted)
    Submit {
        /// Work item id
        id: String,
        /// SHA-256 of the artifact
        #[arg(long)]
        sha: String,
        /// Path to the artifact file
        #[arg(long)]
        path: String,
        /// Review timeout in seconds (default 600)
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Review a work item (accept requires Verified state; reject bumps revision)
    Review {
        /// Work item id
        id: String,
        /// Verdict: accept | reject
        verdict: String,
        /// Review note
        #[arg(long, default_value = "")]
        note: String,
    },
    /// List all work items
    List,
}

#[derive(Subcommand)]
enum GuardAction {
    /// Install the PreToolUse guard into a peer workspace's .claude/hooks.json
    Install {
        /// Workspace (worktree) path to install into
        #[arg(long)]
        workspace: String,
        /// Benchmark arm for guard config parameterization
        #[arg(long)]
        arm: Option<String>,
    },
    /// Classify a tool name (allow/deny) without installing
    Test {
        #[arg(long)]
        tool: String,
    },
    /// Show guard status
    Status,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start daemon (background by default; --foreground keeps attached)
    Start {
        /// Run in foreground (no detach)
        #[arg(long)]
        foreground: bool,
    },
    Stop,
    Status,
}

fn main() {
    let cli = Cli::parse();
    let state_dir = cli.state_dir.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".anti_subagent"))
            .unwrap_or_else(|_| PathBuf::from("."))
    });

    let result = match cli.command {
        Commands::Spawn {
            id,
            role,
            disposition,
            harness,
            task,
            repo,
            parent,
            peer_prompt,
            arm,
        } => commands::spawn(
            &state_dir,
            &id,
            &role,
            disposition.as_deref(),
            &harness,
            task.as_deref(),
            &repo,
            parent.as_deref(),
            peer_prompt.as_deref(),
            arm.as_deref(),
        ),
        Commands::List { role, status, json } => {
            commands::list(&state_dir, role.as_deref(), status.as_deref(), json)
        }
        Commands::Status { id } => commands::status(&state_dir, &id),
        Commands::Wait { id, until, timeout } => commands::wait(&state_dir, &id, &until, timeout),
        Commands::Stop { id } => commands::stop(&state_dir, &id, false),
        Commands::Kill { id } => commands::stop(&state_dir, &id, true),
        Commands::Restart { id } => commands::restart(&state_dir, &id),
        Commands::Daemon { action } => commands::daemon(&state_dir, action),
        Commands::Guard { action } => match action {
            GuardAction::Install { workspace, arm } => {
                commands::guard_install(&state_dir, &workspace, arm.as_deref())
            }
            GuardAction::Test { tool } => commands::guard_test(&state_dir, &tool),
            GuardAction::Status => commands::guard_status(&state_dir),
        },
        Commands::Doctor => commands::doctor(&state_dir),
        Commands::Work { action } => commands::work(&state_dir, action),
        Commands::Escalations => commands::escalations(&state_dir),
        Commands::Report {
            task,
            status,
            commit,
            error,
            message,
        } => commands::report(
            &state_dir,
            &task,
            &status,
            commit.as_deref(),
            error.as_deref(),
            message.as_deref(),
        ),
    };

    match result {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
