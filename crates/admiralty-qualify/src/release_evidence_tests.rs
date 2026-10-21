//! Ports of `tests/qualification/test_release_evidence.py::ReleaseEvidenceTests`.

use std::fs;
use std::os::unix::fs::symlink;

use super::*;

struct Fixture {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    revision: String,
    digest: String,
    now: i64,
}

impl Fixture {
    fn new() -> Self {
        // Keep qualification artifacts in the lane's durable target, never /tmp.
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/p31-qualification");
        fs::create_dir_all(&target).expect("create p31 qualification scratch");
        let scratch = tempfile::tempdir_in(&target).expect("create scratch");
        let root = scratch.path().to_path_buf();
        let artifact = root.join("evidence.json");
        fs::write(&artifact, "{\"synthetic_fixture\":true}\n").expect("write artifact");
        let digest = format!(
            "{:x}",
            Sha256::digest(fs::read(&artifact).expect("read artifact"))
        );
        let (now, offset) = fromisoformat("2026-10-19T14:20:00+00:00").expect("fixture clock");
        assert_eq!(offset, Some(0));
        Self {
            _scratch: scratch,
            root,
            revision: "a".repeat(40),
            digest,
            now,
        }
    }

    fn manifest(&self) -> PyValue {
        let checks: Vec<PyValue> = REQUIRED
            .iter()
            .map(|name| {
                py_dict! {
                    "id" => *name,
                    "status" => "passed",
                    "kind" => if actual_required(name) { "actual" } else { "synthetic" },
                    "revision" => self.revision.as_str(),
                    "started_utc" => "2026-10-18T14:00:00Z",
                    "finished_utc" => "2026-10-19T14:00:00Z",
                    "evidence" => "evidence.json",
                    "sha256" => self.digest.as_str(),
                    "savings_claim" => false,
                }
            })
            .collect();
        py_dict! {
            "schema_version" => 1,
            "source_revision" => self.revision.as_str(),
            "checks" => checks,
        }
    }

    fn qualify(&self, manifest: &PyValue) -> Result<PyDict, EvidenceError> {
        qualify(manifest, &self.root, &self.revision, Some(self.now))
    }
}

fn checks_mut(manifest: &mut PyValue) -> &mut Vec<PyValue> {
    match manifest
        .as_dict_mut()
        .and_then(|dict| dict.get_mut("checks"))
    {
        Some(PyValue::List(checks)) => checks,
        _ => panic!("fixture manifest has a checks list"),
    }
}

fn set(check: &mut PyValue, key: &str, value: impl Into<PyValue>) {
    check
        .as_dict_mut()
        .expect("check is a dict")
        .insert(key, value.into());
}

fn status(report: &PyDict) -> String {
    report
        .get("status")
        .and_then(PyValue::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn list_len(report: &PyDict, key: &str) -> usize {
    report
        .get(key)
        .and_then(PyValue::as_list)
        .map_or(0, <[PyValue]>::len)
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_complete_synthetic_metadata_fixture_only_admits_independent_review
#[test]
fn test_complete_synthetic_metadata_fixture_only_admits_independent_review() {
    let fixture = Fixture::new();
    let result = fixture
        .qualify(&fixture.manifest())
        .expect("valid manifest");
    assert_eq!(status(&result), "ready_for_independent_review");
    assert_eq!(list_len(&result, "verified_artifacts"), 12);
    for key in [
        "full_packet_acceptance",
        "deployment_authorized",
        "savings_claim",
    ] {
        assert!(result.get(key).is_some_and(|v| v.is_bool(false)), "{key}");
    }
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_missing_failed_unknown_and_short_soak_remain_incomplete
#[test]
fn test_missing_failed_unknown_and_short_soak_remain_incomplete() {
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest();
    let checks = checks_mut(&mut manifest);
    checks.pop();
    set(&mut checks[0], "status", "failed");
    set(&mut checks[1], "status", "unavailable");
    set(&mut checks[2], "started_utc", "2026-10-19T13:00:00Z");
    let result = fixture.qualify(&manifest).expect("valid manifest");
    assert_eq!(status(&result), "incomplete");
    assert_eq!(list_len(&result, "remaining"), 4);
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_stale_head_future_timestamp_and_synthetic_provider_cannot_pass
#[test]
fn test_stale_head_future_timestamp_and_synthetic_provider_cannot_pass() {
    let fixture = Fixture::new();
    let changes = [
        ("revision", "b".repeat(40)),
        ("finished_utc", "2026-10-20T14:00:00Z".to_owned()),
        ("kind", "synthetic".to_owned()),
    ];
    for (key, value) in changes {
        let mut manifest = fixture.manifest();
        set(&mut checks_mut(&mut manifest)[4], key, value);
        let result = fixture.qualify(&manifest).expect("valid manifest");
        assert_eq!(status(&result), "incomplete", "{key}");
    }
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_modified_evidence_and_savings_assertions_are_not_accepted
#[test]
fn test_modified_evidence_and_savings_assertions_are_not_accepted() {
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest();
    let checks = checks_mut(&mut manifest);
    let last = checks.len() - 1;
    set(&mut checks[last], "savings_claim", true);
    let result = fixture.qualify(&manifest).expect("valid manifest");
    let first = result
        .get("remaining")
        .and_then(PyValue::as_list)
        .and_then(|remaining| remaining.first())
        .and_then(PyValue::as_str)
        .unwrap_or_default()
        .to_owned();
    assert!(
        first.contains("cannot authorize a savings claim"),
        "{first}"
    );
    fs::write(fixture.root.join("evidence.json"), "changed").expect("modify evidence");
    let result = fixture
        .qualify(&fixture.manifest())
        .expect("valid manifest");
    assert_eq!(list_len(&result, "verified_artifacts"), 0);
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_duplicate_unknown_and_wrong_manifest_revision_are_invalid
#[test]
fn test_duplicate_unknown_and_wrong_manifest_revision_are_invalid() {
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest();
    let checks = checks_mut(&mut manifest);
    let first = checks[0].clone();
    checks.push(first);
    assert!(matches!(
        fixture.qualify(&manifest),
        Err(EvidenceError::Value(_))
    ));
    let mut manifest = fixture.manifest();
    manifest
        .as_dict_mut()
        .expect("manifest dict")
        .insert("source_revision", "b".repeat(40));
    assert!(matches!(
        fixture.qualify(&manifest),
        Err(EvidenceError::Value(_))
    ));
    assert!(load_manifest(br#"{"checks":[],"checks":[]}"#).is_err());
    assert!(py::loads_str(r#"{"checks":[],"checks":[]}"#, true).is_err());
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_traversal_symlink_and_nonregular_artifacts_are_refused
#[test]
fn test_traversal_symlink_and_nonregular_artifacts_are_refused() {
    let fixture = Fixture::new();
    symlink(
        fixture.root.join("evidence.json"),
        fixture.root.join("alias"),
    )
    .expect("alias");
    fs::create_dir(fixture.root.join("directory")).expect("directory");
    symlink(&fixture.root, fixture.root.join("dir-alias")).expect("dir alias");
    for path in [
        "../evidence.json",
        "/etc/passwd",
        "alias",
        "dir-alias/evidence.json",
        "directory",
    ] {
        let outcome = artifact_hash(&fixture.root, Some(&PyValue::from(path)));
        assert!(
            matches!(outcome, Err(EvidenceError::Os(_) | EvidenceError::Value(_))),
            "{path}: {outcome:?}"
        );
    }
}

/// Ported from tests/qualification/test_release_evidence.py::ReleaseEvidenceTests::test_zero_checks_reports_every_missing_gate
#[test]
fn test_zero_checks_reports_every_missing_gate() {
    let fixture = Fixture::new();
    let manifest = py_dict! {
        "schema_version" => 1,
        "source_revision" => fixture.revision.as_str(),
        "checks" => Vec::<PyValue>::new(),
    };
    let result = fixture.qualify(&manifest).expect("valid manifest");
    assert_eq!(list_len(&result, "remaining"), 12);
}
