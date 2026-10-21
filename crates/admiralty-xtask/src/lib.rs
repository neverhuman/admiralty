//! Admiralty repository automation (`cargo xtask ...`).
//!
//! | Former script | Command |
//! | --- | --- |
//! | `scripts/check/all.sh` (`pnpm check`) | `cargo xtask check` |
//! | `scripts/check/security-supply-chain.sh`, `tools/security-lane.sh` | `cargo xtask security` |
//! | `scripts/dev/demo.sh` (`pnpm demo`) | `cargo xtask demo` |
//! | (new) single-binary build with the embedded console | `cargo xtask dist` |
//! | `scripts/dev/live.sh` (`pnpm live`) | `cargo xtask live --status ... [--index ...] [--models ...]` |

#[cfg(not(unix))]
compile_error!("admiralty-xtask supports Unix hosts only, like the scripts it replaces");

pub mod check;
pub mod dist;
pub mod launch;
pub mod process;
pub mod security;
pub mod state;

use clap::{Args, Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Parser, Debug)]
#[command(
    name = "xtask",
    about = "Admiralty repository automation (run as `cargo xtask <task>`)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub task: Task,
}

#[derive(Subcommand, Debug)]
pub enum Task {
    /// Run the complete local CI matrix (`pnpm check`).
    Check(check::CheckArgs),
    /// Fail-closed workflow, secret and lockfile supply-chain checks.
    Security,
    /// Build the dashboard, then release binaries with the console embedded.
    Dist(dist::DistArgs),
    /// Build the dashboard and serve the isolated synthetic demo (`pnpm demo`).
    Demo,
    /// Serve real producer observations read-only on loopback (`pnpm live`).
    #[command(disable_help_flag = true)]
    Live(LiveArgs),
}

#[derive(Args, Debug)]
pub struct LiveArgs {
    /// --status /absolute/status.json [--index /absolute/index.html] [--models /private/models.json]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub args: Vec<OsString>,
}

/// Canonical repository root (two levels above this crate's manifest).
pub fn repo_root() -> Result<PathBuf, Error> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|crates| crates.parent())
        .ok_or("cannot locate the repository root")?;
    Ok(root.canonicalize()?)
}

/// Dispatch a parsed command line; returns the process exit code.
pub fn run(cli: Cli) -> Result<i32, Error> {
    let root = repo_root()?;
    match cli.task {
        Task::Check(args) => check::run(&root, &args),
        Task::Security => security::run(root),
        Task::Dist(args) => dist::run_dist(&root, &args),
        Task::Demo => launch::run_demo(&root),
        Task::Live(args) => launch::run_live(&root, &args.args),
    }
}
