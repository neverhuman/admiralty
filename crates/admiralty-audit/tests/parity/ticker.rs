//! Ticker (port of captains/super_captain_tick.sh) driven by a fake host.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use admiralty_audit::host::CmdOut;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::tick::{TickConfig, TickState, Ticker};
use admiralty_audit::{ScTickArgs, tick_config};

use crate::common::at;

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: String,
    cfg: TickConfig,
}

fn fixture(audit: bool) -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().to_string_lossy().to_string();
    let argv = audit.then(|| {
        ["nice", "-n", "10", "/opt/admiralty", "sc-audit", "--quiet"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    });
    let mut cfg = TickConfig::new(&dir, "super_captain", argv, Duration::from_secs(1800));
    cfg.hc_path = format!("{dir}/hc_sc_tick");
    std::fs::create_dir_all(format!("{dir}/sc_audit")).expect("mkdir");
    Fixture {
        _tmp: tmp,
        dir,
        cfg,
    }
}

/// A host that answers `screen -ls` with `ls`, writes `screen_text` on hardcopy and
/// answers the audit command with `audit_rc`.
fn host(ls: &'static str, screen_text: Rc<RefCell<String>>, audit_rc: i32) -> FakeHost {
    let mut h =
        FakeHost::new(at("2026-10-21T15:30:00+00:00")).with_runner(move |argv, _| match argv {
            ["screen", "-ls"] => CmdOut::rc(1, ls),
            ["screen", "-S", _, "-X", "hardcopy", path] => {
                std::fs::write(path, screen_text.borrow().as_bytes()).expect("write hardcopy");
                CmdOut::ok("")
            }
            ["nice", ..] => CmdOut {
                rc: audit_rc,
                stdout: String::new(),
                stderr: if audit_rc == 0 {
                    String::new()
                } else {
                    "boom\n".to_string()
                },
            },
            _ => CmdOut::ok(""),
        });
    h.sleep_advances_clock = true;
    h
}

fn log_lines(f: &Fixture) -> Vec<String> {
    std::fs::read_to_string(&f.cfg.log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

const LS: &str =
    "There is a screen on:\n\t1234.super_captain\t(Detached)\n1 Socket in /run/screen/S-ubuntu.\n";

#[test]
fn idle_prompt_gets_the_tick_and_audit_summary_is_logged() {
    let f = fixture(true);
    std::fs::write(
        &f.cfg.latest_json,
        r#"{"utc": "x", "summary": {"red": 2, "amber": 1, "must_act": 1}}"#,
    )
    .expect("write");
    let h = host(LS, Rc::new(RefCell::new("output\n> \n".to_string())), 0);
    Ticker {
        cfg: &f.cfg,
        host: &h,
    }
    .run(true);
    let log = log_lines(&f);
    assert_eq!(log[0], "2026-10-21T15:30:00Z ticker start pid 4242");
    assert_eq!(
        log[1],
        "2026-10-21T15:30:00Z audit ok: {'red': 2, 'amber': 1, 'must_act': 1}"
    );
    assert!(log[2].ends_with(" tick sent (09:30 MDT)"), "{log:?}");
    assert_eq!(log.len(), 3);
    let calls = h.calls();
    assert_eq!(
        calls[0],
        ["nice", "-n", "10", "/opt/admiralty", "sc-audit", "--quiet"]
    );
    let tail: Vec<Vec<String>> = calls.iter().skip(3).cloned().collect();
    let tick_txt = format!("{}/super_captain_tick.txt", f.dir);
    assert_eq!(
        tail[0],
        [
            "screen",
            "-S",
            "super_captain",
            "-X",
            "readbuf",
            tick_txt.as_str()
        ]
    );
    assert_eq!(
        tail[1],
        ["screen", "-S", "super_captain", "-X", "paste", "."]
    );
    assert_eq!(
        tail[2],
        ["screen", "-S", "super_captain", "-X", "stuff", "\r"]
    );
    assert_eq!(
        *h.sleeps.borrow(),
        vec![Duration::from_millis(500), Duration::from_secs(1)]
    );
    assert_eq!(h.timeouts.borrow()[0], Some(Duration::from_secs(300)));
    // no initial delay with --once
    assert!(!h.sleeps.borrow().contains(&Duration::from_secs(1800)));
}

#[test]
fn busy_prompt_is_skipped_after_twenty_minutes() {
    let f = fixture(false);
    let h = host(
        LS,
        Rc::new(RefCell::new("working\n>\nesc to cancel\n".to_string())),
        0,
    );
    let mut st = TickState::default();
    Ticker {
        cfg: &f.cfg,
        host: &h,
    }
    .tick_once(&mut st);
    let log = log_lines(&f);
    assert_eq!(
        log,
        vec!["2026-10-21T15:50:40Z tick skipped: super_captain busy 20 min".to_string()]
    );
    let sleeps = h.sleeps.borrow();
    assert_eq!(
        sleeps
            .iter()
            .filter(|d| **d == Duration::from_secs(15))
            .count(),
        80
    );
    assert_eq!(
        sleeps
            .iter()
            .filter(|d| **d == Duration::from_millis(500))
            .count(),
        80
    );
    assert!(
        !h.calls()
            .iter()
            .any(|c| c.iter().any(|a| a == "readbuf" || a == "stuff"))
    );
    assert_eq!(st.last_sent, 0);
}

#[test]
fn typed_draft_is_never_overwritten() {
    let f = fixture(false);
    let screen = Rc::new(RefCell::new(
        "> half-typed message from Jepson\n".to_string(),
    ));
    let h = host(LS, screen.clone(), 0);
    let mut st = TickState::default();
    Ticker {
        cfg: &f.cfg,
        host: &h,
    }
    .tick_once(&mut st);
    assert!(!h.calls().iter().any(|c| c.iter().any(|a| a == "paste")));
    assert!(
        log_lines(&f)
            .last()
            .is_some_and(|l| l.ends_with("tick skipped: super_captain busy 20 min"))
    );
}

#[test]
fn missing_screen_and_failed_audit_are_logged() {
    let f = fixture(true);
    let h = host(
        "No Sockets found in /run/screen/S-ubuntu.\n",
        Rc::new(RefCell::new(String::new())),
        124,
    );
    let mut st = TickState::default();
    Ticker {
        cfg: &f.cfg,
        host: &h,
    }
    .tick_once(&mut st);
    let log = log_lines(&f);
    assert_eq!(
        log,
        vec![
            "2026-10-21T15:30:00Z audit FAILED (see sc_audit_cron.log)".to_string(),
            "2026-10-21T15:30:00Z super_captain screen missing".to_string(),
            "2026-10-21T15:30:00Z tick skipped: super_captain busy 20 min".to_string(),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&f.cfg.cron_log).expect("cron log"),
        "boom\n"
    );
}

#[test]
fn previous_tick_acknowledgement_uses_status_mtime() {
    let f = fixture(false);
    let h = host(LS, Rc::new(RefCell::new("> \n".to_string())), 0);
    let sent = at("2026-10-21T15:00:00+00:00").timestamp();
    h.mtimes
        .borrow_mut()
        .insert(f.cfg.status_json.clone(), sent + 125);
    let mut st = TickState { last_sent: sent };
    let ticker = Ticker {
        cfg: &f.cfg,
        host: &h,
    };
    ticker.tick_once(&mut st);
    let log = log_lines(&f);
    assert_eq!(
        log[0],
        "2026-10-21T15:30:00Z previous tick acted on (status.json updated 15:02:05Z)"
    );
    // the tick was sent again; status.json has not been touched since
    assert!(st.last_sent > sent);
    ticker.tick_once(&mut st);
    let log = log_lines(&f);
    assert!(
        log.iter().any(|l| l
            .ends_with("WARNING previous tick NOT acted on (status.json not updated since tick)")),
        "{log:?}"
    );
}

#[test]
fn audit_summary_missing_logs_empty_text() {
    let f = fixture(true);
    let h = host(LS, Rc::new(RefCell::new("> \n".to_string())), 0);
    let mut st = TickState::default();
    Ticker {
        cfg: &f.cfg,
        host: &h,
    }
    .tick_once(&mut st);
    assert_eq!(log_lines(&f)[0], "2026-10-21T15:30:00Z audit ok: ");
}

#[test]
fn cli_config_defaults_match_the_script() {
    let args = ScTickArgs {
        no_audit: true,
        ..ScTickArgs::default()
    };
    let cfg = tick_config(&args).expect("config");
    assert_eq!(cfg.dir, "/home/ubuntu/xbabe2_kicker/captains-20261017");
    assert_eq!(
        cfg.log,
        "/home/ubuntu/xbabe2_kicker/captains-20261017/super_captain_tick.log"
    );
    assert_eq!(
        cfg.tick_txt,
        "/home/ubuntu/xbabe2_kicker/captains-20261017/super_captain_tick.txt"
    );
    assert_eq!(
        cfg.latest_json,
        "/home/ubuntu/xbabe2_kicker/captains-20261017/sc_audit/latest.json"
    );
    assert_eq!(cfg.screen, "super_captain");
    assert_eq!(cfg.hc_path, "/tmp/hc_sc_tick");
    assert_eq!(cfg.audit_argv, None);
    assert_eq!(cfg.interval, Duration::from_secs(1800));
    assert_eq!(
        (cfg.idle_attempts, cfg.idle_poll),
        (80, Duration::from_secs(15))
    );
    let missing = ScTickArgs {
        audit_exe: Some("/nonexistent/admiralty".into()),
        ..ScTickArgs::default()
    };
    assert_eq!(
        tick_config(&missing).expect("config").audit_argv,
        None,
        "[ -f $AUDIT ] skips a missing audit"
    );
    let present = ScTickArgs {
        audit_exe: Some("/bin/sh".into()),
        ..ScTickArgs::default()
    };
    let argv = tick_config(&present)
        .expect("config")
        .audit_argv
        .expect("argv");
    assert_eq!(&argv[..5], ["nice", "-n", "10", "/bin/sh", "sc-audit"]);
    assert!(argv.contains(&"--quiet".to_string()));
}
