//! Hermetic test doubles: a recording command runner, a fake clock and a GNU
//! screen simulator.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};

use crate::system::{Clock, CmdOutput, CmdSpec, Runner};

type Handler = Box<dyn Fn(&CmdSpec) -> io::Result<CmdOutput>>;

pub struct FakeRunner {
    calls: RefCell<Vec<CmdSpec>>,
    handler: Handler,
    programs: BTreeMap<String, PathBuf>,
}

impl FakeRunner {
    pub fn new(handler: impl Fn(&CmdSpec) -> io::Result<CmdOutput> + 'static) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            handler: Box::new(handler),
            programs: BTreeMap::new(),
        }
    }

    pub fn with_program(mut self, name: &str, path: &str) -> Self {
        self.programs.insert(name.to_string(), PathBuf::from(path));
        self
    }

    pub fn calls(&self) -> Vec<CmdSpec> {
        self.calls.borrow().clone()
    }

    /// Every screen argv issued (lossy), for guarantee checks.
    pub fn screen_argvs(&self) -> Vec<Vec<String>> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| c.program == "screen")
            .map(CmdSpec::args_lossy)
            .collect()
    }
}

impl Runner for FakeRunner {
    fn run(&self, spec: &CmdSpec) -> io::Result<CmdOutput> {
        self.calls.borrow_mut().push(spec.clone());
        (self.handler)(spec)
    }

    fn find_program(&self, program: &OsStr) -> Option<PathBuf> {
        self.programs
            .get(program.to_string_lossy().as_ref())
            .cloned()
    }
}

/// Deterministic clock: `sleep` advances `now` and is recorded.
pub struct FakeClock {
    now: Cell<DateTime<Utc>>,
    offset_secs: i32,
    sleeps: RefCell<Vec<Duration>>,
    /// When set, sleeps do not advance time and are logged as `sleep S`
    /// events into this screen simulator (bash parity harness format).
    frozen_log: Option<Rc<ScreenState>>,
}

impl FakeClock {
    pub fn at(rfc3339: &str) -> Self {
        let now = DateTime::parse_from_rfc3339(rfc3339)
            .expect("valid fake clock timestamp")
            .with_timezone(&Utc);
        Self {
            now: Cell::new(now),
            offset_secs: 0,
            sleeps: RefCell::new(Vec::new()),
            frozen_log: None,
        }
    }

    /// Freeze time and log sleeps into `state`'s event list.
    pub fn frozen_logging_into(mut self, state: &Rc<ScreenState>) -> Self {
        self.frozen_log = Some(Rc::clone(state));
        self
    }

    pub fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.borrow().clone()
    }

    pub fn advance(&self, d: Duration) {
        let delta = chrono::Duration::from_std(d).expect("small duration");
        self.now.set(self.now.get() + delta);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        self.now.get()
    }

    fn local_offset(&self, _at: DateTime<Utc>) -> FixedOffset {
        FixedOffset::east_opt(self.offset_secs).expect("valid offset")
    }

    fn sleep(&self, duration: Duration) {
        self.sleeps.borrow_mut().push(duration);
        match &self.frozen_log {
            Some(state) => {
                let secs = duration.as_secs();
                let millis = duration.subsec_millis();
                let text = if millis == 0 {
                    format!("sleep {secs}")
                } else {
                    let frac = format!("{millis:03}");
                    format!("sleep {secs}.{}", frac.trim_end_matches('0'))
                };
                state.events.borrow_mut().push(text);
            }
            None => self.advance(duration),
        }
    }
}

/// Simulated GNU screen server state.
#[derive(Default)]
pub struct ScreenState {
    /// session name/id -> successive hardcopy contents (the last one repeats).
    pub screens: RefCell<BTreeMap<String, Vec<Vec<u8>>>>,
    /// How many hardcopies each session has served.
    pub served: RefCell<BTreeMap<String, usize>>,
    /// Human-readable log of every input-affecting command.
    pub events: RefCell<Vec<String>>,
    /// `screen -ls` stdout.
    pub ls: RefCell<String>,
    /// When true, `hardcopy -h` fails (old screen) but plain hardcopy works.
    pub no_scrollback: Cell<bool>,
    /// Exit code for `stuff` (0 = success).
    pub stuff_code: Cell<i32>,
    /// Every `stuff` as (session, raw payload).
    pub stuffed: RefCell<Vec<(String, Vec<u8>)>>,
}

impl ScreenState {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    pub fn set_screen(&self, session: &str, contents: &[&str]) {
        self.screens.borrow_mut().insert(
            session.to_string(),
            contents.iter().map(|c| c.as_bytes().to_vec()).collect(),
        );
    }

    pub fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }

    /// `stuff SESSION HEX` lines (the bash parity harness format).
    pub fn stuff_hex_log(&self) -> String {
        self.stuffed
            .borrow()
            .iter()
            .map(|(s, p)| {
                let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
                format!("stuff {s} {hex}\n")
            })
            .collect()
    }

    fn lookup(&self, session: &str) -> Option<Vec<u8>> {
        let screens = self.screens.borrow();
        let frames = screens.get(session).or_else(|| {
            // `-S name` also matches `PID.name`.
            screens
                .iter()
                .find(|(k, _)| k.split_once('.').map(|(_, n)| n) == Some(session))
                .map(|(_, v)| v)
        })?;
        let mut served = self.served.borrow_mut();
        let n = served.entry(session.to_string()).or_insert(0);
        let frame = frames
            .get(*n)
            .or_else(|| frames.last())
            .cloned()
            .unwrap_or_default();
        *n += 1;
        Some(frame)
    }

    /// Handle one `screen` argv.
    pub fn handle(&self, spec: &CmdSpec) -> io::Result<CmdOutput> {
        let args = spec.args.clone();
        let a: Vec<&[u8]> = args.iter().map(|x| x.as_bytes()).collect();
        if a == [b"-ls".as_slice()] {
            return Ok(CmdOutput {
                code: Some(1),
                stdout: self.ls.borrow().as_bytes().to_vec(),
                ..CmdOutput::default()
            });
        }
        let session = String::from_utf8_lossy(a.get(1).copied().unwrap_or_default()).into_owned();
        let mut i = 2;
        if a.get(i) == Some(&b"-p".as_slice()) {
            i += 2;
        }
        i += 1; // -X
        let cmd = a.get(i).copied().unwrap_or_default();
        let rest: Vec<&[u8]> = a.iter().skip(i + 1).copied().collect();
        let exists =
            self.screens.borrow().keys().any(|k| {
                k == &session || k.split_once('.').map(|(_, n)| n) == Some(session.as_str())
            });
        if !exists {
            return Ok(CmdOutput {
                code: Some(1),
                stdout: b"No screen session found.\r\n".to_vec(),
                ..CmdOutput::default()
            });
        }
        match cmd {
            b"hardcopy" => {
                let scrollback = rest.first() == Some(&b"-h".as_slice());
                if scrollback && self.no_scrollback.get() {
                    return Ok(CmdOutput::failed(1, Vec::new()));
                }
                let path = rest.last().copied().unwrap_or_default();
                let content = self.lookup(&session).unwrap_or_default();
                std::fs::write(OsStr::from_bytes(path), content)?;
                self.events.borrow_mut().push(format!(
                    "hardcopy {session}{}",
                    if scrollback { " -h" } else { "" }
                ));
                Ok(CmdOutput::ok())
            }
            b"stuff" => {
                let payload = rest.first().copied().unwrap_or_default();
                self.stuffed
                    .borrow_mut()
                    .push((session.clone(), payload.to_vec()));
                self.events.borrow_mut().push(format!(
                    "stuff {session} {}",
                    String::from_utf8_lossy(payload).escape_debug()
                ));
                Ok(CmdOutput {
                    code: Some(self.stuff_code.get()),
                    ..CmdOutput::default()
                })
            }
            b"readbuf" => {
                let file = rest.first().copied().unwrap_or_default();
                self.events.borrow_mut().push(format!(
                    "readbuf {session} {}",
                    String::from_utf8_lossy(file)
                ));
                Ok(CmdOutput::ok())
            }
            b"paste" => {
                self.events.borrow_mut().push(format!("paste {session}"));
                Ok(CmdOutput::ok())
            }
            other => {
                self.events
                    .borrow_mut()
                    .push(format!("{} {session}", String::from_utf8_lossy(other)));
                Ok(CmdOutput::ok())
            }
        }
    }
}

/// A runner whose `screen` calls go to `state`; anything else fails.
pub fn screen_runner(state: &Rc<ScreenState>) -> FakeRunner {
    let st = Rc::clone(state);
    FakeRunner::new(move |spec| {
        if spec.program == "screen" {
            st.handle(spec)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "unexpected program",
            ))
        }
    })
    .with_program("screen", "/usr/bin/screen")
}

/// Assert the non-attaching guarantee over every recorded screen call.
pub fn assert_never_attached(runner: &FakeRunner) {
    for argv in runner.screen_argvs() {
        let os: Vec<std::ffi::OsString> = argv.iter().map(std::ffi::OsString::from).collect();
        assert!(
            crate::screen::check_non_attaching(&os).is_ok(),
            "attaching screen call issued: {argv:?}"
        );
        for forbidden in ["-r", "-R", "-dr", "-D", "-d", "-x", "-DR", "-RR"] {
            let before_x: Vec<&String> = argv
                .iter()
                .take_while(|a| *a != "-X" && *a != "-Q")
                .collect();
            assert!(
                !before_x.iter().any(|a| a.as_str() == forbidden),
                "forbidden {forbidden} in {argv:?}"
            );
        }
    }
}

/// Collects output written through `&mut dyn Write`.
pub fn text(buf: &[u8]) -> String {
    String::from_utf8_lossy(buf).into_owned()
}
