use std::path::{Path, PathBuf};

use super::*;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/classify")
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixture_dir())
        .expect("fixture dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".txt").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

fn classify_fixture(name: &str) -> (String, String) {
    let dir = fixture_dir();
    let txt = dir.join(format!("{name}.txt"));
    let session = std::fs::read_to_string(dir.join(format!("{name}.session"))).unwrap_or_default();
    let got = classify_file(&txt, &session).expect("fixture exists");
    let expected =
        std::fs::read_to_string(dir.join(format!("{name}.json"))).expect("expected output");
    (
        format!(
            "{}\n",
            got.to_pretty_json().expect("classification serializes")
        ),
        expected,
    )
}

/// Every fixture's JSON (captured from `tools/bin/classify_screen.py` before
/// it was retired) must be reproduced byte-for-byte.
#[test]
fn fixtures_match_python_output_byte_for_byte() {
    let names = fixture_names();
    assert!(
        names.len() >= 30,
        "expected the full fixture set, got {}",
        names.len()
    );
    for name in names {
        let (got, expected) = classify_fixture(&name);
        assert_eq!(got, expected, "fixture {name}");
    }
}

fn state_of(name: &str) -> Classification {
    let dir = fixture_dir();
    let session = std::fs::read_to_string(dir.join(format!("{name}.session"))).unwrap_or_default();
    classify_file(&dir.join(format!("{name}.txt")), &session).expect("fixture")
}

#[test]
fn busy_fixtures_are_working() {
    for name in [
        "codex_working",
        "background_terminal",
        "claude_workflow",
        "error_masked_by_work",
        "fresh_idle_busy",
        "literal_backslash_d_busy",
        "working_mid_flight_pr",
    ] {
        let c = state_of(name);
        assert_eq!(c.state, "working", "{name}");
        assert!(c.approval_hint.is_none());
    }
    assert_eq!(state_of("codex_working").confidence, 0.91);
    assert_eq!(state_of("working_mid_flight_pr").confidence, 0.632);
}

#[test]
fn idle_fixtures_are_idle() {
    for (name, kind, conf) in [
        ("codex_idle", "codex", 0.76),
        ("grok_idle_footer", "grok", 0.74),
        ("claude_grant_wait", "claude", 0.78),
        ("claude_grant_hint_only", "unknown", 0.5),
        ("fresh_idle_override", "claude", 0.5),
    ] {
        let c = state_of(name);
        assert_eq!(
            (c.kind.as_str(), c.state.as_str()),
            (kind, "idle"),
            "{name}"
        );
        assert_eq!(c.confidence, conf, "{name}");
    }
    let c = state_of("fresh_idle_override");
    assert!(c.signals.contains(&"fresh_idle_chrome".to_string()));
    assert!(!c.signals.contains(&"actively_working".to_string()));
}

#[test]
fn approval_fixtures_choose_the_python_hint() {
    for (name, hint) in [
        ("grok_plan_approval", "a"),
        ("claude_approval", "2"),
        ("codex_allow_always", "2"),
        ("codex_yn_approval", "y"),
        ("unknown_always_allow", "always"),
        ("press_enter", "2"),
        ("approval_near", "y"),
    ] {
        let c = state_of(name);
        assert_eq!(c.state, "needs_approval", "{name}");
        assert_eq!(c.approval_hint.as_deref(), Some(hint), "{name}");
    }
    assert!(
        state_of("approval_near")
            .signals
            .contains(&"approval_near:waiting on plan approval".to_string())
    );
    // A mode banner in the last 15 lines suppresses the wider-window cue.
    assert_eq!(state_of("approval_near_masked").state, "idle");
    assert_eq!(
        state_of("codex_yn_approval")
            .signals
            .last()
            .map(String::as_str),
        Some("approval:\\(y/n\\)")
    );
}

#[test]
fn survey_quota_error_and_input_fixtures() {
    // The survey and quota screens have no dedicated branch: they fall
    // through exactly like the Python did.
    assert_eq!(state_of("survey_prompt").state, "unknown");
    assert_eq!(state_of("survey_prompt").confidence, 0.35);
    assert_eq!(state_of("quota_limit").state, "unknown");
    assert_eq!(state_of("quota_limit").kind, "codex");
    assert_eq!(state_of("error_fatal").state, "error");
    assert_eq!(state_of("needs_input_question").state, "needs_input");
    assert_eq!(state_of("needs_input_waiting").state, "needs_input");
    assert_eq!(state_of("pr_blocked_ci").state, "pr_blocked");
    assert_eq!(state_of("hist_pr_idle").state, "pr_blocked");
    assert_eq!(state_of("working_ci_watch").state, "pr_blocked");
    assert_eq!(state_of("empty").state, "unknown");
    assert_eq!(state_of("empty").confidence, 0.29);
}

#[test]
fn pr_refs_are_trimmed_deduped_and_capped() {
    let c = state_of("pr_refs_many");
    assert_eq!(
        c.pr_refs,
        [
            "https://github.com/a/b/pull/1",
            "https://github.com/a/b/pull/2",
            "pull request 4",
            "pull request #8",
            "PR #3",
            "pr 5",
            "PR#6",
            "Pr 7",
        ]
    );
    assert!(c.summary.contains(
        "PRs: https://github.com/a/b/pull/1, https://github.com/a/b/pull/2, pull request 4."
    ));
}

#[test]
fn clean_lines_strips_screen_tags_and_controls() {
    let lines = clean_lines("[ SCREEN: w0 ]a\x07b\x1b[0m  \r\n[\x1fSCREEN:x]c\x00d\x0be");
    assert_eq!(lines, ["ab[0m", "cd", "e"]);
}

#[test]
fn kind_ties_prefer_python_dict_order() {
    // grok and codex both 0.35 from the session name alone -> grok wins.
    let (kind, conf, _) = detect_kind("", "grok_codex");
    assert_eq!((kind.as_str(), conf), ("grok", 0.35));
    let (kind, conf, _) = detect_kind("", "codex_claude");
    assert_eq!((kind.as_str(), conf), ("codex", 0.35));
    let (kind, conf, _) = detect_kind("nothing", "");
    assert_eq!((kind.as_str(), conf), ("unknown", 0.2));
    let (kind, conf, _) = detect_kind("Grok 4 always-approve plan approval a:approve", "grok");
    assert_eq!((kind.as_str(), conf), ("grok", 1.0));
}

#[test]
fn stem_matches_pathlib() {
    assert_eq!(py_stem(Path::new("/x/sess.txt")), "sess");
    assert_eq!(py_stem(Path::new("/x/a.b.txt")), "a.b");
    assert_eq!(py_stem(Path::new("/x/.hidden")), ".hidden");
    assert_eq!(py_stem(Path::new("/x/trailing.")), "trailing.");
    assert_eq!(py_stem(Path::new("/x/plain")), "plain");
}

#[test]
fn missing_snapshot_error_document() {
    let err = classify_file(Path::new("snaps/nope.txt"), "s").expect_err("missing");
    assert_eq!(
        err,
        r#"{"error": "missing snap: snaps/nope.txt", "session": "s"}"#
    );
}

#[test]
fn invalid_utf8_is_replaced_like_python() {
    let c = state_of("invalid_utf8");
    assert!(c.tail.contains('\u{fffd}'));
    assert_eq!(c.session, "invalid_utf8");
}
