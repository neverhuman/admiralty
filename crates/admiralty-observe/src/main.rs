use admiralty_observe::{Result, config::ObserveConfig, io};
use admiralty_telemetry::Level;
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
    /// Optional compatibility cycle format; omit to use structured ALOG telemetry.
    #[arg(long)]
    log_format: Option<admiralty_core::agent_log::Format>,
    #[arg(long, default_value_t = 600)]
    log_rollup_seconds: i64,
    /// Print the latest host resource table (load, memory, disks with growth
    /// and time-to-full, GPUs, pressure, warnings) from status.json and exit.
    /// Reads only; never probes hosts or takes the collector lock.
    #[arg(long, conflicts_with_all = ["once", "validate"])]
    resources: bool,
    /// With --resources, print the global_resources JSON instead of a table.
    #[arg(long, requires = "resources")]
    json: bool,
}
// Fleet hosts can expose hundreds of CPUs; observation does not need one thread per CPU.
#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    admiralty_telemetry::init_from_env("admiralty-observe");
    admiralty_telemetry::spawn_flusher(Duration::from_secs(30));
    let outcome = execute().await;
    if let Err(error) = &outcome {
        // Human-readable line kept for existing operators; structured event for agents.
        eprintln!("Native observer failed: {}", io::clean(&error.to_string()));
        admiralty_telemetry::event(Level::Error, "obs", "observer.failed")
            .field("detail", io::clean(&error.to_string()))
            .emit();
    }
    admiralty_telemetry::global().flush();
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
async fn execute() -> Result<()> {
    let cli = Cli::parse();
    let config = ObserveConfig::load(&cli.config)?;
    if cli.resources {
        let status = io::read_json(&config.output_dir.join("status.json"), 32 * 1024 * 1024)
            .map_err(|_| "no readable status.json yet; start admiralty-observe for one cycle")?;
        let global = &status["global_resources"];
        if cli.json {
            println!("{}", serde_json::to_string_pretty(global)?);
        } else {
            print!(
                "{}",
                admiralty_observe::resources::table(
                    global,
                    status["generated_utc"].as_str().unwrap_or("unknown")
                )
            );
        }
        return Ok(());
    }
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
    let mut stop = StopSignal::install();
    let mut rollup = admiralty_core::agent_log::Rollup::new(
        cli.log_rollup_seconds,
        admiralty_observe::logging::VOLATILE,
    );
    loop {
        let started = Instant::now();
        // Successful cycles report through `obs.cycle` and change events.
        let outcome = admiralty_observe::cycle(&config).await;
        if let Some(format) = cli.log_format {
            let event = admiralty_observe::logging::cycle_event(&outcome, started.elapsed());
            for line in rollup.admit(event, chrono::Utc::now().timestamp()) {
                eprintln!("{}", line.render(format));
            }
        }
        match outcome {
            Ok(_) => {}
            Err(e) => {
                admiralty_telemetry::event(Level::Error, "obs.cycle", "cycle.failed")
                    .field("detail", io::clean(&e.to_string()))
                    .volatile("ms", started.elapsed().as_millis() as u64)
                    .emit();
                if cli.once {
                    return Err(e);
                }
            }
        }
        if cli.once {
            break;
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(config.interval_seconds).saturating_sub(started.elapsed()))=>{},_=stop.requested()=>break}
    }
    if let (Some(format), Some(line)) = (cli.log_format, rollup.flush()) {
        eprintln!("{}", line.render(format));
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
