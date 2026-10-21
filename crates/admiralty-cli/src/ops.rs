//! Small operator commands that replace shell/Python glue with Rust:
//! log digests for agents, private preview state for `pnpm live`, and a
//! dependency-free readiness probe for deployment scripts.
use std::{
    fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Largest log input digested in one call.
pub const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// Read a log file (or stdin for `-`) and render a token-budgeted digest.
pub fn digest_logs(
    file: Option<&Path>,
    budget_tokens: usize,
    min_level: Option<&str>,
    component: &str,
) -> Result<String> {
    let min_level = match min_level {
        None => admiralty_telemetry::Level::Trace,
        Some(text) => admiralty_telemetry::Level::parse(text)
            .ok_or("min-level must be one of error, warn, info, debug, trace")?,
    };
    let mut bytes = Vec::new();
    match file {
        Some(path) if path != Path::new("-") => {
            let file = fs::File::open(path)?;
            if !file.metadata()?.is_file() {
                return Err("log input must be a regular file".into());
            }
            // Keep the newest bytes of very large logs: the tail is what
            // explains the current state.
            let size = file.metadata()?.len();
            let mut file = io::BufReader::new(file);
            if size > MAX_LOG_BYTES {
                io::Seek::seek(&mut file, io::SeekFrom::Start(size - MAX_LOG_BYTES))?;
            }
            file.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
        }
        _ => {
            io::stdin()
                .lock()
                .take(MAX_LOG_BYTES)
                .read_to_end(&mut bytes)?;
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(admiralty_telemetry::digest::render_text(
        component,
        &text,
        &admiralty_telemetry::digest::Options {
            budget_tokens,
            since: None,
            min_level,
        },
    ))
}

/// Create (or verify) the private preview directory and its dashboard token.
///
/// Traverses protected, no-follow directory descriptors and validates the
/// resulting credential with the daemon's reader. Existing invalid tokens
/// are refused without replacement or permission changes. Newly created
/// tokens contain 48 random bytes encoded as 64 URL-safe characters.
#[cfg(unix)]
pub fn init_preview_state(directory: &Path) -> Result<PathBuf> {
    use rustix::fs::{Mode, OFlags, fstat, mkdirat, open, openat};
    use std::path::Component;

    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err("preview state directory must be absolute and normalized".into());
    }
    let uid = rustix::process::geteuid().as_raw();
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut parent = open("/", flags, Mode::empty())?;
    for component in directory.components() {
        let info = fstat(&parent)?;
        if ![0, uid].contains(&info.st_uid)
            || (info.st_mode & 0o022 != 0 && info.st_mode & 0o1000 == 0)
        {
            return Err("preview state ancestor is not protected".into());
        }
        if let Component::Normal(name) = component {
            match mkdirat(&parent, name, Mode::from_raw_mode(0o700)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            parent = openat(&parent, name, flags, Mode::empty())?;
        }
    }
    let info = fstat(&parent)?;
    if info.st_uid != uid || info.st_mode & 0o077 != 0 {
        return Err("existing preview state directory must be owned and mode 0700".into());
    }
    let token = directory.join("dashboard-token");
    match openat(
        &parent,
        "dashboard-token",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    ) {
        Ok(descriptor) => {
            let mut file = fs::File::from(descriptor);
            let mut secret = random_token()?;
            let written = file
                .write_all(secret.as_bytes())
                .and_then(|_| file.sync_all());
            // Best-effort overwrite of the in-memory copy before reporting.
            wipe(&mut secret);
            written?;
            fs::File::from(parent).sync_all()?;
        }
        Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    let mut credential = admiralty_server::startup::read_private_token(&token)?;
    wipe(&mut credential);
    Ok(token)
}

#[cfg(not(unix))]
pub fn init_preview_state(_: &Path) -> Result<PathBuf> {
    Err("private preview state is supported on Unix only".into())
}

/// Overwrite a secret string's bytes without `unsafe` (forbidden here).
#[cfg(unix)]
fn wipe(secret: &mut String) {
    let length = secret.len();
    secret.clear();
    secret.extend(std::iter::repeat_n('0', length));
    secret.clear();
}

/// 48 random bytes, URL-safe base64 without padding (64 characters), the
/// same shape as Python's `secrets.token_urlsafe(48)`.
#[cfg(unix)]
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 48];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(64);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        for shift in [18, 12, 6, 0] {
            out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
        }
    }
    bytes.fill(0);
    Ok(out)
}

/// Outcome of a readiness poll.
#[derive(Debug, serde::Serialize)]
pub struct Readiness {
    pub ready: bool,
    pub attempts: u32,
    pub elapsed_ms: u128,
    pub last_status: Option<u16>,
    pub last_body: Option<serde_json::Value>,
    pub last_error: Option<String>,
}

/// Poll `GET /readyz` until it answers 200 or `timeout` passes. Plain HTTP
/// over one TCP connection per attempt; no proxy, redirects or credentials.
pub fn wait_ready(listen: SocketAddr, timeout: Duration) -> Readiness {
    let started = Instant::now();
    let mut result = Readiness {
        ready: false,
        attempts: 0,
        elapsed_ms: 0,
        last_status: None,
        last_body: None,
        last_error: None,
    };
    let Some(deadline) = started.checked_add(timeout) else {
        result.last_error = Some("readiness deadline exceeds clock range".into());
        return result;
    };
    while Instant::now() < deadline {
        result.attempts += 1;
        match get(listen, "/readyz", deadline) {
            Ok((status, body)) => {
                result.last_status = Some(status);
                result.last_body = serde_json::from_str(&body).ok();
                result.last_error = None;
                if status == 200 {
                    result.ready = true;
                    break;
                }
            }
            Err(error) => result.last_error = Some(error.to_string()),
        }
        std::thread::sleep(
            Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    result.elapsed_ms = started.elapsed().as_millis();
    result
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "readiness deadline elapsed"))
}

fn get(address: SocketAddr, path: &str, deadline: Instant) -> io::Result<(u16, String)> {
    let mut stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?.min(Duration::from_secs(2)))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    let mut pending = request.as_bytes();
    while !pending.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?.min(Duration::from_secs(3))))?;
        match stream.write(pending) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "readiness write failed",
                ));
            }
            Ok(written) => pending = &pending[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let mut response = Vec::new();
    let mut buffer = [0; 4096];
    while response.len() < 256 * 1024 {
        stream.set_read_timeout(Some(remaining(deadline)?.min(Duration::from_secs(3))))?;
        let capacity = buffer.len().min(256 * 1024 - response.len());
        match stream.read(&mut buffer[..capacity]) {
            Ok(0) => break,
            Ok(read) => response.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    remaining(deadline)?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed HTTP status line"))?;
    Ok((status, body.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn private_root() -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .prefix("adm-preview-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(
                std::env::temp_dir()
                    .canonicalize()
                    .expect("canonical scratch"),
            )
            .expect("private scratch")
    }

    #[cfg(unix)]
    #[test]
    fn preview_state_refuses_intermediate_symlinks_before_creation() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = private_root();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).expect("destination");
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).expect("mode");
        symlink(&destination, root.path().join("link")).expect("link");
        assert!(init_preview_state(&root.path().join("link/new")).is_err());
        assert!(!destination.join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn preview_state_refuses_shared_ancestors_before_creation() {
        use std::os::unix::fs::PermissionsExt;
        let root = private_root();
        let shared = root.path().join("shared");
        fs::create_dir(&shared).expect("shared");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).expect("mode");
        assert!(init_preview_state(&shared.join("new")).is_err());
        assert!(!shared.join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn preview_state_validates_existing_token_bytes_without_replacement() {
        let root = private_root();
        let state = root.path().join("state");
        let token = init_preview_state(&state).expect("create");
        for invalid in [
            b"".as_slice(),
            b"short",
            b"a credential containing spaces is not valid",
        ] {
            fs::write(&token, invalid).expect("fixture");
            assert!(init_preview_state(&state).is_err());
            assert_eq!(fs::read(&token).expect("unchanged"), invalid);
        }
    }

    #[cfg(unix)]
    #[test]
    fn preview_state_refuses_hard_linked_tokens_without_replacement() {
        let root = private_root();
        let state = root.path().join("state");
        let token = init_preview_state(&state).expect("create");
        let original = fs::read(&token).expect("token");
        fs::hard_link(&token, root.path().join("alias")).expect("link");
        assert!(init_preview_state(&state).is_err());
        assert_eq!(fs::read(&token).expect("unchanged"), original);
    }

    #[test]
    fn readiness_zero_budget_starts_no_attempt() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let result = wait_ready(listener.local_addr().expect("address"), Duration::ZERO);
        assert!(!result.ready);
        assert_eq!(result.attempts, 0);
    }

    #[test]
    fn readiness_total_deadline_bounds_a_trickling_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("timeout");
            let mut request = [0; 512];
            let _ = stream.read(&mut request);
            if stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .is_ok()
            {
                for _ in 0..40 {
                    std::thread::sleep(Duration::from_millis(30));
                    if stream.write_all(b" ").is_err() {
                        break;
                    }
                }
            }
        });
        let started = Instant::now();
        let result = wait_ready(address, Duration::from_millis(150));
        let elapsed = started.elapsed();
        server.join().expect("server");
        assert!(!result.ready, "slow peer accepted after {elapsed:?}");
        assert_eq!(result.attempts, 1);
        assert!(result.last_error.is_some());
        assert!(
            elapsed < Duration::from_secs(1),
            "deadline exceeded: {elapsed:?}"
        );
    }

    #[test]
    fn digest_reads_files_and_rejects_bad_levels() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("ci.log");
        fs::write(
            &path,
            "error: test failed in 12ms\nerror: test failed in 19ms\ncompiling crate\n",
        )
        .expect("write");
        let digest = digest_logs(Some(&path), 300, Some("warn"), "ci").expect("digest");
        assert!(digest.starts_with("ADMIRALTY-LOG/1 cmp=ci"), "{digest}");
        assert!(
            digest.contains("E×2 raw \"error: test failed in #ms\""),
            "{digest}"
        );
        assert!(!digest.contains("compiling"), "below min level");
        assert!(digest_logs(Some(&path), 300, Some("loud"), "ci").is_err());
        assert!(digest_logs(Some(directory.path()), 300, None, "ci").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preview_state_is_private_idempotent_and_refuses_unsafe_paths() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = private_root();
        let state = root.path().join("nested").join("preview");
        let token = init_preview_state(&state).expect("create");
        let first = fs::read_to_string(&token).expect("token");
        assert_eq!(first.len(), 64);
        assert!(
            first
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(
            fs::metadata(&state).expect("dir").permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&token).expect("file").permissions().mode() & 0o777,
            0o600
        );
        // Re-running keeps the existing token.
        init_preview_state(&state).expect("idempotent");
        assert_eq!(fs::read_to_string(&token).expect("token"), first);
        // Loosened permissions are refused, never repaired silently.
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(init_preview_state(&state).is_err());
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).expect("chmod");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(init_preview_state(&state).is_err());
        let link = root.path().join("link");
        symlink(&state, &link).expect("symlink");
        assert!(init_preview_state(&link).is_err());
        assert!(init_preview_state(Path::new("relative/dir")).is_err());
        assert_ne!(random_token().expect("a"), random_token().expect("b"));
    }

    #[test]
    fn readiness_poll_reports_status_body_and_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            for (index, stream) in listener.incoming().take(2).enumerate() {
                let mut stream = stream.expect("accept");
                let mut request = Vec::new();
                let mut buffer = [0u8; 512];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                    }
                }
                let (status, body) = if index == 0 {
                    ("503 Service Unavailable", r#"{"status":"not_ready"}"#)
                } else {
                    ("200 OK", r#"{"status":"ready"}"#)
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let ready = wait_ready(address, Duration::from_secs(5));
        server.join().expect("server");
        assert!(ready.ready);
        assert_eq!(ready.attempts, 2);
        assert_eq!(ready.last_body.expect("body")["status"], "ready");
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("address");
        let failed = wait_ready(closed, Duration::from_millis(10));
        assert!(!failed.ready);
        assert!(failed.last_error.is_some());
    }
}
