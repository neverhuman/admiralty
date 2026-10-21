//! Parity with the retired `tests/legacy/test_stage_evidence_regressions.py`
//! (`dashboard/collector.py` stage evidence). The legacy JopeDime packet-v4
//! receipt checks map onto the Rust major-release receipt verifier, and the
//! Joil audit/stage checks onto the packet-progress and audit adapters. Git
//! fixtures are real repositories, never mocked commands.
use super::*;

const CRITERION: &str = "The task passes its protected test cases.";
const CASES: [&str; 2] = ["ordinary input", "overflow input"];
const ANCESTOR_DATE: &str = "2026-10-18T11:00:00+00:00";
const MAIN_DATE: &str = "2026-10-18T11:30:00+00:00";

fn git_fixture(root: &Path, date: &str, args: &[&str]) -> String {
    let result = std::process::Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Observer Test",
            "-c",
            "user.email=observer@example.invalid",
            "-C",
        ])
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("fixture Git");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("Git UTF-8")
        .trim()
        .to_owned()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A real repository whose configured `origin/main` contains a tracker task
/// `I01` and two commits: `ancestor` and its child `main`.
struct Release {
    _owned: tempfile::TempDir,
    root: PathBuf,
    verify: PathBuf,
    main: String,
    ancestor: String,
}
impl Release {
    fn new(claim: &str) -> Self {
        let owned = tempfile::tempdir().expect("fixture directory");
        let root = fs::canonicalize(owned.path()).expect("canonical fixture");
        let verify = root.join("verify");
        fs::create_dir(&verify).expect("verify directory");
        git_fixture(&root, ANCESTOR_DATE, &["init", "--quiet"]);
        let tracker = json!({"tasks":[{"id":"I01","status":claim,"title":"Bounded fixture",
            "completion_criterion":CRITERION,"acceptance_cases":CASES}]});
        fs::write(root.join("tracker.json"), tracker.to_string()).expect("tracker");
        git_fixture(&root, ANCESTOR_DATE, &["add", "tracker.json"]);
        git_fixture(
            &root,
            ANCESTOR_DATE,
            &["commit", "--quiet", "-m", "ancestor"],
        );
        let ancestor = git_fixture(&root, ANCESTOR_DATE, &["rev-parse", "HEAD"]);
        fs::write(root.join("notes.txt"), "main\n").expect("second file");
        git_fixture(&root, MAIN_DATE, &["add", "notes.txt"]);
        git_fixture(&root, MAIN_DATE, &["commit", "--quiet", "-m", "main"]);
        let main = git_fixture(&root, MAIN_DATE, &["rev-parse", "HEAD"]);
        git_fixture(
            &root,
            MAIN_DATE,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        Self {
            _owned: owned,
            root,
            verify,
            main,
            ancestor,
        }
    }
    fn project(&self) -> ProjectConfig {
        ProjectConfig {
            id: "release".into(),
            name: "Release".into(),
            repository: self.root.clone(),
            tracker: json!({"kind":"major_release","path":"tracker.json","ref":"origin/main","verify_dir":self.verify}),
            manager_screen: None,
            worker_prefixes: vec![],
            coordination_log: None,
            git_health: None,
        }
    }
    /// Rewrite the shared execution log and re-pin every case digest to it.
    fn relog(&self, receipt: &mut Value, text: &str) {
        let path = self.verify.join("checks.log");
        fs::write(&path, text).expect("execution log");
        for case in receipt["case_to_test_map"]
            .as_array_mut()
            .expect("receipt cases")
        {
            case["log_path"] = json!(path);
            case["log_sha256"] = json!(digest(text.as_bytes()));
        }
    }
    fn receipt(&self, sha: &str) -> Value {
        let mut receipt = json!({"task_id":"I01","status":"PASS","sha":sha,"checkout_sha":sha,
            "verified_at_utc":"2026-10-18T12:00:00Z","completion_criterion":CRITERION,
            "case_to_test_map":CASES.map(|name| json!({"command":"python3 -m unittest task_checks",
                "exit_code":0,"acceptance_case":name}))});
        self.relog(
            &mut receipt,
            &format!("head_sha: {sha}\nexit_code: 0\nRan 2 tests in 0.01s\nOK\n"),
        );
        receipt
    }
    async fn observe(&self) -> Value {
        major_release(&self.project(), &[], &json!([]))
            .await
            .expect("major-release observation")
    }
    /// Publish `receipt` as `verify/I01.json` and return the observed stage
    /// and its reason.
    async fn stage(&self, receipt: &Value) -> (String, String) {
        fs::write(self.verify.join("I01.json"), receipt.to_string()).expect("receipt");
        let observed = self.observe().await;
        let row = &observed["stage_by_task"]["I01"];
        (
            string(&row["stage"]).to_owned(),
            string(&row["why"]).to_owned(),
        )
    }
    async fn done(&self, receipt: &Value) -> bool {
        self.stage(receipt).await.0 == "done"
    }
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_bare_pass_and_live_verified_claim_do_not_establish_done.
/// Adapted: a tracker `verified` claim and a supplemental `live_status` play
/// the LIVE_STATUS role. Intentional Rust difference: claim-only tasks stay
/// `not_started` rather than the legacy `on_main`.
#[tokio::test]
async fn bare_pass_and_live_verified_claim_do_not_establish_done() {
    let release = Release::new("verified");
    let (stage, why) = release
        .stage(&json!({"status":"PASS","sha":release.main}))
        .await;
    assert_eq!(stage, "in_progress");
    assert!(why.contains("Unverified PASS claim"), "{why}");
    fs::remove_file(release.verify.join("I01.json")).expect("remove receipt");
    let observed = release.observe().await;
    assert_eq!(observed["done"], 0);
    assert_eq!(observed["stage_by_task"]["I01"]["stage"], "not_started");
    assert_eq!(observed["raw_status_counts"]["verified"], 1);

    let tasks = release.root.join("tasks.json");
    fs::write(
        &tasks,
        json!({"tasks":[{"id":"I01","status":"open"}]}).to_string(),
    )
    .expect("ledger");
    let live = json!({"verified_task_claims":["I01"],"programme_progress":{"verified_task_ids":["I01"]},
        "receiving_main":{"sha":release.main}});
    let supplemental = release.root.join("live.json");
    fs::write(&supplemental, json!({"live_status":live}).to_string()).expect("live status");
    let config: ObserveConfig = serde_json::from_value(json!({"schema_version":1,"node_id":"node",
        "output_dir":release.root,"projects":[{"id":"jopedime","name":"JopeDime","repository":release.root,
        "tracker":{"kind":"task_array","path":tasks,"supplemental_path":supplemental}}]}))
    .expect("configuration");
    let records = collect(&config, &json!([])).await.expect("observation");
    let project = &records["jopedime"];
    assert!(project.get("error").is_none());
    assert_eq!(project["done"], 0);
    assert_eq!(project["stages"]["done"], 0);
    assert_eq!(project["live_status"], live);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_valid_packet_matched_receipt_and_ancestor_are_supported.
#[tokio::test]
async fn valid_packet_matched_receipt_and_ancestor_are_supported() {
    let release = Release::new("todo");
    let (stage, why) = release.stage(&release.receipt(&release.main)).await;
    assert_eq!(stage, "done", "{why}");
    assert!(release.done(&release.receipt(&release.ancestor)).await);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_missing_or_invalid_time_never_counts_as_verified.
/// Fixed in the observer: a stated verification time that is malformed,
/// timezone-less, in the future or older than its commit (relabeled) no longer
/// reaches done. Intentional Rust difference: an absent time follows the
/// major-release receipt contract (as in the legacy major-release path and
/// `sc_audit`'s relabel guard) and does not by itself block done.
#[tokio::test]
async fn missing_or_invalid_time_never_counts_as_verified() {
    let release = Release::new("todo");
    for value in [
        json!("bad time"),
        json!("2026-10-18T12:00:00"),
        json!("9999-01-01T00:00:00Z"),
        json!(1_760_788_800),
        json!(""),
    ] {
        let mut receipt = release.receipt(&release.main);
        receipt["verified_at_utc"] = value.clone();
        assert!(!release.done(&receipt).await, "{value}");
    }
    let mut receipt = release.receipt(&release.main);
    receipt["verified_at_utc"] = json!("1970-01-01T00:00:00Z");
    let (stage, why) = release.stage(&receipt).await;
    assert_ne!(stage, "done");
    assert!(why.contains("relabeled"), "{why}");
    receipt
        .as_object_mut()
        .expect("receipt object")
        .remove("verified_at_utc");
    receipt["verified_at"] = json!("1970-01-01T00:00:00Z");
    assert!(!release.done(&receipt).await);
    receipt
        .as_object_mut()
        .expect("receipt object")
        .remove("verified_at");
    assert!(release.done(&receipt).await);
    receipt["verified_at_utc"] = Value::Null;
    assert!(release.done(&receipt).await);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_wrong_task_checkout_criterion_or_unmapped_cases_are_rejected.
#[tokio::test]
async fn wrong_task_checkout_criterion_or_unmapped_cases_are_rejected() {
    let release = Release::new("todo");
    for (key, value) in [
        ("task_id", json!("I02")),
        ("checkout_sha", json!(release.ancestor)),
        ("completion_criterion", json!("An easier criterion")),
        ("case_to_test_map", json!([])),
    ] {
        let mut receipt = release.receipt(&release.main);
        receipt[key] = value;
        assert!(!release.done(&receipt).await, "{key}");
    }
    let mut receipt = release.receipt(&release.main);
    receipt["case_to_test_map"][0]["acceptance_case"] = json!("unrelated case");
    assert!(!release.done(&receipt).await);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_unsuccessful_missing_or_forged_execution_evidence_is_rejected.
/// Fixed in the observer: a stated non-execution command (`echo PASS`) was
/// accepted as successful case evidence.
#[tokio::test]
async fn unsuccessful_missing_or_forged_execution_evidence_is_rejected() {
    let release = Release::new("todo");
    for (key, value) in [
        ("exit_code", json!(false)),
        ("exit_code", json!("0")),
        ("exit_code", json!(1)),
        ("exit_code", Value::Null),
        ("command", json!("echo PASS")),
        ("command", json!("")),
        ("log_path", json!("/nonexistent")),
        ("log_sha256", json!("0".repeat(64))),
        ("log_sha256", Value::Null),
    ] {
        let mut receipt = release.receipt(&release.main);
        receipt["case_to_test_map"][0][key] = value.clone();
        assert!(!release.done(&receipt).await, "{key}={value}");
    }
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_zero_test_log_is_not_successful_case_coverage.
#[tokio::test]
async fn zero_test_log_is_not_successful_case_coverage() {
    let release = Release::new("todo");
    let mut receipt = release.receipt(&release.main);
    release.relog(&mut receipt, "Ran 0 tests in 0.00s\nOK\n");
    assert!(!release.done(&receipt).await);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_contradictory_log_head_and_exit_headers_are_rejected.
/// Fixed in the observer: whitespace before a header colon (`exit_code : 1`)
/// no longer hides a contradictory header.
#[tokio::test]
async fn contradictory_log_head_and_exit_headers_are_rejected() {
    let release = Release::new("todo");
    for header in [
        format!("head_sha: {}", release.ancestor),
        format!("checkout_sha: {}", release.ancestor),
        "exit_code: 1".to_owned(),
        "exit_code : 1".to_owned(),
    ] {
        let mut receipt = release.receipt(&release.main);
        release.relog(
            &mut receipt,
            &format!("{header}\n--- STDOUT ---\nRan 2 tests in 0.01s\nOK\n"),
        );
        assert!(!release.done(&receipt).await, "{header}");
    }
    let mut receipt = release.receipt(&release.main);
    release.relog(
        &mut receipt,
        &format!(
            "head_sha: {}\nexit_code: 0\n--- STDOUT ---\nexpected negative control printed exit_code: 1\nRan 2 tests in 0.01s\nOK\n",
            release.main
        ),
    );
    assert!(release.done(&receipt).await);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_final_test_failure_is_not_overridden_by_receipt_exit_zero.
/// Intentional Rust difference: any unquoted failed summary rejects the log,
/// even when a later summary passes (Python accepted `failed + passed`).
#[tokio::test]
async fn final_test_failure_is_not_overridden_by_receipt_exit_zero() {
    let release = Release::new("todo");
    let failed = "test result: FAILED. 0 passed; 1 failed; 0 ignored;\n";
    let passed = "test result: ok. 2 passed; 0 failed; 0 ignored;\n";
    for (output, expected) in [
        (failed.to_owned(), false),
        (
            "Ran 2 tests in 0.01s\nFAILED (failures=1)\n".to_owned(),
            false,
        ),
        (format!("{failed}{passed}"), false),
        (format!("{passed}```text\n{failed}```\n> {failed}"), true),
    ] {
        let mut receipt = release.receipt(&release.main);
        release.relog(
            &mut receipt,
            &format!(
                "head_sha: {}\nexit_code: 0\n--- STDOUT ---\n{output}",
                release.main
            ),
        );
        assert_eq!(release.done(&receipt).await, expected, "{output}");
    }
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_duplicate_json_and_symlink_or_oversized_evidence_are_rejected.
#[tokio::test]
async fn duplicate_json_and_symlink_or_oversized_evidence_are_rejected() {
    let release = Release::new("todo");
    fs::write(
        release.verify.join("I01.json"),
        format!(
            r#"{{"status":"FAIL","status":"PASS","sha":"{}"}}"#,
            release.main
        ),
    )
    .expect("ambiguous receipt");
    let observed = release.observe().await;
    assert_eq!(observed["stage_by_task"]["I01"]["stage"], "not_started");
    let mut receipt = release.receipt(&release.main);
    let original = release.verify.join("checks.log");
    let link = release.verify.join("linked.log");
    std::os::unix::fs::symlink(&original, &link).expect("symlink fixture");
    receipt["case_to_test_map"][0]["log_path"] = json!(link);
    assert!(!release.done(&receipt).await);
    let large = fs::File::create(&original).expect("oversized log");
    large
        .set_len(MAX_LOG_BYTES as u64 + 1)
        .expect("sparse oversized log");
    assert!(proof_log(&original, MAX_LOG_BYTES).is_err());
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_aggregate_execution_log_budget_fails_closed.
/// Adapted: Rust bounds hashed-log reads by one aggregate budget per project
/// observation cycle (`TOTAL_LOG_BYTES`, shared by all receipts) instead of
/// the legacy 32 MiB per receipt; a receipt whose distinct logs exceed the
/// remaining budget fails closed.
#[test]
fn aggregate_execution_log_budget_fails_closed() {
    let owned = tempfile::tempdir().expect("fixture directory");
    let root = fs::canonicalize(owned.path()).expect("canonical fixture");
    let sha = "a".repeat(40);
    let task = json!({"id":"I01","completion_criterion":CRITERION,"acceptance_cases":CASES});
    let mut total = 0;
    let cases = CASES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let text =
                format!("head_sha: {sha}\nexit_code: 0\nRan 2 tests in 0.01s\nOK\nlog {index}\n");
            let path = root.join(format!("large-{index}.log"));
            fs::write(&path, &text).expect("log");
            total += text.len();
            json!({"acceptance_case":name,"command":"python3 -m unittest task_checks","exit_code":0,
                "log_path":path,"log_sha256":digest(text.as_bytes())})
        })
        .collect::<Vec<_>>();
    let receipt =
        json!({"task_id":"I01","completion_criterion":CRITERION,"case_to_test_map":cases});
    let receipt_path = root.join("I01.json");
    let mut remaining = total - 1;
    assert!(
        verify_cases(
            &task,
            &receipt,
            &sha,
            &receipt_path,
            &mut remaining,
            &mut BTreeMap::new()
        )
        .is_err()
    );
    let mut remaining = total;
    assert!(
        verify_cases(
            &task,
            &receipt,
            &sha,
            &receipt_path,
            &mut remaining,
            &mut BTreeMap::new()
        )
        .is_ok()
    );
    assert_eq!(remaining, 0);
}

fn packet_project(root: &Path, statuses: &[(&str, &str)], audit: &str) -> Value {
    let tasks = statuses
        .iter()
        .map(|(id, _)| json!({"id":id}))
        .collect::<Vec<_>>();
    fs::write(
        root.join("TASKS.json"),
        json!({"packets":[{"id":"P1","tasks":tasks}]}).to_string(),
    )
    .expect("packet ledger");
    let progress = statuses
        .iter()
        .map(|(id, status)| ((*id).to_owned(), json!({"status":status})))
        .collect::<Map<_, _>>();
    fs::write(root.join("P1.json"), json!({"tasks":progress}).to_string())
        .expect("packet progress");
    fs::write(root.join("LATEST.md"), audit).expect("audit");
    fs::write(
        root.join("extra.json"),
        json!({"active_claims":{"T-01":["reported active claim"]}}).to_string(),
    )
    .expect("supplemental claims");
    json!({"kind":"packet_progress","path":root.join("TASKS.json"),"progress_dir":root,
        "validator_path":root.join("LATEST.md"),"supplemental_path":root.join("extra.json")})
}
async fn packet_record(root: &Path, statuses: &[(&str, &str)], audit: &str) -> Value {
    let tracker = packet_project(root, statuses, audit);
    let config: ObserveConfig = serde_json::from_value(json!({"schema_version":1,"node_id":"node",
        "output_dir":root,"projects":[{"id":"joil","name":"Joil","repository":root,"tracker":tracker}]}))
    .expect("configuration");
    collect(&config, &json!([])).await.expect("observation")["joil"].clone()
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_blocked_or_audit_held_task_stays_blocked_even_with_active_lane.
/// The legacy `blocked_code_stage` metadata does not apply: Rust never promotes
/// a ledger stage from lanes or main-ledger comparisons, so no hidden code
/// stage exists to report.
#[tokio::test]
async fn blocked_or_audit_held_task_stays_blocked_even_with_active_lane() {
    for (status, audit) in [
        ("blocked", "Verdict: none\n"),
        ("review", "T-01 AUDIT-HELD pending review\n"),
    ] {
        let owned = tempfile::tempdir().expect("fixture directory");
        let record = packet_record(owned.path(), &[("T-01", status)], audit).await;
        assert!(record.get("error").is_none(), "{record}");
        assert_eq!(record["stages"]["blocked"], 1, "{status}");
        assert_eq!(record["stages"]["in_progress"], 0, "{status}");
        assert_eq!(record["blocked_ids"], json!(["T-01"]));
        assert_eq!(
            record["active_claims"],
            json!({"T-01":["reported active claim"]})
        );
    }
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_ordinary_completed_main_task_still_promotes_to_done.
/// Adapted: the configured progress source is authoritative (the Joil-specific
/// integration-branch versus origin/main ledger comparison is not ported); a
/// done task stays done even under an audit hold.
#[tokio::test]
async fn ordinary_completed_main_task_still_promotes_to_done() {
    let owned = tempfile::tempdir().expect("fixture directory");
    let record = packet_record(
        owned.path(),
        &[("T-01", "done")],
        "T-01 AUDIT-HELD pending review\n",
    )
    .await;
    assert_eq!(record["stages"]["done"], 1);
    assert_eq!(record["stages"]["blocked"], 0);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_current_real_failed_verdicts_and_explicit_negations.
#[test]
fn current_real_failed_verdicts_and_explicit_negations() {
    let text = r#"# LATEST audit
  - Verdict: **FAILED on check 3. Blocking: 1.**
  - Verdict: **FAIL. Blocking: 1.**
  - Verdicts: no FAIL. Blocking: none.
  - Verdicts: no FAIL, no HOLD. Blocking: none.
  - Verdict: 0 FAIL
  - Verdict: FAIL: 0
    - Red logs: guards each FAILED with exit 101, as expected.
Documentation example: "FAILED" means the check needs repair.
> - Verdict: FAILED (quoted historical documentation)
```
- Verdict: FAILED (code example)
```
## Previous
- Verdict: FAILED
"#;
    assert_eq!(audit(text)["fail_mentions"], 2);
}

/// Parity with tests/legacy/test_stage_evidence_regressions.py::test_explicit_markdown_verdict_formats_and_zero_counts.
#[test]
fn explicit_markdown_verdict_formats_and_zero_counts() {
    assert_eq!(
        audit("**Verdict:** FAILED\n### Verdict: FAIL\n- **Verdict**: FAILED")["fail_mentions"],
        3
    );
    assert_eq!(
        audit("**Verdict:** **FAIL**: 0\nVerdict: zero failures\nVerdict: no FAILED checks")["fail_mentions"],
        0
    );
}

/// Synthetic legacy case `wrong_checkout_pass`: a PASS naming one commit but
/// executed on another stays unaccepted; the matching receipt is accepted.
/// Fixture commit placeholders map onto the real fixture commits.
#[tokio::test]
async fn legacy_case_wrong_checkout_pass() {
    let case = crate::legacy_cases::case("wrong_checkout_pass");
    let release = Release::new("todo");
    let matching = &case["input"]["matching_receipt"]["sha"];
    let commit = |placeholder: &Value| {
        if placeholder == matching {
            release.main.clone()
        } else {
            release.ancestor.clone()
        }
    };
    for (input, expected) in [
        (&case["input"]["receipt"], &case["expected"]["accepted"]),
        (
            &case["input"]["matching_receipt"],
            &case["expected"]["matching_accepted"],
        ),
    ] {
        let mut receipt = release.receipt(&commit(&input["sha"]));
        receipt["status"] = input["status"].clone();
        receipt["checkout_sha"] = json!(commit(&input["checkout_sha"]));
        for entry in receipt["case_to_test_map"]
            .as_array_mut()
            .expect("receipt cases")
        {
            entry["exit_code"] = input["command_exit"].clone();
        }
        let (stage, why) = release.stage(&receipt).await;
        assert_eq!(json!(stage == "done"), *expected, "{why}");
        if expected == false {
            assert!(why.contains("mismatched"), "{why}");
        }
    }
}

/// Synthetic legacy case `docs_only_implementation`: self-reported claims and
/// documentation-only commands cannot reach done.
#[tokio::test]
async fn legacy_case_docs_only_implementation() {
    let case = crate::legacy_cases::case("docs_only_implementation");
    let release = Release::new("todo");
    let verification = &case["input"]["verification"];
    let receipt = json!({"task_id":"I01","status":"PASS","sha":release.main,"checkout_sha":release.main,
        "verified_at_utc":"2026-10-18T12:00:00Z","completion_criterion":CRITERION,
        "claims":verification["claims"],"commands":verification["commands"],
        "receipt_kind":verification["receipt_kind"],"changed_paths":case["input"]["changed_paths"]});
    assert_eq!(
        json!(release.done(&receipt).await),
        case["expected"]["accepted"]
    );
    let mut receipt = release.receipt(&release.main);
    for entry in receipt["case_to_test_map"]
        .as_array_mut()
        .expect("receipt cases")
    {
        entry["command"] = json!("reviewed docs/parser.md");
    }
    assert!(!release.done(&receipt).await);
}

/// Regex-equivalence table for the legacy execution-command pattern, including
/// both original cases (`python3 -m unittest task_checks`, `echo PASS`).
#[test]
fn execution_command_matches_the_legacy_pattern() {
    for command in [
        "python3 -m unittest task_checks",
        "python -m pytest -q",
        "python tests/run_checks.py --fast",
        "cargo test -p jope-engine --test mr02 -- --exact mr02_case",
        "cargo-deny check",
        "RUST_LOG=1 cargo nextest run",
        "pytest -q",
        "just check",
        "npm test",
        "pnpm vitest",
        "make check",
        "ctest --output-on-failure",
        "uv run pytest",
        "go test ./...",
        "bash scripts/check.sh",
        "bash  a.sh;b",
        "env x./run",
    ] {
        assert!(execution_command(command), "{command}");
    }
    for command in [
        "echo PASS",
        "",
        "reviewed docs/parser.md",
        "./run.sh",
        "scargo test",
        "makefile",
        "python3.11 -m x",
        "python x.pyc",
        "bash run.shx",
        "bash .sh",
        "gotest",
        "go  test",
        "uv  run",
        "python -m ",
    ] {
        assert!(!execution_command(command), "{command}");
    }
}
