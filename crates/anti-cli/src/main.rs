//! anti CLI — the control plane (plan §26). CLI-only; no MCP.
//! P0 commands: spawn, list, status, wait, daemon (start/stop/status), doctor.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

#[derive(Parser)]
#[command(name = "anti", version, about = "Deploy peers, not subagents. SLP orchestration for coding agents.")]
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
}

#[derive(Subcommand)]
enum GuardAction {
    /// Install the PreToolUse guard into a peer workspace's .claude/hooks.json
    Install {
        /// Workspace (worktree) path to install into
        #[arg(long)]
        workspace: String,
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
    Start,
    Stop,
    Status,
}

fn main() {
    let cli = Cli::parse();
    let state_dir = cli
        .state_dir
        .unwrap_or_else(|| {
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
        } => commands::spawn(&state_dir, &id, &role, disposition.as_deref(), &harness, task.as_deref(), &repo, parent.as_deref()),
        Commands::List { role, status, json } => commands::list(&state_dir, role.as_deref(), status.as_deref(), json),
        Commands::Status { id } => commands::status(&state_dir, &id),
        Commands::Wait { id, until, timeout } => commands::wait(&state_dir, &id, &until, timeout),
        Commands::Daemon { action } => commands::daemon(&state_dir, action),
        Commands::Guard { action } => commands::guard(&state_dir, action),
        Commands::Doctor => commands::doctor(&state_dir),
    };

    match result {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
