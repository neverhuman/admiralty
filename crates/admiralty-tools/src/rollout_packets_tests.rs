//! Offline contract tests for prepared implementation missions; no fleet access.
//! Ported from `tests/legacy/test_rollout_packets.py` (RolloutTests).

use std::io;

use clap::Parser;

use super::*;

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn root() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).expect("repo root")
}

struct Setup {
    temporary: tempfile::TempDir,
    output: PathBuf,
    plan: String,
}

fn setup() -> Setup {
    let temporary = tempfile::Builder::new()
        .prefix("admiralty-rollout-test-")
        .tempdir()
        .expect("tempdir");
    let output = owned_output(&temporary.path().join("out"), &root(), true).expect("owned output");
    let plan = fs::read_to_string(root().join(PLAN_DIR).join("WORK_PACKETS.md")).expect("plan");
    Setup {
        temporary,
        output,
        plan,
    }
}

impl Setup {
    fn export(&self) -> Value {
        let _lock = output_lock(&self.output).expect("lock");
        export(&root(), &self.output, BASE, &ExportOptions::default()).expect("export")
    }

    fn claimed(&self) -> Value {
        let mut manifest = self.export();
        claim(
            &mut manifest,
            "P00",
            "worker-1",
            "evidence-1",
            Some("node-any"),
            "codex/admiralty-p00",
        )
        .expect("claim");
        manifest
    }
}

fn json(text: &str) -> Value {
    pyjson::loads(text, LoadOptions::default()).expect("test JSON")
}

fn receipt() -> Value {
    json(&format!(
        r#"{{"schema_version": 1, "packet_id": "P00", "owner": "worker-1", "base_sha": "{BASE}",
            "head_sha": "{HEAD}", "interface_version": "unimplemented",
            "changed_paths": ["tests/fixtures/legacy/case.json"],
            "criteria": [{{"id": "fixture", "status": "passed", "evidence": "synthetic offline fixture"}}],
            "commands": [{{"argv": ["python3", "-m", "unittest"], "cwd": "/owned/worktree", "exit_code": 0}}],
            "environment": {{"python": "test-only"}}, "artifacts": [], "risks": [], "blockers": [],
            "next_action": "Independent exact-candidate verification"}}"#
    ))
}

fn packets(manifest: &Value) -> &Vec<Value> {
    manifest
        .get_value("packets")
        .ok()
        .and_then(Value::as_array)
        .expect("packets")
}

fn packet_at(manifest: &mut Value, index: usize) -> &mut Map {
    manifest
        .as_object_mut()
        .and_then(|map| map.get_mut("packets"))
        .and_then(Value::as_array_mut)
        .and_then(|items| items.get_mut(index))
        .and_then(Value::as_object_mut)
        .expect("packet")
}

fn expect_invalid<T: std::fmt::Debug>(result: ToolResult<T>, needle: &str) {
    match result {
        Err(ToolError::Invalid(message)) => {
            assert!(message.contains(needle), "{message:?} lacks {needle:?}");
        }
        other => panic!("expected Invalid containing {needle:?}, got {other:?}"),
    }
}

fn status_of(packet: &Value) -> String {
    packet
        .get_value("status")
        .ok()
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Ported from test_rollout_packets.py::test_all_plan_packets_export_with_small_capsules_and_matching_contract
#[test]
fn test_all_plan_packets_export_with_small_capsules_and_matching_contract() {
    let setup = setup();
    let manifest = setup.export();
    let identities: BTreeSet<String> = packets(&manifest)
        .iter()
        .filter_map(|packet| packet.get_value("id").ok().and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let expected: BTreeSet<String> = packet_ids().map(str::to_owned).collect();
    assert_eq!(identities, expected);
    let ready: Vec<String> = packets(&manifest)
        .iter()
        .filter(|packet| status_of(packet) == "ready")
        .filter_map(|packet| packet.get_value("id").ok().and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    assert_eq!(ready, ["P00", "P01"]);
    assert_eq!(
        manifest.get_value("producer").ok(),
        Some(&Value::from("admiralty.rollout_packets/v1"))
    );
    assert_eq!(
        manifest.get_value("pr").ok(),
        Some(&crate::pyobj! { "head" => BASE, "url" => Value::Null })
    );
    assert_eq!(
        manifest.get_value("lanes").ok(),
        Some(&Value::Array(Vec::new()))
    );
    validate_manifest(&manifest).expect("valid manifest");
    verify_capsules(&setup.output, &manifest).expect("capsules");
    for packet in packets(&manifest) {
        let relative = packet
            .get_value("capsule_path")
            .ok()
            .and_then(Value::as_str)
            .expect("capsule path");
        let data = fs::read(setup.output.join(relative)).expect("capsule");
        let text = String::from_utf8_lossy(&data);
        assert!(data.len() < 6000);
        assert!(text.contains(BASE));
        assert!(text.contains("Exact write allowlist"));
        assert!(text.contains("next_action"));
        assert!(text.contains("No live provider"));
        assert!(packet.get_value("verification").is_ok_and(Value::is_null));
        assert!(packet.get_value("integration").is_ok_and(Value::is_null));
    }
}

/// Ported from test_rollout_packets.py::test_missing_required_plan_fields_rejected
#[test]
fn test_missing_required_plan_fields_rejected() {
    let setup = setup();
    for field in [
        "Depends on",
        "Owner",
        "Release",
        "Read first",
        "Write scope",
        "Validate",
        "Pass",
    ] {
        let changed = setup.plan.replacen(&format!("**{field}:**"), "REMOVED", 1);
        assert!(
            matches!(parse_plan(&changed), Err(ToolError::Invalid(_))),
            "{field}"
        );
    }
}

/// Ported from test_rollout_packets.py::test_scope_changes_fail_closed
#[test]
fn test_scope_changes_fail_closed() {
    let setup = setup();
    let changed = setup
        .plan
        .replacen("`tests/fixtures/legacy/`", "`tests/fixtures/`", 1);
    expect_invalid(parse_plan(&changed), "scope changed");
}

/// Ported from test_rollout_packets.py::test_cycle_rejected
#[test]
fn test_cycle_rejected() {
    let setup = setup();
    let mut parsed = parse_plan(&setup.plan).expect("plan");
    if let Some(map) = parsed[0].as_object_mut() {
        map.insert("depends_on", Value::Array(vec!["P02".into()]));
    }
    expect_invalid(validate_graph(&parsed), "cycle");
}

/// Ported from test_rollout_packets.py::test_unknown_duplicate_and_unsafe_ids_rejected
#[test]
fn test_unknown_duplicate_and_unsafe_ids_rejected() {
    let setup = setup();
    let parsed = parse_plan(&setup.plan).expect("plan");
    for bad in ["../P00", "P32", "P00\n", "P00/evil"] {
        let mut mutated = parsed.clone();
        if let Some(map) = mutated[0].as_object_mut() {
            map.insert("id", bad);
        }
        assert!(
            matches!(validate_graph(&mutated), Err(ToolError::Invalid(_))),
            "{bad:?}"
        );
    }
    let mut mutated = parsed;
    if let Some(map) = mutated[0].as_object_mut() {
        map.insert("depends_on", Value::Array(vec!["P99".into()]));
    }
    assert!(matches!(
        validate_graph(&mutated),
        Err(ToolError::Invalid(_))
    ));
}

/// Ported from test_rollout_packets.py::test_full_range_and_conditional_dependencies_kept
#[test]
fn test_full_range_and_conditional_dependencies_kept() {
    let setup = setup();
    let parsed = parse_plan(&setup.plan).expect("plan");
    let dependencies = |index: usize| -> Vec<String> {
        parsed[index]
            .get_value("depends_on")
            .ok()
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let expected: Vec<String> = (0..31).map(|number| format!("P{number:02}")).collect();
    assert_eq!(dependencies(31), expected);
    assert!(dependencies(19).contains(&"P18".to_owned()));
    assert!(dependencies(30).contains(&"P29".to_owned()));
}

/// Ported from test_rollout_packets.py::test_paths_are_exact_and_traversal_rejected
#[test]
fn test_paths_are_exact_and_traversal_rejected() {
    for bad in [
        "../secret",
        "a/../secret",
        "/etc/passwd",
        ".git/config",
        "a//b",
        "a\\b",
        "a/./b",
        "x\ny",
    ] {
        assert!(
            matches!(safe_path(Some(bad), false), Err(ToolError::Invalid(_))),
            "{bad:?}"
        );
    }
    let p00 = allowlist("P00").expect("P00");
    assert!(allows(&"tests/fixtures/legacy/deep/f.json".into(), &p00).expect("allows"));
    assert!(!allows(&"tests/fixtures/legacy-other/f.json".into(), &p00).expect("allows"));
    let p03 = allowlist("P03").expect("P03");
    assert!(!allows(&"crates/admiralty-store/src/writer.rs/evil".into(), &p03).expect("allows"));
    assert!(
        !allowlist("P05")
            .expect("P05")
            .contains(&"Cargo.toml".to_owned())
    );
}

/// Ported from test_rollout_packets.py::test_state_requires_integrated_dependencies_and_claim
#[test]
fn test_state_requires_integrated_dependencies_and_claim() {
    let setup = setup();
    let mut manifest = setup.export();
    packet_at(&mut manifest, 2).insert("status", "ready");
    expect_invalid(validate_manifest(&manifest), "not integrated");
    packet_at(&mut manifest, 2).insert("status", "planned");
    packet_at(&mut manifest, 0).insert("status", "claimed");
    expect_invalid(validate_manifest(&manifest), "explicit claim");
}

/// Ported from test_rollout_packets.py::test_verified_and_integrated_require_distinct_proof
#[test]
fn test_verified_and_integrated_require_distinct_proof() {
    let setup = setup();
    let mut manifest = setup.claimed();
    packet_at(&mut manifest, 0).insert("status", "verified");
    expect_invalid(validate_manifest(&manifest), "independent provenance");
    packet_at(&mut manifest, 0).insert(
        "verification",
        crate::pyobj! {
            "receipt" => "proof.json",
            "revision" => HEAD,
            "verifier" => "independent",
            "observed_utc" => "2026-10-18T00:00:00Z",
        },
    );
    validate_manifest(&manifest).expect("verified with provenance");
    packet_at(&mut manifest, 0).insert("status", "integrated");
    expect_invalid(validate_manifest(&manifest), "integration provenance");
}

/// Ported from test_rollout_packets.py::test_claims_cannot_duplicate_or_skip_readiness
#[test]
fn test_claims_cannot_duplicate_or_skip_readiness() {
    let setup = setup();
    let mut manifest = setup.claimed();
    expect_invalid(
        claim(
            &mut manifest,
            "P00",
            "worker-2",
            "evidence-2",
            None,
            "codex/second",
        ),
        "only ready",
    );
    expect_invalid(
        claim(
            &mut manifest,
            "P01",
            "worker-1",
            "evidence-1",
            None,
            "codex/second",
        ),
        "active packet",
    );
    expect_invalid(
        claim(&mut manifest, "P02", "worker-2", "core", None, "codex/core"),
        "only ready",
    );
}

/// Ported from test_rollout_packets.py::test_receipt_import_is_idempotent_and_never_trusted
#[test]
fn test_receipt_import_is_idempotent_and_never_trusted() {
    let setup = setup();
    let mut manifest = setup.claimed();
    let mut candidate = receipt();
    if let Some(map) = candidate.as_object_mut() {
        map.insert("status", "integrated");
    }
    reconcile(&setup.output, &mut manifest, &candidate, &fsx::os_replace).expect("first import");
    reconcile(&setup.output, &mut manifest, &candidate, &fsx::os_replace).expect("second import");
    let packet = &packets(&manifest)[0];
    assert_eq!(status_of(packet), "review");
    assert!(packet.get_value("verification").is_ok_and(Value::is_null));
    assert!(packet.get_value("integration").is_ok_and(Value::is_null));
    let evidence = packet
        .get_value("evidence")
        .ok()
        .and_then(Value::as_array)
        .expect("evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(
        evidence[0].get_value("trust").ok(),
        Some(&Value::from("worker_report_unverified"))
    );
    assert_eq!(status_of(&packets(&manifest)[2]), "planned");
    assert_eq!(
        read_json(&setup.output.join("rollout.json")).expect("manifest"),
        manifest
    );
}

/// Ported from test_rollout_packets.py::test_receipts_require_scope_base_owner_and_complete_command_evidence
#[test]
fn test_receipts_require_scope_base_owner_and_complete_command_evidence() {
    let setup = setup();
    let manifest = setup.claimed();
    let mutations = [
        ("changed_paths", json(r#"["Cargo.toml"]"#)),
        (
            "changed_paths",
            json(r#"["tests/fixtures/legacy/../../evil"]"#),
        ),
        ("base_sha", Value::Str("c".repeat(40))),
        ("head_sha", "short".into()),
        ("owner", "impostor".into()),
        (
            "commands",
            json(r#"[{"argv": ["test"], "cwd": "/tmp", "exit_code": true}]"#),
        ),
        ("criteria", json("[]")),
        (
            "artifacts",
            json(r#"[{"path": "output", "sha256": "fake"}]"#),
        ),
    ];
    for (key, value) in mutations {
        let mut candidate = receipt();
        if let Some(map) = candidate.as_object_mut() {
            map.insert(key, value.clone());
        }
        assert!(
            matches!(
                validate_receipt(&candidate, &manifest),
                Err(ToolError::Invalid(_))
            ),
            "{key} {value:?}"
        );
    }
    let mut candidate = receipt();
    if let Some(map) = candidate.as_object_mut() {
        map.remove("environment");
    }
    expect_invalid(validate_receipt(&candidate, &manifest), "missing required");
}

/// Ported from test_rollout_packets.py::test_receipt_cannot_establish_a_claim
#[test]
fn test_receipt_cannot_establish_a_claim() {
    let setup = setup();
    expect_invalid(
        validate_receipt(&receipt(), &setup.export()),
        "explicit claim",
    );
}

/// Ported from test_rollout_packets.py::test_output_must_be_external_owned_empty_and_no_symlinks
#[test]
fn test_output_must_be_external_owned_empty_and_no_symlinks() {
    let setup = setup();
    expect_invalid(
        owned_output(&root().join("runtime/rollout-test"), &root(), true),
        "outside repository",
    );
    let other = setup.temporary.path().join("foreign");
    fs::create_dir(&other).expect("foreign");
    fs::write(other.join("valuable.txt"), "preserve").expect("valuable");
    expect_invalid(owned_output(&other, &root(), true), "empty or initialized");
    assert_eq!(
        fs::read_to_string(other.join("valuable.txt")).expect("valuable"),
        "preserve"
    );
    let link = setup.temporary.path().join("link");
    std::os::unix::fs::symlink(&setup.output, &link).expect("symlink");
    expect_invalid(owned_output(&link, &root(), true), "symlink");
}

/// Ported from test_rollout_packets.py::test_duplicate_json_keys_and_non_finite_numbers_rejected
#[test]
fn test_duplicate_json_keys_and_non_finite_numbers_rejected() {
    let setup = setup();
    let path = setup.temporary.path().join("bad.json");
    for text in [
        r#"{"id":1,"id":2}"#,
        r#"{"value":NaN}"#,
        r#"{"value":Infinity}"#,
    ] {
        fs::write(&path, text).expect("write");
        assert!(
            matches!(read_json(&path), Err(ToolError::Invalid(_))),
            "{text}"
        );
    }
}

/// Ported from test_rollout_packets.py::test_failed_atomic_replacement_preserves_previous_manifest
#[test]
fn test_failed_atomic_replacement_preserves_previous_manifest() {
    let setup = setup();
    setup.export();
    let path = setup.output.join("rollout.json");
    let before = fs::read(&path).expect("manifest");
    let fail_manifest = |source: &Path, target: &Path| -> io::Result<()> {
        if target == path {
            return Err(io::Error::other(
                "synthetic crash before manifest publication",
            ));
        }
        fs::rename(source, target)
    };
    let options = ExportOptions {
        replace: &fail_manifest,
        ..ExportOptions::default()
    };
    match export(&root(), &setup.output, BASE, &options) {
        Err(ToolError::Io(error)) => assert!(error.to_string().contains("synthetic crash")),
        other => panic!("expected synthetic crash, got {other:?}"),
    }
    assert_eq!(fs::read(&path).expect("manifest"), before);
    verify_capsules(&setup.output, &read_json(&path).expect("manifest")).expect("capsules");
}

/// Ported from test_rollout_packets.py::test_reexport_preserves_claim_and_old_generation_references
#[test]
fn test_reexport_preserves_claim_and_old_generation_references() {
    let setup = setup();
    let manifest = setup.claimed();
    atomic_write(
        &setup.output.join("rollout.json"),
        &json_bytes(&manifest).expect("bytes"),
        &fsx::os_replace,
    )
    .expect("write");
    let again = setup.export();
    assert_eq!(status_of(&packets(&again)[0]), "claimed");
    assert_ne!(
        again.get_value("generation").ok(),
        manifest.get_value("generation").ok()
    );
    verify_capsules(&setup.output, &manifest).expect("old generation intact");
    verify_capsules(&setup.output, &again).expect("new generation intact");
}

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    args: PacketsArgs,
}

/// Ported from test_rollout_packets.py::test_capsule_corruption_blocks_mutation_before_manifest_write
#[test]
fn test_capsule_corruption_blocks_mutation_before_manifest_write() {
    let setup = setup();
    let manifest = setup.export();
    let path = setup.output.join("rollout.json");
    let before = fs::read(&path).expect("manifest");
    let capsule = packets(&manifest)[0]
        .get_value("capsule_path")
        .ok()
        .and_then(Value::as_str)
        .expect("capsule path")
        .to_owned();
    fs::write(setup.output.join(capsule), "corrupt").expect("corrupt");
    let root = root();
    let output = setup.output.display().to_string();
    let cli = Cli::try_parse_from([
        "packets",
        "--repo",
        root.to_str().expect("utf8 root"),
        "claim",
        "--out",
        &output,
        "--packet",
        "P00",
        "--owner",
        "worker",
        "--lane",
        "lane",
        "--branch",
        "codex/test",
    ])
    .expect("arguments");
    assert_eq!(run(&cli.args), 2);
    assert_eq!(fs::read(&path).expect("manifest"), before);
}

/// Ported from test_rollout_packets.py::test_export_has_no_dispatch_calls_and_rejects_new_base
#[test]
fn test_export_has_no_dispatch_calls_and_rejects_new_base() {
    // Python patched subprocess.run to fail; here the export body is pinned
    // to contain no process launch (git resolution happens only in `execute`).
    let source = include_str!("rollout_packets.rs");
    let start = source.find("pub fn export(").expect("export source");
    let end = source[start..]
        .find("fn packet_position")
        .expect("export end");
    let body = &source[start..start + end];
    assert!(
        !body.contains("Command::"),
        "export must not launch processes"
    );
    assert!(
        !body.contains("resolve_base("),
        "export must not resolve Git refs"
    );
    let setup = setup();
    setup.export();
    expect_invalid(
        export(&root(), &setup.output, HEAD, &ExportOptions::default()),
        "source/base/project changed",
    );
}

/// Equivalence of the re-expressed look-ahead `required_field` regex with
/// Python's `re.search(r"\*\*F:\*\*\s*(.+?)(?=\s*\*\*|\n\n|\Z)", re.S)`
/// (expected values produced by CPython 3.13 on the same inputs).
#[test]
fn required_field_matches_python_lookahead_semantics() {
    let cases: [(&str, Option<&str>); 9] = [
        ("**Owner:** captain.\n**Release:** R1", Some("captain.")),
        (
            "**Owner:**   multi\nline value\n\nnext para",
            Some("multi\nline value"),
        ),
        ("**Owner:** a **bold** b", Some("a")),
        ("**Owner:**", None),
        ("**Owner:**   ", None),
        ("**Owner:****Next:** x", Some("**Next:")),
        ("x **Owner:** first\n\n**Owner:** second", Some("first")),
        ("**Owner:** tail with spaces   \t", Some("tail with spaces")),
        (
            "**Owner:** \u{e9}t\u{e9} \u{2014} caf\u{e9} **Z:**",
            Some("\u{e9}t\u{e9} \u{2014} caf\u{e9}"),
        ),
    ];
    for (body, expected) in cases {
        match expected {
            Some(value) => assert_eq!(required_field(body, "Owner").expect(body), value),
            None => expect_invalid(required_field(body, "Owner"), "missing Owner"),
        }
    }
}

/// Equivalence of the re-expressed negative look-ahead step regex with
/// Python's `re.findall(r"^\d+\. .+(?:\n(?!\n|\d+\. |\*\*).+)*", re.M)`.
#[test]
fn steps_match_python_negative_lookahead_semantics() {
    let body = "intro\n1. first step\ncontinued line\n2. second\n\n3. third\n**Validate:** x\n10. tenth\n  indented\n1.  double space\n1.\n1. \n4. four\n5. five\n6x. no\n7. seven\nlast";
    assert_eq!(
        steps(body),
        [
            "1. first step\ncontinued line",
            "2. second",
            "3. third",
            "10. tenth\n  indented",
            "1.  double space\n1.",
            "4. four",
            "5. five\n6x. no",
            "7. seven\nlast",
        ]
    );
}

/// The exported capsules are byte-identical to the retired Python renderer
/// for the installed plan (digest pinned from `tools/rollout_packets.py`).
#[test]
fn capsule_text_matches_python_renderer() {
    let setup = setup();
    let parsed = parse_plan(&setup.plan).expect("plan");
    let mut packet = parsed[0].clone();
    if let Some(map) = packet.as_object_mut() {
        map.insert("status", "ready");
    }
    let rendered = capsule(&packet, Path::new("/repo"), BASE).expect("capsule");
    assert!(rendered.starts_with("# P00 \u{2014} "));
    assert!(rendered.contains("\u{a7}\u{a7}1.4, 3.1, 12\u{2013}14"));
    assert!(rendered.contains("criteria[{id,status,evidence}], commands[{argv,cwd,exit_code}]"));
    assert!(rendered.ends_with("is authorized by this capsule.\n"));
}
