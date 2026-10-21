//! Every external effect of the audit and the ticker goes through [`Host`]:
//! child processes (`screen`, `ssh`, `git`, `gh`, `nvidia-smi`, `ps`, `nice`),
//! the clock, sleeping and `/proc` reads. Production uses [`RealHost`]; tests
//! inject a fake so they are hermetic (the Python tests monkeypatched the
//! equivalent module functions).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{SubsecRound, Utc};

use crate::py::{self, Dt};

/// Result of a child process: `(returncode, stdout, stderr)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CmdOut {
    pub rc: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOut {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            rc: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    pub fn rc(rc: i32, stdout: impl Into<String>) -> Self {
        Self {
            rc,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    /// The Python `run()` failure tuple: `(99, '', str(exception))`.
    pub fn failed(msg: impl Into<String>) -> Self {
        Self {
            rc: 99,
            stdout: String::new(),
            stderr: msg.into(),
        }
    }
}

/// Injectable side effects.
pub trait Host {
    /// Run `argv` with stdin=/dev/null, capturing text output. `timeout=None` waits forever.
    /// Spawn failures, timeouts and undecodable output return rc 99 (Python parity).
    fn run(&self, argv: &[&str], timeout: Option<Duration>, cwd: Option<&Path>) -> CmdOut;
    /// Current UTC time (microsecond precision, like Python's datetime).
    fn now(&self) -> Dt;
    fn sleep(&self, d: Duration);
    /// `screen -S name -X hardcopy path`, wait 0.3 s, read the file (text mode, errors=replace).
    fn hardcopy(&self, name: &str, path: &str) -> String {
        let _ = std::fs::remove_file(path);
        self.run(
            &["screen", "-S", name, "-X", "hardcopy", path],
            Some(Duration::from_secs(10)),
            None,
        );
        self.sleep(Duration::from_millis(300));
        read_text_replace(path).unwrap_or_default()
    }
    /// Numeric entries of `/proc` (ascending).
    fn proc_pids(&self) -> Vec<i64>;
    /// `os.readlink(path)`.
    fn read_link(&self, path: &str) -> Option<String>;
    /// Raw bytes of a `/proc` file.
    fn read_proc(&self, path: &str) -> Option<Vec<u8>>;
    /// `os.stat(path).st_atime` (float seconds).
    fn atime(&self, path: &str) -> Option<f64>;
    /// `stat -c %Y path` (integer mtime seconds).
    fn mtime(&self, path: &str) -> Option<i64>;
    /// This process id (`$$`).
    fn pid(&self) -> u32;
}

/// `open(path, errors='replace').read()`: lossy UTF-8 plus universal newlines.
pub fn read_text_replace(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(py::universal_newlines(&String::from_utf8_lossy(&bytes)))
}

/// The real host.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealHost;

fn decode(bytes: &[u8]) -> Result<String, String> {
    match String::from_utf8(bytes.to_vec()) {
        Ok(s) => Ok(py::universal_newlines(&s)),
        Err(e) => Err(format!("'utf-8' codec can't decode bytes: {e}")),
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match status.code() {
        Some(c) => c,
        None => status.signal().map_or(-1, |s| -s),
    }
}

/// Spawn one child process. This is the only place that creates processes:
/// stdin is always `/dev/null` (a child that touches the ticker's tty would get
/// SIGTTIN and wedge the audit), output is captured, and the timeout kills it.
pub fn spawn_capture(argv: &[&str], timeout: Option<Duration>, cwd: Option<&Path>) -> CmdOut {
    let Some((program, args)) = argv.split_first() else {
        return CmdOut::failed("empty argv");
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = PyErrText::spawn(program, &e);
            return CmdOut::failed(msg);
        }
    };
    let (tx, rx) = mpsc::channel::<(u8, Vec<u8>)>();
    if let Some(mut out) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send((1, buf));
        });
    }
    if let Some(mut err) = child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            let _ = tx.send((2, buf));
        });
    }
    drop(tx);
    let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
    let timed_out = || {
        format!(
            "Command '{}' timed out after {} seconds",
            py::py_repr(&py::J::Arr(
                argv.iter().map(|a| py::J::Str((*a).to_string())).collect()
            )),
            timeout.map_or(0, |t| t.as_secs())
        )
    };
    let (mut stdout, mut stderr) = (None, None);
    let mut status = None;
    // Done only when the child has exited and both pipe readers have reported.
    let (status, out_bytes, err_bytes) = loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(s)) => status = Some(s),
                Ok(None) => {}
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CmdOut::failed(e.to_string());
                }
            }
        }
        match (status, stdout.take(), stderr.take()) {
            (Some(exited), Some(out), Some(err)) => break (exited, out, err),
            (_, out, err) => (stdout, stderr) = (out, err),
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            let _ = child.kill();
            let _ = child.wait();
            return CmdOut::failed(timed_out());
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok((1, b)) => stdout = Some(b),
            Ok((_, b)) => stderr = Some(b),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stdout.get_or_insert_with(Vec::new);
                stderr.get_or_insert_with(Vec::new);
                if status.is_none() {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    };
    let rc = exit_code(status);
    let out = decode(&out_bytes);
    let err = decode(&err_bytes);
    match (out, err) {
        (Ok(stdout), Ok(stderr)) => CmdOut { rc, stdout, stderr },
        (Err(e), _) | (_, Err(e)) => CmdOut::failed(e),
    }
}

struct PyErrText;
impl PyErrText {
    fn spawn(program: &str, e: &std::io::Error) -> String {
        let code = e.raw_os_error().unwrap_or(0);
        let full = e.to_string();
        let strerror = full
            .rfind(" (os error ")
            .map_or(full.clone(), |i| full[..i].to_string());
        format!("[Errno {code}] {strerror}: {}", py::py_str_repr(program))
    }
}

impl Host for RealHost {
    fn run(&self, argv: &[&str], timeout: Option<Duration>, cwd: Option<&Path>) -> CmdOut {
        spawn_capture(argv, timeout, cwd)
    }
    fn now(&self) -> Dt {
        Utc::now().trunc_subsecs(6).with_timezone(&py::utc())
    }
    fn sleep(&self, d: Duration) {
        thread::sleep(d);
    }
    fn proc_pids(&self) -> Vec<i64> {
        let mut pids: Vec<i64> = match std::fs::read_dir("/proc") {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with(|c: char| c.is_ascii_digit()) {
                        name.parse().ok()
                    } else {
                        None
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        pids.sort_unstable();
        pids
    }
    fn read_link(&self, path: &str) -> Option<String> {
        std::fs::read_link(path)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }
    fn read_proc(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
    fn atime(&self, path: &str) -> Option<f64> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some(m.atime() as f64 + m.atime_nsec() as f64 / 1e9)
    }
    fn mtime(&self, path: &str) -> Option<i64> {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.mtime())
    }
    fn pid(&self) -> u32 {
        std::process::id()
    }
}

/// A scriptable fake host for hermetic tests.
#[doc(hidden)]
pub mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    type Runner = Box<dyn Fn(&[&str], Option<&Path>) -> CmdOut>;
    type Hardcopy = Box<dyn Fn(&str, &str) -> String>;

    /// Records every call; answers from closures.
    pub struct FakeHost {
        pub now: RefCell<Dt>,
        pub runner: Runner,
        pub hardcopy_fn: Option<Hardcopy>,
        pub calls: RefCell<Vec<Vec<String>>>,
        pub timeouts: RefCell<Vec<Option<Duration>>>,
        pub sleeps: RefCell<Vec<Duration>>,
        pub pids: Vec<i64>,
        pub links: BTreeMap<String, String>,
        pub proc_files: BTreeMap<String, Vec<u8>>,
        pub atimes: BTreeMap<String, f64>,
        pub mtimes: RefCell<BTreeMap<String, i64>>,
        /// Advance the clock by each sleep (useful for the ticker).
        pub sleep_advances_clock: bool,
    }

    impl FakeHost {
        pub fn new(now: Dt) -> Self {
            Self {
                now: RefCell::new(now),
                runner: Box::new(|_, _| CmdOut::ok("")),
                hardcopy_fn: None,
                calls: RefCell::new(Vec::new()),
                timeouts: RefCell::new(Vec::new()),
                sleeps: RefCell::new(Vec::new()),
                pids: Vec::new(),
                links: BTreeMap::new(),
                proc_files: BTreeMap::new(),
                atimes: BTreeMap::new(),
                mtimes: RefCell::new(BTreeMap::new()),
                sleep_advances_clock: false,
            }
        }
        pub fn with_runner(
            mut self,
            f: impl Fn(&[&str], Option<&Path>) -> CmdOut + 'static,
        ) -> Self {
            self.runner = Box::new(f);
            self
        }
        pub fn with_hardcopy(mut self, f: impl Fn(&str, &str) -> String + 'static) -> Self {
            self.hardcopy_fn = Some(Box::new(f));
            self
        }
        /// argv of every `run` call.
        pub fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl Host for FakeHost {
        fn run(&self, argv: &[&str], timeout: Option<Duration>, cwd: Option<&Path>) -> CmdOut {
            self.calls
                .borrow_mut()
                .push(argv.iter().map(|a| (*a).to_string()).collect());
            self.timeouts.borrow_mut().push(timeout);
            (self.runner)(argv, cwd)
        }
        fn now(&self) -> Dt {
            *self.now.borrow()
        }
        fn sleep(&self, d: Duration) {
            self.sleeps.borrow_mut().push(d);
            if self.sleep_advances_clock {
                let mut now = self.now.borrow_mut();
                *now += chrono::Duration::from_std(d).unwrap_or_default();
            }
        }
        fn hardcopy(&self, name: &str, path: &str) -> String {
            match &self.hardcopy_fn {
                Some(f) => f(name, path),
                None => {
                    let _ = std::fs::remove_file(path);
                    self.run(
                        &["screen", "-S", name, "-X", "hardcopy", path],
                        Some(Duration::from_secs(10)),
                        None,
                    );
                    self.sleep(Duration::from_millis(300));
                    read_text_replace(path).unwrap_or_default()
                }
            }
        }
        fn proc_pids(&self) -> Vec<i64> {
            self.pids.clone()
        }
        fn read_link(&self, path: &str) -> Option<String> {
            self.links.get(path).cloned()
        }
        fn read_proc(&self, path: &str) -> Option<Vec<u8>> {
            self.proc_files.get(path).cloned()
        }
        fn atime(&self, path: &str) -> Option<f64> {
            self.atimes.get(path).copied()
        }
        fn mtime(&self, path: &str) -> Option<i64> {
            self.mtimes.borrow().get(path).copied()
        }
        fn pid(&self) -> u32 {
            4242
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_returns_99() {
        let out = spawn_capture(&["sleep", "5"], Some(Duration::from_millis(200)), None);
        assert_eq!(out.rc, 99);
        assert!(out.stdout.is_empty());
        assert!(out.stderr.contains("timed out"));
    }

    #[test]
    fn spawn_failure_returns_99() {
        let out = spawn_capture(
            &["/nonexistent/definitely-not-a-binary"],
            Some(Duration::from_secs(5)),
            None,
        );
        assert_eq!(out.rc, 99);
        assert!(out.stderr.contains("No such file or directory"));
    }

    #[test]
    fn captures_text_with_universal_newlines() {
        let out = spawn_capture(
            &["printf", "a\\r\\nb\\rc"],
            Some(Duration::from_secs(5)),
            None,
        );
        assert_eq!(out.rc, 0);
        assert_eq!(out.stdout, "a\nb\nc");
    }

    #[test]
    fn invalid_utf8_output_is_a_failure_like_python_text_mode() {
        let out = spawn_capture(&["printf", "\\377"], Some(Duration::from_secs(5)), None);
        assert_eq!(out.rc, 99);
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn exit_code_is_reported() {
        let out = spawn_capture(&["sh", "-c", "exit 3"], Some(Duration::from_secs(5)), None);
        assert_eq!(out.rc, 3);
    }
}
