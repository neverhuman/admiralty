use admiralty_core::agent_log::{Level, Rollup};
use admiralty_observe::{Result, config::ObserveConfig, io};
use clap::Parser;
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Parser)]
#[command(about = "Rust fleet observations and real dashboard history; never launches agents")]
struct Cli {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    once: bool,
    #[arg(long)]
    validate: bool,
    /// Cycle log format: compact `logfmt` (default) or NDJSON `json`.
    #[arg(long, default_value = "logfmt")]
    log_format: admiralty_core::agent_log::Format,
    /// Collapse identical consecutive cycle outcomes for this many seconds;
    /// repeats are counted (`repeat=N`), never dropped. 0 writes every cycle.
    #[arg(long, default_value_t = 600)]
    log_rollup_seconds: i64,
}
// Fleet hosts can expose hundreds of CPUs; observation does not need one thread per CPU.
#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match execute().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Native observer failed: {}", io::clean(&error.to_string()));
            std::process::ExitCode::FAILURE
        }
    }
}
async fn execute() -> Result<()> {
    let cli = Cli::parse();
    let config = ObserveConfig::load(&cli.config)?;
    if cli.validate {
        println!(
            "Native observer configuration valid: {} projects, {} nodes",
            config.projects.len(),
            config.nodes.len()
        );
        return Ok(());
    }
    if !config.output_dir.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&config.output_dir)?;
    }
    config::validate_output(&config.output_dir)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let lock = options.open(config.output_dir.join("collector.lock"))?;
    if !lock.metadata()?.is_file() {
        return Err("collector lock must be a regular file".into());
    }
    lock.try_lock()
        .map_err(|_| "another native observer owns this output directory")?;
    let mut rollup = Rollup::new(cli.log_rollup_seconds, admiralty_observe::logging::VOLATILE);
    let mut stop = StopSignal::install();
    loop {
        let started = Instant::now();
        let outcome = admiralty_observe::cycle(&config).await;
        let event = admiralty_observe::logging::cycle_event(&outcome, started.elapsed());
        let failed = event.level == Level::Error;
        for line in rollup.admit(event, chrono::Utc::now().timestamp()) {
            let text = line.render(cli.log_format);
            if failed {
                eprintln!("{text}")
            } else {
                println!("{text}")
            }
        }
        if let Err(e) = outcome
            && cli.once
        {
            return Err(e);
        }
        if cli.once {
            break;
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(config.interval_seconds).saturating_sub(started.elapsed()))=>{},_=stop.requested()=>break}
    }
    if let Some(line) = rollup.flush() {
        println!("{}", line.render(cli.log_format));
    }
    Ok(())
}
/// Ctrl-C or, on Unix, SIGTERM (what systemd and most supervisors send).
/// The handler is installed once before the first cycle, so a stop request
/// that arrives mid-cycle lets that cycle finish and publish atomically.
struct StopSignal {
    #[cfg(unix)]
    term: Option<tokio::signal::unix::Signal>,
}

impl StopSignal {
    fn install() -> Self {
        StopSignal {
            #[cfg(unix)]
            term: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok(),
        }
    }

    async fn requested(&mut self) {
        #[cfg(unix)]
        if let Some(term) = self.term.as_mut() {
            tokio::select! {_ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {}}
            return;
        }
        let _ = tokio::signal::ctrl_c().await;
    }
}
mod config {
    pub use admiralty_observe::config::validate_output;
}
