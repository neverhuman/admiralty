//! Fleet operator tools (`admiralty fleet ...`): typed, tested replacements
//! for the retired `tools/bin/{screen_snap,screen_send,classify_screen.py,
//! status,kick}`, `tools/{owner_peek,peek_all,owner_send,mr_probe,
//! codex-luna,send_*,sweep}.sh` and `dashboard/globalres.py`.
//!
//! Every external program goes through [`system::Runner`] (null stdin,
//! bounded timeouts) and every GNU screen call is shape-checked so the tools
//! can never attach or detach a session (`screen -r/-dr/-D` are impossible).

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

pub mod classify;
pub mod codex_luna;
pub mod compat;
pub mod config;
pub mod deliver;
pub mod kick;
pub mod peek;
pub mod probe_models;
pub mod resources;
pub mod screen;
pub mod send;
pub mod snap;
pub mod status;
pub mod sweep;
pub mod system;

#[cfg(test)]
mod testing;

pub use config::{CaptainTarget, FleetConfig};

use system::{Clock, Runner, SystemClock, SystemRunner};

/// Shared dependencies of every subcommand.
pub struct Ctx<'a> {
    pub runner: &'a dyn Runner,
    pub clock: &'a dyn Clock,
    pub cfg: &'a FleetConfig,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    /// Whether the caller's locale is UTF-8 (bash `${#s}` / `${s:0:n}`
    /// count characters then, bytes otherwise — e.g. under cron).
    pub utf8_locale: bool,
}

/// `LC_ALL` / `LC_CTYPE` / `LANG` (first non-empty) names a UTF-8 locale.
pub fn locale_is_utf8(env: &dyn Fn(&str) -> Option<String>) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.is_empty()))
        .is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("utf-8") || v.contains("utf8")
        })
}

impl<'a> Ctx<'a> {
    pub fn new(
        runner: &'a dyn Runner,
        clock: &'a dyn Clock,
        cfg: &'a FleetConfig,
        out: &'a mut dyn Write,
        err: &'a mut dyn Write,
    ) -> Self {
        Self {
            runner,
            clock,
            cfg,
            out,
            err,
            utf8_locale: false,
        }
    }

    /// Set the locale flag (see [`Ctx::utf8_locale`]).
    pub fn with_utf8_locale(mut self, utf8: bool) -> Self {
        self.utf8_locale = utf8;
        self
    }

    /// Guarded screen client.
    pub fn screen(&self) -> screen::Screen<'a> {
        screen::Screen::new(self.runner, &self.cfg.screen_bin, self.cfg.screen_timeout())
    }
}

/// `admiralty fleet ...`.
#[derive(Debug, clap::Args)]
pub struct FleetArgs {
    /// JSON file overriding host-specific defaults (see `FleetConfig`).
    #[arg(long, value_name = "JSON")]
    pub config: Option<PathBuf>,
    /// Kicker base directory (default /home/ubuntu/xbabe2_kicker; env
    /// ADMIRALTY_KICKER_BASE).
    #[arg(long, value_name = "DIR")]
    pub kicker_base: Option<PathBuf>,
    #[command(subcommand)]
    pub command: FleetCommand,
}

/// Fleet subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum FleetCommand {
    /// Hardcopy a screen session (scrollback, NUL-stripped) to a text file
    /// and print its path (was tools/bin/screen_snap).
    Snap {
        session: String,
        /// Default: <kicker-base>/state/snaps/SESSION.txt.
        outpath: Option<PathBuf>,
    },
    /// Stuff text (+CR) or a named key into window 0 (was tools/bin/screen_send).
    Send(SendArgs),
    /// Classify a hardcopy snapshot; prints JSON (was tools/bin/classify_screen.py).
    Classify {
        snap_file: PathBuf,
        #[arg(long, default_value = "")]
        session: String,
    },
    /// Snapshot + classify the monitored sessions (was tools/bin/status).
    Status {
        /// Sessions to report (repeatable or comma-separated; default: config).
        #[arg(long = "session", value_delimiter = ',')]
        sessions: Vec<String>,
    },
    /// Approve / nudge monitored sessions once (was tools/bin/kick).
    /// Usage: kick [--dry-run] [--session NAME] [--once] [--inject TEXT] |
    /// kick --inject SESSION TEXT
    #[command(disable_help_flag = true)]
    Kick {
        #[arg(num_args = 0.., allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Print the last printable lines of a session (was tools/owner_peek.sh
    /// and tools/peek_all.sh with --all).
    Peek {
        /// Peek every session of the peek_all list (or --sessions).
        #[arg(long)]
        all: bool,
        /// Session list for --all (comma-separated).
        #[arg(long, value_delimiter = ',', requires = "all")]
        sessions: Vec<String>,
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        session: Option<String>,
        /// Number of lines (default 25).
        #[arg(conflicts_with = "all")]
        lines: Option<usize>,
    },
    /// readbuf FILE, paste, wait 1 s, CR (was tools/owner_send.sh).
    Paste { session: String, file: PathBuf },
    /// Idle-gated delivery driven by a JSON plan (was tools/send_*.sh).
    Deliver {
        #[arg(long, value_name = "PLAN.json")]
        plan: PathBuf,
    },
    /// Table of screen name, TUI model banner and model flags of the process
    /// tree (was tools/mr_probe.sh).
    ProbeModels { outdir: Option<PathBuf> },
    /// Exec codex pinned to gpt-6-luna / medium in an existing directory
    /// (was tools/codex-luna.sh). Exit 64 on bad arguments, 127 without codex.
    #[command(name = "codex-luna", disable_help_flag = true)]
    CodexLuna {
        #[arg(num_args = 0.., allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<OsString>,
    },
    /// Read-only xbabe1/2/3 resource sampler (was dashboard/globalres.py).
    Resources {
        #[arg(long)]
        quiet: bool,
    },
    /// Read-only host survey: remotes, PRs and main runs, GPUs, download
    /// status, recent transcripts, worktrees, cron, timers, disks, load
    /// (was tools/sweep.sh).
    Sweep,
}

/// `fleet send SESSION --text TEXT | --keys KEYS`.
#[derive(Debug, clap::Args)]
#[command(group = clap::ArgGroup::new("payload").required(true).multiple(false))]
pub struct SendArgs {
    pub session: String,
    /// Text to type, followed by CR.
    #[arg(long, group = "payload", allow_hyphen_values = true)]
    pub text: Option<String>,
    /// Named key (enter, esc, y, a, n, tab, shifttab, altup, ctrlc, ctrlo,
    /// ctrlt, ctrlu, always, bypass, 2, yes) or a printf %b escape string.
    #[arg(long, group = "payload", allow_hyphen_values = true)]
    pub keys: Option<String>,
}

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Resolve the effective config for `args` from the process environment.
pub fn effective_config(args: &FleetArgs) -> Result<FleetConfig, BoxError> {
    let env = |k: &str| std::env::var(k).ok();
    let mut cfg = FleetConfig::load(args.config.as_deref(), &env)?;
    if let Some(base) = &args.kicker_base {
        cfg.kicker_base = base.clone();
    }
    Ok(cfg)
}

/// Run a fleet subcommand with the real runner and clock; returns the exit
/// code.
pub fn run(args: FleetArgs) -> Result<i32, BoxError> {
    if let FleetCommand::CodexLuna { args: luna_args } = &args.command {
        let cfg = effective_config(&args)?;
        return Ok(codex_luna::run(
            luna_args,
            &cfg.codex_model,
            &cfg.codex_reasoning_effort,
            &mut std::io::stderr(),
        ));
    }
    let cfg = effective_config(&args)?;
    let runner = SystemRunner;
    let clock = SystemClock;
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let utf8 = locale_is_utf8(&|k: &str| std::env::var(k).ok());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err).with_utf8_locale(utf8);
    let code = dispatch(&mut ctx, args.command)?;
    ctx.out.flush()?;
    Ok(code)
}

/// Run a subcommand against an explicit context (tests, embedding).
pub fn dispatch(ctx: &mut Ctx<'_>, command: FleetCommand) -> Result<i32, BoxError> {
    match command {
        FleetCommand::Snap { session, outpath } => {
            let outcome = snap::snap(ctx, &session, outpath.as_deref());
            ctx.out.write_all(&outcome.passthrough)?;
            match outcome.result {
                Ok(path) => {
                    writeln!(ctx.out, "{}", path.display())?;
                    Ok(0)
                }
                Err(msg) => {
                    writeln!(ctx.err, "{msg}")?;
                    Ok(1)
                }
            }
        }
        FleetCommand::Send(a) => {
            let (outcome, mode) = match (&a.text, &a.keys) {
                (Some(text), _) => (send::send_text(ctx, &a.session, text), "text"),
                (None, Some(keys)) => (send::send_keys(ctx, &a.session, keys), "keys"),
                (None, None) => {
                    writeln!(
                        ctx.err,
                        "usage: admiralty fleet send SESSION --text 'message'"
                    )?;
                    return Ok(2);
                }
            };
            ctx.out.write_all(&outcome.stdout)?;
            ctx.err.write_all(&outcome.stderr)?;
            if outcome.code != 0 {
                return Ok(outcome.code);
            }
            writeln!(ctx.out, "sent to {} ({mode})", a.session)?;
            Ok(0)
        }
        FleetCommand::Classify { snap_file, session } => {
            match classify::classify_file(&snap_file, &session) {
                Ok(c) => {
                    writeln!(ctx.out, "{}", c.to_pretty_json()?)?;
                    Ok(0)
                }
                Err(json) => {
                    writeln!(ctx.out, "{json}")?;
                    Ok(1)
                }
            }
        }
        FleetCommand::Status { sessions } => {
            let sessions = if sessions.is_empty() {
                ctx.cfg.monitored_sessions.clone()
            } else {
                sessions
            };
            Ok(status::status(ctx, &sessions)?)
        }
        FleetCommand::Kick { args } => Ok(kick::kick(ctx, &args)?),
        FleetCommand::Peek {
            all,
            sessions,
            session,
            lines,
        } => {
            if all {
                let list = if sessions.is_empty() {
                    ctx.cfg.peek_all_sessions.clone()
                } else {
                    sessions
                };
                Ok(peek::peek_all(ctx, &list)?)
            } else {
                let session = session.unwrap_or_default();
                Ok(peek::peek(ctx, &session, lines.unwrap_or(25))?)
            }
        }
        FleetCommand::Paste { session, file } => Ok(peek::paste(ctx, &session, &file)?),
        FleetCommand::Deliver { plan } => match deliver::DeliverPlan::load(&plan) {
            Ok(p) => Ok(deliver::deliver(ctx, &p)?),
            Err(e) => {
                writeln!(ctx.err, "error: {e}")?;
                Ok(2)
            }
        },
        FleetCommand::ProbeModels { outdir } => {
            Ok(probe_models::probe_models(ctx, outdir.as_deref())?)
        }
        FleetCommand::CodexLuna { args } => Ok(codex_luna::run(
            &args,
            &ctx.cfg.codex_model.clone(),
            &ctx.cfg.codex_reasoning_effort.clone(),
            ctx.err,
        )),
        FleetCommand::Resources { quiet } => Ok(resources::resources(ctx, quiet)?),
        FleetCommand::Sweep => Ok(sweep::sweep(ctx)?),
    }
}

#[cfg(test)]
mod cli_tests;
