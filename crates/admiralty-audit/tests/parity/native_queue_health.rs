//! Ported from tests/test_native_queue_health.py (synthetic/mocked draft,
//! queue and endpoint regression controls).

use std::collections::BTreeSet;
use std::path::PathBuf;

use regex::Regex;
use rusqlite::Connection;

use admiralty_audit::ctx::Audit;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::py::{self, Dt, J, Obj};
use admiralty_audit::queue_health::{read_queue_health, require_live_target};
use admiralty_audit::screen::{Screens, screen_state, tail_hash};

use crate::common::{at, hermetic_config, j};

/// Ported from tests/test_native_queue_health.py::ScreenRegression::test_codex_running_is_not_idle
#[test]
fn test_codex_running_is_not_idle() {
    let state = screen_state("Working (4s • esc to interrupt)\n›\n? for shortcuts");
    assert!(state.busy);
    assert!(!state.idle_prompt);
}

/// Ported from tests/test_native_queue_health.py::ScreenRegression::test_codex_pasted_draft_is_protected_even_with_lossy_prompt
#[test]
fn test_codex_pasted_draft_is_protected_even_with_lossy_prompt() {
    let state = screen_state(
        "Working (4s • esc to interrupt)\n: [Pasted Content 1024 chars]\n  tab to queue message",
    );
    assert!(state.typed_input);
    assert!(
        !state.queued_msgs,
        "unsent composer is distinct from persisted queue"
    );
}

/// Ported from tests/test_native_queue_health.py::ScreenRegression::test_old_client_running_remains_supported
#[test]
fn test_old_client_running_remains_supported() {
    assert!(screen_state("working esc to cancel").busy);
}

/// Ported from tests/test_native_queue_health.py::ScreenRegression::test_explicit_empty_modern_prompt_can_be_idle
#[test]
fn test_explicit_empty_modern_prompt_can_be_idle() {
    assert!(screen_state("›\n? for shortcuts").idle_prompt);
}

/// Ported from tests/test_native_queue_health.py::ScreenRegression::test_missing_prompt_is_unknown_not_idle
#[test]
fn test_missing_prompt_is_unknown_not_idle() {
    assert!(!screen_state("unrecognized terminal").idle_prompt);
}

struct Finding {
    code: String,
    text: String,
}

/// `ManagedDraftRegression.observe_stable(text)`.
fn observe_stable(text: &str) -> (Obj, Vec<Finding>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = hermetic_config(&tmp.path().to_string_lossy());
    let now: Dt = at("2026-10-21T02:00:00+00:00");
    let since = py::iso_seconds(&(now - chrono::Duration::minutes(21)));
    let prev = admiralty_audit::jobj! {"watch:captain_fixture" => admiralty_audit::jobj! {"hash" => tail_hash(text), "since" => since}};
    let owned = text.to_string();
    let host = FakeHost::new(now).with_hardcopy(move |_, _| owned.clone());
    let mut audit = Audit::new(&cfg, &host);
    let scr: Screens = [("captain_fixture".to_string(), 1)].into_iter().collect();
    let res = audit
        .idle_watch(
            &scr,
            &[("captain_fixture".to_string(), "dope".to_string())],
            &prev,
            &mut Obj::new(),
            &now,
        )
        .expect("idle_watch");
    let state = res
        .as_obj()
        .and_then(|o| o.get("captain_fixture"))
        .and_then(|s| s.as_obj())
        .cloned()
        .unwrap_or_default();
    let findings = audit
        .findings
        .iter()
        .map(|f| Finding {
            code: f.code.clone(),
            text: f.text.clone(),
        })
        .collect();
    (state, findings)
}

fn flag(state: &Obj, k: &str) -> bool {
    state.get(k).is_some_and(J::truthy)
}

fn code_set(findings: &[Finding]) -> BTreeSet<String> {
    findings.iter().map(|f| f.code.clone()).collect()
}

/// Ported from tests/test_native_queue_health.py::ManagedDraftRegression::test_blank_prompt_cannot_hide_pasted_draft
#[test]
fn test_blank_prompt_cannot_hide_pasted_draft() {
    let state = screen_state("›\n[Pasted Content 1024 chars]\n? for shortcuts");
    assert!(state.typed_input);
    assert!(!state.idle_prompt);
}

/// Ported from tests/test_native_queue_health.py::ManagedDraftRegression::test_managed_unsent_draft_never_gets_redrive_instruction
#[test]
fn test_managed_unsent_draft_never_gets_redrive_instruction() {
    let (state, findings) = observe_stable(": [Pasted Content 1024 chars]\n  tab to queue message");
    assert!(flag(&state, "typed_input"));
    assert!(!flag(&state, "idle_eligible"));
    assert!(code_set(&findings).contains("MANAGED_SCREEN_DRAFT_PROTECTED"));
    assert!(!code_set(&findings).contains("MANAGED_SCREEN_IDLE"));
    assert!(!findings.iter().any(|f| f.text.contains("CR + alt+m")));
}

/// Ported from tests/test_native_queue_health.py::ManagedDraftRegression::test_managed_unknown_composer_is_unverified_not_blank
#[test]
fn test_managed_unknown_composer_is_unverified_not_blank() {
    let (state, findings) = observe_stable("unrecognized native terminal");
    assert!(!flag(&state, "idle_eligible"));
    assert!(code_set(&findings).contains("MANAGED_SCREEN_INPUT_UNVERIFIED"));
    assert!(!code_set(&findings).contains("MANAGED_SCREEN_IDLE"));
}

/// Ported from tests/test_native_queue_health.py::ManagedDraftRegression::test_managed_verified_blank_prompt_still_reports_idle
#[test]
fn test_managed_verified_blank_prompt_still_reports_idle() {
    let (state, findings) = observe_stable("›\n? for shortcuts");
    assert!(flag(&state, "idle_eligible"));
    assert!(code_set(&findings).contains("MANAGED_SCREEN_IDLE"));
}

/// Ported from tests/test_native_queue_health.py::ManagedDraftRegression::test_managed_busy_draft_remains_busy
#[test]
fn test_managed_busy_draft_remains_busy() {
    let (state, findings) =
        observe_stable("Working (4s • esc to interrupt)\n: [Pasted text 10 chars]");
    assert!(flag(&state, "busy"));
    assert!(flag(&state, "typed_input"));
    assert!(!flag(&state, "idle_eligible"));
    assert!(!code_set(&findings).contains("MANAGED_SCREEN_IDLE"));
}

/// Ported from tests/test_native_queue_health.py::ManagedDraftRegression::test_managed_queue_hint_does_not_request_duplicate_input
#[test]
fn test_managed_queue_hint_does_not_request_duplicate_input() {
    let (state, findings) = observe_stable("queued messages\n›\n? for shortcuts");
    assert!(flag(&state, "queued_msgs"));
    assert!(!flag(&state, "idle_eligible"));
    assert!(code_set(&findings).contains("MANAGED_SCREEN_INPUT_UNVERIFIED"));
    assert!(!code_set(&findings).contains("MANAGED_SCREEN_IDLE"));
}

/// `ContinuationDraftRegression.assert_unverified_without_input_instruction(text)`.
fn assert_unverified_without_input_instruction(text: &str) {
    let (state, findings) = observe_stable(text);
    assert_eq!(
        state.get("composer_state"),
        Some(&J::from("unknown")),
        "{text}"
    );
    assert!(!flag(&state, "idle_eligible"), "{text}");
    assert!(
        code_set(&findings).contains("MANAGED_SCREEN_INPUT_UNVERIFIED"),
        "{text}"
    );
    assert!(
        !code_set(&findings).contains("MANAGED_SCREEN_IDLE"),
        "{text}"
    );
}

/// Ported from tests/test_native_queue_health.py::ContinuationDraftRegression::test_exact_multiline_draft_after_old_blank_is_unverified
#[test]
fn test_exact_multiline_draft_after_old_blank_is_unverified() {
    assert_unverified_without_input_instruction(
        "›\n  preserve my unsent multiline draft\n? for shortcuts",
    );
}

/// Ported from tests/test_native_queue_health.py::ContinuationDraftRegression::test_stale_footer_with_unknown_following_text_is_unverified
#[test]
fn test_stale_footer_with_unknown_following_text_is_unverified() {
    assert_unverified_without_input_instruction(
        "›\n? for shortcuts\n  unknown current continuation",
    );
}

/// Ported from tests/test_native_queue_health.py::ContinuationDraftRegression::test_unsupported_current_composer_after_old_footer_is_unverified
#[test]
fn test_unsupported_current_composer_after_old_footer_is_unverified() {
    assert_unverified_without_input_instruction(
        "›\n? for shortcuts\n[unsupported current composer]\n  unsent text",
    );
}

/// Ported from tests/test_native_queue_health.py::ContinuationDraftRegression::test_verified_blank_finding_never_requests_terminal_keys
#[test]
fn test_verified_blank_finding_never_requests_terminal_keys() {
    let (state, findings) = observe_stable("›\n? for shortcuts");
    assert!(flag(&state, "idle_eligible"));
    let finding = findings
        .iter()
        .find(|f| f.code == "MANAGED_SCREEN_IDLE")
        .expect("idle finding");
    assert!(finding.text.contains("BLOCKED_TRANSPORT"));
    assert!(finding.text.contains("queue/priority"));
    let bad =
        Regex::new(r"(?i)alt\+m|send\s+CR|file pointer \+ CR|paste the pointer").expect("regex");
    assert!(!bad.is_match(&finding.text));
}

/// Ported from tests/test_native_queue_health.py::ContinuationDraftRegression::test_unverified_historical_redrive_never_requests_terminal_keys
#[test]
fn test_unverified_historical_redrive_never_requests_terminal_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = hermetic_config(&tmp.path().to_string_lossy());
    let now = at("2026-10-21T02:00:00+00:00");
    let host = FakeHost::new(now);
    let mut audit = Audit::new(&cfg, &host);
    let log = "## observed 2026-10-21T02:00:00Z\nre-drove 1234.captain_fixture";
    let scr: Screens = [("captain_fixture".to_string(), 1)].into_iter().collect();
    let li = move |_: &str| Some(now - chrono::Duration::minutes(30));
    audit.check_redrive_delivered(log, &scr, &now, Some(&li));
    let finding = audit
        .findings
        .iter()
        .find(|f| f.code == "REDRIVE_NOT_DELIVERED")
        .expect("finding");
    assert!(finding.text.contains("BLOCKED_TRANSPORT"));
    let bad = Regex::new(r"(?i)alt\+m|send\s+CR|paste the pointer").expect("regex");
    assert!(!bad.is_match(&finding.text));
}

/// `QueueRegression.setUp`.
struct Queue {
    _tmp: tempfile::TempDir,
    db: PathBuf,
}

impl Queue {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("queue.sqlite");
        let c = Connection::open(&db).expect("open");
        c.execute_batch(
            "create table queued_items(id text primary key,thread_id text,payload_json text,queue_order integer,created_at_ms integer,updated_at_ms integer)",
        )
        .expect("create");
        drop(c);
        Self { _tmp: tmp, db }
    }

    fn insert(&self, count: i64, thread: &str, created: i64) {
        let mut c = Connection::open(&self.db).expect("open");
        let tx = c.transaction().expect("tx");
        for i in 0..count {
            tx.execute(
                "insert into queued_items values(?,?,?,?,?,?)",
                rusqlite::params![
                    format!("{thread}-{i}"),
                    thread,
                    "SECRET_MUST_NOT_BE_EXPORTED",
                    i,
                    created,
                    created
                ],
            )
            .expect("insert");
        }
        tx.commit().expect("commit");
    }

    fn health(&self, now_ms: i64, max_threads: i64) -> J {
        read_queue_health(&self.db, now_ms, max_threads, 300_000).expect("report")
    }
}

fn thread0(report: &J) -> Obj {
    report
        .as_obj()
        .and_then(|o| o.get("threads"))
        .and_then(J::as_arr)
        .and_then(|a| a.first())
        .and_then(J::as_obj)
        .cloned()
        .expect("thread")
}

fn status(report: &J) -> J {
    report
        .as_obj()
        .and_then(|o| o.get("status"))
        .cloned()
        .unwrap_or_default()
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_incident_36_pending_is_red_without_terminal_hint
#[test]
fn test_incident_36_pending_is_red_without_terminal_hint() {
    let q = Queue::new();
    q.insert(36, "admiral", 1_000);
    let before = std::fs::read(&q.db).expect("read");
    let report = q.health(25_000_000, 128);
    let t = thread0(&report);
    assert_eq!(t.get("pending"), Some(&J::Int(36)));
    assert_eq!(t.get("state"), Some(&J::from("stalled")));
    assert_eq!(t.get("oldest_age_ms"), Some(&J::Int(24_999_000)));
    assert!(!py::py_repr(&report).contains("SECRET"));
    assert!(!py::dumps(&report, None).contains("SECRET"));
    assert_eq!(before, std::fs::read(&q.db).expect("read"));
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_missing_database_is_unknown_and_not_created
#[test]
fn test_missing_database_is_unknown_and_not_created() {
    let q = Queue::new();
    std::fs::remove_file(&q.db).expect("unlink");
    let report = q.health(1000, 128);
    assert_eq!(status(&report), J::from("unknown"));
    assert!(!q.db.exists());
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_corrupt_database_is_unknown_not_empty
#[test]
fn test_corrupt_database_is_unknown_not_empty() {
    let q = Queue::new();
    std::fs::write(&q.db, b"not SQLite").expect("write");
    assert_eq!(status(&q.health(1000, 128)), J::from("unknown"));
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_wal_committed_rows_are_included_and_writer_is_untouched
#[test]
fn test_wal_committed_rows_are_included_and_writer_is_untouched() {
    let q = Queue::new();
    let writer = Connection::open(&q.db).expect("open");
    let mode: String = writer
        .query_row("pragma journal_mode=wal", [], |r| r.get(0))
        .expect("wal");
    assert_eq!(mode, "wal");
    writer
        .execute(
            "insert into queued_items values(?,?,?,?,?,?)",
            rusqlite::params!["wal-1", "admiral", "secret", 1, 1000, 1000],
        )
        .expect("insert");
    let t = thread0(&q.health(1001, 128));
    assert_eq!(t.get("pending"), Some(&J::Int(1)));
    writer.execute_batch("BEGIN").expect("begin");
    writer
        .execute(
            "insert into queued_items values(?,?,?,?,?,?)",
            rusqlite::params!["not-committed", "admiral", "secret", 2, 1000, 1000],
        )
        .expect("insert");
    let t = thread0(&q.health(1001, 128));
    assert_eq!(t.get("pending"), Some(&J::Int(1)));
    writer.execute_batch("ROLLBACK").expect("rollback");
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_recent_queue_is_pending_not_delivered
#[test]
fn test_recent_queue_is_pending_not_delivered() {
    let q = Queue::new();
    q.insert(1, "admiral", 1_000);
    let item = thread0(&q.health(1001, 128));
    assert_eq!(item.get("state"), Some(&J::from("pending")));
    assert_eq!(item.get("consumption_verified"), Some(&J::Bool(false)));
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_future_timestamp_reports_clock_unknown
#[test]
fn test_future_timestamp_reports_clock_unknown() {
    let q = Queue::new();
    q.insert(1, "admiral", 9999);
    assert_eq!(
        thread0(&q.health(1001, 128)).get("state"),
        Some(&J::from("clock_unknown"))
    );
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_result_limit_is_explicitly_incomplete
#[test]
fn test_result_limit_is_explicitly_incomplete() {
    let q = Queue::new();
    q.insert(1, "a", 1_000);
    q.insert(1, "b", 1_000);
    let out = q.health(1001, 1);
    let o = out.as_obj().expect("obj");
    assert_eq!(o.get("complete"), Some(&J::Bool(false)));
    assert_eq!(o.get("threads").and_then(J::as_arr).map(Vec::len), Some(1));
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_wrong_runtime_is_rejected_before_send
#[test]
fn test_wrong_runtime_is_rejected_before_send() {
    let e = require_live_target(
        "admiral",
        "/captains",
        &j(r#"{"id":"admiral","cwd":"/captains","status":{"type":"notLoaded"}}"#),
        &J::Null,
    )
    .expect_err("not loaded must be rejected");
    assert!(e.msg.contains("not loaded"));
    let e = require_live_target(
        "admiral",
        "/captains",
        &j(r#"{"id":"other","cwd":"/captains","status":{"type":"active"}}"#),
        &j(r#"{"id":"turn","status":"inProgress"}"#),
    )
    .expect_err("identity mismatch must be rejected");
    assert!(e.msg.contains("identity"));
}

/// Ported from tests/test_native_queue_health.py::QueueRegression::test_live_guard_requires_actual_in_progress_turn
#[test]
fn test_live_guard_requires_actual_in_progress_turn() {
    let thread = j(r#"{"id":"admiral","cwd":"/captains","status":{"type":"active"}}"#);
    let got = require_live_target(
        "admiral",
        "/captains",
        &thread,
        &j(r#"{"id":"turn","status":"inProgress"}"#),
    )
    .expect("live");
    assert_eq!(got, J::from("turn"));
    assert!(
        require_live_target(
            "admiral",
            "/captains",
            &thread,
            &j(r#"{"id":"turn","status":"interrupted"}"#)
        )
        .is_err()
    );
}

/// Bounds are validated like the Python ValueError.
#[test]
fn invalid_bounds_raise_value_error() {
    let q = Queue::new();
    let e = read_queue_health(&q.db, 1, 0, 300_000).expect_err("bounds");
    assert_eq!(e.kind, "ValueError");
    assert!(read_queue_health(&q.db, 1, 129, 300_000).is_err());
    assert!(read_queue_health(&q.db, 1, 128, 0).is_err());
}
