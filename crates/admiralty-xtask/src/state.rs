//! Private preview state for `cargo xtask live`: the owner-only state
//! directory and its `dashboard-token` (formerly the inline Python heredoc in
//! `scripts/dev/live.sh`). The token value is never printed or returned.

use rustix::{
    fs::{Mode, OFlags, fstat, fsync, open, openat},
    io::Errno,
    process::getuid,
};
use std::{
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt},
    path::{Component, Path, PathBuf},
};

/// File name of the bearer credential inside the state directory.
pub const TOKEN_NAME: &str = "dashboard-token";
/// Random bytes per generated token (`secrets.token_urlsafe(48)`).
pub const TOKEN_BYTES: usize = 48;
/// Accepted credential length after trimming (mirrors admiraltyd).
pub const TOKEN_MIN_LEN: usize = 32;
pub const TOKEN_MAX_LEN: usize = 4096;
/// Byte bound for reading an existing token (mirrors admiraltyd).
const TOKEN_READ_LIMIT: u64 = 4098;

#[derive(Debug)]
pub enum StateError {
    /// Not absolute, or the final component is a symlink.
    NotAbsoluteOrSymlink,
    /// Exists but is not a directory.
    NotDirectory,
    /// Not owned by the caller or has group/other permission bits.
    NotPrivate,
    /// The existing token is not a private, owned, single-link regular file.
    TokenNotPrivate,
    /// The existing token's content is not a valid bearer credential.
    TokenInvalid,
    /// An I/O failure, with context (never contains the token value).
    Io(&'static str, io::Error),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsoluteOrSymlink => {
                f.write_str("Preview state directory must be absolute and not a symlink")
            }
            Self::NotDirectory => f.write_str("Preview state directory must be a directory"),
            Self::NotPrivate => {
                f.write_str("Existing preview state directory must be owned and mode 0700")
            }
            Self::TokenNotPrivate => {
                f.write_str("Existing dashboard token must be a private owned regular file")
            }
            Self::TokenInvalid => f.write_str(
                "Existing dashboard token must hold 32-4096 printable ASCII characters (value not displayed)",
            ),
            Self::Io(context, error) => write!(f, "{context}: {error}"),
        }
    }
}

impl std::error::Error for StateError {}

fn io_err(context: &'static str) -> impl FnOnce(io::Error) -> StateError {
    move |error| StateError::Io(context, error)
}

fn errno_err(context: &'static str) -> impl FnOnce(Errno) -> StateError {
    move |errno| StateError::Io(context, io::Error::from(errno))
}

/// `${ADMIRALTY_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/admiralty-preview}`
/// followed by Python's `Path(...).expanduser()` and normalization.
pub fn state_dir_from(
    state_dir: Option<OsString>,
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    let home = home.filter(|value| !value.is_empty());
    let raw = match state_dir.filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => match xdg_state_home.filter(|value| !value.is_empty()) {
            Some(value) => PathBuf::from(value).join("admiralty-preview"),
            None => PathBuf::from(home.clone().ok_or("HOME is not set")?)
                .join(".local/state/admiralty-preview"),
        },
    };
    Ok(normalize(&expand_user(&raw, home.as_deref())))
}

/// Expand a leading `~` or `~/` with `HOME` (other forms are left as-is).
pub fn expand_user(path: &Path, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let mut components = path.components();
    match (components.next(), home) {
        (Some(Component::Normal(first)), Some(home)) if first == "~" => {
            Path::new(home).join(components.as_path())
        }
        _ => path.to_path_buf(),
    }
}

/// `pathlib` normalization: drop `.` components, repeated and trailing slashes.
pub fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Create (0700) or validate the preview state directory. Missing parents
/// are created with the default mode, as Python's `mkdir(parents=True)` did.
pub fn ensure_state_dir(path: &Path) -> Result<(), StateError> {
    let path = normalize(path);
    let is_symlink = fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink());
    if !path.is_absolute() || is_symlink {
        return Err(StateError::NotAbsoluteOrSymlink);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err("cannot create preview state parent"))?;
    }
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(StateError::Io(
                "cannot create preview state directory",
                error,
            ));
        }
    }
    let meta =
        fs::symlink_metadata(&path).map_err(io_err("cannot inspect preview state directory"))?;
    if meta.file_type().is_symlink() {
        return Err(StateError::NotAbsoluteOrSymlink);
    }
    if !meta.is_dir() {
        return Err(StateError::NotDirectory);
    }
    if meta.uid() != getuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(StateError::NotPrivate);
    }
    Ok(())
}

/// URL-safe base64 without padding (`base64.urlsafe_b64encode(...).rstrip(b'=')`).
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        let symbols = chunk.len() + 1;
        for index in 0..symbols {
            let shift = 18 - 6 * index;
            let value = (triple >> shift) & 0x3f;
            text.push(char::from(ALPHABET[value as usize]));
        }
    }
    text
}

/// Read `TOKEN_BYTES` fresh random bytes from `/dev/urandom` (refusing
/// anything that is not a character device) and encode them.
pub fn generate_token() -> Result<String, StateError> {
    let mut urandom = File::open("/dev/urandom").map_err(io_err("cannot open /dev/urandom"))?;
    let meta = urandom
        .metadata()
        .map_err(io_err("cannot inspect /dev/urandom"))?;
    if !meta.file_type().is_char_device() {
        return Err(StateError::Io(
            "cannot use /dev/urandom",
            io::Error::other("not a character device"),
        ));
    }
    let mut bytes = [0u8; TOKEN_BYTES];
    urandom
        .read_exact(&mut bytes)
        .map_err(io_err("cannot read /dev/urandom"))?;
    let token = base64url(&bytes);
    bytes.fill(0);
    Ok(token)
}

/// Whether trimmed token text is a credential admiraltyd will accept.
pub fn valid_token_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .map(str::trim)
        .is_some_and(|value| {
            (TOKEN_MIN_LEN..=TOKEN_MAX_LEN).contains(&value.len())
                && value.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
        })
}

/// Whether the token was freshly created or an existing one was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenOutcome {
    Created,
    Existing,
}

/// Create `<dir>/dashboard-token` with `O_EXCL|O_NOFOLLOW` mode 0600 holding
/// a fresh token (fsyncing file and directory), or validate the existing one
/// without changing it. Returns the token path, never the value.
pub fn ensure_token(dir: &Path) -> Result<(PathBuf, TokenOutcome), StateError> {
    let dir = normalize(dir);
    let token_path = dir.join(TOKEN_NAME);
    let dir_fd = open(
        &dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_err("cannot open preview state directory"))?;
    let created = openat(
        &dir_fd,
        TOKEN_NAME,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    );
    match created {
        Ok(fd) => {
            let mut file = File::from(fd);
            let written = generate_token().and_then(|token| {
                let mut token = token.into_bytes();
                let result = file
                    .write_all(&token)
                    .and_then(|()| file.flush())
                    .and_then(|()| file.sync_all())
                    .map_err(io_err("cannot write dashboard token"));
                token.fill(0);
                result
            });
            if let Err(error) = written {
                drop(file);
                let _ = fs::remove_file(&token_path);
                return Err(error);
            }
            fsync(&dir_fd).map_err(errno_err("cannot sync preview state directory"))?;
            Ok((token_path, TokenOutcome::Created))
        }
        Err(Errno::EXIST) => {
            validate_existing_token(&dir_fd)?;
            Ok((token_path, TokenOutcome::Existing))
        }
        Err(errno) => Err(StateError::Io(
            "cannot create dashboard token",
            io::Error::from(errno),
        )),
    }
}

fn validate_existing_token(dir_fd: &rustix::fd::OwnedFd) -> Result<(), StateError> {
    let fd = match openat(
        dir_fd,
        TOKEN_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::LOOP | Errno::NOTDIR | Errno::ACCESS | Errno::NXIO) => {
            return Err(StateError::TokenNotPrivate);
        }
        Err(errno) => {
            return Err(StateError::Io(
                "cannot open dashboard token",
                io::Error::from(errno),
            ));
        }
    };
    let stat = fstat(&fd).map_err(errno_err("cannot inspect dashboard token"))?;
    let regular =
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::RegularFile;
    if !regular
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o077 != 0
        || stat.st_nlink != 1
    {
        return Err(StateError::TokenNotPrivate);
    }
    let mut bytes = Vec::new();
    File::from(fd)
        .take(TOKEN_READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err("cannot read dashboard token"))?;
    let valid = bytes.len() as u64 <= TOKEN_READ_LIMIT && valid_token_text(&bytes);
    bytes.fill(0);
    if valid {
        Ok(())
    } else {
        Err(StateError::TokenInvalid)
    }
}

/// Prepare the state directory and token, printing only the token path.
pub fn prepare(state_dir: &Path, out: &mut dyn Write) -> Result<PathBuf, StateError> {
    ensure_state_dir(state_dir)?;
    let (token_path, _) = ensure_token(state_dir)?;
    writeln!(
        out,
        "Dashboard access token is in {} (value not displayed)",
        token_path.display()
    )
    .map_err(io_err("cannot write output"))?;
    Ok(token_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("chmod");
        dir
    }

    fn write_token(path: &Path, value: &[u8], mode: u32) {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .expect("create token");
        file.write_all(value).expect("write token");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod token");
    }

    #[test]
    fn base64url_matches_python_token_urlsafe_encoding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(&[0xfb, 0xff, 0xfe]), "-__-");
        assert_eq!(base64url(&[0u8; 48]).len(), 64);
        assert_eq!(base64url(&[0xffu8; 48]), "_".repeat(64));
    }

    #[test]
    fn generated_tokens_are_fresh_64_char_urlsafe_and_admissible() {
        let first = generate_token().expect("token");
        let second = generate_token().expect("token");
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
        assert!(
            first
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert!(valid_token_text(first.as_bytes()));
    }

    #[test]
    fn token_text_validation_mirrors_admiraltyd() {
        assert!(valid_token_text(&[b'a'; 32]));
        assert!(valid_token_text(format!("{}\n", "a".repeat(40)).as_bytes()));
        assert!(valid_token_text(&[b'~'; 4096]));
        assert!(!valid_token_text(&[b'a'; 31]));
        assert!(!valid_token_text(&[b'a'; 4097]));
        assert!(!valid_token_text(b"0123456789 0123456789 0123456789 x"));
        assert!(!valid_token_text(&[0xff; 40]));
    }

    #[test]
    fn state_dir_resolution_follows_the_shell_defaults() {
        let home = Some(OsString::from("/home/u"));
        assert_eq!(
            state_dir_from(None, None, home.clone()),
            Ok(PathBuf::from("/home/u/.local/state/admiralty-preview"))
        );
        assert_eq!(
            state_dir_from(None, Some(OsString::new()), home.clone()),
            Ok(PathBuf::from("/home/u/.local/state/admiralty-preview"))
        );
        assert_eq!(
            state_dir_from(None, Some(OsString::from("/xdg/")), home.clone()),
            Ok(PathBuf::from("/xdg/admiralty-preview"))
        );
        assert_eq!(
            state_dir_from(Some(OsString::from("/custom/state/")), None, home.clone()),
            Ok(PathBuf::from("/custom/state"))
        );
        assert_eq!(
            state_dir_from(Some(OsString::from("~/prev")), None, home.clone()),
            Ok(PathBuf::from("/home/u/prev"))
        );
        assert_eq!(
            state_dir_from(Some(OsString::from("~")), None, home.clone()),
            Ok(PathBuf::from("/home/u"))
        );
        assert_eq!(
            state_dir_from(Some(OsString::from("~other/x")), None, home.clone()),
            Ok(PathBuf::from("~other/x"))
        );
        assert_eq!(
            state_dir_from(Some(OsString::from("rel")), None, home),
            Ok(PathBuf::from("rel"))
        );
        assert!(state_dir_from(None, None, None).is_err());
    }

    #[test]
    fn creates_private_state_dir_and_token_once() {
        let base = private_dir();
        let state = base.path().join("a/b/state");
        let mut out = Vec::new();
        let token_path = prepare(&state, &mut out).expect("prepare");
        assert_eq!(token_path, state.join(TOKEN_NAME));
        let dir_meta = fs::symlink_metadata(&state).expect("stat state");
        assert!(dir_meta.is_dir());
        assert_eq!(dir_meta.mode() & 0o777, 0o700);
        let token_meta = fs::symlink_metadata(&token_path).expect("stat token");
        assert!(token_meta.is_file());
        assert_eq!(token_meta.mode() & 0o777, 0o600);
        assert_eq!(token_meta.nlink(), 1);
        let value = fs::read_to_string(&token_path).expect("read token");
        assert_eq!(value.len(), 64);
        assert!(!value.ends_with('\n'));
        let printed = String::from_utf8(out).expect("utf8");
        assert_eq!(
            printed,
            format!(
                "Dashboard access token is in {} (value not displayed)\n",
                token_path.display()
            )
        );
        assert!(
            !printed.contains(&value),
            "token value must never be printed"
        );

        // A second run keeps the existing valid token unchanged.
        let mut again = Vec::new();
        prepare(&state, &mut again).expect("prepare again");
        assert_eq!(fs::read_to_string(&token_path).expect("reread"), value);
        assert_eq!(again, printed.as_bytes());
        assert_eq!(
            ensure_token(&state).expect("existing").1,
            TokenOutcome::Existing
        );
    }

    #[test]
    fn keeps_operator_supplied_tokens_including_owner_read_only() {
        let base = private_dir();
        let state = base.path().join("state");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&state)
            .expect("mkdir");
        let custom = "operator-chosen-credential-0123456789-abcdef";
        write_token(&state.join(TOKEN_NAME), custom.as_bytes(), 0o400);
        let mut out = Vec::new();
        prepare(&state, &mut out).expect("existing 0400 token is private");
        assert!(!String::from_utf8(out).expect("utf8").contains(custom));
        assert_eq!(
            fs::read_to_string(state.join(TOKEN_NAME)).expect("read"),
            custom
        );
    }

    #[test]
    fn refuses_relative_and_symlinked_state_dirs() {
        assert!(matches!(
            ensure_state_dir(Path::new("relative/state")),
            Err(StateError::NotAbsoluteOrSymlink)
        ));
        let base = private_dir();
        let real = base.path().join("real");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&real)
            .expect("mkdir");
        let link = base.path().join("link");
        symlink(&real, &link).expect("symlink");
        let error = ensure_state_dir(&link).expect_err("symlink refused");
        assert_eq!(
            error.to_string(),
            "Preview state directory must be absolute and not a symlink"
        );
        // A trailing slash must not let the symlink through (pathlib strips it).
        let with_slash = PathBuf::from(format!("{}/", link.display()));
        assert!(matches!(
            ensure_state_dir(&with_slash),
            Err(StateError::NotAbsoluteOrSymlink)
        ));
        let mut out = Vec::new();
        assert!(prepare(&link, &mut out).is_err());
        assert!(out.is_empty());
        assert!(!real.join(TOKEN_NAME).exists());
    }

    #[test]
    fn refuses_group_or_world_accessible_state_dirs_without_relaxing() {
        let base = private_dir();
        let state = base.path().join("state");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&state)
            .expect("mkdir");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o750)).expect("chmod");
        let error = ensure_state_dir(&state).expect_err("0750 refused");
        assert_eq!(
            error.to_string(),
            "Existing preview state directory must be owned and mode 0700"
        );
        assert_eq!(
            fs::metadata(&state).expect("stat").mode() & 0o777,
            0o750,
            "an existing directory is never relaxed or tightened"
        );
        let file = base.path().join("file");
        fs::write(&file, "x").expect("write");
        assert!(matches!(
            ensure_state_dir(&file),
            Err(StateError::NotDirectory)
        ));
    }

    #[test]
    fn refuses_unsafe_existing_tokens_without_revealing_them() {
        let secret = "s3cr3t-token-value-that-must-never-be-printed-0123";
        type Setup = Box<dyn Fn(&Path)>;
        let cases: Vec<(&str, Setup)> = vec![
            (
                "group-readable",
                Box::new(|dir: &Path| write_token(&dir.join(TOKEN_NAME), secret.as_bytes(), 0o640)),
            ),
            (
                "symlink",
                Box::new(|dir: &Path| {
                    write_token(&dir.join("elsewhere"), secret.as_bytes(), 0o600);
                    symlink(dir.join("elsewhere"), dir.join(TOKEN_NAME)).expect("symlink");
                }),
            ),
            (
                "dangling symlink",
                Box::new(|dir: &Path| {
                    symlink(dir.join("missing"), dir.join(TOKEN_NAME)).expect("symlink");
                }),
            ),
            (
                "hard link",
                Box::new(|dir: &Path| {
                    write_token(&dir.join("other"), secret.as_bytes(), 0o600);
                    fs::hard_link(dir.join("other"), dir.join(TOKEN_NAME)).expect("link");
                }),
            ),
            (
                "directory",
                Box::new(|dir: &Path| {
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(dir.join(TOKEN_NAME))
                        .expect("mkdir");
                }),
            ),
        ];
        for (label, setup) in cases {
            let base = private_dir();
            let state = base.path().join("state");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&state)
                .expect("mkdir");
            setup(&state);
            let mut out = Vec::new();
            let error = prepare(&state, &mut out).expect_err(label);
            assert_eq!(
                error.to_string(),
                "Existing dashboard token must be a private owned regular file",
                "{label}"
            );
            assert!(out.is_empty(), "{label}");
            assert!(!format!("{error:?}").contains(secret), "{label}");
        }
        assert!(
            !Path::new("/nonexistent-xtask-check").exists(),
            "sanity: fixtures stay inside tempdirs"
        );
    }

    #[test]
    fn refuses_existing_tokens_with_invalid_content_without_revealing_them() {
        for (label, value) in [
            ("short", b"too-short-token".to_vec()),
            ("empty", Vec::new()),
            (
                "spaces",
                b"0123456789 0123456789 0123456789 0123456789".to_vec(),
            ),
            ("oversize", vec![b'x'; 4099]),
            ("binary", vec![0xff; 64]),
        ] {
            let base = private_dir();
            let state = base.path().join("state");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&state)
                .expect("mkdir");
            write_token(&state.join(TOKEN_NAME), &value, 0o600);
            let error = ensure_token(&state).expect_err(label);
            assert!(matches!(error, StateError::TokenInvalid), "{label}");
            let shown = error.to_string();
            assert!(shown.contains("value not displayed"), "{label}");
            if !value.is_empty() {
                assert!(
                    !shown.contains(&String::from_utf8_lossy(&value).into_owned()),
                    "{label}"
                );
            }
            assert_eq!(fs::read(state.join(TOKEN_NAME)).expect("read"), value);
        }
    }
}
