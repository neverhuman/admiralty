//! Read-only super_captain fleet audit.
//!
//! Rust port of `tools/sc_audit.py`, `tools/native_queue_health.py` and
//! `captains/super_captain_tick.sh`. The audit recomputes project progress
//! from ledgers/receipts instead of trusting captain claims, and checks the
//! captain/lane screens for problems an LLM summary tends to miss. It writes
//! `<OUT>/latest.json`, `<OUT>/latest.md`, appends `<OUT>/audit.jsonl` and
//! keeps `<OUT>/state.json`. It never writes to project repos, never types
//! into screens and never kills anything (the ticker is the only component
//! that pastes into `super_captain`, and only at a verified idle prompt).
//!
//! CLI entry points (mounted by `admiralty-cli`):
//! - `admiralty sc-audit [--out DIR] [--quiet] [--no-gh] [--config JSON]`
//! - `admiralty sc-tick [--once] [--dir DIR] [--screen NAME] ...`
//! - `admiralty queue-health [--database PATH] [--stale-ms N] [--max-threads N]`

pub mod config;
pub mod ctx;
pub mod github;
pub mod host;
pub mod lanes;
pub mod ledgers;
pub mod progress;
pub mod py;
pub mod queue_health;
pub mod report;
pub mod screen;
pub mod tick;
pub mod watchdog;

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;

pub use config::{AuditConfig, ProjectConfig};
pub use ctx::{Audit, Finding};
pub use host::{CmdOut, Host, RealHost};
pub use report::{AuditOptions, AuditOutput, run_audit};

/// Boxed error type of the CLI entry points.
pub type CliError = Box<dyn Error + Send + Sync>;

/// `admiralty sc-audit`: the deterministic super_captain audit.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct ScAuditArgs {
    /// Output directory (default: <HOME>/xbabe2_kicker/captains-20261017/sc_audit).
    #[arg(long)]
    pub out: Option<String>,
    /// Do not print the Markdown report.
    #[arg(long)]
    pub quiet: bool,
    /// Skip every GitHub (gh) and GPU query.
    #[arg(long = "no-gh")]
    pub no_gh: bool,
    /// Optional JSON object overriding top-level AuditConfig fields.
    #[arg(long)]
    pub config: Option<PathBuf>,
}

/// Run `sc-audit`; prints the Markdown unless `--quiet`. Exit code 0, or 1
/// when the run fails (where the Python raised an uncaught exception).
pub fn run_sc_audit(args: &ScAuditArgs) -> Result<i32, CliError> {
    let cfg = AuditConfig::load(args.config.as_deref())?;
    let out = args.out.clone().unwrap_or_else(|| cfg.out_default.clone());
    let opts = AuditOptions {
        out,
        quiet: args.quiet,
        no_gh: args.no_gh,
    };
    match run_audit(&cfg, &RealHost, &opts) {
        Ok(result) => {
            if !opts.quiet {
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(result.markdown.as_bytes());
                let _ = stdout.flush();
            }
            Ok(0)
        }
        Err(e) => {
            eprintln!("sc-audit failed: {e}");
            Ok(1)
        }
    }
}

/// `admiralty sc-tick`: the super_captain ticker loop.
#[derive(Debug, Clone, clap::Args)]
pub struct ScTickArgs {
    /// Captains coordination directory (`D` in the original script).
    #[arg(long, default_value = tick::DEFAULT_TICK_DIR)]
    pub dir: String,
    /// Screen to tick (`S`).
    #[arg(long, default_value = tick::DEFAULT_SCREEN)]
    pub screen: String,
    /// Run one iteration without the initial delay or the trailing 30-minute sleep.
    #[arg(long)]
    pub once: bool,
    /// Binary used to run `sc-audit --quiet` (default: this executable).
    #[arg(long)]
    pub audit_exe: Option<PathBuf>,
    /// Skip running the audit before ticking.
    #[arg(long)]
    pub no_audit: bool,
    /// Screen hardcopy file used by the idle check.
    #[arg(long, default_value = "/tmp/hc_sc_tick")]
    pub hardcopy: String,
}

impl Default for ScTickArgs {
    fn default() -> Self {
        Self {
            dir: tick::DEFAULT_TICK_DIR.to_string(),
            screen: tick::DEFAULT_SCREEN.to_string(),
            once: false,
            audit_exe: None,
            no_audit: false,
            hardcopy: "/tmp/hc_sc_tick".to_string(),
        }
    }
}

/// Build the ticker configuration (`FIRST_DELAY` env var honored).
pub fn tick_config(args: &ScTickArgs) -> Result<tick::TickConfig, CliError> {
    let first_delay = tick::parse_sleep(std::env::var("FIRST_DELAY").ok().as_deref());
    let audit_argv = if args.no_audit {
        None
    } else {
        let exe = match &args.audit_exe {
            Some(p) => p.clone(),
            None => std::env::current_exe()?,
        };
        // `[ -f $AUDIT ]`: skip the audit when the program is missing.
        if exe.is_file() {
            Some(vec![
                "nice".to_string(),
                "-n".to_string(),
                "10".to_string(),
                exe.to_string_lossy().to_string(),
                "sc-audit".to_string(),
                "--quiet".to_string(),
                "--out".to_string(),
                format!("{}/sc_audit", args.dir),
            ])
        } else {
            None
        }
    };
    let mut cfg = tick::TickConfig::new(&args.dir, &args.screen, audit_argv, first_delay);
    cfg.hc_path = args.hardcopy.clone();
    Ok(cfg)
}

/// Run `sc-tick` (loops forever unless `--once`).
pub fn run_sc_tick(args: &ScTickArgs) -> Result<i32, CliError> {
    let cfg = tick_config(args)?;
    let ticker = tick::Ticker {
        cfg: &cfg,
        host: &RealHost,
    };
    ticker.run(args.once);
    Ok(0)
}

/// `admiralty queue-health`: read-only native Codex queue observation.
#[derive(Debug, Clone, clap::Args)]
pub struct QueueHealthArgs {
    /// Queue database (default: <HOME>/.codex/queue_1.sqlite).
    #[arg(long)]
    pub database: Option<PathBuf>,
    /// Age after which a pending thread is reported stalled.
    #[arg(long, default_value_t = queue_health::DEFAULT_STALE_MS)]
    pub stale_ms: i64,
    /// Maximum threads reported (1..=128).
    #[arg(long, default_value_t = queue_health::DEFAULT_MAX_THREADS)]
    pub max_threads: i64,
    /// Observation clock in epoch milliseconds (default: now).
    #[arg(long)]
    pub now_ms: Option<i64>,
}

impl Default for QueueHealthArgs {
    fn default() -> Self {
        Self {
            database: None,
            stale_ms: queue_health::DEFAULT_STALE_MS,
            max_threads: queue_health::DEFAULT_MAX_THREADS,
            now_ms: None,
        }
    }
}

/// Run `queue-health`: prints the JSON report. Exit 0, or 2 on invalid bounds.
pub fn run_queue_health(args: &QueueHealthArgs) -> Result<i32, CliError> {
    let db = args
        .database
        .clone()
        .unwrap_or_else(|| PathBuf::from(AuditConfig::default().queue_db));
    let now_ms = args
        .now_ms
        .unwrap_or_else(|| (py::timestamp_f64(&RealHost.now()) * 1000.0) as i64);
    match queue_health::read_queue_health(&db, now_ms, args.max_threads, args.stale_ms) {
        Ok(report) => {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(format!("{}\n", py::dumps(&report, Some(2))).as_bytes());
            Ok(0)
        }
        Err(e) => {
            eprintln!("queue-health: {}: {}", e.kind, e.msg);
            Ok(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_health_cli_reports_unknown_and_rejects_bad_bounds() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("missing.sqlite");
        let ok = QueueHealthArgs {
            database: Some(db.clone()),
            now_ms: Some(1),
            ..QueueHealthArgs::default()
        };
        assert_eq!(run_queue_health(&ok).expect("run"), 0);
        assert!(!db.exists(), "observation never creates the database");
        let bad = QueueHealthArgs {
            max_threads: 0,
            ..ok
        };
        assert_eq!(run_queue_health(&bad).expect("run"), 2);
    }

    #[test]
    fn sc_audit_config_file_is_a_shallow_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("cfg.json");
        std::fs::write(&path, r#"{"hc_dir": "/var/tmp/x", "gh": "/usr/bin/gh"}"#).expect("write");
        let cfg = AuditConfig::load(Some(&path)).expect("load");
        assert_eq!(cfg.hc_dir, "/var/tmp/x");
        assert_eq!(cfg.gh, "/usr/bin/gh");
        assert_eq!(cfg.projects.len(), 5);
        std::fs::write(&path, r#"{"typo_key": 1}"#).expect("write");
        assert!(AuditConfig::load(Some(&path)).is_err());
    }
}
