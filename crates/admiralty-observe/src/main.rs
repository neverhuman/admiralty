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
    /// Structured bounded metadata for agents/log processors; source prose is excluded.
    #[arg(long, value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,
}
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}
// Fleet hosts can expose hundreds of CPUs; observation does not need one thread per CPU.
#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match execute(&cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if cli.log_format == LogFormat::Json {
                eprintln!(
                    "{}",
                    admiralty_observe::logging::failure("observer.stopped")
                );
            } else {
                eprintln!("Native observer failed: {}", io::clean(&error.to_string()));
            }
            std::process::ExitCode::FAILURE
        }
    }
}
async fn execute(cli: &Cli) -> Result<()> {
    let config = ObserveConfig::load(&cli.config)?;
    if cli.validate {
        if cli.log_format == LogFormat::Json {
            println!(
                "{}",
                serde_json::json!({"schema_version":1,"event":"configuration.valid","project_count":config.projects.len(),"node_count":config.nodes.len()})
            );
        } else {
            println!(
                "Native observer configuration valid: {} projects, {} nodes",
                config.projects.len(),
                config.nodes.len()
            );
        }
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
    loop {
        let started = Instant::now();
        match admiralty_observe::cycle(&config).await {
            Ok(snapshot) if cli.log_format == LogFormat::Json => {
                let bytes = fs::metadata(config.output_dir.join("status.json"))
                    .ok()
                    .map(|m| m.len());
                println!(
                    "{}",
                    admiralty_observe::logging::published(&snapshot, bytes)
                );
            }
            Ok(snapshot) => println!(
                "{} Rust observation published: {} projects",
                snapshot["generated_utc"].as_str().unwrap_or("unknown"),
                config.projects.len()
            ),
            Err(e) => {
                if cli.log_format == LogFormat::Json {
                    eprintln!(
                        "{}",
                        admiralty_observe::logging::failure("observation.failed")
                    );
                } else {
                    eprintln!("Native observation failed: {}", io::clean(&e.to_string()));
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
