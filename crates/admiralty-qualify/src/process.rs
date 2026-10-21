//! Owned child processes: bounded output capture, timed waits, TERM→KILL
//! cleanup through the owned handle, and small OS helpers.

use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Python `Popen.returncode` (`-signal` for a signal death).
pub fn returncode(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0))
}

/// `Popen.wait(timeout)`: `Ok(None)` when the child is still running.
pub fn wait_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let end = Instant::now() + timeout;
    let mut delay = Duration::from_micros(500);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= end {
            return Ok(None);
        }
        thread::sleep(delay.min(end - now));
        delay = (delay * 2).min(Duration::from_millis(50));
    }
}

/// `Popen.poll()` on something that can report an exit code.
pub trait ChildPoll {
    /// The return code once exited.
    fn poll(&mut self) -> Option<i32>;
}

impl ChildPoll for Child {
    fn poll(&mut self) -> Option<i32> {
        self.try_wait().ok().flatten().map(returncode)
    }
}

/// How [`cleanup_owned_child`] ended the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// It had exited before cleanup.
    AlreadyExited,
    /// It exited after SIGTERM.
    Terminated,
    /// It ignored SIGTERM and was killed.
    KilledAfterTermTimeout,
}

impl CleanupOutcome {
    /// The receipt string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyExited => "already_exited",
            Self::Terminated => "terminated",
            Self::KilledAfterTermTimeout => "killed_after_term_timeout",
        }
    }
}

/// Seconds to wait after SIGTERM.
pub const TERM_WAIT_SECONDS: f64 = 1.0;
/// Seconds to wait after SIGKILL.
pub const FINAL_WAIT_SECONDS: f64 = 1.0;

/// Terminate only this owned child (never a numeric PID or group): SIGTERM,
/// bounded wait, SIGKILL fallback, and reaping. The child is not reaped
/// before the signal, so its PID cannot have been recycled.
pub fn cleanup_owned_child(child: &mut Child) -> Result<CleanupOutcome, String> {
    if child.poll().is_some() {
        return Ok(CleanupOutcome::AlreadyExited);
    }
    let pid = rustix::process::Pid::from_child(child);
    if rustix::process::kill_process(pid, rustix::process::Signal::TERM).is_err() {
        let _ = wait_timeout(child, Duration::from_secs_f64(FINAL_WAIT_SECONDS));
        return Ok(CleanupOutcome::AlreadyExited);
    }
    if let Ok(Some(_)) = wait_timeout(child, Duration::from_secs_f64(TERM_WAIT_SECONDS)) {
        return Ok(CleanupOutcome::Terminated);
    }
    if child.poll().is_some() {
        return Ok(CleanupOutcome::AlreadyExited);
    }
    if child.kill().is_err() {
        let _ = wait_timeout(child, Duration::from_secs_f64(FINAL_WAIT_SECONDS));
        return Ok(CleanupOutcome::AlreadyExited);
    }
    match wait_timeout(child, Duration::from_secs_f64(FINAL_WAIT_SECONDS)) {
        Ok(Some(_)) => Ok(CleanupOutcome::KilledAfterTermTimeout),
        _ => Err("owned child did not exit after handle-bound kill".into()),
    }
}

#[derive(Debug, Default)]
struct CaptureState {
    data: Vec<u8>,
    total_bytes: usize,
    oversized: bool,
    eof: bool,
}

/// Thread-safe output capture retaining at most `limit` bytes.
#[derive(Debug, Clone)]
pub struct BoundedCapture {
    limit: usize,
    state: Arc<Mutex<CaptureState>>,
}

impl BoundedCapture {
    /// A capture with the given retention limit.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            state: Arc::new(Mutex::new(CaptureState::default())),
        }
    }

    fn with<T>(&self, action: impl FnOnce(&mut CaptureState) -> T) -> T {
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        action(&mut guard)
    }

    /// Record bytes, retaining only the first `limit`.
    pub fn append(&self, data: &[u8]) {
        let limit = self.limit;
        self.with(|state| {
            state.total_bytes += data.len();
            let remaining = limit.saturating_sub(state.data.len());
            if remaining > 0 {
                state
                    .data
                    .extend_from_slice(&data[..data.len().min(remaining)]);
            }
            if state.total_bytes > limit {
                state.oversized = true;
            }
        });
    }

    /// Retained bytes.
    pub fn snapshot(&self) -> Vec<u8> {
        self.with(|state| state.data.clone())
    }

    /// Whether more than `limit` bytes arrived.
    pub fn oversized(&self) -> bool {
        self.with(|state| state.oversized)
    }

    /// Whether the producer reached EOF.
    pub fn eof(&self) -> bool {
        self.with(|state| state.eof)
    }

    /// Mark EOF.
    pub fn set_eof(&self) {
        self.with(|state| state.eof = true);
    }

    /// Total bytes observed.
    pub fn total_bytes(&self) -> usize {
        self.with(|state| state.total_bytes)
    }
}

/// Drain `pipe` into `capture` on a background thread (4 KiB reads).
pub fn drain_pipe<R: Read + Send + 'static>(
    mut pipe: R,
    capture: &BoundedCapture,
) -> JoinHandle<()> {
    let capture = capture.clone();
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => capture.append(&chunk[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        capture.set_eof();
    })
}

/// Wait up to `timeout` for a capture's producer to reach EOF (`join(timeout)`).
pub fn join_capture(capture: &BoundedCapture, timeout: Duration) {
    let end = Instant::now() + timeout;
    while !capture.eof() && Instant::now() < end {
        thread::sleep(Duration::from_millis(2));
    }
}

/// `secrets.token_urlsafe(32)`: 32 bytes from the OS CSPRNG, base64url
/// without padding.
pub fn token_urlsafe() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(base64_url(&bytes))
}

fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let b0 = u32::from(group[0]);
        let b1 = u32::from(group.get(1).copied().unwrap_or(0));
        let b2 = u32::from(group.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        let symbols = group.len() + 1;
        for index in 0..symbols {
            let shift = 18 - 6 * index;
            out.push(char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]));
        }
    }
    out
}

/// `datetime.now(timezone.utc).isoformat()`.
pub fn utc_isoformat_now() -> String {
    let now = chrono::Utc::now();
    let micros = now.timestamp_subsec_micros();
    if micros == 0 {
        now.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        format!("{}.{micros:06}+00:00", now.format("%Y-%m-%dT%H:%M:%S"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_url_matches_python_token_urlsafe_alphabet() {
        assert_eq!(base64_url(b"\xfb\xff"), "-_8");
        assert_eq!(base64_url(b"abc"), "YWJj");
        assert_eq!(base64_url(&[0u8; 32]).len(), 43);
    }
}
