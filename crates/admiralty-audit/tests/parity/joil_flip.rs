//! Ported from tests/test_sc_audit_joil_flip.py (builds real temporary git repos).

use std::process::{Command, Stdio};

use serde_json::{Value, json};

use admiralty_audit::ctx::Audit;
use admiralty_audit::ledgers::joil::joil_log_is_summary;
use admiralty_audit::py::J;
use admiralty_audit::{AuditConfig, RealHost};

fn git(repo: &str, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn write(repo: &str, path: &str, obj: &Value) {
    let p = std::path::Path::new(repo).join(path);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    let text = match obj {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).expect("json"),
    };
    std::fs::write(p, text).expect("write");
}

fn prog(tasks: Value) -> Value {
    json!({"schema_version": 1, "packet": "V5-90", "tasks": tasks})
}

struct Repo {
    _tmp: tempfile::TempDir,
    d: String,
    cfg: AuditConfig,
}

impl Repo {
    /// `setUp`.
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let d = tmp.path().to_string_lossy().to_string();
        git(&d, &["init", "-q", "-b", "main"]);
        write(
            &d,
            "docs/v5-plan/progress/V5-90.json",
            &prog(json!({
                "V5-90-01": {"status": "blocked", "note": "Cause B: measured data absent"},
                "V5-90-02": {"status": "blocked", "note": "Cause B: measured data absent"},
                "V5-90-03": {"status": "todo", "note": ""}})),
        );
        write(&d, "crates/a/src/lib.rs", &json!("pub fn a() {}\n"));
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-qm", "base"]);
        let cfg = AuditConfig::with_home(&format!("{d}/home"));
        Self { _tmp: tmp, d, cfg }
    }

    /// `flip(prod_lines, labelled, open_summary, own_receipt)`; returns (result, findings (code, severity)).
    fn flip(
        &self,
        prod_lines: usize,
        labelled: bool,
        open_summary: bool,
        own_receipt: bool,
    ) -> (J, Vec<(String, String)>) {
        let r = &self.d;
        let lbl = if labelled { " substitute:proxy" } else { "" };
        write(
            r,
            "docs/v5-plan/progress/V5-90.json",
            &prog(json!({
                "V5-90-01": {"status": "done", "note": format!("verified{lbl}"), "evidence": [{"path": "docs/receipts/pk.json"}]},
                "V5-90-02": {"status": "done", "note": format!("verified{lbl}"), "evidence": [{"path": "docs/receipts/pk.json"}]},
                "V5-90-03": {"status": "done", "note": "verified", "evidence": [{"path": "docs/receipts/pk.json"}]}})),
        );
        write(
            r,
            "docs/receipts/pk.json",
            &json!({"task_ids": ["V5-90-01", "V5-90-02", "V5-90-03"], "commands": []}),
        );
        if own_receipt {
            write(
                r,
                "docs/receipts/own.json",
                &json!({"task_ids": ["V5-90-01"], "commands": []}),
            );
        }
        let summary = if open_summary {
            "planted negatives not yet executed, so verification remains open"
        } else {
            "ok"
        };
        write(
            r,
            "docs/release/v5/FLEET_ADMIRAL_DISPOSITIONS_X.json",
            &json!({"decisions": [{"decision_id": "SC-DEC-900", "task_id": "V5-90-01", "disposition_mode": "substitute:proxy", "summary": summary}]}),
        );
        let mut lib = String::from("pub fn a() {}\n");
        for i in 0..prod_lines {
            lib.push_str(&format!("pub fn f{i}() {{}}\n"));
        }
        lib.push_str("#[cfg(test)]\nmod tests {\n");
        for i in 0..200 {
            lib.push_str(&format!("fn t{i}() {{}}\n"));
        }
        lib.push_str("}\n");
        write(r, "crates/a/src/lib.rs", &json!(lib));
        write(
            r,
            "crates/a/tests/t.rs",
            &json!(vec!["// t"; 500].join("\n")),
        );
        git(r, &["add", "-A"]);
        git(r, &["commit", "-qm", "flip"]);
        self.audit(true)
    }

    fn audit(&self, emit: bool) -> (J, Vec<(String, String)>) {
        let host = RealHost;
        let mut a = Audit::new(&self.cfg, &host);
        let res = a
            .audit_joil_flips(&self.d, "main", emit)
            .expect("flip audit");
        let findings = a
            .findings
            .iter()
            .map(|f| (f.code.clone(), f.severity.clone()))
            .collect();
        (res, findings)
    }
}

fn flags(res: &J, task: &str) -> Vec<String> {
    res.as_obj()
        .and_then(|o| o.get("flags"))
        .and_then(|f| f.as_obj())
        .and_then(|f| f.get(task))
        .and_then(|v| v.as_arr())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn int(res: &J, key: &str) -> i64 {
    res.as_obj()
        .and_then(|o| o.get(key))
        .and_then(J::as_inst_int)
        .expect("int field")
}

/// Ported from tests/test_sc_audit_joil_flip.py::JoilFlip::test_flip_with_only_tests_and_receipts_is_red
#[test]
fn test_flip_with_only_tests_and_receipts_is_red() {
    let repo = Repo::new();
    let (res, findings) = repo.flip(3, false, false, false);
    assert!(flags(&res, "V5-90-01").contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    assert!(flags(&res, "V5-90-02").contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    // never externally blocked
    assert!(!flags(&res, "V5-90-03").contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    assert_eq!(int(&res, "done_on_main"), 3);
    assert_eq!(int(&res, "done_unflagged_on_main"), 1);
    assert!(
        findings
            .iter()
            .any(|(c, s)| c == "JOIL_DONE_STATUS_FLIP" && s == "red")
    );
}

/// Ported from tests/test_sc_audit_joil_flip.py::JoilFlip::test_cfg_test_lines_do_not_count_as_production
#[test]
fn test_cfg_test_lines_do_not_count_as_production() {
    let repo = Repo::new();
    let host = RealHost;
    let a = Audit::new(&repo.cfg, &host);
    let (total, _) = a.joil_prod_added_lines(&repo.d, "main", "main");
    assert_eq!(total, 0);
    repo.flip(3, false, false, false);
    let (total, _) = a.joil_prod_added_lines(&repo.d, "main~1", "main");
    assert_eq!(total, 3);
}

/// Ported from tests/test_sc_audit_joil_flip.py::JoilFlip::test_real_implementation_is_not_a_flip
#[test]
fn test_real_implementation_is_not_a_flip() {
    let repo = Repo::new();
    let (res, _) = repo.flip(60, true, false, true);
    let f1 = flags(&res, "V5-90-01");
    assert!(!f1.contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    assert!(!f1.contains(&"JOIL_SUBSTITUTE_UNLABELLED".to_string()));
    assert!(!f1.contains(&"JOIL_DONE_SHARED_RECEIPT".to_string()));
    assert!(flags(&res, "V5-90-02").contains(&"JOIL_DONE_SHARED_RECEIPT".to_string()));
}

/// Ported from tests/test_sc_audit_joil_flip.py::JoilFlip::test_unlabelled_substitute_and_open_decision_are_red
#[test]
fn test_unlabelled_substitute_and_open_decision_are_red() {
    let repo = Repo::new();
    let (res, _) = repo.flip(60, false, true, false);
    let f1 = flags(&res, "V5-90-01");
    assert!(f1.contains(&"JOIL_SUBSTITUTE_UNLABELLED".to_string()));
    assert!(f1.contains(&"JOIL_DONE_DECISION_SAYS_OPEN".to_string()));
    let red = res
        .as_obj()
        .and_then(|o| o.get("red_flagged_tasks"))
        .cloned()
        .unwrap_or_default();
    assert!(
        red.as_arr()
            .is_some_and(|a| a.contains(&J::from("V5-90-01")))
    );
}

/// Ported from tests/test_sc_audit_joil_flip.py::JoilFlip::test_summary_only_log_detection
#[test]
fn test_summary_only_log_detection() {
    let repo = Repo::new();
    let f = format!("{}/l.log", repo.d);
    std::fs::write(
        &f,
        "cargo test: 1 passed, 18 filtered out (1 suite, 0.00s)\n",
    )
    .expect("write");
    assert_eq!(joil_log_is_summary(&J::from(f.as_str())), Some(true));
    std::fs::write(&f, "running 1 test\ntest v5_x ... ok\n").expect("write");
    assert_eq!(joil_log_is_summary(&J::from(f.as_str())), Some(false));
    assert_eq!(joil_log_is_summary(&J::from(format!("{f}.missing"))), None);
}

/// Ported from tests/test_sc_audit_joil_flip.py::JoilFlip::test_own_receipt_with_resolution_and_full_log_waives_flip
#[test]
fn test_own_receipt_with_resolution_and_full_log_waives_flip() {
    let repo = Repo::new();
    let r = repo.d.clone();
    let log = format!("{r}/full.log");
    std::fs::write(&log, "running 1 test\ntest v5_90_01 ... ok\n").expect("write");
    write(
        &r,
        "docs/receipts/own.json",
        &json!({"task_ids": ["V5-90-01"], "decision_resolution": "public dataset X accessioned, sha256 ...",
            "commands": [{"command": "cargo test -p a v5_90_01 --exact", "exit_code": 0, "log_path": log}]}),
    );
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "own receipt"]);
    let (res, _) = repo.flip(3, true, false, false);
    assert!(!flags(&res, "V5-90-01").contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    assert!(flags(&res, "V5-90-02").contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    std::fs::write(
        &log,
        "cargo test: 1 passed, 18 filtered out (1 suite, 0.00s)\n",
    )
    .expect("write");
    let (res, findings) = repo.audit(false);
    // summary-only log does not waive
    assert!(flags(&res, "V5-90-01").contains(&"JOIL_DONE_STATUS_FLIP".to_string()));
    assert!(findings.is_empty(), "emit=False records no findings");
}
