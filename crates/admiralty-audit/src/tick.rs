//! Ticker for screen `super_captain` (port of `captains/super_captain_tick.sh`).
//!
//! Every 30 min: run the read-only audit, then paste `super_captain_tick.txt`
//! into super_captain ONLY when its prompt is idle. Waits up to 80 x 15 s
//! (20 min) for idle; otherwise skips that tick. Also logs whether
//! super_captain acted on the previous tick (status.json updated after it).
//! Read-only otherwise.

use std::io::Write;
use std::time::Duration;

use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone, Timelike};
use regex::Regex;

use crate::ctx::rj;
use crate::host::Host;
use crate::py::{self, Dt};

/// `D` in the shell script.
pub const DEFAULT_TICK_DIR: &str = "/home/ubuntu/xbabe2_kicker/captains-20261017";
/// `S` in the shell script.
pub const DEFAULT_SCREEN: &str = "super_captain";

/// Ticker configuration (defaults equal the script's constants).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickConfig {
    pub dir: String,
    pub screen: String,
    pub log: String,
    /// Audit command (`nice -n 10 <admiralty> sc-audit --quiet ...`); `None` skips the audit
    /// (the script skipped it when tools/sc_audit.py was missing).
    pub audit_argv: Option<Vec<String>>,
    pub audit_timeout: Duration,
    pub cron_log: String,
    pub latest_json: String,
    pub status_json: String,
    pub tick_txt: String,
    pub hc_path: String,
    pub first_delay: Duration,
    pub interval: Duration,
    pub idle_attempts: u32,
    pub idle_poll: Duration,
}

/// `sleep ${FIRST_DELAY:-1800}` argument parsing (GNU sleep: number + optional s/m/h/d).
pub fn parse_sleep(value: Option<&str>) -> Duration {
    let v = match value {
        Some(v) if !v.is_empty() => v,
        _ => "1800",
    };
    let (num, mult) = match v.chars().last() {
        Some('s') => (&v[..v.len() - 1], 1.0),
        Some('m') => (&v[..v.len() - 1], 60.0),
        Some('h') => (&v[..v.len() - 1], 3600.0),
        Some('d') => (&v[..v.len() - 1], 86400.0),
        _ => (v, 1.0),
    };
    match num.parse::<f64>() {
        Ok(n) if n.is_finite() && n >= 0.0 => Duration::from_secs_f64(n * mult),
        // `sleep` rejects the argument and the script continues immediately
        _ => Duration::ZERO,
    }
}

impl TickConfig {
    /// Defaults for a directory, screen and audit command.
    pub fn new(
        dir: &str,
        screen: &str,
        audit_argv: Option<Vec<String>>,
        first_delay: Duration,
    ) -> Self {
        Self {
            dir: dir.to_string(),
            screen: screen.to_string(),
            log: format!("{dir}/super_captain_tick.log"),
            audit_argv,
            audit_timeout: Duration::from_secs(300),
            cron_log: format!("{dir}/sc_audit_cron.log"),
            latest_json: format!("{dir}/sc_audit/latest.json"),
            status_json: format!("{dir}/super_captain_status.json"),
            tick_txt: format!("{dir}/super_captain_tick.txt"),
            hc_path: "/tmp/hc_sc_tick".to_string(),
            first_delay,
            interval: Duration::from_secs(1800),
            idle_attempts: 80,
            idle_poll: Duration::from_secs(15),
        }
    }
}

/// Mutable ticker state (`last_sent` epoch seconds).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickState {
    pub last_sent: i64,
}

/// `date -u +%FT%TZ`.
pub fn utc_stamp(now: &Dt) -> String {
    now.with_timezone(&py::utc())
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn nth_sunday(year: i32, month: u32, n: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let offset = (7 - first.weekday().num_days_from_sunday()) % 7;
    first.checked_add_days(chrono::Days::new(u64::from(offset + 7 * (n - 1))))
}

/// `TZ=America/Denver date +%H:%M` (current US DST rules).
pub fn denver_hhmm(now: &Dt) -> String {
    let utc_now = now.with_timezone(&py::utc());
    let year = utc_now.year();
    let dst = match (nth_sunday(year, 3, 2), nth_sunday(year, 11, 1)) {
        (Some(start), Some(end)) => {
            // 02:00 MST = 09:00 UTC; 02:00 MDT = 08:00 UTC
            let start =
                py::utc().from_utc_datetime(&start.and_hms_opt(9, 0, 0).unwrap_or_default());
            let end = py::utc().from_utc_datetime(&end.and_hms_opt(8, 0, 0).unwrap_or_default());
            utc_now >= start && utc_now < end
        }
        _ => false,
    };
    let offset =
        FixedOffset::west_opt(if dst { 6 * 3600 } else { 7 * 3600 }).unwrap_or_else(py::utc);
    let local = utc_now.with_timezone(&offset);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

/// The script's `idle()` predicate over the hardcopy bytes.
pub fn idle_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    if bytes
        .windows(b"esc to cancel".len())
        .any(|w| w == b"esc to cancel")
    {
        return false;
    }
    let last_prompt = bytes.split(|b| *b == b'\n').rfind(|l| l.starts_with(b">"));
    match last_prompt {
        Some(line) => {
            line.iter()
                .filter(|b| **b != b' ')
                .copied()
                .collect::<Vec<u8>>()
                == b">"
        }
        None => false,
    }
}

fn append(path: &str, text: &str) {
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()));
    if let Err(e) = result {
        eprintln!("{path}: {e}");
    }
}

/// One ticker pass.
pub struct Ticker<'a> {
    pub cfg: &'a TickConfig,
    pub host: &'a dyn Host,
}

impl Ticker<'_> {
    fn log(&self, msg: &str) {
        append(
            &self.cfg.log,
            &format!("{} {msg}\n", utc_stamp(&self.host.now())),
        );
    }

    fn screen(&self, args: &[&str]) {
        let mut argv = vec!["screen", "-S", self.cfg.screen.as_str(), "-X"];
        argv.extend_from_slice(args);
        self.host.run(&argv, Some(Duration::from_secs(30)), None);
    }

    /// `idle()`: hardcopy to the tick file, wait 0.5 s, apply the heuristic.
    pub fn idle(&self) -> bool {
        let f = self.cfg.hc_path.as_str();
        let _ = std::fs::remove_file(f);
        self.screen(&["hardcopy", f]);
        self.host.sleep(Duration::from_millis(500));
        std::fs::read(f).is_ok_and(|b| idle_text(&b))
    }

    fn screen_present(&self) -> bool {
        let out = self
            .host
            .run(&["screen", "-ls"], Some(Duration::from_secs(30)), None);
        let pattern = format!(r"\.{}[[:space:]]", regex::escape(&self.cfg.screen));
        Regex::new(&pattern).is_ok_and(|re| re.is_match(&out.stdout))
    }

    fn summary_text(&self) -> String {
        rj(&self.cfg.latest_json)
            .and_then(|j| j.as_obj().and_then(|o| o.get("summary")).cloned())
            .map_or_else(String::new, |s| py::py_repr(&s))
    }

    /// The body of the `while true` loop (without the trailing interval sleep).
    pub fn tick_once(&self, st: &mut TickState) {
        if st.last_sent != 0 {
            let mtime = self.host.mtime(&self.cfg.status_json).unwrap_or(0);
            if mtime > st.last_sent {
                let when = py::utc()
                    .timestamp_opt(mtime, 0)
                    .single()
                    .map_or_else(String::new, |t| t.format("%H:%M:%SZ").to_string());
                self.log(&format!(
                    "previous tick acted on (status.json updated {when})"
                ));
            } else {
                self.log("WARNING previous tick NOT acted on (status.json not updated since tick)");
            }
        }
        if let Some(argv) = self.cfg.audit_argv.as_ref() {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.host.run(&refs, Some(self.cfg.audit_timeout), None);
            append(&self.cfg.cron_log, &format!("{}{}", out.stdout, out.stderr));
            if out.rc == 0 {
                self.log(&format!("audit ok: {}", self.summary_text()));
            } else {
                self.log("audit FAILED (see sc_audit_cron.log)");
            }
        }
        let mut sent = false;
        for _ in 0..self.cfg.idle_attempts {
            if !self.screen_present() {
                self.log("super_captain screen missing");
                break;
            }
            if self.idle() {
                let tick_txt = self.cfg.tick_txt.clone();
                self.screen(&["readbuf", &tick_txt]);
                self.screen(&["paste", "."]);
                self.host.sleep(Duration::from_secs(1));
                self.screen(&["stuff", "\r"]);
                let now = self.host.now();
                self.log(&format!("tick sent ({} MDT)", denver_hhmm(&now)));
                sent = true;
                st.last_sent = self.host.now().timestamp();
                break;
            }
            self.host.sleep(self.cfg.idle_poll);
        }
        if !sent {
            self.log("tick skipped: super_captain busy 20 min");
        }
    }

    /// The whole script: start line, first delay, then tick every interval.
    /// With `once`, run a single pass without the initial delay or trailing sleep.
    pub fn run(&self, once: bool) {
        self.log(&format!("ticker start pid {}", self.host.pid()));
        let mut st = TickState::default();
        if !once {
            self.host.sleep(self.cfg.first_delay);
        }
        loop {
            self.tick_once(&mut st);
            if once {
                return;
            }
            self.host.sleep(self.cfg.interval);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Dt {
        py::fromiso_aware(s).expect("datetime")
    }

    #[test]
    fn denver_time_follows_us_dst() {
        assert_eq!(denver_hhmm(&at("2026-10-21T15:30:00Z")), "09:30");
        assert_eq!(denver_hhmm(&at("2026-12-01T15:30:00Z")), "08:30");
        // 2026: DST from Mar 8 09:00Z to Nov 1 08:00Z
        assert_eq!(denver_hhmm(&at("2026-03-08T08:59:00Z")), "01:59");
        assert_eq!(denver_hhmm(&at("2026-03-08T09:00:00Z")), "03:00");
        assert_eq!(denver_hhmm(&at("2026-11-01T07:59:00Z")), "01:59");
        assert_eq!(denver_hhmm(&at("2026-11-01T08:00:00Z")), "01:00");
    }

    #[test]
    fn sleep_argument_parsing() {
        assert_eq!(parse_sleep(None), Duration::from_secs(1800));
        assert_eq!(parse_sleep(Some("")), Duration::from_secs(1800));
        assert_eq!(parse_sleep(Some("0")), Duration::ZERO);
        assert_eq!(parse_sleep(Some("2m")), Duration::from_secs(120));
        assert_eq!(parse_sleep(Some("1.5")), Duration::from_millis(1500));
        assert_eq!(parse_sleep(Some("bogus")), Duration::ZERO);
    }

    #[test]
    fn idle_heuristic_matches_shell() {
        assert!(!idle_text(b""));
        assert!(idle_text(b"some output\n> \n"));
        assert!(idle_text(b"> old typed\n>   \n  status line\n"));
        assert!(!idle_text(b"> typed text\n"));
        assert!(!idle_text(b">\nworking esc to cancel\n"));
        assert!(!idle_text(b"no prompt here\n"));
        assert!(!idle_text(b" >\n"));
    }
}
