use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;
use crate::FleetConfig;
use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner, text};

fn kick_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kick")
}

fn screens() -> Rc<ScreenState> {
    let state = ScreenState::new();
    for entry in std::fs::read_dir(kick_fixtures().join("screens")).expect("screens") {
        let path = entry.expect("entry").path();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let body = std::fs::read_to_string(&path).expect("screen");
        state.set_screen(&name, &[&body]);
    }
    state
}

fn read_opt(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
    log: Option<String>,
    states: Vec<(String, String)>,
    stuffed: String,
}

fn run_scenario(name: &str, utf8: bool) -> Run {
    let sc = kick_fixtures().join(name);
    let args: Vec<String> = std::fs::read_to_string(sc.join("args.txt"))
        .expect("args")
        .lines()
        .map(str::to_string)
        .collect();
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    std::fs::create_dir_all(base.join("state")).expect("state dir");
    if let Ok(entries) = std::fs::read_dir(sc.join("seed_state")) {
        for e in entries.flatten() {
            std::fs::copy(e.path(), base.join("state").join(e.file_name())).expect("seed");
        }
    }
    if sc.join("disk_hold.txt").exists() {
        std::fs::copy(sc.join("disk_hold.txt"), base.join("state/DISK_HOLD")).expect("hold");
    }
    let mut cfg = FleetConfig {
        kicker_base: base.clone(),
        tmp_dir: dir.path().to_path_buf(),
        ..FleetConfig::default()
    };
    if let Some(list) = read_opt(&sc.join("sessions.txt")) {
        cfg.monitored_sessions = list.lines().map(str::to_string).collect();
    }
    let state = screens();
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-21T18:30:05Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err).with_utf8_locale(utf8);
        kick(&mut ctx, &args).expect("kick runs")
    };
    assert_never_attached(&runner);
    let norm = |s: String| s.replace(&base.display().to_string(), "{BASE}");
    let mut states: Vec<(String, String)> = std::fs::read_dir(base.join("state"))
        .expect("state dir")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read_to_string(e.path()).expect("state file"),
            )
        })
        .collect();
    states.sort();
    Run {
        code,
        stdout: norm(text(&out)),
        stderr: norm(text(&err)),
        log: read_opt(&base.join("logs/kick.log")).map(norm),
        states,
        stuffed: state.stuff_hex_log(),
    }
}

/// Compare a scenario against the outputs captured from the original
/// `tools/bin/kick` (run in a POSIX locale against a fake `screen`, `date`
/// and `sleep`; state-file timestamps normalized).
fn assert_matches_bash(name: &str) {
    let sc = kick_fixtures().join(name);
    let got = run_scenario(name, false);
    let exp_code: i32 = read_opt(&sc.join("expected_exit.txt"))
        .expect("exit")
        .trim()
        .parse()
        .expect("exit code");
    assert_eq!(got.code, exp_code, "{name}: exit code");
    assert_eq!(
        got.stdout,
        read_opt(&sc.join("expected_stdout.txt")).expect("stdout"),
        "{name}: stdout"
    );
    let exp_err = read_opt(&sc.join("expected_stderr.txt"))
        .expect("stderr")
        .replace("usage: kick [", "usage: admiralty fleet kick [")
        .replace(
            "       kick --inject",
            "       admiralty fleet kick --inject",
        );
    assert_eq!(got.stderr, exp_err, "{name}: stderr");
    assert_eq!(
        got.log,
        read_opt(&sc.join("expected_kick.txt")),
        "{name}: kick.log"
    );
    assert_eq!(
        got.stuffed,
        read_opt(&sc.join("expected_stuff.txt")).expect("stuff"),
        "{name}: screen input"
    );
    let mut expected_states: Vec<(String, String)> = std::fs::read_dir(sc.join("expected_state"))
        .map(|rd| {
            rd.flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        std::fs::read_to_string(e.path()).expect("expected state"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    expected_states.sort();
    assert_eq!(got.states, expected_states, "{name}: state files");
}

#[test]
fn full_pass_matches_bash() {
    assert_matches_bash("pass");
}

#[test]
fn approvals_cooldowns_and_inputs_match_bash() {
    assert_matches_bash("approvals");
}

#[test]
fn dry_run_matches_bash() {
    assert_matches_bash("dry_run");
}

#[test]
fn session_inject_matches_bash() {
    assert_matches_bash("session_inject");
}

#[test]
fn inject_only_variants_match_bash() {
    assert_matches_bash("inject_only");
    assert_matches_bash("inject_only_dry");
    assert_matches_bash("inject_only_missing");
}

#[test]
fn disk_hold_matches_bash() {
    assert_matches_bash("disk_hold");
    assert_matches_bash("disk_hold_dry");
}

#[test]
fn usage_errors_match_bash() {
    assert_matches_bash("usage_unknown");
    assert_matches_bash("usage_help");
}

#[test]
fn utf8_locale_counts_characters() {
    let got = run_scenario("pass", true);
    assert!(got.stdout.contains("  action: pr_nudge (193 chars)\n"));
    assert!(got.stdout.contains("  action: input_yes (124 chars)\n"));
    let posix = run_scenario("pass", false);
    assert!(posix.stdout.contains("  action: pr_nudge (195 chars)\n"));
    assert_eq!(shell_prefix("Yes — x", 5, false), b"Yes \xe2");
    assert_eq!(shell_prefix("Yes — x", 5, true), "Yes —".as_bytes());
    assert_eq!(shell_len("—", true), 1);
    assert_eq!(shell_len("—", false), 3);
}

#[test]
fn second_pass_respects_pr_cooldown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = FleetConfig {
        kicker_base: dir.path().join("base"),
        tmp_dir: dir.path().to_path_buf(),
        monitored_sessions: vec!["pr_cool".to_string()],
        ..FleetConfig::default()
    };
    let state = screens();
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-21T18:30:05Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    assert_eq!(kick(&mut ctx, &[]).expect("first"), 0);
    clock.advance(Duration::from_secs(899));
    assert_eq!(kick(&mut ctx, &[]).expect("second"), 0);
    clock.advance(Duration::from_secs(1));
    assert_eq!(kick(&mut ctx, &[]).expect("third"), 0);
    let o = text(&out);
    assert_eq!(o.matches("action: pr_nudge (195 chars)").count(), 2);
    assert!(o.contains("skip pr_nudge (cooldown 1s remaining)"));
    let st: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("base/state/pr_cool.json")).expect("state"),
    )
    .expect("json");
    assert_eq!(st["last_pr_nudge_epoch"], 1_792_607_405 + 900);
    assert_eq!(st["last_action"], "pr_nudge");
}

#[test]
fn parser_follows_the_script_rules() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        parse_kick_args(&s(&["--inject", "sess", "hello there"])),
        Ok(KickOpts {
            inject_session: Some("sess".into()),
            inject_text: Some("hello there".into()),
            inject_only: true,
            ..KickOpts::default()
        })
    );
    // Text with a space, or starting with "Continue", is the text itself.
    assert_eq!(
        parse_kick_args(&s(&["--inject", "two words", "--session", "x"])),
        Ok(KickOpts {
            inject_text: Some("two words".into()),
            session_filter: Some("x".into()),
            ..KickOpts::default()
        })
    );
    assert_eq!(
        parse_kick_args(&s(&["--inject", "Continue.", "--dry-run"])),
        Ok(KickOpts {
            inject_text: Some("Continue.".into()),
            dry_run: true,
            ..KickOpts::default()
        })
    );
    // A single word followed by anything is SESSION TEXT (even a flag).
    assert_eq!(
        parse_kick_args(&s(&["--inject", "sess", "--dry-run"])),
        Ok(KickOpts {
            inject_session: Some("sess".into()),
            inject_text: Some("--dry-run".into()),
            inject_only: true,
            ..KickOpts::default()
        })
    );
    assert_eq!(
        parse_kick_args(&s(&["--once", "--dry-run"])),
        Ok(KickOpts {
            dry_run: true,
            ..KickOpts::default()
        })
    );
    assert_eq!(
        parse_kick_args(&s(&["--inject"])),
        Err(KickUsage { unknown: None })
    );
    assert_eq!(
        parse_kick_args(&s(&["--session", ""])),
        Err(KickUsage { unknown: None })
    );
    assert_eq!(
        parse_kick_args(&s(&["-h"])),
        Err(KickUsage { unknown: None })
    );
    assert_eq!(
        parse_kick_args(&s(&["x"])),
        Err(KickUsage {
            unknown: Some("x".into())
        })
    );
}

#[test]
fn state_epoch_parsing_mirrors_python_int() {
    use serde_json::json;
    assert_eq!(py_int_or_zero(None), 0);
    assert_eq!(py_int_or_zero(Some(&json!(null))), 0);
    assert_eq!(py_int_or_zero(Some(&json!(12))), 12);
    assert_eq!(py_int_or_zero(Some(&json!(12.9))), 12);
    assert_eq!(py_int_or_zero(Some(&json!(" 42 "))), 42);
    assert_eq!(py_int_or_zero(Some(&json!("abc"))), 0);
    assert_eq!(py_int_or_zero(Some(&json!(""))), 0);
    assert_eq!(py_int_or_zero(Some(&json!(true))), 1);
    assert_eq!(py_int_or_zero(Some(&json!([1]))), 0);
}
