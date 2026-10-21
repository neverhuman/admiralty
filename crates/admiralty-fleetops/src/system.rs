//! Injectable side effects: external commands ([`Runner`]) and time ([`Clock`]).
//!
//! Every external program (`screen`, `ssh`, `bash`) goes through [`Runner`],
//! so tests are hermetic. The production [`SystemRunner`] gives children a
//! null stdin, captures stdout/stderr and enforces a hard timeout (the whole
//! process group is killed when it expires).

use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Offset, TimeZone, Utc, Weekday};

/// One external command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub timeout: Duration,
}

impl CmdSpec {
    pub fn new(program: impl Into<OsString>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            timeout,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Arguments as lossy UTF-8 strings (diagnostics and tests).
    pub fn args_lossy(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Program followed by arguments, lossy.
    pub fn argv_lossy(&self) -> Vec<String> {
        let mut v = vec![self.program.to_string_lossy().into_owned()];
        v.extend(self.args_lossy());
        v
    }
}

/// Captured result of a finished (or killed) command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdOutput {
    /// Exit code; `None` when killed by a signal (including our timeout).
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

impl CmdOutput {
    pub fn ok() -> Self {
        Self {
            code: Some(0),
            ..Self::default()
        }
    }

    pub fn failed(code: i32, stderr: impl Into<Vec<u8>>) -> Self {
        Self {
            code: Some(code),
            stderr: stderr.into(),
            ..Self::default()
        }
    }

    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// Shell-style exit status (`128+signal` is not recoverable here; a
    /// timeout or signal maps to 124 / 1).
    pub fn exit_code(&self) -> i32 {
        if self.timed_out {
            return 124;
        }
        self.code.unwrap_or(1)
    }
}

/// External command executor.
pub trait Runner {
    /// Run to completion (or timeout). `Err` means the program could not be
    /// started at all.
    fn run(&self, spec: &CmdSpec) -> io::Result<CmdOutput>;

    /// Resolve a program like `command -v` would.
    fn find_program(&self, program: &OsStr) -> Option<PathBuf>;
}

/// Real process runner.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

impl Runner for SystemRunner {
    fn run(&self, spec: &CmdSpec) -> io::Result<CmdOutput> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
        if let Some(mut out) = child.stdout.take() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = tx.send((true, buf));
            });
        }
        if let Some(mut err) = child.stderr.take() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
                let _ = tx.send((false, buf));
            });
        }
        drop(tx);
        let deadline = Instant::now() + spec.timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                timed_out = true;
                let pid = rustix::process::Pid::from_child(&child);
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                let _ = child.kill();
                break child.wait().ok();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut output = CmdOutput {
            code: status.and_then(|s| s.code()),
            timed_out,
            ..CmdOutput::default()
        };
        // Grandchildren may keep the pipes open; never wait unboundedly.
        let drain_deadline = Instant::now() + PIPE_DRAIN_GRACE;
        for _ in 0..2 {
            let left = drain_deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok((true, buf)) => output.stdout = buf,
                Ok((false, buf)) => output.stderr = buf,
                Err(_) => break,
            }
        }
        Ok(output)
    }

    fn find_program(&self, program: &OsStr) -> Option<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        find_in_path(program, &path)
    }
}

/// `command -v PROGRAM` against an explicit `PATH` value.
pub fn find_in_path(program: &OsStr, path_var: &OsStr) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    if program.as_bytes().contains(&b'/') {
        let p = PathBuf::from(program);
        return is_executable_file(&p).then_some(p);
    }
    for dir in path_var.as_bytes().split(|b| *b == b':') {
        let dir = if dir.is_empty() {
            Path::new(".")
        } else {
            Path::new(OsStr::from_bytes(dir))
        };
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
}

/// Wall clock, local zone and sleeping.
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
    /// UTC offset of the host's local zone at `at` (what `date` would use).
    fn local_offset(&self, at: DateTime<Utc>) -> FixedOffset;
    fn sleep(&self, duration: Duration);
}

/// Real clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn local_offset(&self, at: DateTime<Utc>) -> FixedOffset {
        chrono::Local
            .offset_from_utc_datetime(&at.naive_utc())
            .fix()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Zone abbreviation for `date +%Z`: `UTC` for a zero offset, otherwise a
/// tzdata-style numeric abbreviation (`-06`, `+0530`).
pub fn zone_abbrev(offset: FixedOffset) -> String {
    let secs = offset.local_minus_utc();
    if secs == 0 {
        return "UTC".to_string();
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let abs = secs.unsigned_abs();
    let (h, m) = (abs / 3600, (abs % 3600) / 60);
    if m == 0 {
        format!("{sign}{h:02}")
    } else {
        format!("{sign}{h:02}{m:02}")
    }
}

/// `date '+%Y-%m-%d %H:%M:%S %Z'` (also Python `time.strftime` with `%Z`).
pub fn local_stamp(clock: &dyn Clock, at: DateTime<Utc>) -> String {
    let offset = clock.local_offset(at);
    let local = at.with_timezone(&offset);
    format!(
        "{} {}",
        local.format("%Y-%m-%d %H:%M:%S"),
        zone_abbrev(offset)
    )
}

/// `date -u +%Y-%m-%dT%H:%M:%SZ`.
pub fn utc_iso(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// `date -u +%T`.
pub fn utc_hms(at: DateTime<Utc>) -> String {
    at.format("%H:%M:%S").to_string()
}

fn nth_sunday(year: i32, month: u32, n: u32) -> Option<NaiveDate> {
    NaiveDate::from_weekday_of_month_opt(year, month, Weekday::Sun, u8::try_from(n).ok()?)
}

/// UTC offset of `America/Denver` (US rules since 2007: MDT from the second
/// Sunday of March 02:00 MST to the first Sunday of November 02:00 MDT).
pub fn denver_offset(at: DateTime<Utc>) -> FixedOffset {
    let mst = FixedOffset::west_opt(7 * 3600).unwrap_or_else(|| Utc.fix());
    let mdt = FixedOffset::west_opt(6 * 3600).unwrap_or_else(|| Utc.fix());
    let year = at.year();
    let (Some(start), Some(end)) = (nth_sunday(year, 3, 2), nth_sunday(year, 11, 1)) else {
        return mst;
    };
    // 02:00 MST = 09:00 UTC; 02:00 MDT = 08:00 UTC.
    let start_utc = start.and_hms_opt(9, 0, 0).map(|n| n.and_utc());
    let end_utc = end.and_hms_opt(8, 0, 0).map(|n| n.and_utc());
    match (start_utc, end_utc) {
        (Some(s), Some(e)) if at >= s && at < e => mdt,
        _ => mst,
    }
}

/// `TZ=America/Denver date +%H:%M:%S`.
pub fn denver_hms(at: DateTime<Utc>) -> String {
    at.with_timezone(&denver_offset(at))
        .format("%H:%M:%S")
        .to_string()
}

/// Replace the current process image (used by `codex-luna`). Returns only on
/// failure.
pub fn exec_replace(program: &Path, argv0: &str, args: &[OsString], cwd: &Path) -> io::Error {
    Command::new(program)
        .arg0(argv0)
        .args(args)
        .current_dir(cwd)
        .exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn denver_dst_boundaries() {
        // 2026: DST starts Mar 8, ends Nov 1.
        assert_eq!(denver_hms(utc("2026-03-08T08:59:59Z")), "01:59:59");
        assert_eq!(denver_hms(utc("2026-03-08T09:00:00Z")), "03:00:00");
        assert_eq!(denver_hms(utc("2026-10-18T03:24:00Z")), "21:24:00");
        assert_eq!(denver_hms(utc("2026-11-01T07:59:59Z")), "01:59:59");
        assert_eq!(denver_hms(utc("2026-11-01T08:00:00Z")), "01:00:00");
        assert_eq!(denver_hms(utc("2026-01-15T12:00:00Z")), "05:00:00");
    }

    #[test]
    fn zone_abbreviations() {
        let z = |s: i32| FixedOffset::east_opt(s).expect("offset");
        assert_eq!(zone_abbrev(z(0)), "UTC");
        assert_eq!(zone_abbrev(z(-6 * 3600)), "-06");
        assert_eq!(zone_abbrev(z(5 * 3600 + 1800)), "+0530");
    }

    #[test]
    fn system_runner_null_stdin_and_capture() {
        let spec = CmdSpec::new("sh", Duration::from_secs(10))
            .arg("-c")
            .arg("cat; printf out; printf err >&2; exit 3");
        let out = SystemRunner.run(&spec).expect("sh runs");
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        assert!(!out.timed_out);
    }

    #[test]
    fn system_runner_enforces_timeout_on_process_group() {
        let started = Instant::now();
        let spec = CmdSpec::new("sh", Duration::from_millis(200))
            .arg("-c")
            .arg("sleep 30 & sleep 30");
        let out = SystemRunner.run(&spec).expect("sh runs");
        assert!(out.timed_out);
        assert_eq!(out.exit_code(), 124);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn system_runner_reports_missing_program() {
        let spec = CmdSpec::new("/nonexistent/definitely-missing", Duration::from_secs(1));
        let err = SystemRunner.run(&spec).expect_err("missing program");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn path_lookup_requires_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").expect("write");
        let path_var = dir.path().as_os_str().to_owned();
        assert!(find_in_path(OsStr::new("tool"), &path_var).is_none());
        let mut perm = std::fs::metadata(&exe).expect("meta").permissions();
        perm.set_mode(0o700);
        std::fs::set_permissions(&exe, perm).expect("chmod");
        assert_eq!(find_in_path(OsStr::new("tool"), &path_var), Some(exe));
        assert!(find_in_path(OsStr::new("other"), &path_var).is_none());
    }
}
