use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;
use crate::FleetConfig;
use crate::testing::{FakeClock, ScreenState, assert_never_attached, screen_runner, text};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("read dir").flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            std::fs::create_dir_all(&dst).expect("mkdir");
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

fn walk(root: &Path) -> Vec<(String, String)> {
    fn rec(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                rec(root, &p, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .expect("under root")
                    .display()
                    .to_string();
                out.push((rel, std::fs::read_to_string(&p).expect("utf-8 file")));
            }
        }
    }
    let mut out = Vec::new();
    rec(root, root, &mut out);
    out.sort();
    out
}

/// Prefix every `/home/ubuntu` and `/tmp/` path with the sandbox root in one
/// pass (the same rewrite the bash harness applied to the scripts).
fn sandbox_json(text: &str, sb: &str) -> String {
    let rx = regex::Regex::new(r"/home/ubuntu|/tmp/").expect("regex");
    rx.replace_all(text, |c: &regex::Captures| format!("{sb}{}", &c[0]))
        .into_owned()
}

fn screens_for(dir: &Path) -> Rc<ScreenState> {
    let state = ScreenState::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let sid = e.file_name().to_string_lossy().into_owned();
            let mut frames: Vec<(u32, String)> = std::fs::read_dir(e.path())
                .expect("frames")
                .flatten()
                .map(|f| {
                    let n: u32 = f
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.parse().ok())
                        .expect("numbered frame");
                    (n, std::fs::read_to_string(f.path()).expect("frame"))
                })
                .collect();
            frames.sort();
            let refs: Vec<&str> = frames.iter().map(|(_, f)| f.as_str()).collect();
            state.set_screen(&sid, &refs);
        }
    }
    state
}

/// Run a shipped plan (`plans/<name>.json`) in a sandbox and compare stdout,
/// stderr, the screen/sleep event sequence and the resulting file tree with
/// what the original `tools/<name>.sh` produced in the same sandbox (fake
/// `screen`, `date` frozen at 2026-10-18T04:05:06Z, `sleep` logged).
fn assert_plan_matches_script(name: &str) {
    let fx = crate_dir().join("tests/fixtures/deliver").join(name);
    let dir = tempfile::tempdir().expect("tempdir");
    let sb = dir.path().join("sb");
    std::fs::create_dir_all(&sb).expect("sandbox");
    // /tmp exists on the original host even when no seed file occupies it.
    std::fs::create_dir_all(sb.join("tmp")).expect("sandbox tmp");
    std::fs::create_dir_all(sb.join("home/ubuntu/xbabe2_kicker/captains-20261017"))
        .expect("original host captain directory");
    if fx.join("seed").is_dir() {
        copy_tree(&fx.join("seed"), &sb);
    } else {
        assert_eq!(
            std::fs::read_to_string(fx.join("empty-seed.txt")).expect("explicit empty seed"),
            "empty\n",
            "a missing nonempty fixture must fail"
        );
    }
    let sb_str = sb.display().to_string();
    let plan_text = std::fs::read_to_string(crate_dir().join("plans").join(format!("{name}.json")))
        .expect("plan file");
    let plan: DeliverPlan =
        serde_json::from_str(&sandbox_json(&plan_text, &sb_str)).expect("plan parses");
    plan.validate().expect("plan validates");

    let state = screens_for(&fx.join("screens"));
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-18T04:05:06Z").frozen_logging_into(&state);
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        deliver(&mut ctx, &plan).expect("deliver runs")
    };
    assert_never_attached(&runner);
    let norm = |s: String| s.replace(&sb_str, "@SB@");
    let read = |f: &str| std::fs::read_to_string(fx.join("expected").join(f)).expect("expected");
    assert_eq!(code.to_string(), read("exit"), "{name}: exit");
    assert_eq!(norm(text(&out)), read("stdout"), "{name}: stdout");
    assert_eq!(norm(text(&err)), read("stderr"), "{name}: stderr");
    let events: String = state
        .events()
        .into_iter()
        .map(|e| format!("{e}\n"))
        .collect();
    assert_eq!(norm(events), read("events"), "{name}: screen/sleep events");
    let got_tree: Vec<(String, String)> =
        walk(&sb).into_iter().map(|(p, c)| (p, norm(c))).collect();
    assert_eq!(got_tree, walk(&fx.join("expected_tree")), "{name}: files");
}

/// Ported behavior of tools/send_add1.sh (prepare texts, 45 passes, `ls`).
#[test]
fn plan_send_add1_matches_script() {
    assert_plan_matches_script("send_add1");
}

/// Ported behavior of tools/send_add2.sh (idle gate, retries, locked fallback).
#[test]
fn plan_send_add2_matches_script() {
    assert_plan_matches_script("send_add2");
}

/// Ported behavior of tools/send_add2b.sh.
#[test]
fn plan_send_add2b_matches_script() {
    assert_plan_matches_script("send_add2b");
}

/// Ported behavior of tools/send_compute.sh (fresh hardcopy gate).
#[test]
fn plan_send_compute_matches_script() {
    assert_plan_matches_script("send_compute");
}

/// Ported behavior of tools/send_notice2.sh (pre-log, compose, reset).
#[test]
fn plan_send_notice2_matches_script() {
    assert_plan_matches_script("send_notice2");
}

#[test]
fn operator_doc_embeds_the_shipped_example_plans() {
    let doc = std::fs::read_to_string(crate_dir().join("../../docs/operators/fleet-ops.md"))
        .expect("docs/operators/fleet-ops.md");
    for name in ["send_add2", "send_notice2"] {
        let plan = std::fs::read_to_string(crate_dir().join("plans").join(format!("{name}.json")))
            .expect("plan");
        assert!(
            doc.contains(&format!("```json\n{plan}```")),
            "fleet-ops.md must embed plans/{name}.json verbatim"
        );
    }
}

#[test]
fn idle_heuristics() {
    let idle = b"done\n\n> \n  ? for shortcuts\n";
    assert!(prompt_is_idle(idle, true));
    assert!(prompt_is_idle(b">\n? for shortcuts", true));
    assert!(prompt_is_idle(b">   \n? for shortcuts\n", true));
    // Busy: cancel hint, draft text, no prompt, or missing footer.
    assert!(!prompt_is_idle(
        b"> \n? for shortcuts esc to cancel\n",
        true
    ));
    assert!(!prompt_is_idle(b"> \n> draft\n? for shortcuts\n", true));
    assert!(!prompt_is_idle(b"? for shortcuts\n", true));
    assert!(!prompt_is_idle(b"", true));
    assert!(!prompt_is_idle(b"> \naccept edits on\n", true));
    // Prompt-only heuristic: footer not required, cancel still blocks.
    assert!(prompt_is_idle(b"> \naccept edits on\n", false));
    assert!(!prompt_is_idle(b"> \nesc to cancel\n", false));
    // Tabs and CRs are not squeezed (only spaces are, like `tr -d " "`).
    assert!(!prompt_is_idle(b">\t\n? for shortcuts\n", true));
    assert!(!prompt_is_idle(b">\r\n? for shortcuts\n", true));
}

fn plan_json(extra: &str) -> String {
    format!(
        r#"{{"passes": 2, "interval_secs": 15,
            "idle": {{"hardcopy_prefix": "@D@/hc_", "settle_ms": 300{extra}}},
            "message_file": "@D@/msg_{{key}}.txt", "sent_marker": "@D@/sent_{{key}}",
            "sent_echo": "sent {{key}} {{message_basename}}",
            "fallback": {{"template": "ptr {{screen}} {{message}}", "echo": "logged {{key}} -> {{coord_log}}",
                          "logged_marker": "@D@/logged_{{key}}", "lock_wait_secs": 2}},
            "done_echo": "DONE",
            "targets": [{{"key": "joil", "coord_log": "@D@/COORD.md", "lock": "@D@/coord.lock"}}]}}"#
    )
}

#[test]
fn prompt_only_heuristic_and_config_captain_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path().display().to_string();
    std::fs::write(dir.path().join("msg_joil.txt"), "hello\n").expect("msg");
    let plan: DeliverPlan = serde_json::from_str(
        &plan_json(r#", "require_shortcuts_footer": false"#).replace("@D@", &d),
    )
    .expect("plan");
    let state = ScreenState::new();
    // Screen id comes from the config's captain table.
    state.set_screen("2708475.captain_joil", &["> \n  accept edits on\n"]);
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-18T04:05:06Z");
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    assert_eq!(deliver(&mut ctx, &plan).expect("deliver"), 0);
    assert_eq!(text(&out), "sent joil msg_joil.txt\nDONE\n");
    assert!(dir.path().join("sent_joil").exists());
    assert_eq!(
        state.events(),
        [
            "hardcopy 2708475.captain_joil".to_string(),
            format!("readbuf 2708475.captain_joil {d}/msg_joil.txt"),
            "paste 2708475.captain_joil".to_string(),
            "stuff 2708475.captain_joil \\r".to_string(),
        ]
    );
    // One pass delivered, one more pass saw nothing left: sleeps are
    // settle, paste, interval (left was true), then break.
    assert_eq!(
        clock.sleeps(),
        [
            Duration::from_millis(300),
            Duration::from_millis(1000),
            Duration::from_secs(15)
        ]
    );
    assert_never_attached(&runner);
}

#[test]
fn fallback_waits_for_lock_then_gives_up_like_flock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path().display().to_string();
    std::fs::write(dir.path().join("msg_joil.txt"), "notice body\n\n").expect("msg");
    let plan: DeliverPlan = serde_json::from_str(&plan_json("").replace("@D@", &d)).expect("plan");
    let state = ScreenState::new();
    state.set_screen("2708475.captain_joil", &["busy\nesc to cancel\n"]);
    let runner = screen_runner(&state);
    let cfg = FleetConfig::default();

    // Another process holds the coordination lock for the whole wait.
    let lock_path = dir.path().join("coord.lock");
    let holder = std::fs::File::create(&lock_path).expect("lock file");
    rustix::fs::flock(&holder, rustix::fs::FlockOperation::LockExclusive).expect("hold lock");
    let clock = FakeClock::at("2026-10-18T04:05:06Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(deliver(&mut ctx, &plan).expect("deliver"), 0);
    }
    assert!(
        !dir.path().join("COORD.md").exists(),
        "nothing appended without the lock"
    );
    assert!(text(&err).contains("could not lock"));
    // Like the script, the marker and echo still happen.
    assert!(dir.path().join("logged_joil").exists());
    assert_eq!(text(&out), format!("logged joil -> {d}/COORD.md\nDONE\n"));
    let lock_polls = clock
        .sleeps()
        .iter()
        .filter(|s| **s == Duration::from_millis(100))
        .count();
    assert_eq!(lock_polls, 20, "2 s wait polled every 100 ms");

    // Released: the pointer is appended (template + newline, message with
    // trailing newlines stripped like `$(cat)`).
    rustix::fs::flock(&holder, rustix::fs::FlockOperation::Unlock).expect("unlock");
    std::fs::remove_file(dir.path().join("logged_joil")).expect("reset marker");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    assert_eq!(deliver(&mut ctx, &plan).expect("deliver"), 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("COORD.md")).expect("log"),
        "ptr 2708475.captain_joil notice body\n"
    );
    assert!(err.is_empty());
}

#[test]
fn append_locked_without_lock_creates_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = FakeClock::at("2026-10-18T04:05:06Z");
    let log = dir.path().join("log.md");
    let r = append_locked(
        &clock,
        &log,
        None,
        Duration::ZERO,
        Duration::from_millis(100),
        b"a\n",
    )
    .expect("append");
    assert_eq!(r, AppendOutcome::Appended);
    append_locked(
        &clock,
        &log,
        Some(&dir.path().join("new.lock")),
        Duration::ZERO,
        Duration::from_millis(100),
        b"b\n",
    )
    .expect("append");
    assert_eq!(std::fs::read_to_string(&log).expect("log"), "a\nb\n");
    assert!(
        dir.path().join("new.lock").exists(),
        "lock file created like flock(1)"
    );
    assert!(clock.sleeps().is_empty());
}

#[test]
fn plan_validation_rejects_bad_plans() {
    let ok = plan_json("").replace("@D@", "/x");
    let plan: DeliverPlan = serde_json::from_str(&ok).expect("plan");
    assert!(plan.validate().is_ok());
    let bad_placeholder = ok.replace("sent {key}", "sent {nope}");
    let plan: DeliverPlan = serde_json::from_str(&bad_placeholder).expect("plan");
    assert!(plan.validate().is_err());
    let message_in_path = ok.replace("/x/sent_{key}", "/x/{message}");
    let plan: DeliverPlan = serde_json::from_str(&message_in_path).expect("plan");
    assert!(plan.validate().is_err());
    assert!(serde_json::from_str::<DeliverPlan>(&ok.replace("\"passes\"", "\"pases\"")).is_err());
    let dup = ok.replace(
        r#""targets": [{"key": "joil""#,
        r#""targets": [{"key": "joil"}, {"key": "joil""#,
    );
    let plan: DeliverPlan = serde_json::from_str(&dup).expect("plan");
    assert!(plan.validate().is_err());
    let unknown = ok.replace(r#"{"key": "joil""#, r#"{"key": "nobody""#);
    let plan: DeliverPlan = serde_json::from_str(&unknown).expect("plan");
    assert!(plan.resolve(&FleetConfig::default().captains).is_err());
}

#[test]
fn template_rendering() {
    let mut vars = |n: &str| (n == "a").then(|| b"A".to_vec());
    assert_eq!(render("x{a}y{{z}}", &mut vars).expect("render"), b"xAy{z}");
    assert!(render("{b}", &mut vars).is_err());
    assert!(render("{a", &mut vars).is_err());
    assert!(render("a}", &mut vars).is_err());
}

#[test]
fn shipped_plans_resolve_against_production_paths() {
    for name in [
        "send_add1",
        "send_add2",
        "send_add2b",
        "send_compute",
        "send_notice2",
    ] {
        let plan = DeliverPlan::load(&crate_dir().join("plans").join(format!("{name}.json")))
            .expect("shipped plan loads");
        let targets = plan
            .resolve(&FleetConfig::default().captains)
            .expect("shipped plan resolves");
        assert_eq!(
            targets.len(),
            if name == "send_add2b" { 2 } else { 4 },
            "{name}"
        );
        for t in targets {
            assert!(
                t.message_file
                    .starts_with("/home/ubuntu/xbabe2_kicker/captains-20261017/")
            );
            assert!(t.sent_marker.starts_with("/tmp/"));
        }
    }
}

fn send_sc_plan(dir: &Path) -> DeliverPlan {
    let text = std::fs::read_to_string(crate_dir().join("plans/send_sc.json")).expect("plan file");
    let sb = dir.display().to_string();
    let plan: DeliverPlan = serde_json::from_str(&sandbox_json(&text, &sb)).expect("plan parses");
    plan.validate().expect("plan validates");
    std::fs::create_dir_all(dir.join("tmp")).expect("tmp");
    let orders = dir.join("home/ubuntu/xbabe2_kicker/captains-20261017");
    std::fs::create_dir_all(&orders).expect("orders dir");
    std::fs::write(orders.join("sc_paste_orders.txt"), "orders\n").expect("orders");
    plan
}

/// Ported behavior of tools/send_sc.sh: one target, fresh hardcopy, paste
/// as soon as the prompt is idle and stop.
#[test]
fn plan_send_sc_pastes_once_the_super_captain_is_idle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plan = send_sc_plan(dir.path());
    // A stale marker from an earlier run must not suppress delivery.
    std::fs::write(dir.path().join("tmp/sent_sc_orders"), "").expect("stale marker");
    let state = ScreenState::new();
    state.set_screen(
        "4060655.super_captain",
        &["working\nesc to cancel\n", "done\n> \n  ? for shortcuts\n"],
    );
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-18T04:05:06Z");
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(deliver(&mut ctx, &plan).expect("deliver"), 0);
    }
    assert_never_attached(&runner);
    // 04:05:06Z + 0.5 s busy check + 10 s + 0.5 s idle check + 1 s paste settle
    // = 04:05:18Z, which is 22:05:18 in Denver (MDT, UTC-6).
    assert_eq!(text(&out), "SC orders pasted 22:05:18 MDT\n");
    assert!(err.is_empty(), "{}", text(&err));
    let sb = dir.path().display();
    assert_eq!(
        state.events(),
        [
            "hardcopy 4060655.super_captain".to_string(),
            "hardcopy 4060655.super_captain".to_string(),
            format!(
                "readbuf 4060655.super_captain {sb}/home/ubuntu/xbabe2_kicker/captains-20261017/sc_paste_orders.txt"
            ),
            "paste 4060655.super_captain".to_string(),
            "stuff 4060655.super_captain \\r".to_string(),
        ]
    );
}

#[test]
fn plan_send_sc_reports_a_busy_super_captain_after_twenty_minutes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plan = send_sc_plan(dir.path());
    let state = ScreenState::new();
    state.set_screen("4060655.super_captain", &["working\nesc to cancel\n"]);
    let runner = screen_runner(&state);
    let clock = FakeClock::at("2026-10-18T04:05:06Z");
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    {
        let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
        assert_eq!(deliver(&mut ctx, &plan).expect("deliver"), 0);
    }
    assert_eq!(text(&out), "SC busy 20 min; not pasted\n");
    assert_eq!(
        state.events().len(),
        120,
        "one fresh hardcopy per pass, no paste"
    );
    let waited: Duration = clock.sleeps().iter().sum();
    assert_eq!(
        waited,
        Duration::from_secs(120 * 10) + Duration::from_millis(120 * 500)
    );
}
