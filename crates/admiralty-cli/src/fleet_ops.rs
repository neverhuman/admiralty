//! Operator conveniences that previously needed Python or a full snapshot.
//!
//! * `admiralty brief` turns an observer `status.json` into a token-budgeted
//!   brief for agents (see `admiralty_core::fleet_brief`). Read-only.
use admiralty_core::fleet_brief::{BriefOptions, MAX_BUDGET, MIN_BUDGET, brief};
use clap::{Args, ValueEnum};
use std::error::Error;
use std::path::{Path, PathBuf};

type OpsResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Same bound as the observer's own status writer.
pub const MAX_STATUS_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum BriefFormat {
    Text,
    Json,
}

#[derive(Args, Debug)]
pub struct BriefArgs {
    /// Observer status.json (regular file, at most 32 MiB).
    #[arg(long)]
    pub status: PathBuf,
    /// Maximum brief size in UTF-8 bytes (roughly four bytes per token).
    #[arg(long, default_value_t = 2_000, value_parser = clap::value_parser!(u32).range(MIN_BUDGET as i64..=MAX_BUDGET as i64))]
    pub budget: u32,
    /// Only project-scoped lines for this project id.
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, value_enum, default_value_t = BriefFormat::Text)]
    pub format: BriefFormat,
}

pub fn read_status(path: &Path) -> OpsResult<serde_json::Value> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err("status source must be a regular file, not a symlink or device".into());
    }
    if metadata.len() > MAX_STATUS_BYTES {
        return Err("status source exceeds the 32 MiB bound".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_STATUS_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STATUS_BYTES {
        return Err("status source grew past the 32 MiB bound while reading".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn run_brief(args: BriefArgs) -> OpsResult<String> {
    let snapshot = read_status(&args.status)?;
    let options = BriefOptions {
        budget_bytes: args.budget as usize,
        project: args.project,
        now_utc: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    };
    let result = brief(&snapshot, &options);
    Ok(match args.format {
        BriefFormat::Text => result.text,
        BriefFormat::Json => serde_json::to_string(&result)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brief_reads_a_bounded_regular_status_file() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("status.json");
        std::fs::write(
            &path,
            r#"{"generated_utc":"2026-10-21T00:00:00Z","host":"n","order":["p"],
                "projects":{"p":{"done":1,"total":2,"counts":{"in_progress":1,"blocked":0}}}}"#,
        )
        .expect("write");
        let text = run_brief(BriefArgs {
            status: path.clone(),
            budget: 400,
            project: None,
            format: BriefFormat::Text,
        })
        .expect("brief");
        assert!(text.starts_with("# fleet gen=2026-10-21T00:00:00Z"));
        assert!(text.contains("proj p done=1/2 ip=1 blk=0"));
        let json = run_brief(BriefArgs {
            status: path,
            budget: 400,
            project: None,
            format: BriefFormat::Json,
        })
        .expect("json");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(parsed["omitted_lines"], 0);
        #[cfg(unix)]
        {
            let link = dir.path().join("link.json");
            std::os::unix::fs::symlink(dir.path().join("status.json"), &link).expect("link");
            assert!(read_status(&link).is_err());
        }
    }
}
