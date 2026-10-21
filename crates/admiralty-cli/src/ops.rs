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
/// Replaces the Python heredoc formerly embedded in `scripts/dev/live.sh`
/// with identical rules: the directory is absolute, not a symlink, owned by
/// the caller and mode 0700; the token is created exclusively with 0600 and
/// 48 random bytes encoded URL-safe (64 characters); an existing token must
/// be a private regular file owned by the caller. The token value is never
/// printed.
#[cfg(unix)]
pub fn init_preview_state(directory: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    if !directory.is_absolute() {
        return Err("preview state directory must be absolute".into());
    }
    if fs::symlink_metadata(directory).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("preview state directory must not be a symlink".into());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let uid = rustix::process::geteuid().as_raw();
    let info = fs::symlink_metadata(directory)?;
    if !info.is_dir() || info.uid() != uid || info.permissions().mode() & 0o077 != 0 {
        return Err("existing preview state directory must be owned and mode 0700".into());
    }
    let token = directory.join("dashboard-token");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&token)
    {
        Ok(mut file) => {
            let mut secret = random_token()?;
            let written = file
                .write_all(secret.as_bytes())
                .and_then(|_| file.sync_all());
            // Best-effort overwrite of the in-memory copy before reporting.
            wipe(&mut secret);
            written?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let info = fs::symlink_metadata(&token)?;
            if !info.file_type().is_file()
                || info.uid() != uid
                || info.permissions().mode() & 0o077 != 0
            {
                return Err("existing dashboard token must be a private owned regular file".into());
            }
        }
        Err(error) => return Err(error.into()),
    }
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
    loop {
        result.attempts += 1;
        match get(listen, "/readyz") {
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
        if started.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    result.elapsed_ms = started.elapsed().as_millis();
    result
}

fn get(address: SocketAddr, path: &str) -> io::Result<(u16, String)> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    // One write: a peer that answers after its first read must see the whole
    // request, or closing with unread bytes would reset the connection.
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    stream.take(256 * 1024).read_to_end(&mut response)?;
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
        let root = tempfile::tempdir().expect("tempdir");
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
