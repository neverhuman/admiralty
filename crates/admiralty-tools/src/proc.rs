//! Bounded child-process execution (Python `subprocess.run(..., timeout=)`).

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::ToolError;

/// Captured result of a finished child.
#[derive(Debug)]
pub struct Captured {
    /// Exit status.
    pub status: ExitStatus,
    /// Collected stdout (bounded by `stdout_limit` when given).
    pub stdout: Vec<u8>,
    /// Collected stderr (empty when not captured).
    pub stderr: Vec<u8>,
}

impl Captured {
    /// Exit code as Python reports it (`-signal` for signal deaths).
    pub fn code(&self) -> i32 {
        exit_code(&self.status)
    }
}

/// Python-style return code.
pub fn exit_code(status: &ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| status.signal().map_or(-1, |signal| -signal))
}

fn reader<R: Read + Send + 'static>(
    mut source: R,
    limit: Option<usize>,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut collected = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match source.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let keep =
                        limit.map_or(count, |cap| count.min(cap.saturating_sub(collected.len())));
                    collected.extend_from_slice(&buffer[..keep]);
                }
            }
        }
        collected
    })
}

/// Wait for `child` until `deadline`; kill and reap it on expiry.
pub fn wait_until(child: &mut Child, deadline: Instant) -> Result<ExitStatus, ToolError> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolError::Timeout("child process timed out".to_owned()));
        }
        thread::sleep((deadline - now).min(Duration::from_millis(10)));
    }
}

/// Run `command` with stdin from `input` (or null), capturing stdout and
/// optionally stderr, killing it after `timeout`.
///
/// `stdout_limit` keeps at most that many bytes (one more than the caller's
/// bound lets the caller detect overflow) while still draining the pipe.
pub fn run_bounded(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    capture_stderr: bool,
    stdout_limit: Option<usize>,
) -> Result<Captured, ToolError> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(if capture_stderr {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let deadline = Instant::now() + timeout;
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().map(|pipe| reader(pipe, stdout_limit));
    let stderr = child.stderr.take().map(|pipe| reader(pipe, None));
    let writer = match (child.stdin.take(), input) {
        (Some(mut pipe), Some(data)) => {
            let data = data.to_vec();
            Some(thread::spawn(move || {
                let _ = pipe.write_all(&data);
            }))
        }
        _ => None,
    };
    let status = wait_until(&mut child, deadline)?;
    if let Some(handle) = writer {
        let _ = handle.join();
    }
    let stdout = stdout
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();
    let stderr = stderr
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();
    Ok(Captured {
        status,
        stdout,
        stderr,
    })
}

/// Read at most `limit + 1` bytes from `child`'s stdout until EOF or
/// `deadline`, then wait for exit. The child is always killed and reaped on
/// any failure (Python `_bounded_git_head` semantics).
pub fn read_bounded_then_wait(
    mut child: Child,
    limit: usize,
    deadline: Instant,
    timed_out: &str,
    oversized: &str,
) -> Result<(Vec<u8>, ExitStatus), ToolError> {
    let result = (|| {
        let pipe = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::invalid(timed_out.to_owned()))?;
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut pipe = pipe;
            let mut total = 0usize;
            let mut buffer = vec![0u8; limit + 1];
            while total <= limit {
                match pipe.read(&mut buffer[..limit + 1 - total]) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        total += count;
                        if sender.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut output = Vec::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(ToolError::invalid(timed_out.to_owned()));
            }
            match receiver.recv_timeout(deadline - now) {
                Ok(chunk) => {
                    output.extend_from_slice(&chunk);
                    if output.len() > limit {
                        return Err(ToolError::invalid(oversized.to_owned()));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(ToolError::invalid(timed_out.to_owned()));
                }
            }
        }
        let wait_deadline = deadline.max(Instant::now() + Duration::from_millis(10));
        let status = wait_until(&mut child, wait_deadline)
            .map_err(|_| ToolError::invalid(timed_out.to_owned()))?;
        Ok((output, status))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
