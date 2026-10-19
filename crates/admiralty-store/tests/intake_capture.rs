//! Actual private Store/writer capture tests; fixture authorization is not a
//! production issuer and no task/provider/dispatch effect is installed.
use admiralty_core::{CommandId, PrincipalId, ProjectId, RootId, intake::*};
use admiralty_store::{
    Store, StoreError,
    intake::AuthorizedIntakeContext,
    intake_capture::{CaptureError, CaptureIntakeRequest, IntakeCaptureAuthority},
};
use rusqlite::Connection;
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
fn scope() -> AuthorizedIntakeScope {
    AuthorizedIntakeScope {
        principal: PrincipalId::new("operator").expect("principal"),
        project_id: Some(ProjectId::new("p").expect("project")),
        root_id: Some(RootId::new("r").expect("root")),
        authority_ref: "scope:capture".into(),
        authority_generation: 7,
    }
}
struct Authority {
    denied: AtomicBool,
    generation: AtomicU64,
    cap: AtomicUsize,
    calls: AtomicUsize,
}
impl Default for Authority {
    fn default() -> Self {
        Self {
            denied: AtomicBool::new(false),
            generation: AtomicU64::new(7),
            cap: AtomicUsize::new(MAX_SOURCE_BYTES),
            calls: AtomicUsize::new(0),
        }
    }
}
impl IntakeCaptureAuthority for Authority {
    fn authorize(&self, _: &CaptureIntakeRequest) -> Result<AuthorizedIntakeContext, CaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.denied.load(Ordering::SeqCst) {
            return Err(CaptureError::Denied);
        }
        let mut scope = scope();
        scope.authority_generation = self.generation.load(Ordering::SeqCst);
        Ok(AuthorizedIntakeContext {
            scope,
            policy: IntakePolicy {
                max_source_bytes: self.cap.load(Ordering::SeqCst),
                ..IntakePolicy::default()
            },
        })
    }
}
fn private() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().expect("scratch");
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
    d
}
fn open(path: &Path, authority: Arc<Authority>) -> Store {
    Store::open_with_intake_capture_authority(path, authority).expect("fixture Store")
}
fn request(id: &str, source: &str, text: &str, format: IntakeFormat) -> CaptureIntakeRequest {
    let provenance = SourceProvenance {
        schema_version: 1,
        source_id: source.into(),
        revision: "1".into(),
        captured_at: "2026-10-19T00:00:00Z".into(),
        requester: RequesterAttribution::Unknown {},
        origin: None,
    };
    let p = preview_intake(
        text.as_bytes(),
        format,
        &provenance,
        &scope(),
        &IntakePolicy::default(),
    )
    .expect("bounded preview");
    CaptureIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new(format!("command-{id}")).expect("command"),
        capture_id: id.into(),
        revision: 1,
        expected_revision: 0,
        parent_receipt_digest: None,
        scope: p.authorized_scope,
        format,
        provenance,
        source_payload: SourcePayload::InlineUtf8 { text: text.into() },
        preview_manifest_digest: p.manifest_digest,
        policy_revision: p.policy_revision,
        policy_digest: p.policy_digest,
    }
}
fn repreview(r: &mut CaptureIntakeRequest) {
    let SourcePayload::InlineUtf8 { text } = &r.source_payload else {
        panic!("inline fixture")
    };
    let p = preview_intake(
        text.as_bytes(),
        r.format,
        &r.provenance,
        &scope(),
        &IntakePolicy::default(),
    )
    .expect("repreview");
    r.preview_manifest_digest = p.manifest_digest;
    r.policy_digest = p.policy_digest;
    r.policy_revision = p.policy_revision;
}
fn count(path: &Path, table: &str) -> i64 {
    assert!(
        [
            "intake_capture_heads",
            "intake_capture_sources",
            "intake_capture_revisions",
            "work_order_intake_orders",
            "workplan_registry_plans",
            "workplan_registry_tasks",
            "domain_events",
            "outbox_intents"
        ]
        .contains(&table)
    );
    Connection::open(path)
        .expect("inspect")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .expect("count")
}
fn assert_no_enrichment(path: &Path) {
    for table in [
        "work_order_intake_orders",
        "workplan_registry_plans",
        "workplan_registry_tasks",
        "domain_events",
        "outbox_intents",
    ] {
        assert_eq!(count(path, table), 0, "unexpected effect in {table}");
    }
}
#[test]
fn original_multiline_unicode_request_survives_without_inferred_tasks() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let raw =
        "Do not remove the 5 ms guard. 🦀\n\nCopied: owner 10:14\n```rust\nassert!(guard);\n```\n";
    let r = request("request", "source", raw, IntakeFormat::PlainText);
    let result = store.capture_intake(r.clone()).expect("capture");
    assert!(!result.replayed);
    let saved = store
        .captured_intake(scope(), "request".into(), None)
        .expect("read")
        .expect("present");
    assert_eq!(saved.request, r);
    assert_eq!(saved.preview.requests.len(), 1);
    assert_eq!(saved.preview.requests[0].text, raw);
    assert_eq!(saved.preview.requests[0].raw_span.end, raw.len() as u32);
    assert!(saved.preview.requests[0].outcomes.is_empty());
    assert!(
        saved.preview.requests[0]
            .unresolved
            .contains(&UnresolvedFact::CriteriaUnknown)
    );
    assert_eq!(saved.receipt, result.receipt);
    let retry = store.capture_intake(r).expect("lost reply retry");
    assert!(retry.replayed);
    assert_eq!(retry.receipt, result.receipt);
    assert_eq!(retry.receipt_digest, result.receipt_digest);
    assert_eq!(count(&path, "intake_capture_revisions"), 1);
    assert_no_enrichment(&path);
}
#[test]
fn explicit_json_batch_and_unresolved_claims_are_saved_as_observations() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let raw=serde_json::to_string(&json!({"schema_version":1,"items":[{"text":"first\n\nparagraph","claimed_project_id":"foreign"},{"text":"second","outcomes":[{"id":"proposed","description":"Still needs criteria","source_spans":[],"constraints":[]}]}]})).expect("batch");
    let r = request("batch", "source", &raw, IntakeFormat::JsonBatch);
    store.capture_intake(r.clone()).expect("capture unresolved");
    let saved = store
        .captured_intake(scope(), "batch".into(), None)
        .expect("read")
        .expect("present");
    assert_eq!(saved.preview.requests.len(), 2);
    assert!(
        saved.preview.requests[0]
            .unresolved
            .contains(&UnresolvedFact::ClaimedScopeMismatch)
    );
    assert!(
        saved.preview.requests[1].outcomes[0]
            .unresolved
            .contains(&UnresolvedFact::TaskContractMissing)
    );
    assert_eq!(saved.request.scope.project_id, r.scope.project_id);
    assert_no_enrichment(&path);
}
#[test]
fn full_one_mib_plaintext_and_escaping_fit_the_explicit_serialization_budget() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    // Six serialized bytes per original byte exercises escaping above 1MiB
    // without changing the source budget or the generic Store read helper.
    let text = "\u{1}".repeat(MAX_SOURCE_BYTES);
    let r = request("full", "source", &text, IntakeFormat::PlainText);
    let receipt = store
        .capture_intake(r.clone())
        .expect("full source capture");
    let saved = store
        .captured_intake(scope(), "full".into(), None)
        .expect("read full")
        .expect("present");
    assert_eq!(saved.request.source_payload, r.source_payload);
    assert_eq!(saved.preview.raw_byte_length, MAX_SOURCE_BYTES as u32);
    assert_eq!(saved.receipt, receipt.receipt);
    let mut oversized = r;
    oversized.command_id = CommandId::new("oversize").expect("command");
    oversized.source_payload = SourcePayload::InlineUtf8 {
        text: "x".repeat(MAX_SOURCE_BYTES + 1),
    };
    assert!(matches!(
        store.capture_intake(oversized),
        Err(StoreError::IntakeCapture(CaptureError::Capacity))
    ));
    assert_eq!(count(&path, "intake_capture_revisions"), 1);
}
#[test]
fn current_authority_and_full_policy_are_required_on_every_historical_retry() {
    let d = private();
    let path = d.path().join("store.db");
    let r = request("request", "source", "unknown work", IntakeFormat::PlainText);
    let denied = Store::open(&path).expect("default");
    assert!(matches!(
        denied.capture_intake(r.clone()),
        Err(StoreError::IntakeCapture(CaptureError::Denied))
    ));
    drop(denied);
    let authority = Arc::new(Authority::default());
    let store = open(&path, authority.clone());
    let original = store.capture_intake(r.clone()).expect("record");
    authority.cap.store(MAX_SOURCE_BYTES - 1, Ordering::SeqCst);
    assert!(matches!(
        store.capture_intake(r.clone()),
        Err(StoreError::IntakeCapture(CaptureError::Denied))
    ));
    authority.cap.store(MAX_SOURCE_BYTES, Ordering::SeqCst);
    authority.generation.store(8, Ordering::SeqCst);
    assert!(matches!(
        store.capture_intake(r.clone()),
        Err(StoreError::IntakeCapture(CaptureError::Denied))
    ));
    authority.generation.store(7, Ordering::SeqCst);
    authority.denied.store(true, Ordering::SeqCst);
    assert!(matches!(
        store.capture_intake(r.clone()),
        Err(StoreError::IntakeCapture(CaptureError::Denied))
    ));
    authority.denied.store(false, Ordering::SeqCst);
    let retry = store.capture_intake(r).expect("authorized exact retry");
    assert_eq!(retry.receipt, original.receipt);
    assert_eq!(authority.calls.load(Ordering::SeqCst), 5);
    assert_eq!(
        store
            .captured_intake(scope(), "request".into(), None)
            .expect("historical read")
            .expect("present")
            .receipt,
        original.receipt
    );
}
#[test]
fn revisions_require_exact_parent_cas_and_a_new_immutable_source_identity() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let first = request("request", "source", "original", IntakeFormat::PlainText);
    let one = store.capture_intake(first.clone()).expect("one");
    let mut second = first.clone();
    second.command_id = CommandId::new("revision-two").expect("command");
    second.revision = 2;
    second.expected_revision = 1;
    second.parent_receipt_digest = Some(one.receipt_digest.clone());
    second.source_payload = SourcePayload::InlineUtf8 {
        text: "explicit revision; do not weaken guard".into(),
    };
    repreview(&mut second);
    assert!(matches!(
        store.capture_intake(second.clone()),
        Err(StoreError::IntakeCapture(CaptureError::Collision))
    ));
    second.provenance.revision = "2".into();
    repreview(&mut second);
    let mut bad = second.clone();
    bad.parent_receipt_digest = Some(format!("sha256:{}", "0".repeat(64)));
    assert!(matches!(
        store.capture_intake(bad),
        Err(StoreError::IntakeCapture(CaptureError::StaleCurrent))
    ));
    let two = store.capture_intake(second).expect("explicit lineage");
    assert_eq!(two.receipt.parent_receipt_digest, Some(one.receipt_digest));
    assert_eq!(
        store
            .capture_intake(first.clone())
            .expect("historical retry")
            .receipt,
        one.receipt
    );
    assert_eq!(
        store
            .captured_intake(scope(), "request".into(), Some(1))
            .expect("old read")
            .expect("present")
            .request,
        first
    );
    assert_eq!(count(&path, "intake_capture_revisions"), 2);
    assert_no_enrichment(&path);
}
#[test]
fn command_source_collisions_and_advisory_equal_content_remain_distinct() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("first", "source", "same content", IntakeFormat::PlainText);
    store.capture_intake(r.clone()).expect("record");
    let mut changed = r.clone();
    changed.source_payload = SourcePayload::InlineUtf8 {
        text: "changed".into(),
    };
    repreview(&mut changed);
    assert!(matches!(
        store.capture_intake(changed),
        Err(StoreError::IntakeCapture(CaptureError::Collision))
    ));
    let collision = request("other", "source", "same content", IntakeFormat::PlainText);
    assert!(matches!(
        store.capture_intake(collision),
        Err(StoreError::IntakeCapture(CaptureError::Collision))
    ));
    let intentional = request(
        "intentional",
        "new-source",
        "same content",
        IntakeFormat::PlainText,
    );
    store
        .capture_intake(intentional)
        .expect("intentional equal wording");
    let mut other = scope();
    other.principal = PrincipalId::new("other").expect("principal");
    assert!(
        store
            .captured_intake(other, "first".into(), None)
            .expect("redacted")
            .is_none()
    );
    let mut other = scope();
    other.authority_generation = 8;
    assert!(
        store
            .captured_intake(other, "first".into(), None)
            .expect("redacted")
            .is_none()
    );
    let mut other = scope();
    other.project_id = Some(ProjectId::new("foreign").expect("project"));
    assert!(
        store
            .captured_intake(other, "first".into(), None)
            .expect("redacted")
            .is_none()
    );
    assert!(
        store
            .captured_intake(scope(), "x".repeat(129), None)
            .is_err()
    );
    assert_eq!(count(&path, "intake_capture_revisions"), 2);
}
#[test]
fn late_sql_failure_rolls_back_all_capture_custody_before_any_ack() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("request", "source", "original", IntakeFormat::PlainText);
    let fault = Connection::open(&path).expect("fault");
    fault.execute_batch("CREATE TRIGGER fail_capture BEFORE INSERT ON intake_capture_revisions BEGIN SELECT RAISE(ABORT,'owned fixture write fault');END;").expect("install fault");
    assert!(matches!(
        store.capture_intake(r.clone()),
        Err(StoreError::IntakeCapture(CaptureError::Sqlite(_)))
    ));
    for table in [
        "intake_capture_heads",
        "intake_capture_sources",
        "intake_capture_revisions",
    ] {
        assert_eq!(count(&path, table), 0);
    }
    assert_no_enrichment(&path);
    fault
        .execute_batch("DROP TRIGGER fail_capture")
        .expect("remove fault");
    assert!(
        !store
            .capture_intake(r)
            .expect("same unaccepted command")
            .replayed
    );
}
#[test]
fn concurrent_identical_and_competing_cas_commands_have_single_durable_winners() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("request", "source", "first", IntakeFormat::PlainText);
    let replies = std::thread::scope(|s| {
        let a = s.spawn(|| store.capture_intake(r.clone()));
        let b = s.spawn(|| store.capture_intake(r.clone()));
        [a.join().expect("a"), b.join().expect("b")]
    });
    let results: Vec<_> = replies
        .into_iter()
        .map(|r| r.expect("record/replay"))
        .collect();
    assert_eq!(results.iter().filter(|r| r.replayed).count(), 1);
    assert_eq!(results[0].receipt, results[1].receipt);
    let mut a = r.clone();
    a.command_id = CommandId::new("next-a").expect("command");
    a.revision = 2;
    a.expected_revision = 1;
    a.parent_receipt_digest = Some(results[0].receipt_digest.clone());
    a.provenance.revision = "2-a".into();
    repreview(&mut a);
    let mut b = a.clone();
    b.command_id = CommandId::new("next-b").expect("command");
    b.provenance.revision = "2-b".into();
    repreview(&mut b);
    let replies = std::thread::scope(|s| {
        let a = s.spawn(|| store.capture_intake(a));
        let b = s.spawn(|| store.capture_intake(b));
        [a.join().expect("a"), b.join().expect("b")]
    });
    assert_eq!(replies.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        replies
            .iter()
            .filter(|r| matches!(
                r,
                Err(StoreError::IntakeCapture(CaptureError::StaleCurrent))
            ))
            .count(),
        1
    );
    assert_eq!(count(&path, "intake_capture_revisions"), 2);
}
#[test]
fn backup_rebuild_and_reopen_preserve_bytes_and_never_install_authority() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request(
        "request",
        "source",
        "original\n\nDo not infer",
        IntakeFormat::PlainText,
    );
    let receipt = store.capture_intake(r.clone()).expect("capture");
    store.rebuild().expect("rebuild");
    let backup = d.path().join("backup.db");
    store.backup(&backup).expect("backup");
    drop(store);
    for path in [path, backup] {
        let store = Store::open(path).expect("reopen");
        assert!(matches!(
            store.capture_intake(r.clone()),
            Err(StoreError::IntakeCapture(CaptureError::Denied))
        ));
        let saved = store
            .captured_intake(scope(), "request".into(), None)
            .expect("read")
            .expect("present");
        assert_eq!(saved.request, r);
        assert_eq!(saved.receipt, receipt.receipt);
    }
}
fn corrupt_without_schema_change(c: &Connection, sql: &str) {
    let trigger: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='intake_capture_revision_update'",
            [],
            |r| r.get(0),
        )
        .expect("original trigger");
    c.execute_batch("DROP TRIGGER intake_capture_revision_update")
        .expect("fixture unlock");
    c.execute_batch(sql).expect("controlled corruption");
    c.execute_batch(&trigger)
        .expect("restore exact trigger schema");
}
#[test]
fn malformed_oversized_and_corrupt_metadata_are_rejected_before_publication() {
    for corruption in [
        "preview",
        "scope",
        "malformed",
        "oversized",
        "raw_oversized",
        "receipt",
    ] {
        let d = private();
        let path = d.path().join("store.db");
        let store = open(&path, Arc::new(Authority::default()));
        let r = request("request", "source", "original", IntakeFormat::PlainText);
        store.capture_intake(r).expect("capture");
        let injection = Connection::open(&path).expect("fault");
        let sql = match corruption {
            "scope" => "UPDATE intake_capture_revisions SET principal_id='forged';",
            "malformed" => {
                "PRAGMA ignore_check_constraints=ON;UPDATE intake_capture_revisions SET request_json='not JSON';"
            }
            "oversized" => {
                "PRAGMA ignore_check_constraints=ON;UPDATE intake_capture_revisions SET preview_json=CAST(zeroblob(8388609) AS TEXT);"
            }
            "raw_oversized" => {
                "UPDATE intake_capture_revisions SET request_json=json_set(request_json,'$.source_payload.text',replace(hex(zeroblob(524289)),'0','x'));"
            }
            "receipt" => {
                "UPDATE intake_capture_revisions SET receipt_json=json_set(receipt_json,'$.revision',2);"
            }
            _ => {
                "UPDATE intake_capture_revisions SET preview_json=json_set(preview_json,'$.requests',json('[]'));"
            }
        };
        corrupt_without_schema_change(&injection, sql);
        let mut read_scope = scope();
        if corruption == "scope" {
            read_scope.principal = PrincipalId::new("forged").expect("read principal");
        }
        assert!(
            store
                .captured_intake(read_scope, "request".into(), None)
                .is_err(),
            "corrupt {corruption} must not escape"
        );
        assert!(store.rebuild().is_err());
        let backup = d.path().join("not-published.db");
        assert!(store.backup(&backup).is_err());
        assert!(!backup.exists());
        drop(store);
        drop(injection);
        assert!(Store::open(&path).is_err());
    }
}
#[test]
fn unsupported_artifacts_and_changed_format_or_manifest_never_create_rows() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("request", "source", "original", IntakeFormat::PlainText);
    let mut artifact = r.clone();
    artifact.source_payload = SourcePayload::ArtifactClaim {
        artifact_ref: "artifact:unverified".into(),
        pin_receipt_ref: "pin:unverified".into(),
        raw_digest: format!("sha256:{}", "0".repeat(64)),
        size_bytes: 8,
    };
    assert!(matches!(
        store.capture_intake(artifact),
        Err(StoreError::IntakeCapture(CaptureError::UnsupportedSource))
    ));
    let mut changed = r.clone();
    changed.format = IntakeFormat::JsonBatch;
    assert!(store.capture_intake(changed).is_err());
    let mut changed = r;
    changed.preview_manifest_digest = format!("sha256:{}", "0".repeat(64));
    assert!(store.capture_intake(changed).is_err());
    assert_eq!(count(&path, "intake_capture_revisions"), 0);
}
#[test]
fn capture_count_budget_rejects_new_records_without_evicting_originals() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    for n in 0..128 {
        store
            .capture_intake(request(
                &format!("capture-{n}"),
                &format!("source-{n}"),
                "original",
                IntakeFormat::PlainText,
            ))
            .expect("within budget");
    }
    assert!(matches!(
        store.capture_intake(request(
            "overflow",
            "overflow",
            "original",
            IntakeFormat::PlainText
        )),
        Err(StoreError::IntakeCapture(CaptureError::Capacity))
    ));
    assert_eq!(count(&path, "intake_capture_revisions"), 128);
    assert!(
        store
            .captured_intake(scope(), "capture-0".into(), None)
            .expect("read")
            .is_some()
    );
}
