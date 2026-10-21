use admiralty_observe::{Result, config::ObserveConfig, io};
use clap::{Parser, ValueEnum};
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
    /// JSON lines are compact allowlisted records; text retains the original console format.
    #[arg(long, value_enum, default_value = "json")]
    log_format: LogFormat,
}
#[derive(Clone, Copy, ValueEnum)]
enum LogFormat {
    Json,
    Text,
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
    let mut previous_health_digest: Option<String> = None;
    loop {
        let started = Instant::now();
        match admiralty_observe::cycle(&config).await {
            Ok(snapshot) => match cli.log_format {
                LogFormat::Text => println!(
                    "{} Rust observation published: {} projects",
                    snapshot["generated_utc"].as_str().unwrap_or("unknown"),
                    config.projects.len()
                ),
                LogFormat::Json => {
                    let record = admiralty_observe::telemetry::published(
                        &snapshot,
                        previous_health_digest.as_deref(),
                    );
                    previous_health_digest = record["health_digest"].as_str().map(str::to_owned);
                    println!("{record}");
                }
            },
            Err(e) => {
                match cli.log_format {
                    LogFormat::Text => {
                        eprintln!("Native observation failed: {}", io::clean(&e.to_string()))
                    }
                    LogFormat::Json => eprintln!(
                        "{}",
                        serde_json::json!({"schema_version":1,
                        "event_name":"observer.cycle_failed","severity_text":"ERROR",
                        "observed_utc":admiralty_observe::now_utc(),"retrying":!cli.once,
                        "reason_code":"observation_or_publication_failed","evidence_file":"status.json"})
                    ),
                }
                if cli.once {
                    return Err(e);
                }
            }
        }
        if cli.once {
            break;
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(config.interval_seconds).saturating_sub(started.elapsed()))=>{},_=tokio::signal::ctrl_c()=>break}
    }
    Ok(())
}
mod config {
    pub use admiralty_observe::config::validate_output;
}
