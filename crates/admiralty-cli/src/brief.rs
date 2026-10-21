use crate::{CliResult, bounded_absolute_path};
use clap::Args;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Args)]
pub struct BriefArgs {
    #[arg(long, value_parser = bounded_absolute_path)]
    status: PathBuf,
    /// Exact UTF-8 JSON byte budget; omitted project rows are declared.
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u32).range(2048..=16384))]
    max_bytes: u32,
    /// Fixed RFC3339 time for reproducible offline analysis; defaults to now.
    #[arg(long)]
    now: Option<chrono::DateTime<chrono::Utc>>,
}

pub fn run(args: BriefArgs) -> CliResult<()> {
    let limit = admiralty_server::fleet_brief::MAX_SOURCE_BYTES as u64;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(args.status)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("brief input must be a regular file of at most 32 MiB".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    let output = admiralty_server::fleet_brief::compile(
        &bytes,
        args.now.unwrap_or_else(chrono::Utc::now),
        args.max_bytes as usize,
    )?;
    // No pretty-printing or extra newline outside the declared exact budget.
    std::io::stdout().lock().write_all(&output)?;
    Ok(())
}
