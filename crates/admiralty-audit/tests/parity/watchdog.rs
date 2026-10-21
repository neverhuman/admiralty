//! Ported from tests/test_sc_audit_watchdog.py.

use regex::Regex;

use admiralty_audit::AuditConfig;
use admiralty_audit::ctx::Audit;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::py::{Dt, J, Obj};
use admiralty_audit::screen::{Screens, screen_state, tail_busy, tail_hash};
use admiralty_audit::watchdog::{ETA_GRACE_MIN, eta_overdue, managed_screens};

use crate::common::{at, hermetic_config, j};

const NOW: &str = "2026-10-18T15:12:00+00:00";
const CLAUDE_IDLE: &str = "  No GPU was dispatched.\n Cooked for 36s · done 3:19 PM\n\n ────\n >\n ────\n  ⏵⏵ auto mode on (shift+tab to cycle)\n";
const AGY_BUSY: &str =
    "> URGENT RED 1\n  Running command...\n >\n esc to cancel      Gemini 4 Argon · high\n";

fn now() -> Dt {
    at(NOW)
}

fn cfg() -> (tempfile::TempDir, AuditConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = hermetic_config(&tmp.path().to_string_lossy());
    (tmp, cfg)
}

/// Result of one `idle_watch` call: (watch entry, findings).
struct Watched {
    state: Obj,
    codes: Vec<String>,
    must_act: Vec<bool>,
    texts: Vec<String>,
}

fn idle_watch(screen: &str, project: &str, text: &str, prev: J) -> Watched {
    let (_t, cfg) = cfg();
    let owned = text.to_string();
    let host = FakeHost::new(now()).with_hardcopy(move |_, _| owned.clone());
    let mut audit = Audit::new(&cfg, &host);
    let scr: Screens = [(screen.to_string(), 1)].into_iter().collect();
    let res = audit
        .idle_watch(
            &scr,
            &[(screen.to_string(), project.to_string())],
            &prev,
            &mut Obj::new(),
            &now(),
        )
        .expect("idle_watch");
    let state = res
        .as_obj()
        .and_then(|o| o.get(screen))
        .and_then(|s| s.as_obj())
        .cloned()
        .unwrap_or_default();
    Watched {
        state,
        codes: audit.codes(),
        must_act: audit.findings.iter().map(|f| f.must_act).collect(),
        texts: audit.findings.iter().map(|f| f.text.clone()).collect(),
    }
}

/// `Watch.watch_stale(text)`.
fn watch_stale(text: &str) -> Watched {
    let prev = admiralty_audit::jobj! {"watch:fixture_worker" => admiralty_audit::jobj! {"hash" => tail_hash(text), "since" => "2026-10-18T14:32:00+00:00"}};
    let w = idle_watch("fixture_worker", "fixture-project", text, prev);
    assert_eq!(w.state.get("busy"), Some(&J::Bool(screen_state(text).busy)));
    w
}

fn b(w: &Watched, k: &str) -> bool {
    w.state.get(k).is_some_and(J::truthy)
}

fn composer(w: &Watched) -> String {
    w.state
        .get("composer_state")
        .and_then(J::as_str)
        .unwrap_or("")
        .to_string()
}

/// `Watch.assert_protected_finding(code)`.
fn assert_protected_finding(w: &Watched, code: &str) {
    assert_eq!(w.codes, vec![code.to_string()]);
    assert!(w.must_act[0]);
    let bad = Regex::new(r"(?i)alt\+m|file pointer \+ CR").expect("regex");
    assert!(!bad.is_match(&w.texts[0]), "{}", w.texts[0]);
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_newer_completed_status_does_not_revive_foreground_history
#[test]
fn test_idle_watch_newer_completed_status_does_not_revive_foreground_history() {
    let w = watch_stale(&format!("{AGY_BUSY}{CLAUDE_IDLE}"));
    assert!(!b(&w, "busy"));
    assert_eq!(composer(&w), "unknown");
    assert!(!b(&w, "idle_eligible"));
    assert_protected_finding(&w, "MANAGED_SCREEN_INPUT_UNVERIFIED");
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_quoted_and_fenced_foreground_history_requires_inspection
#[test]
fn test_idle_watch_quoted_and_fenced_foreground_history_requires_inspection() {
    for hint in ["esc to cancel", "esc to interrupt"] {
        for history in [
            format!("> {hint}"),
            format!("`{hint}`"),
            format!("\"{hint}\""),
            format!("```text\n{hint}\n```"),
            format!("~~~text\n{hint}\n~~~"),
        ] {
            let w = watch_stale(&format!("{history}\n >\n"));
            assert!(!b(&w, "busy"), "{history}");
            assert!(!b(&w, "idle_eligible"), "{history}");
            assert_eq!(composer(&w), "unknown", "{history}");
            assert_protected_finding(&w, "MANAGED_SCREEN_INPUT_UNVERIFIED");
        }
    }
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_newer_completed_status_preserves_current_draft
#[test]
fn test_idle_watch_newer_completed_status_preserves_current_draft() {
    let w = watch_stale(&format!(
        "{AGY_BUSY}{CLAUDE_IDLE}❯ preserve this unsent patch\n ? for shortcuts\n"
    ));
    assert!(!b(&w, "busy"));
    assert!(b(&w, "typed_input"));
    assert_eq!(composer(&w), "draft");
    assert!(!b(&w, "idle_eligible"));
    assert_protected_finding(&w, "MANAGED_SCREEN_DRAFT_PROTECTED");
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_newer_completed_status_preserves_queued_input
#[test]
fn test_idle_watch_newer_completed_status_preserves_queued_input() {
    let w = watch_stale(&format!(
        "{AGY_BUSY}{CLAUDE_IDLE}queued messages\n >\n ? for shortcuts\n"
    ));
    assert!(!b(&w, "busy"));
    assert!(b(&w, "queued_msgs"));
    assert_eq!(composer(&w), "empty_hint");
    assert!(!b(&w, "idle_eligible"));
    assert_protected_finding(&w, "MANAGED_SCREEN_INPUT_UNVERIFIED");
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_real_current_foreground_busy_still_reports_frozen
#[test]
fn test_idle_watch_real_current_foreground_busy_still_reports_frozen() {
    for text in [AGY_BUSY.to_string(), format!("{CLAUDE_IDLE}{AGY_BUSY}")] {
        let w = watch_stale(&text);
        assert!(b(&w, "busy"), "{text}");
        assert!(!b(&w, "idle_eligible"), "{text}");
        assert_protected_finding(&w, "MANAGED_SCREEN_FROZEN");
    }
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_real_running_background_footer_is_busy_in_both_parsers
#[test]
fn test_idle_watch_real_running_background_footer_is_busy_in_both_parsers() {
    let w = watch_stale(
        "Brewed for 39s · done 3:46 PM · 1 monitor still running\n >\n ? for shortcuts\n",
    );
    assert!(b(&w, "busy"));
    assert!(!b(&w, "idle_eligible"));
    assert_protected_finding(&w, "MANAGED_SCREEN_FROZEN");
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_idle_watch_completed_history_requires_positive_current_empty_hint
#[test]
fn test_idle_watch_completed_history_requires_positive_current_empty_hint() {
    let w = watch_stale(&format!("{AGY_BUSY}{CLAUDE_IDLE} >\n ? for shortcuts\n"));
    assert!(!b(&w, "busy"));
    assert_eq!(composer(&w), "empty_hint");
    assert!(b(&w, "idle_eligible"));
    assert_protected_finding(&w, "MANAGED_SCREEN_IDLE");
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_claude_footer_without_verified_empty_composer_requires_inspection
#[test]
fn test_claude_footer_without_verified_empty_composer_requires_inspection() {
    let prev = admiralty_audit::jobj! {"watch:jopedime_flywheel" => admiralty_audit::jobj! {"hash" => tail_hash(CLAUDE_IDLE), "since" => "2026-10-18T14:32:00+00:00"}};
    let w = idle_watch("jopedime_flywheel", "jopedime", CLAUDE_IDLE, prev);
    assert!(!b(&w, "busy"));
    assert_eq!(w.codes, vec!["MANAGED_SCREEN_INPUT_UNVERIFIED".to_string()]);
    assert_eq!(composer(&w), "unknown");
    assert!(!b(&w, "idle_eligible"));
    let bad = Regex::new(r"(?i)alt\+m|file pointer \+ CR").expect("regex");
    assert!(!bad.is_match(&w.texts[0]));
    assert!(w.must_act[0]);
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_busy_agy_not_idle_and_first_sighting_not_flagged
#[test]
fn test_busy_agy_not_idle_and_first_sighting_not_flagged() {
    let a = idle_watch("jopedime_agy", "jopedime", AGY_BUSY, J::Obj(Obj::new()));
    let c = idle_watch(
        "jopedime_flywheel",
        "jopedime",
        CLAUDE_IDLE,
        J::Obj(Obj::new()),
    );
    assert!(a.codes.is_empty());
    assert!(c.codes.is_empty());
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_claude_background_monitor_counts_as_busy
#[test]
fn test_claude_background_monitor_counts_as_busy() {
    let txt = " The five runs are in progress.\n Brewed for 39s · done 3:46 PM · 1 monitor still running\n >\n  auto mode on · 1 monitor · ↓ to manage\n";
    assert!(tail_busy(txt));
    assert!(!tail_busy(CLAUDE_IDLE));
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_positive_running_background_ui_footer_is_busy
#[test]
fn test_positive_running_background_ui_footer_is_busy() {
    for label in [
        "1 monitor still running",
        "2 monitors running",
        "1 background task running",
        "2 shells still running",
    ] {
        assert!(
            tail_busy(&format!(
                " Brewed for 1m 39s · done 3:46 PM · {label}\n >\n"
            )),
            "{label}"
        );
    }
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_zero_completed_and_plaintext_counts_do_not_suppress_idle
#[test]
fn test_zero_completed_and_plaintext_counts_do_not_suppress_idle() {
    for text in [
        "auto mode on · 0 monitors · ↓ to manage",
        "Brewed for 39s · done 3:46 PM · 0 monitors still running",
        "Brewed for 39s · done 3:46 PM · 0 background tasks running",
        "Brewed for 39s · done 3:46 PM · 1 monitor completed",
        "auto mode on · 1 monitor · ↓ to manage",
        "A previous task had 1 monitor running.",
        "1 monitor still running",
        "Documentation: 2 background tasks running",
    ] {
        assert!(!tail_busy(&format!("{text}\n >\n")), "{text}");
    }
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_quoted_or_fenced_busy_examples_are_not_live_ui
#[test]
fn test_quoted_or_fenced_busy_examples_are_not_live_ui() {
    let example = "Brewed for 39s · done 3:46 PM · 1 monitor still running";
    for text in [
        format!("> {example}"),
        format!("`{example}`"),
        format!("```text\n{example}\n```"),
        format!("~~~text\n{example}\n~~~"),
    ] {
        assert!(!tail_busy(&format!("{text}\n >\n")), "{text}");
    }
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_zero_monitor_unknown_composer_still_requires_action
#[test]
fn test_zero_monitor_unknown_composer_still_requires_action() {
    let text = format!("{CLAUDE_IDLE}auto mode on · 0 monitors · ↓ to manage\n");
    let prev = admiralty_audit::jobj! {"watch:fixture_worker" => admiralty_audit::jobj! {"hash" => tail_hash(&text), "since" => "2026-10-18T14:32:00+00:00"}};
    let w = idle_watch("fixture_worker", "fixture-project", &text, prev);
    assert!(!b(&w, "busy"));
    assert_eq!(w.codes, vec!["MANAGED_SCREEN_INPUT_UNVERIFIED".to_string()]);
    assert!(!b(&w, "idle_eligible"));
    assert_eq!(composer(&w), "unknown");
    assert!(w.must_act[0]);
    let bad = Regex::new(r"(?i)alt\+m|file pointer \+ CR").expect("regex");
    assert!(!bad.is_match(&w.texts[0]));
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_newer_completed_or_zero_footer_overrides_old_running_footer
#[test]
fn test_newer_completed_or_zero_footer_overrides_old_running_footer() {
    let prior = "Brewed for 39s · done 3:46 PM · 1 monitor still running\n";
    for current in [
        "Cooked for 2s · done 3:48 PM\n",
        "auto mode on · 0 monitors · ↓ to manage\n",
        "Cooked for 2s · done 3:48 PM · 1 monitor completed\n",
    ] {
        assert!(!tail_busy(&format!("{prior}{current}>\n")), "{current}");
    }
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_old_running_footer_outside_raw_tail_is_not_revived_by_filtering
#[test]
fn test_old_running_footer_outside_raw_tail_is_not_revived_by_filtering() {
    let prior = "Brewed for 39s · done 3:46 PM · 1 monitor still running\n";
    let quoted: String = (0..20)
        .map(|i| format!("> archived quotation line {i}\n"))
        .collect();
    let fenced = format!(
        "```text\n{}```\n",
        (0..20)
            .map(|i| format!("archived example line {i}\n"))
            .collect::<String>()
    );
    for later in [quoted, fenced] {
        assert!(!tail_busy(&format!("{prior}{later}>\n")));
    }
    assert!(tail_busy(&format!("{prior}>\n")));
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_foreground_examples_are_not_live_ui
#[test]
fn test_foreground_examples_are_not_live_ui() {
    let example = "esc to cancel      Gemini 4 Argon · high";
    for text in [
        format!("> {example}"),
        format!("`{example}`"),
        format!("```text\n{example}\n```"),
        format!("~~~text\n{example}\n~~~"),
    ] {
        assert!(!tail_busy(&format!("{text}\n >\n")), "{text}");
    }
    assert!(tail_busy(AGY_BUSY));
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_newer_completed_status_overrides_old_foreground_busy_hint
#[test]
fn test_newer_completed_status_overrides_old_foreground_busy_hint() {
    assert!(!tail_busy(&format!("{AGY_BUSY}{CLAUDE_IDLE}")));
    assert!(tail_busy(&format!("{CLAUDE_IDLE}{AGY_BUSY}")));
}

/// Ported from tests/test_sc_audit_watchdog.py::Watch::test_parked_and_positional_lane_lines_are_managed
#[test]
fn test_parked_and_positional_lane_lines_are_managed() {
    let scs = j(
        r#"{"captains": {"jopedime": {"state": "jopedime_flywheel working on spec", "pr": "1061448.jopedime_flywheel ETA 15:05Z"}}}"#,
    );
    let mut lanes = Obj::new();
    lanes.set("jopedime_agy", "LANE | 2830191.jopedime_agy | X | active");
    let cfg = AuditConfig::with_home("/home/ubuntu");
    let captains: Vec<(String, String)> = cfg
        .projects
        .iter()
        .map(|p| (p.name.clone(), p.captain.clone()))
        .collect();
    let m = managed_screens(&captains, &scs, &[("jopedime".to_string(), lanes)]).expect("managed");
    let get = |n: &str| m.iter().find(|(k, _)| k == n).map(|(_, p)| p.clone());
    assert_eq!(get("jopedime_flywheel").as_deref(), Some("jopedime"));
    assert_eq!(get("jopedime_agy").as_deref(), Some("jopedime"));
    assert_eq!(get("super_captain"), None);
}

fn audit_with<'a>(cfg: &'a AuditConfig, host: &'a FakeHost) -> Audit<'a> {
    Audit::new(cfg, host)
}

/// Ported from tests/test_sc_audit_watchdog.py::Eta::test_passed_eta_in_live_status_is_red
#[test]
fn test_passed_eta_in_live_status_is_red() {
    let (_t, cfg) = cfg();
    let host = FakeHost::new(now());
    let mut audit = audit_with(&cfg, &host);
    let scs = j(
        r#"{"captains": {"jopedime": {"state": "jopedime_flywheel working on AUTH-GAP-01/02 closure spec (ETA 15:05Z)", "pr": "merge ETA 14:55Z"}},
        "decisions": ["old thing ETA 09:00Z"]}"#,
    );
    audit.check_eta_overdue(&scs, &now()).expect("eta");
    assert_eq!(audit.codes(), vec!["OWED_ETA_OVERDUE".to_string()]);
    assert_eq!(audit.findings[0].scope, "jopedime");
    assert!(audit.findings[0].text.contains("2 ETA(s)"));
}

/// Ported from tests/test_sc_audit_watchdog.py::Eta::test_future_mdt_and_merged_not_flagged
#[test]
fn test_future_mdt_and_merged_not_flagged() {
    let (_t, cfg) = cfg();
    let host = FakeHost::new(now());
    let mut audit = audit_with(&cfg, &host);
    let scs = j(
        r#"{"captains": {"dope": {"pr": "PR #272 MERGED (ETA 15:05Z)", "state": "next PR ETA 09:40 AM MDT"}}}"#,
    );
    audit.check_eta_overdue(&scs, &now()).expect("eta");
    assert_eq!(audit.codes(), Vec::<String>::new());
}

/// Ported from tests/test_sc_audit_watchdog.py::Eta::test_mdt_conversion
#[test]
fn test_mdt_conversion() {
    let hits = eta_overdue(
        &j(r#"{"captains": {"x": {"s": "spec due 09:05 AM MDT"}}}"#),
        &now(),
        ETA_GRACE_MIN,
    )
    .expect("eta");
    assert_eq!(hits[0].1, 7);
}

const REDRIVE_LOG: &str = "## 2026-10-18T14:49:30Z (08:49 AM MDT) — ACK\n- **WATCH:** Verified `1061448.jopedime_flywheel` completed X at 14:32Z and re-drove it to write SPEC.json (ETA 15:05Z).\n";

/// Ported from tests/test_sc_audit_watchdog.py::Redrive::test_claim_without_terminal_input_is_red
#[test]
fn test_claim_without_terminal_input_is_red() {
    let (_t, cfg) = cfg();
    let host = FakeHost::new(now());
    let mut audit = audit_with(&cfg, &host);
    let li = |_: &str| Some(at("2026-10-18T14:31:31+00:00"));
    let scr: Screens = [("jopedime_flywheel".to_string(), 1)].into_iter().collect();
    audit.check_redrive_delivered(REDRIVE_LOG, &scr, &now(), Some(&li));
    assert_eq!(audit.codes(), vec!["REDRIVE_NOT_DELIVERED".to_string()]);
}

/// Ported from tests/test_sc_audit_watchdog.py::Redrive::test_claim_with_input_ok
#[test]
fn test_claim_with_input_ok() {
    let (_t, cfg) = cfg();
    let host = FakeHost::new(now());
    let mut audit = audit_with(&cfg, &host);
    let li = |_: &str| Some(at("2026-10-18T14:48:00+00:00"));
    let scr: Screens = [("jopedime_flywheel".to_string(), 1)].into_iter().collect();
    audit.check_redrive_delivered(REDRIVE_LOG, &scr, &now(), Some(&li));
    assert_eq!(audit.codes(), Vec::<String>::new());
}

/// Ported from tests/test_sc_audit_watchdog.py::PushAndClaims::test_no_push_30m_with_open_work
#[test]
fn test_no_push_30m_with_open_work() {
    let (_t, cfg) = cfg();
    let host = FakeHost::new(now());
    let mut audit = audit_with(&cfg, &host);
    let mut rep = match j(
        r#"{"projects": {"jarambit": {"done_recomputed": 6, "total": 37}, "admiralty": {"done_recomputed": 32, "total": 32, "open_prs": []}}}"#,
    ) {
        J::Obj(o) => o,
        _ => Obj::new(),
    };
    let push = |_: &str| Some(at("2026-10-18T14:30:00+00:00"));
    audit
        .check_no_push(&mut rep, &now(), Some(&push))
        .expect("no push");
    assert_eq!(audit.codes(), vec!["NO_PUSH_30M".to_string()]);
    assert_eq!(audit.findings[0].scope, "jarambit");
}

/// Ported from tests/test_sc_audit_watchdog.py::PushAndClaims::test_state_text_and_idle_claims
#[test]
fn test_state_text_and_idle_claims() {
    let (_t, cfg) = cfg();
    let host = FakeHost::new(now());
    let mut audit = audit_with(&cfg, &host);
    let rep = match j(r#"{"projects": {"jopedime": {"done_recomputed": 0, "total": 66}}}"#) {
        J::Obj(o) => o,
        _ => Obj::new(),
    };
    let scs = j(
        r#"{"captains": {"jopedime": {"state": "P1-A TOP PRIORITY — 66/66 done on main", "done": 0, "workers_idle": 0}}}"#,
    );
    let watch =
        j(r#"{"jopedime_flywheel": {"project": "jopedime", "busy": false, "unchanged_min": 40}}"#);
    audit.check_sc_claims(&rep, &scs, &watch).expect("claims");
    let mut codes = audit.codes();
    codes.sort();
    assert_eq!(
        codes,
        vec![
            "SC_IDLE_CLAIM_FALSE".to_string(),
            "SC_STATE_TEXT_OVERCLAIM".to_string()
        ]
    );
}
