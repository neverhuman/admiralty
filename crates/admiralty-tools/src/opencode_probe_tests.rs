//! Tests for the OpenCode SSH qualification port. The retired Python tool had
//! no tests; expected digests and strings below were produced by CPython 3.13
//! from `tools/probe_opencode.py` before its removal.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::*;

const ORACLE_SHA256: &str = "15d78c3be42eb5cfa1b3b8103d87d4138d8b6e291a2406e548b9db9f2e417754";
const PROMPT_SHA256: &str = "b9afb94f454a2b96e23a6703e3deff1ccd5dca3441917c62b4763e5da2925e70";
const REMOTE_SHA256: &str = "2474cbefd5869dfe5dc303132c6e87991d3130b1e516d4a5352daac724794489";
const REMOTE_COMMAND_SHA256: &str =
    "9b21999a22260298a1acd98d81cd53d428b9884ebe17d3a7d71f5485ca0bc64d";
const PAYLOAD_SHA256: &str = "d342468beb0cf8c934888fb418f0171aa7512eee14c727e5b7f7d72fb48e4e1d";
const RECEIPT_SHA256: &str = "f9e564bcf4638bf41baf9f5f252961a560420017a6bb143617ee890fec6f63ff";

fn args(output: &Path) -> ProbeArgs {
    ProbeArgs {
        node: "registered-node".to_owned(),
        remote_parent: "/q".to_owned(),
        remote_helper: "/bin/h".to_owned(),
        opencode: "/bin/o".to_owned(),
        model: "prov/m\u{e9}".to_owned(),
        output: output.to_path_buf(),
        correct_fixture: None,
        qualify: true,
        ssh_binary: PathBuf::from("/usr/bin/ssh"),
    }
}

#[test]
fn workload_is_byte_identical_to_the_retired_python_constants() {
    assert_eq!(sha256_hex(ORACLE.as_bytes()), ORACLE_SHA256);
    assert_eq!(sha256_hex(PROMPT.as_bytes()), PROMPT_SHA256);
    assert_eq!(sha256_hex(REMOTE.as_bytes()), REMOTE_SHA256);
}

#[test]
fn shell_quoting_matches_shlex() {
    let cases = [
        ("", "''"),
        ("abc", "abc"),
        ("a b", "'a b'"),
        ("it's", "'it'\"'\"'s'"),
        ("x=y,z:@%+/.-_", "x=y,z:@%+/.-_"),
        ("tab\tx", "'tab\tx'"),
        ("\u{e9}", "'\u{e9}'"),
        ("$HOME", "'$HOME'"),
        ("a\"b", "'a\"b'"),
    ];
    for (value, quoted) in cases {
        assert_eq!(shell_quote(value), quoted, "{value:?}");
    }
    let command = shell_join(&[REMOTE_PYTHON, "-c", REMOTE]);
    assert_eq!(sha256_hex(command.as_bytes()), REMOTE_COMMAND_SHA256);
}

#[test]
fn plan_builds_the_exact_python_payload_and_fixed_ssh_argv() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("receipt.json");
    let plan = plan(&args(&output), "0123456789abcdef").expect("plan");
    assert_eq!(plan.root, "/q/admiralty-qualification-0123456789abcdef");
    assert_eq!(sha256_hex(plan.payload.as_bytes()), PAYLOAD_SHA256);
    assert_eq!(
        plan.argv[..7],
        [
            "/usr/bin/ssh",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "registered-node"
        ]
    );
    assert_eq!(sha256_hex(plan.argv[7].as_bytes()), REMOTE_COMMAND_SHA256);
    assert_eq!(plan.output, output);
}

#[test]
fn plan_refuses_unsafe_or_unauthorized_requests_before_any_effect() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("receipt.json");
    let base = args(&output);
    let refused = |change: &dyn Fn(&mut ProbeArgs), needle: &str| {
        let mut candidate = base.clone();
        change(&mut candidate);
        let error = plan(&candidate, "0123456789abcdef").expect_err(needle);
        assert!(error.contains(needle), "{error} lacks {needle}");
    };
    refused(
        &|a| a.node = "-oProxyCommand=x".to_owned(),
        "registered safe SSH alias",
    );
    refused(
        &|a| a.remote_parent = "relative".to_owned(),
        "absolute, non-traversing",
    );
    refused(
        &|a| a.remote_helper = "/a/../b".to_owned(),
        "absolute, non-traversing",
    );
    refused(
        &|a| a.opencode = "/a\0b".to_owned(),
        "absolute, non-traversing",
    );
    refused(&|a| a.qualify = false, "requires --qualify");
    refused(
        &|a| a.output = Path::new("/nonexistent-dir-for-probe/receipt.json").to_path_buf(),
        "new receipt file",
    );
    std::fs::write(&output, "existing").expect("existing receipt");
    refused(&|_| {}, "new receipt file");
    std::fs::remove_file(&output).expect("remove");
    refused(
        &|a| a.correct_fixture = Some("/other/admiralty-qualification-0123456789abcdef".to_owned()),
        "immediately under remote parent",
    );
    refused(
        &|a| a.correct_fixture = Some("/q/admiralty-qualification-0123".to_owned()),
        "immediately under remote parent",
    );
    let mut corrected = base.clone();
    corrected.remote_parent = "/q/".to_owned();
    corrected.correct_fixture = Some("/q/admiralty-qualification-fedcba9876543210".to_owned());
    let plan = plan(&corrected, "0123456789abcdef").expect("correction plan");
    assert_eq!(plan.root, "/q/admiralty-qualification-fedcba9876543210");
    assert!(plan.payload.ends_with("\"corrected\": true}"));
}

fn fake_ssh(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("ssh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ncat > \"$0.stdin\"\nprintf '%s\\n' \"$1\" \"$2\" \"$3\" \"$4\" \"$5\" \"$6\" > \"$0.args\"\nprintf '%s' \"$7\" > \"$0.command\"\n{body}\n"
        ),
    )
    .expect("fake ssh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn remote_receipt(oracle_after: &str) -> String {
    let escape = format!("{}u00e9", '\\');
    format!(
        r#"{{"schema_version": 1, "helper_exit": 0, "verifier_exit": 0, "oracle_sha256_before": "{ORACLE_SHA256}", "oracle_sha256_after": "{oracle_after}", "scope_valid": true, "model_observed": [{{"providerID": "p", "modelID": "m"}}], "elapsed_s": 12.345, "timed_out": false, "z": "{escape}", "a": [1e-05, 2.0]}}"#
    )
}

#[test]
fn execute_writes_exclusive_receipt_in_remote_order_and_passes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("receipt.json");
    let mut request = args(&output);
    request.ssh_binary = fake_ssh(
        directory.path(),
        &format!("printf '%s' '{}'", remote_receipt(ORACLE_SHA256)),
    );
    let plan = plan(&request, "0123456789abcdef").expect("plan");
    let mut stdout = Vec::new();
    let code = execute(&plan, &mut stdout, Duration::from_secs(30)).expect("execute");
    assert_eq!(code, 0);
    let receipt = std::fs::read(&output).expect("receipt");
    assert_eq!(sha256_hex(&receipt), RECEIPT_SHA256);
    let sent = std::fs::read(directory.path().join("ssh.stdin")).expect("stdin payload");
    assert_eq!(sha256_hex(&sent), PAYLOAD_SHA256);
    let command = std::fs::read(directory.path().join("ssh.command")).expect("command");
    assert_eq!(sha256_hex(&command), REMOTE_COMMAND_SHA256);
    let options = std::fs::read_to_string(directory.path().join("ssh.args")).expect("args");
    assert_eq!(
        options,
        "-T\n-o\nBatchMode=yes\n-o\nConnectTimeout=8\nregistered-node\n"
    );
    let summary = String::from_utf8(stdout).expect("utf8");
    assert_eq!(
        summary,
        format!(
            "{{\"receipt\": \"{}\", \"remote_root\": \"/q/admiralty-qualification-0123456789abcdef\", \"helper_exit\": 0, \"verifier_exit\": 0, \"oracle_unchanged\": true, \"scope_valid\": true, \"model_observed\": [{{\"providerID\": \"p\", \"modelID\": \"m\"}}], \"elapsed_s\": 12.345}}\n",
            output.display()
        )
    );
    // The receipt is never overwritten.
    assert!(execute(&plan, &mut Vec::new(), Duration::from_secs(30)).is_err());
}

#[test]
fn execute_reports_changed_oracle_as_failure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("receipt.json");
    let mut request = args(&output);
    request.ssh_binary = fake_ssh(
        directory.path(),
        &format!("printf '%s' '{}'", remote_receipt(&"0".repeat(64))),
    );
    let plan = plan(&request, "0123456789abcdef").expect("plan");
    let code = execute(&plan, &mut Vec::new(), Duration::from_secs(30)).expect("execute");
    assert_eq!(code, 1);
    let receipt = std::fs::read_to_string(&output).expect("receipt");
    assert!(receipt.contains("\"owner_oracle_matches\": false"));
}

#[test]
fn ssh_failure_is_unresolved_with_translated_stderr_digest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("receipt.json");
    let mut request = args(&output);
    request.ssh_binary = fake_ssh(
        directory.path(),
        "printf 'line1\\r\\nline2\\n' >&2\nexit 255",
    );
    let plan = plan(&request, "0123456789abcdef").expect("plan");
    let mut stdout = Vec::new();
    let code = execute(&plan, &mut stdout, Duration::from_secs(30)).expect("execute");
    assert_eq!(code, 2);
    assert_eq!(
        String::from_utf8(stdout).expect("utf8"),
        "{\"state\": \"unresolved\", \"remote_root\": \"/q/admiralty-qualification-0123456789abcdef\", \"ssh_exit\": 255, \"stderr_sha256\": \"2751a3a2f303ad21752038085e2b8c5f98ecff61a2e4ebbd43506a941725be80\"}\n"
    );
    assert!(!output.exists());
}

#[test]
fn ssh_deadline_kills_the_session_without_a_receipt() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("receipt.json");
    let mut request = args(&output);
    request.ssh_binary = fake_ssh(directory.path(), "exec sleep 30");
    let plan = plan(&request, "0123456789abcdef").expect("plan");
    let started = std::time::Instant::now();
    let result = execute(&plan, &mut Vec::new(), Duration::from_secs(1));
    assert!(matches!(result, Err(ToolError::Timeout(_))));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!output.exists());
}
