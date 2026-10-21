//! Serialized activation, unit restart and readiness, retaining one rollback.
use crate::CliResult;
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(clap::Args, Debug, Clone)]
pub struct RolloutArgs {
    #[arg(long)]
    releases_root: PathBuf,
    #[arg(long)]
    version: String,
    #[arg(long, default_value = "127.0.0.1:18891")]
    listen: SocketAddr,
    #[arg(long, default_value = "admiraltyd admiralty-observe")]
    units: String,
    #[arg(long)]
    user: bool,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(0..=3600))]
    timeout_seconds: u64,
    #[arg(long)]
    dry_run: bool,
}

fn bounded(command: &mut Command, timeout: Duration) -> CliResult<i32> {
    let mut child = command.stdin(Stdio::null()).spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code().unwrap_or(1));
        }
        if Instant::now() >= deadline {
            // This exact Child is owned by this invocation; no name-based signal.
            if let Err(error) = child.kill()
                && error.kind() != std::io::ErrorKind::InvalidInput
            {
                return Err(error.into());
            }
            child.wait()?;
            return Ok(1);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn restart(args: &RolloutArgs, units: &[&str]) -> CliResult<i32> {
    let mut command = Command::new("systemctl");
    if args.user {
        command.arg("--user");
    }
    bounded(command.arg("restart").args(units), Duration::from_secs(30))
}

fn ready(args: &RolloutArgs, current: &Path) -> CliResult<i32> {
    bounded(
        Command::new(current).args([
            "ready",
            "--listen",
            &args.listen.to_string(),
            "--timeout-seconds",
            &args.timeout_seconds.to_string(),
        ]),
        // The child retains the single monotonic readiness deadline. This
        // outer bound also accounts for process startup and orderly exit.
        Duration::from_secs(args.timeout_seconds + 5),
    )
}

pub fn run(args: RolloutArgs) -> CliResult<i32> {
    let units: Vec<_> = args.units.split_whitespace().collect();
    let version_is_component =
        !matches!(args.version.as_str(), "" | "." | "..") && !args.version.contains('/');
    if !args.releases_root.is_absolute()
        || !version_is_component
        || units.is_empty()
        || units.len() > 16
        || units.iter().any(|unit| {
            unit.len() > 255
                || unit.starts_with('-')
                || !unit
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.@-".contains(&c))
        })
    {
        eprintln!("rollout: require an absolute releases root, one version and bounded unit names");
        return Ok(2);
    }
    let version = args.releases_root.join(&args.version);
    let candidate = version.join("bin/admiralty");
    let installed = fs::symlink_metadata(&version).is_ok_and(|m| m.is_dir())
        && fs::symlink_metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if !installed {
        eprintln!(
            "rollout: {} is not an installed release directory",
            version.display()
        );
        return Ok(2);
    }
    if args.dry_run {
        println!(
            "rollout (dry run): would activate {}, restart {} and require {}/readyz",
            args.version,
            units.join(" "),
            args.listen
        );
        return Ok(0);
    }
    let activated = bounded(
        Command::new(&candidate)
            .args(["release", "activate", "--releases-root"])
            .arg(&args.releases_root)
            .args(["--version", &args.version]),
        Duration::from_secs(30),
    )?;
    if activated != 0 {
        return Ok(activated);
    }
    let current = args.releases_root.join("current/bin/admiralty");
    if restart(&args, &units).unwrap_or(1) == 0 && ready(&args, &current).unwrap_or(1) == 0 {
        println!("rollout: {} is active and ready", args.version);
        return Ok(0);
    }
    eprintln!(
        "rollout: {} did not become ready within {}s; rolling back",
        args.version, args.timeout_seconds
    );
    let rollback = bounded(
        Command::new(&current)
            .args(["release", "rollback", "--releases-root"])
            .arg(&args.releases_root),
        Duration::from_secs(30),
    )?;
    if rollback == 0 {
        if restart(&args, &units).unwrap_or(1) != 0 || ready(&args, &current).unwrap_or(1) != 0 {
            eprintln!(
                "rollout: the restored version is not ready either; inspect journalctl and admiralty logs digest"
            );
        }
    } else {
        eprintln!(
            "rollout: rollback failed; retain the activation journal and inspect the actual current version"
        );
    }
    Ok(1)
}
