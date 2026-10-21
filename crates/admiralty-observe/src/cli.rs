//! `admiralty-observe` command line, shared by the standalone binary and the
//! single-binary `admiralty observe` subcommand.
use crate::{Result, config::ObserveConfig, io};
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(clap::Args, Debug, Clone)]
pub struct ObserveArgs {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub once: bool,
    #[arg(long)]
    pub validate: bool,
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Validate configuration, take the output-directory lock and observe until
/// stopped (or once with `--once`). Never launches agents.
pub async fn run(cli: ObserveArgs) -> Result<()> {
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
    crate::config::validate_output(&config.output_dir)?;
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
        match crate::cycle(&config).await {
            Ok(snapshot) => println!(
                "{} Rust observation published: {} projects",
                snapshot["generated_utc"].as_str().unwrap_or("unknown"),
                config.projects.len()
            ),
            Err(e) => {
                eprintln!("Native observation failed: {}", io::clean(&e.to_string()));
                if cli.once {
                    return Err(e);
                }
            }
        }
        if cli.once {
            break;
        }
        // SIGINT or SIGTERM (systemd stop) ends the loop between cycles, so an
        // in-flight cycle always finishes its atomic publication first.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(config.interval_seconds).saturating_sub(started.elapsed())) => {},
            _ = shutdown_signal() => break,
        }
    }
    Ok(())
}
