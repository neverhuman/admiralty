//! Operator conveniences that previously needed Python or a full snapshot.
//!
//! * `admiralty brief` turns an observer `status.json` into a token-budgeted
//!   brief for agents (see `admiralty_core::fleet_brief`). Read-only.
//! * `admiralty state init` creates the private live-console state directory
//!   and bearer token that `scripts/dev/live.sh` used to create with Python.
use admiralty_core::fleet_brief::{BriefOptions, MAX_BUDGET, MIN_BUDGET, brief};
use clap::{Args, Subcommand, ValueEnum};
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
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err("status source must be a regular file, not a symlink or device".into());
    }
    if metadata.len() > MAX_STATUS_BYTES {
        return Err("status source exceeds the 32 MiB bound".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_STATUS_BYTES + 1)
        .read_to_end(&mut bytes)?;
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

#[derive(Subcommand, Debug)]
pub enum StateCommand {
    /// Create (or verify) a private 0700 state directory with a 0600 bearer
    /// token file. Existing files are never rewritten or loosened; the token
    /// value is never printed.
    Init {
        /// Absolute directory path; must not be a symlink.
        #[arg(long)]
        dir: PathBuf,
    },
}

const TOKEN_FILE: &str = "dashboard-token";

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextets = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for sextet in sextets.iter().take(chunk.len() + 1) {
            out.push(char::from(ALPHABET[*sextet as usize]));
        }
    }
    out
}

/// 48 bytes from the operating system's CSPRNG (via UUIDv4's getrandom),
/// encoded as 64 URL-safe characters — the same shape as Python's
/// `secrets.token_urlsafe(48)` that this replaces.
pub fn new_token() -> String {
    let mut bytes = Vec::with_capacity(48);
    while bytes.len() < 48 {
        // Bytes 6 and 8 of a v4 UUID carry version/variant bits; use only the
        // 14 fully random bytes from each draw.
        let id = uuid::Uuid::new_v4();
        let raw = id.as_bytes();
        bytes.extend(
            raw.iter()
                .enumerate()
                .filter(|(i, _)| *i != 6 && *i != 8)
                .map(|(_, b)| *b),
        );
    }
    bytes.truncate(48);
    base64url(&bytes)
}

#[cfg(unix)]
pub fn run_state(command: StateCommand) -> OpsResult<String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    let StateCommand::Init { dir } = command;
    if !dir.is_absolute() {
        return Err("state directory must be an absolute path".into());
    }
    if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("state directory must not be a symlink".into());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let uid = rustix::process::getuid().as_raw();
    let info = std::fs::symlink_metadata(&dir)?;
    if !info.is_dir() || info.uid() != uid || info.permissions().mode() & 0o077 != 0 {
        return Err("existing state directory must be owned by this user with mode 0700".into());
    }
    let token = dir.join(TOKEN_FILE);
    let created = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&token)
    {
        Ok(mut file) => {
            file.write_all(new_token().as_bytes())?;
            file.sync_all()?;
            std::fs::File::open(&dir)?.sync_all()?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let info = std::fs::symlink_metadata(&token)?;
            if !info.is_file() || info.uid() != uid || info.permissions().mode() & 0o077 != 0 {
                return Err("existing dashboard token must be a private owned regular file".into());
            }
            false
        }
        Err(error) => return Err(error.into()),
    };
    Ok(format!(
        "Dashboard access token {} {} (value not displayed)",
        if created { "created in" } else { "is in" },
        token.display()
    ))
}

#[cfg(not(unix))]
pub fn run_state(_: StateCommand) -> OpsResult<String> {
    Err("private state initialization is qualified on Unix only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_python_token_urlsafe_48_shape_and_is_unique() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(a, b);
        assert_eq!(base64url(b"\xfb\xff"), "-_8");
        assert_eq!(base64url(b"Man"), "TWFu");
    }

    #[cfg(unix)]
    #[test]
    fn state_init_is_private_idempotent_and_refuses_loose_or_linked_state() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("root");
        let dir = root.path().join("nested/state");
        let first = run_state(StateCommand::Init { dir: dir.clone() }).expect("create");
        assert!(first.contains("created in"));
        let token = std::fs::read_to_string(dir.join(TOKEN_FILE)).expect("token");
        assert_eq!(token.len(), 64);
        assert!(!first.contains(&token), "value must never be printed");
        let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(TOKEN_FILE)), 0o600);
        // Re-running keeps the same token.
        let second = run_state(StateCommand::Init { dir: dir.clone() }).expect("verify");
        assert!(second.contains("is in"));
        assert_eq!(
            std::fs::read_to_string(dir.join(TOKEN_FILE)).expect("same"),
            token
        );
        // A loosened token is refused, never silently repaired.
        std::fs::set_permissions(dir.join(TOKEN_FILE), std::fs::Permissions::from_mode(0o644))
            .expect("loosen");
        assert!(run_state(StateCommand::Init { dir: dir.clone() }).is_err());
        // Relative and symlinked directories are refused.
        assert!(
            run_state(StateCommand::Init {
                dir: "relative".into()
            })
            .is_err()
        );
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).expect("link");
        assert!(run_state(StateCommand::Init { dir: link }).is_err());
        let loose = root.path().join("loose");
        std::fs::create_dir(&loose).expect("loose");
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(run_state(StateCommand::Init { dir: loose }).is_err());
    }

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
