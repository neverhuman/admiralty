//! Ported operator tools.
//!
//! Rust replacements for the retired Python/shell utilities:
//!
//! | Retired source                  | Module               | Command                         |
//! |---------------------------------|----------------------|---------------------------------|
//! | `tools/rollout_observe.py`      | [`rollout_observe`]  | `admiralty rollout observe`     |
//! | `tools/rollout_packets.py`      | [`rollout_packets`]  | `admiralty rollout packets`     |
//! | `tools/export_history.py`       | [`history_export`]   | `admiralty history export`      |
//! | `tools/history_export_cron.sh`  | [`history_publish`]  | `admiralty history publish`     |
//! | `tools/probe_opencode.py`       | [`opencode_probe`]   | `admiralty opencode probe`      |

pub mod error;
pub mod fsx;
pub mod history_export;
pub mod history_publish;
pub mod opencode_probe;
pub mod proc;
pub mod pycsv;
pub mod pyjson;
pub mod pytime;
pub mod rollout_observe;
pub mod rollout_packets;

pub use error::{ToolError, ToolResult};

/// Boxed error returned by the command entry points.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `admiralty rollout ...`
#[derive(clap::Subcommand, Debug, Clone)]
pub enum RolloutCommand {
    /// Temporary read-only lane observer; never dispatches, types input or accepts work.
    Observe(rollout_observe::ObserveArgs),
    /// Prepare bounded implementation missions; only a caller-selected owned directory outside the repository is writable.
    Packets(rollout_packets::PacketsArgs),
}

/// `admiralty history ...`
#[derive(clap::Subcommand, Debug, Clone)]
pub enum HistoryCommand {
    /// Export the captains dashboard's running history into a sanitized, git-friendly dataset.
    Export(history_export::ExportArgs),
    /// Hourly publisher (cron entry point): export in its own clone and push allowlisted data/history files.
    Publish(history_publish::PublishArgs),
}

/// `admiralty opencode ...`
#[derive(clap::Subcommand, Debug, Clone)]
pub enum OpencodeCommand {
    /// One explicit, bounded OpenCode SSH qualification; never reads credential files.
    Probe(opencode_probe::ProbeArgs),
}

/// Run `admiralty rollout ...`; returns the process exit code.
pub fn run_rollout(command: RolloutCommand) -> Result<i32, BoxError> {
    Ok(match command {
        RolloutCommand::Observe(args) => rollout_observe::run(&args),
        RolloutCommand::Packets(args) => rollout_packets::run(&args),
    })
}

/// Run `admiralty history ...`; returns the process exit code.
pub fn run_history(command: HistoryCommand) -> Result<i32, BoxError> {
    match command {
        HistoryCommand::Export(args) => history_export::run(&args),
        HistoryCommand::Publish(args) => history_publish::run(&args),
    }
}

/// Run `admiralty opencode ...`; returns the process exit code.
pub fn run_opencode(command: OpencodeCommand) -> Result<i32, BoxError> {
    match command {
        OpencodeCommand::Probe(args) => opencode_probe::run(&args),
    }
}
