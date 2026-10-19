pub use admiralty_server::services::native_lifecycle;
#[path = "../src/services/native_evidence_catalog.rs"]
pub mod native_evidence_catalog;

use admiralty_core::{
    AttemptId, CheckSpec, DomainEvent, LifecycleAction, LifecycleCommand, PrincipalId, ProjectId,
    RootId, SchedulingState, SourceIdentity, TaskContract, TaskId, TaskView, content_digest,
    nodes::NodeId,
    verification::{
        RequiredNegativeCheck, VerificationInputs, VerificationPolicy, validate_receipt,
    },
};
use admiralty_runtime::{
    resource_gate::{AdmissionIdentity, EnforcementObservation, RecoveryIdentity},
    session::SessionHost,
    trusted_checks::{ExecutedChecks, execute_checks},
};
use admiralty_store::Store;
use native_evidence_catalog::{
    MAX_EVIDENCE_CATALOG_RECORDS, NativeEvidenceCatalog, NativeEvidenceCatalogError,
};
use native_lifecycle::{
    NativeDecisionBinding, NativeEvidenceAuthority, NativeLifecycleAuthority, NativeLifecyclePolicy,
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

struct PrivateTemp(PathBuf);

impl PrivateTemp {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateTemp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn private_tempdir(prefix: &str) -> PrivateTemp {
    let path = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temp root")
        .join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .expect("create 0700 tempdir");
    PrivateTemp(path)
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(args)
        .current_dir(path)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "fixture git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8 git stdout")
        .trim()
        .to_owned()
}

fn create_private_checkout(base: &Path, name: &str, content: &str) -> (PathBuf, String, String) {
    let checkout = base.join(name);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&checkout)
        .expect("create 0700 checkout");
    git(&checkout, &["init", "-q"]);
    fs::write(checkout.join("file.txt"), content).expect("write file.txt");
    git(&checkout, &["add", "file.txt"]);
    git(
        &checkout,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "candidate commit",
        ],
    );
    fs::set_permissions(checkout.join(".git"), fs::Permissions::from_mode(0o700))
        .expect("chmod 0700 .git");
    let canonical = fs::canonicalize(&checkout).expect("canonical checkout");
    let commit_sha = git(&canonical, &["rev-parse", "--verify", "HEAD"]);
    let tree_sha = git(&canonical, &["rev-parse", "--verify", "HEAD^{tree}"]);
    (canonical, commit_sha, tree_sha)
}

fn make_policy() -> NativeLifecyclePolicy {
    NativeLifecyclePolicy {
        project_id: ProjectId::new("project").expect("project"),
        root_id: RootId::new("root").expect("root"),
        authority_ref: "grant:project".into(),
        source_producer: "trusted-verifier-service".into(),
        source_incarnation: "service-boot".into(),
        verification: VerificationPolicy {
            schema_version: 1,
            verifier: PrincipalId::new("independent-verifier").expect("verifier"),
            protected_oracle_digest: content_digest(&"protected-oracle-v1").expect("digest"),
            required_negative_checks: vec![RequiredNegativeCheck {
                check: CheckSpec {
                    id: "reject-missing".into(),
                    program: "git".into(),
                    args: vec![
                        "rev-parse".into(),
                        "--verify".into(),
                        "refs/heads/definitely-missing".into(),
                    ],
                    cwd: ".".into(),
                    timeout_secs: 10,
                },
                expected_exit_code: 128,
                fixture_digest: content_digest(&"missing-ref").expect("digest"),
            }],
        },
    }
}

fn make_contract(task_id: &str) -> TaskContract {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "project_id": "project",
        "root_id": "root",
        "task_id": task_id,
        "revision": 1,
        "title": "Verify candidate custody",
        "objective": "Ensure only authentic custody, checks, and closure are cataloged",
        "non_goals": [],
        "owned_paths": ["file.txt"],
        "forbidden_paths": [],
        "read_first": [],
        "dependencies": [],
        "criteria": [{"id": "clean", "text": "candidate is clean", "check_ids": ["clean"]}],
        "checks": [{
            "id": "clean",
            "program": "git",
            "args": ["diff", "--exit-code", "HEAD", "--", "file.txt"],
            "cwd": ".",
            "timeout_secs": 10
        }],
        "authority_ref": "grant:project",
        "max_attempts": 2,
        "simulated": false
    }))
    .expect("valid contract")
}

fn advance_task_to_awaiting_verification(
    store: &Store,
    task_id_str: &str,
    attempt_id_str: &str,
    candidate_ref: &str,
    seq_offset: usize,
) -> TaskView {
    let task_id = TaskId::new(task_id_str).expect("task_id");
    let attempt_id = AttemptId::new(attempt_id_str).expect("attempt_id");
    let events = [
        DomainEvent::TaskSubmitted {
            contract: make_contract(task_id_str),
        },
        DomainEvent::TaskClaimed {
            task_id: task_id.clone(),
            revision: 1,
            attempt_id: attempt_id.clone(),
            owner: PrincipalId::new("worker-node-1").expect("worker"),
        },
        DomainEvent::TaskStarted {
            task_id: task_id.clone(),
            attempt_id: attempt_id.clone(),
        },
        DomainEvent::CandidateSubmitted {
            task_id: task_id.clone(),
            attempt_id,
            candidate_ref: candidate_ref.to_owned(),
        },
    ];
    for (idx, event) in events.into_iter().enumerate() {
        store
            .append(
                SourceIdentity {
                    producer_id: "task-service".into(),
                    incarnation: "boot".into(),
                    sequence: (seq_offset + idx + 1).to_string(),
                },
                event,
                "2026-10-19T00:00:00Z".into(),
            )
            .expect("append task event");
    }
    store.snapshot().expect("snapshot").projection.tasks[&task_id].clone()
}

struct VerifiedFixture {
    task: TaskView,
    source_workspace: PathBuf,
    checkout: PathBuf,
    commit_sha: String,
    tree_sha: String,
    artifact_digest: String,
    custody_digest: String,
    closure_digest: String,
    receipt_digest: String,
    closed_identity: RecoveryIdentity,
    closed_observation: EnforcementObservation,
    checks: ExecutedChecks,
}

fn build_verified_fixture(
    base: &Path,
    store: &Store,
    policy: &NativeLifecyclePolicy,
    task_id_str: &str,
    attempt_id_str: &str,
    candidate_ref: &str,
    seq_offset: usize,
) -> VerifiedFixture {
    let task = advance_task_to_awaiting_verification(
        store,
        task_id_str,
        attempt_id_str,
        candidate_ref,
        seq_offset,
    );
    build_verified_fixture_for_existing_task(
        base,
        policy,
        task,
        &format!("workspace-{task_id_str}-{candidate_ref}"),
        &format!("checkout-{task_id_str}-{candidate_ref}"),
        &format!("spool-{task_id_str}-{candidate_ref}"),
        &format!("content-{task_id_str}-{candidate_ref}\n"),
    )
}

fn build_verified_fixture_for_existing_task(
    base: &Path,
    policy: &NativeLifecyclePolicy,
    task: TaskView,
    workspace_name: &str,
    checkout_name: &str,
    spool_name: &str,
    file_content: &str,
) -> VerifiedFixture {
    let (source_workspace, _, _) = create_private_checkout(base, workspace_name, file_content);
    let (checkout, commit_sha, tree_sha) =
        create_private_checkout(base, checkout_name, file_content);
    assert_ne!(source_workspace, checkout);
    let spool = base.join(spool_name);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&spool)
        .expect("create 0700 spool");
    let artifact_digest =
        content_digest(&("source-artifact", &commit_sha, &tree_sha)).expect("artifact digest");
    let inputs = VerificationInputs {
        project_id: task.contract.project_id.clone(),
        root_id: task.contract.root_id.clone(),
        task_id: task.contract.task_id.clone(),
        attempt_id: task.attempt_id.clone().expect("attempt_id"),
        candidate_ref: task.candidate_ref.clone().expect("candidate_ref"),
        contract_revision: task.contract.revision,
        contract_digest: content_digest(&task.contract).expect("contract digest"),
        verification_policy_digest: content_digest(&policy.verification).expect("policy digest"),
        source_commit_sha: commit_sha.clone(),
        source_tree_sha: tree_sha.clone(),
        source_artifact_digest: artifact_digest.clone(),
        protected_oracle_digest: policy.verification.protected_oracle_digest.clone(),
    };
    let checks = execute_checks(
        &SessionHost::new(),
        &inputs,
        &task.contract,
        &policy.verification,
        &checkout,
        &spool,
        &format!("checks-{checkout_name}"),
    )
    .expect("execute real independent checks");
    let validated = validate_receipt(
        &task.contract,
        &inputs,
        &policy.verification,
        checks.receipt(),
    )
    .expect("validate_receipt");
    let receipt_digest = validated.receipt_digest().to_owned();

    let unit = format!(
        "adm-resource-{}-{}.service",
        task.contract.task_id.as_str(),
        checkout_name
    );
    let invocation_id = "0123456789abcdef0123456789abcdef".to_owned();
    let main_pid = 4242_u32;
    let process_start = "987654".to_owned();
    let closed_identity = RecoveryIdentity {
        identity: AdmissionIdentity {
            project_id: task.contract.project_id.clone(),
            task_id: task.contract.task_id.clone(),
            attempt_id: task.attempt_id.clone().expect("attempt_id"),
            node_id: NodeId::new("node-1").expect("node_id"),
            lease_id: format!("lease-{checkout_name}"),
        },
        launch_spec_digest: content_digest(&("launch-spec", checkout_name)).expect("digest"),
        worker_environment_digest: content_digest(&("worker-env", checkout_name)).expect("digest"),
        unit: unit.clone(),
        run_id: format!("run-{checkout_name}"),
        permit_id: format!("permit-{checkout_name}"),
        reservation_ref: format!("res-{checkout_name}"),
        host_incarnation: "boot-incarnation-1".into(),
        backend_config_digest: content_digest(&"backend-config").expect("digest"),
        invocation_id: Some(invocation_id.clone()),
        main_pid: Some(main_pid),
        process_start: Some(process_start.clone()),
    };
    // Actual ClosedAdmission.observation.actual_cwd is the ORIGINAL PreparedWorkspace,
    // NOT the detached candidate checkout!
    let closed_observation = EnforcementObservation {
        unit,
        invocation_id,
        main_pid,
        process_start,
        process_ids: vec![main_pid],
        cpu_set: BTreeSet::from([0]),
        memory_max_bytes: 64 * 1024 * 1024,
        tasks_max: 32,
        actual_executable: PathBuf::from("/usr/bin/git"),
        actual_cwd: source_workspace.clone(),
        actual_argv_digest: content_digest(&("argv", checkout_name)).expect("digest"),
        actual_environment_digest: content_digest(&("env", checkout_name)).expect("digest"),
        cgroup_path: PathBuf::from("/sys/fs/cgroup/admiralty.slice/unit.service"),
    };
    let closure_digest = content_digest(&(
        "closed-admission-v1",
        &closed_identity,
        &closed_observation,
        Some(0_i32),
        true,
    ))
    .expect("closure digest");
    let custody_digest = content_digest(&(
        "candidate_custody",
        &task.contract.task_id,
        &task.attempt_id,
        &task.candidate_ref,
        &commit_sha,
        &tree_sha,
        &artifact_digest,
        &closure_digest,
    ))
    .expect("custody digest");

    VerifiedFixture {
        task,
        source_workspace,
        checkout,
        commit_sha,
        tree_sha,
        artifact_digest,
        custody_digest,
        closure_digest,
        receipt_digest,
        closed_identity,
        closed_observation,
        checks,
    }
}

#[test]
fn default_deny_and_policy_validation_reject_unrecorded_or_malformed_evidence() {
    let dir = private_tempdir("nec-default-deny");
    let policy = make_policy();

    assert_eq!(
        NativeEvidenceCatalog::new(policy.clone(), 0).expect_err("zero limit rejected"),
        NativeEvidenceCatalogError::InvalidPolicy(
            "catalog limit must be within 1..=MAX_EVIDENCE_CATALOG_RECORDS"
        )
    );
    assert_eq!(
        NativeEvidenceCatalog::new(policy.clone(), MAX_EVIDENCE_CATALOG_RECORDS + 1)
            .expect_err("over-max limit rejected"),
        NativeEvidenceCatalogError::InvalidPolicy(
            "catalog limit must be within 1..=MAX_EVIDENCE_CATALOG_RECORDS"
        )
    );
    let mut bad_policy = policy.clone();
    bad_policy.authority_ref = String::new();
    assert!(matches!(
        NativeEvidenceCatalog::new(bad_policy, 8),
        Err(NativeEvidenceCatalogError::InvalidPolicy(_))
    ));

    let catalog = Arc::new(NativeEvidenceCatalog::new(policy.clone(), 16).expect("valid catalog"));
    assert_eq!(catalog.limit(), 16);
    assert!(!catalog.is_fail_closed());
    assert_eq!(catalog.policy().project_id, policy.project_id);
    assert_eq!(catalog.active_count().expect("active_count"), 0);
    assert_eq!(catalog.tombstone_count().expect("tombstone_count"), 0);

    let authority = Arc::new(
        NativeLifecycleAuthority::new(policy.clone(), catalog.clone(), 8)
            .expect("lifecycle authority"),
    );
    let store = Store::open_with_lifecycle_authority(dir.path().join("store.sqlite"), authority)
        .expect("open store");
    let fx = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );

    let unrecorded_binding = NativeDecisionBinding {
        project_id: fx.task.contract.project_id.clone(),
        root_id: fx.task.contract.root_id.clone(),
        task_id: fx.task.contract.task_id.clone(),
        contract_digest: content_digest(&fx.task.contract).expect("digest"),
        attempt_id: fx.task.attempt_id.clone().expect("attempt"),
        worker: fx.task.owner.clone().expect("worker"),
        verifier: policy.verification.verifier.clone(),
        authority_ref: fx.task.contract.authority_ref.clone(),
        policy_digest: content_digest(&policy.verification).expect("digest"),
        checks_execution_digest: fx.checks.execution_digest().to_owned(),
        receipt_digest: fx.receipt_digest.clone(),
        closure_digest: fx.closure_digest.clone(),
        source_artifact_digest: fx.artifact_digest.clone(),
        protected_oracle_digest: policy.verification.protected_oracle_digest.clone(),
        candidate_ref: "cand-1".into(),
        source_commit_sha: fx.commit_sha.clone(),
        source_tree_sha: fx.tree_sha.clone(),
    };
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &unrecorded_binding,
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
    assert!(!catalog.remains_current(&unrecorded_binding, &fx.custody_digest));
    assert!(!catalog.remains_current(&unrecorded_binding, "not-a-valid-digest"));

    let forged_cmd = LifecycleCommand {
        protocol_version: 2,
        task_id: fx.task.contract.task_id.clone(),
        expected_task_digest: content_digest(&fx.task).expect("task digest"),
        actor: policy.verification.verifier,
        action: LifecycleAction::CancelPending {
            reason: "unissued".into(),
        },
    };
    assert!(
        store
            .execute_lifecycle(
                SourceIdentity {
                    producer_id: "trusted-verifier-service".into(),
                    incarnation: "service-boot".into(),
                    sequence: "1".into(),
                },
                forged_cmd,
                "2026-10-19T00:01:00Z".into(),
                None,
            )
            .is_err()
    );
}

#[test]
fn exact_cwd_alignment_requires_original_prepared_workspace_not_detached_candidate_checkout() {
    let dir = private_tempdir("nec-cwd-alignment");
    let policy = make_policy();
    let catalog = NativeEvidenceCatalog::new(policy.clone(), 16).expect("catalog");
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");
    let fx = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );

    // 1. Passing identical path for source_workspace_path and candidate_checkout_path must fail closed
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx.task,
            &fx.checkout,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::CandidateMismatch(_))
    ));

    // 2. If ClosedAdmission.observation.actual_cwd points to detached candidate_checkout_path instead
    // of original source_workspace_path, registration MUST fail closed with EvidenceMismatch!
    let mut obs_with_checkout_cwd = fx.closed_observation.clone();
    obs_with_checkout_cwd.actual_cwd = fx.checkout.clone();
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &obs_with_checkout_cwd,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::EvidenceMismatch(_))
    ));

    // 3. If ClosedAdmission.observation.actual_cwd points to an unrelated directory, fail closed!
    let mut obs_with_unrelated_cwd = fx.closed_observation.clone();
    obs_with_unrelated_cwd.actual_cwd = dir.path().join("unrelated-cwd");
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &obs_with_unrelated_cwd,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::EvidenceMismatch(_))
    ));

    // 4. Positive path: actual_cwd == fx.source_workspace and fx.source_workspace != fx.checkout succeeds
    let published = catalog
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record with distinct source_workspace and candidate_checkout");
    assert_eq!(published.source_workspace_path(), fx.source_workspace);
    assert_eq!(published.candidate_checkout_path(), fx.checkout);
    assert_ne!(
        published.source_workspace_path(),
        published.candidate_checkout_path()
    );

    // 5. During authenticate_evidence, supplying an observation whose actual_cwd was swapped to
    // candidate_checkout_path fails closed!
    assert_eq!(
        catalog.authenticate_parts_for_test(
            published.binding(),
            &fx.checks,
            &fx.closed_identity,
            &obs_with_checkout_cwd,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
}

#[test]
fn exact_binding_checks_and_closure_match_and_every_single_field_mismatch_fails_closed() {
    let dir = private_tempdir("nec-exact-match");
    let policy = make_policy();
    let catalog = NativeEvidenceCatalog::new(policy.clone(), 32).expect("catalog");
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");
    let fx = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );
    let fx_other = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-2",
        "attempt-1",
        "cand-2",
        10,
    );

    // Pre-registration negative checks: bad task state, simulated flag, worker==verifier, non-zero exit
    let mut simulated_task = fx.task.clone();
    simulated_task.contract.simulated = true;
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &simulated_task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::CandidateMismatch(_))
    ));

    let mut wrong_state_task = fx.task.clone();
    wrong_state_task.state = SchedulingState::Running;
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &wrong_state_task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::CandidateMismatch(_))
    ));

    let mut self_verified_task = fx.task.clone();
    self_verified_task.owner = Some(policy.verification.verifier.clone());
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &self_verified_task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::CandidateMismatch(_))
    ));

    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(1),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::EvidenceMismatch(_))
    ));

    // Positive registration
    let published = catalog
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record valid candidate");

    assert_eq!(published.custody_digest(), fx.custody_digest);
    assert!(published.publication_digest().starts_with("sha256:"));
    assert!(published.proof_key().starts_with("sha256:"));
    assert_eq!(
        published.decision_evidence().custody_digest,
        fx.custody_digest
    );
    assert_eq!(published.recorded_sequence(), 1);
    assert_eq!(catalog.active_count().expect("active"), 1);
    assert_eq!(catalog.tombstone_count().expect("tombstones"), 0);

    // Exact idempotent replay returns identical publication record
    let replay = catalog
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("idempotent replay");
    assert_eq!(replay.publication_digest(), published.publication_digest());
    assert_eq!(replay.recorded_sequence(), published.recorded_sequence());

    // Conflicting proof_key under a different custody_digest fails with RecordConflict
    let different_custody = content_digest(&"different-custody").expect("digest");
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &different_custody,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::RecordConflict(_))
    ));

    let summary = catalog
        .lookup_by_custody_digest(&fx.custody_digest)
        .expect("lookup")
        .expect("present");
    assert_eq!(summary.custody_digest, fx.custody_digest);
    assert_eq!(summary.source_workspace_path, fx.source_workspace);
    assert_eq!(summary.candidate_checkout_path, fx.checkout);
    assert_eq!(
        catalog
            .lookup_by_task_attempt(
                &fx.task.contract.task_id,
                fx.task.attempt_id.as_ref().expect("attempt")
            )
            .expect("lookup by attempt"),
        Some(summary)
    );

    let binding = published.binding().clone();
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        Some(fx.custody_digest.clone())
    );
    assert!(catalog.remains_current(&binding, &fx.custody_digest));

    // Every single field of NativeDecisionBinding must be checked
    let other_digest = content_digest(&"tampered-field").expect("digest");
    let binding_mutations: &[fn(&mut NativeDecisionBinding, &str)] = &[
        |b, _| b.project_id = ProjectId::new("other-project").expect("id"),
        |b, _| b.root_id = RootId::new("other-root").expect("id"),
        |b, _| b.task_id = TaskId::new("other-task").expect("id"),
        |b, d| b.contract_digest = d.to_owned(),
        |b, _| b.attempt_id = AttemptId::new("other-attempt").expect("id"),
        |b, _| b.worker = PrincipalId::new("other-worker").expect("id"),
        |b, _| b.verifier = PrincipalId::new("other-verifier").expect("id"),
        |b, _| b.authority_ref = "grant:other".into(),
        |b, d| b.policy_digest = d.to_owned(),
        |b, d| b.checks_execution_digest = d.to_owned(),
        |b, d| b.receipt_digest = d.to_owned(),
        |b, d| b.closure_digest = d.to_owned(),
        |b, d| b.source_artifact_digest = d.to_owned(),
        |b, d| b.protected_oracle_digest = d.to_owned(),
        |b, _| b.candidate_ref = "cand-tampered".into(),
        |b, _| b.source_commit_sha = "0000000000000000000000000000000000000000".into(),
        |b, _| b.source_tree_sha = "0000000000000000000000000000000000000000".into(),
    ];
    for mutate in binding_mutations {
        let mut tampered = binding.clone();
        mutate(&mut tampered, &other_digest);
        assert_eq!(
            catalog.authenticate_parts_for_test(
                &tampered,
                &fx.checks,
                &fx.closed_identity,
                &fx.closed_observation,
                Some(0),
                true,
                &fx.closure_digest,
            ),
            None
        );
        assert!(!catalog.remains_current(&tampered, &fx.custody_digest));
    }

    // Mismatched ExecutedChecks, exit_code, success, closure_digest, RecoveryIdentity, EnforcementObservation
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx_other.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(1),
            true,
            &fx.closure_digest,
        ),
        None
    );
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            false,
            &fx.closure_digest,
        ),
        None
    );
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &other_digest,
        ),
        None
    );

    let mut bad_identity = fx.closed_identity.clone();
    bad_identity.run_id = "tampered-run-id".into();
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx.checks,
            &bad_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );

    let mut bad_obs = fx.closed_observation.clone();
    bad_obs.main_pid = 9999;
    assert_eq!(
        catalog.authenticate_parts_for_test(
            &binding,
            &fx.checks,
            &fx.closed_identity,
            &bad_obs,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
}

#[test]
fn revocation_and_supersession_tombstones_are_permanent_and_block_replay() {
    let dir = private_tempdir("nec-revocation");
    let policy = make_policy();
    let catalog = NativeEvidenceCatalog::new(policy.clone(), 32).expect("catalog");
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");
    let fx1 = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );

    let pub1 = catalog
        .record_component_fixture_for_test(
            &fx1.task,
            &fx1.source_workspace,
            &fx1.checkout,
            &fx1.commit_sha,
            &fx1.tree_sha,
            &fx1.artifact_digest,
            &fx1.custody_digest,
            &fx1.closure_digest,
            &fx1.closed_identity,
            &fx1.closed_observation,
            Some(0),
            true,
            &fx1.checks,
        )
        .expect("record fx1");
    assert!(catalog.remains_current(pub1.binding(), &fx1.custody_digest));

    // Supersede task-1 / attempt-1 with a second distinct checkout & check run for the same task state
    let fx1_v2 = build_verified_fixture_for_existing_task(
        dir.path(),
        &policy,
        fx1.task.clone(),
        "workspace-task-1-v2",
        "checkout-task-1-v2",
        "spool-task-1-v2",
        "updated candidate content\n",
    );
    let pub1_v2 = catalog
        .record_component_fixture_for_test(
            &fx1_v2.task,
            &fx1_v2.source_workspace,
            &fx1_v2.checkout,
            &fx1_v2.commit_sha,
            &fx1_v2.tree_sha,
            &fx1_v2.artifact_digest,
            &fx1_v2.custody_digest,
            &fx1_v2.closure_digest,
            &fx1_v2.closed_identity,
            &fx1_v2.closed_observation,
            Some(0),
            true,
            &fx1_v2.checks,
        )
        .expect("supersede fx1 with fx1_v2");

    // Prior candidate is now permanently tombstoned; new candidate is current
    assert!(!catalog.remains_current(pub1.binding(), &fx1.custody_digest));
    assert_eq!(
        catalog.authenticate_parts_for_test(
            pub1.binding(),
            &fx1.checks,
            &fx1.closed_identity,
            &fx1.closed_observation,
            Some(0),
            true,
            &fx1.closure_digest,
        ),
        None
    );
    assert!(
        catalog
            .revocation_status(&fx1.custody_digest)
            .expect("status")
            .is_some()
    );
    assert!(catalog.remains_current(pub1_v2.binding(), &fx1_v2.custody_digest));

    // Replay of superseded fx1 fails closed with RevokedEvidence
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx1.task,
            &fx1.source_workspace,
            &fx1.checkout,
            &fx1.commit_sha,
            &fx1.tree_sha,
            &fx1.artifact_digest,
            &fx1.custody_digest,
            &fx1.closure_digest,
            &fx1.closed_identity,
            &fx1.closed_observation,
            Some(0),
            true,
            &fx1.checks,
        ),
        Err(NativeEvidenceCatalogError::RevokedEvidence(_))
    ));

    // Explicit revocation by task_attempt revokes fx1_v2 and blocks replay
    let tombstone = catalog
        .revoke_task_attempt(
            &fx1_v2.task.contract.task_id,
            fx1_v2.task.attempt_id.as_ref().expect("attempt"),
            "operator revoked task attempt",
        )
        .expect("revoke_task_attempt")
        .expect("tombstone present");
    assert_eq!(tombstone.custody_digest, fx1_v2.custody_digest);
    assert_eq!(tombstone.reason, "operator revoked task attempt");
    assert!(!catalog.remains_current(pub1_v2.binding(), &fx1_v2.custody_digest));
    assert_eq!(
        catalog
            .lookup_by_custody_digest(&fx1_v2.custody_digest)
            .expect("lookup"),
        None
    );
    assert_eq!(
        catalog
            .lookup_by_task_attempt(
                &fx1_v2.task.contract.task_id,
                fx1_v2.task.attempt_id.as_ref().expect("attempt")
            )
            .expect("lookup"),
        None
    );
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx1_v2.task,
            &fx1_v2.source_workspace,
            &fx1_v2.checkout,
            &fx1_v2.commit_sha,
            &fx1_v2.tree_sha,
            &fx1_v2.artifact_digest,
            &fx1_v2.custody_digest,
            &fx1_v2.closure_digest,
            &fx1_v2.closed_identity,
            &fx1_v2.closed_observation,
            Some(0),
            true,
            &fx1_v2.checks,
        ),
        Err(NativeEvidenceCatalogError::RevokedEvidence(_))
    ));
}

#[test]
fn callbacks_perform_no_disk_io_and_pre_grant_revalidation_revokes_drifted_checkout() {
    let dir = private_tempdir("nec-custody-drift");
    let policy = make_policy();
    let catalog = NativeEvidenceCatalog::new(policy.clone(), 16).expect("catalog");
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");
    let fx = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );

    let published = catalog
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record candidate");

    // Clean revalidation passes
    catalog
        .revalidate_with_probe_or_revoke_for_test(
            &fx.custody_digest,
            &fx.source_workspace,
            &fx.checkout,
            || Ok(()),
        )
        .expect("clean candidate custody revalidates");

    // Removing the checkout directory on disk does NOT cause remains_current to perform disk I/O
    fs::remove_dir_all(&fx.checkout).expect("remove checkout on disk");
    assert!(catalog.remains_current(published.binding(), &fx.custody_digest));

    // Pre-grant revalidation outside the Store callback detects custody failure and atomically revokes!
    assert!(matches!(
        catalog.revalidate_with_probe_or_revoke_for_test(
            &fx.custody_digest,
            &fx.source_workspace,
            &fx.checkout,
            || Err("SourceCandidate::revalidate_custody failed on disk"),
        ),
        Err(NativeEvidenceCatalogError::CustodyDrifted(_))
    ));

    // Now remains_current and authenticate_parts_for_test fail closed, and tombstone is permanent
    assert!(!catalog.remains_current(published.binding(), &fx.custody_digest));
    assert_eq!(
        catalog.authenticate_parts_for_test(
            published.binding(),
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
    assert!(
        catalog
            .revocation_status(&fx.custody_digest)
            .expect("revocation status")
            .is_some()
    );
    assert!(matches!(
        catalog.record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        ),
        Err(NativeEvidenceCatalogError::RevokedEvidence(_))
    ));
}

#[test]
fn bounded_capacity_reserves_revocation_slots_and_revoking_active_entry_with_full_tombstones_succeeds()
 {
    let dir = private_tempdir("nec-capacity");
    let policy = make_policy();
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");

    let fx1 = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );
    let fx2 = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-2",
        "attempt-1",
        "cand-2",
        10,
    );
    let fx3 = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-3",
        "attempt-1",
        "cand-3",
        20,
    );

    // Part A: Total slots (active + tombstones) are bounded by `limit = 2`, reserving tombstone space
    let catalog = NativeEvidenceCatalog::new(policy.clone(), 2).expect("catalog limit 2");
    let pub1 = catalog
        .record_component_fixture_for_test(
            &fx1.task,
            &fx1.source_workspace,
            &fx1.checkout,
            &fx1.commit_sha,
            &fx1.tree_sha,
            &fx1.artifact_digest,
            &fx1.custody_digest,
            &fx1.closure_digest,
            &fx1.closed_identity,
            &fx1.closed_observation,
            Some(0),
            true,
            &fx1.checks,
        )
        .expect("record 1");
    let pub2 = catalog
        .record_component_fixture_for_test(
            &fx2.task,
            &fx2.source_workspace,
            &fx2.checkout,
            &fx2.commit_sha,
            &fx2.tree_sha,
            &fx2.artifact_digest,
            &fx2.custody_digest,
            &fx2.closure_digest,
            &fx2.closed_identity,
            &fx2.closed_observation,
            Some(0),
            true,
            &fx2.checks,
        )
        .expect("record 2");
    assert_eq!(catalog.active_count().expect("active"), 2);

    // 3rd record hits capacity and fails closed without evicting 1 or 2
    assert_eq!(
        catalog
            .record_component_fixture_for_test(
                &fx3.task,
                &fx3.source_workspace,
                &fx3.checkout,
                &fx3.commit_sha,
                &fx3.tree_sha,
                &fx3.artifact_digest,
                &fx3.custody_digest,
                &fx3.closure_digest,
                &fx3.closed_identity,
                &fx3.closed_observation,
                Some(0),
                true,
                &fx3.checks,
            )
            .expect_err("3rd active record rejected at capacity"),
        NativeEvidenceCatalogError::CatalogFull
    );
    assert!(catalog.remains_current(pub1.binding(), &fx1.custody_digest));
    assert!(catalog.remains_current(pub2.binding(), &fx2.custody_digest));

    // Revoking active record 1 transitions 1 active -> 1 tombstone (1 active + 1 tombstone == 2 == limit)
    catalog
        .revoke_custody_digest(&fx1.custody_digest, "revoke 1")
        .expect("revoke 1 succeeds");
    assert!(!catalog.remains_current(pub1.binding(), &fx1.custody_digest));
    assert!(catalog.remains_current(pub2.binding(), &fx2.custody_digest));

    // Because 1 tombstone + 1 active == 2 == limit, inserting a new 3rd record is rejected so active 2's
    // reserved revocation slot cannot be stolen!
    assert_eq!(
        catalog
            .record_component_fixture_for_test(
                &fx3.task,
                &fx3.source_workspace,
                &fx3.checkout,
                &fx3.commit_sha,
                &fx3.tree_sha,
                &fx3.artifact_digest,
                &fx3.custody_digest,
                &fx3.closure_digest,
                &fx3.closed_identity,
                &fx3.closed_observation,
                Some(0),
                true,
                &fx3.checks,
            )
            .expect_err("new record rejected when active + tombstones == limit"),
        NativeEvidenceCatalogError::CatalogFull
    );

    // Revoking active record 2 succeeds (0 active + 2 tombstones == 2 == limit)
    catalog
        .revoke_custody_digest(&fx2.custody_digest, "revoke 2")
        .expect("revoke 2 succeeds");
    assert!(!catalog.remains_current(pub2.binding(), &fx2.custody_digest));
    assert_eq!(catalog.active_count().expect("active"), 0);
    assert_eq!(catalog.tombstone_count().expect("tombstones"), 2);

    // Attempting to revoke an unrecorded 3rd digest when full returns CatalogFull AND latches fail-closed!
    assert_eq!(
        catalog
            .revoke_custody_digest(&fx3.custody_digest, "revoke unrecorded 3")
            .expect_err("3rd tombstone rejected at capacity"),
        NativeEvidenceCatalogError::CatalogFull
    );
    assert!(catalog.is_fail_closed());

    // Part B: Even if a limit=1 catalog holds tombstone A + active B simultaneously, revoking B
    // removes active B first and succeeds so remains_current(B) becomes false immediately!
    let catalog_limit1 = NativeEvidenceCatalog::new(policy, 1).expect("catalog limit 1");
    let pub_b = catalog_limit1
        .record_component_fixture_for_test(
            &fx2.task,
            &fx2.source_workspace,
            &fx2.checkout,
            &fx2.commit_sha,
            &fx2.tree_sha,
            &fx2.artifact_digest,
            &fx2.custody_digest,
            &fx2.closure_digest,
            &fx2.closed_identity,
            &fx2.closed_observation,
            Some(0),
            true,
            &fx2.checks,
        )
        .expect("record B in limit 1");
    catalog_limit1.force_insert_tombstone_for_test(&fx1.custody_digest, "seeded tombstone A");
    assert_eq!(catalog_limit1.active_count().expect("active"), 1);
    assert_eq!(catalog_limit1.tombstone_count().expect("tombstones"), 1);

    // Drift revalidation on B when tombstone A is already at limit=1 STILL revokes B and turns off authority!
    assert!(matches!(
        catalog_limit1.revalidate_with_probe_or_revoke_for_test(
            &fx2.custody_digest,
            &fx2.source_workspace,
            &fx2.checkout,
            || Err("drift on B with full tombstone table"),
        ),
        Err(NativeEvidenceCatalogError::CustodyDrifted(_))
    ));
    assert!(!catalog_limit1.remains_current(pub_b.binding(), &fx2.custody_digest));
    assert_eq!(catalog_limit1.active_count().expect("active"), 0);
}

#[test]
fn contended_revocation_or_drift_revalidation_atomically_latches_catalog_fail_closed() {
    let dir = private_tempdir("nec-contended-revoke");
    let policy = make_policy();
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");
    let fx = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );

    // Case 1: Drift revalidation while catalog mutex is contended latches fail_closed = true
    let catalog1 = NativeEvidenceCatalog::new(policy.clone(), 8).expect("catalog1");
    let pub1 = catalog1
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record in catalog1");
    assert!(catalog1.remains_current(pub1.binding(), &fx.custody_digest));

    catalog1.with_locked_state_for_test(|| {
        let err = catalog1
            .revalidate_with_probe_or_revoke_for_test(
                &fx.custody_digest,
                &fx.source_workspace,
                &fx.checkout,
                || Err("custody drifted while mutex contended"),
            )
            .expect_err("drift revalidation fails under contention");
        assert!(matches!(err, NativeEvidenceCatalogError::CustodyDrifted(_)));
        assert!(catalog1.is_fail_closed());
    });

    // Even after the mutex is released, remains_current and authenticate_evidence are permanently false/None!
    assert!(catalog1.is_fail_closed());
    assert!(!catalog1.remains_current(pub1.binding(), &fx.custody_digest));
    assert_eq!(
        catalog1.authenticate_parts_for_test(
            pub1.binding(),
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
    assert_eq!(
        catalog1.lookup_by_custody_digest(&fx.custody_digest),
        Err(NativeEvidenceCatalogError::Unavailable)
    );

    // Case 2: Explicit revoke_custody_digest while catalog mutex is contended latches fail_closed = true
    let catalog2 = NativeEvidenceCatalog::new(policy, 8).expect("catalog2");
    let pub2 = catalog2
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record in catalog2");
    assert!(catalog2.remains_current(pub2.binding(), &fx.custody_digest));

    catalog2.with_locked_state_for_test(|| {
        assert_eq!(
            catalog2
                .revoke_custody_digest(&fx.custody_digest, "contended operator revoke")
                .expect_err("contended revoke returns Unavailable"),
            NativeEvidenceCatalogError::Unavailable
        );
        assert!(catalog2.is_fail_closed());
    });
    assert!(!catalog2.remains_current(pub2.binding(), &fx.custody_digest));
    assert_eq!(
        catalog2.authenticate_parts_for_test(
            pub2.binding(),
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None
    );
}

#[test]
fn in_flight_revocation_while_holding_state_guard_causes_callbacks_and_revalidation_to_deny() {
    let dir = private_tempdir("nec-inflight-latch-race");
    let policy = make_policy();
    let store = Store::open(dir.path().join("store.sqlite")).expect("store");
    let fx = build_verified_fixture(
        dir.path(),
        &store,
        &policy,
        "task-1",
        "attempt-1",
        "cand-1",
        0,
    );

    // 0. Production services::native_evidence_catalog module is registered and usable
    let prod_catalog =
        admiralty_server::services::native_evidence_catalog::NativeEvidenceCatalog::new(
            policy.clone(),
            8,
        )
        .expect("production module catalog");
    assert_eq!(prod_catalog.limit(), 8);
    assert!(!prod_catalog.is_fail_closed());

    // 1. In-flight revocation while `remains_current` holds `state` after initial check -> must return false
    let catalog_remains = Arc::new(NativeEvidenceCatalog::new(policy.clone(), 8).expect("catalog"));
    let pub_remains = catalog_remains
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record in catalog_remains");
    {
        let cat = catalog_remains.clone();
        let digest = fx.custody_digest.clone();
        catalog_remains.set_in_flight_hook_for_test(move || {
            assert_eq!(
                cat.revoke_custody_digest(&digest, "contended revoke inside remains_current"),
                Err(NativeEvidenceCatalogError::Unavailable)
            );
            assert!(cat.is_fail_closed());
        });
    }
    assert!(
        !catalog_remains.remains_current(pub_remains.binding(), &fx.custody_digest),
        "remains_current must deny when fail_closed latches while holding state guard"
    );
    assert!(catalog_remains.is_fail_closed());

    // 2. In-flight drift revocation while `authenticate_evidence` holds `state` -> must return None
    let catalog_auth = Arc::new(NativeEvidenceCatalog::new(policy.clone(), 8).expect("catalog"));
    let pub_auth = catalog_auth
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record in catalog_auth");
    {
        let cat = catalog_auth.clone();
        let digest = fx.custody_digest.clone();
        let ws = fx.source_workspace.clone();
        let co = fx.checkout.clone();
        catalog_auth.set_in_flight_hook_for_test(move || {
            let err = cat
                .revalidate_with_probe_or_revoke_for_test(&digest, &ws, &co, || {
                    Err("in-flight drift while authenticate_evidence holds state")
                })
                .expect_err("contended drift revalidation fails");
            assert!(matches!(err, NativeEvidenceCatalogError::CustodyDrifted(_)));
            assert!(cat.is_fail_closed());
        });
    }
    assert_eq!(
        catalog_auth.authenticate_parts_for_test(
            pub_auth.binding(),
            &fx.checks,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.closure_digest,
        ),
        None,
        "authenticate_evidence must deny when fail_closed latches while holding state guard"
    );
    assert!(catalog_auth.is_fail_closed());

    // 3. In-flight revocation while `record_candidate` holds `state` -> must return Err(Unavailable)
    let catalog_record = Arc::new(NativeEvidenceCatalog::new(policy.clone(), 8).expect("catalog"));
    {
        let cat = catalog_record.clone();
        let digest = fx.custody_digest.clone();
        catalog_record.set_in_flight_hook_for_test(move || {
            assert_eq!(
                cat.revoke_custody_digest(&digest, "contended revoke inside record_candidate"),
                Err(NativeEvidenceCatalogError::Unavailable)
            );
            assert!(cat.is_fail_closed());
        });
    }
    assert_eq!(
        catalog_record
            .record_component_fixture_for_test(
                &fx.task,
                &fx.source_workspace,
                &fx.checkout,
                &fx.commit_sha,
                &fx.tree_sha,
                &fx.artifact_digest,
                &fx.custody_digest,
                &fx.closure_digest,
                &fx.closed_identity,
                &fx.closed_observation,
                Some(0),
                true,
                &fx.checks,
            )
            .expect_err("in-flight latch during publication must deny"),
        NativeEvidenceCatalogError::Unavailable
    );
    assert!(catalog_record.is_fail_closed());

    // 4. In-flight revocation while `revalidate_candidate_or_revoke` holds `state` -> must return Err(Unavailable)
    let catalog_reval = Arc::new(NativeEvidenceCatalog::new(policy.clone(), 8).expect("catalog"));
    catalog_reval
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record in catalog_reval");
    {
        let cat = catalog_reval.clone();
        let digest = fx.custody_digest.clone();
        catalog_reval.set_in_flight_hook_for_test(move || {
            assert_eq!(
                cat.revoke_custody_digest(&digest, "contended revoke inside revalidation"),
                Err(NativeEvidenceCatalogError::Unavailable)
            );
            assert!(cat.is_fail_closed());
        });
    }
    assert_eq!(
        catalog_reval
            .revalidate_with_probe_or_revoke_for_test(
                &fx.custody_digest,
                &fx.source_workspace,
                &fx.checkout,
                || Ok(()),
            )
            .expect_err("in-flight latch during clean revalidation must deny"),
        NativeEvidenceCatalogError::Unavailable
    );
    assert!(catalog_reval.is_fail_closed());

    // 5. In-flight revocation while `lookup_by_custody_digest` / `lookup_by_task_attempt` holds `state` -> Err(Unavailable)
    let catalog_lookup = Arc::new(NativeEvidenceCatalog::new(policy, 8).expect("catalog"));
    catalog_lookup
        .record_component_fixture_for_test(
            &fx.task,
            &fx.source_workspace,
            &fx.checkout,
            &fx.commit_sha,
            &fx.tree_sha,
            &fx.artifact_digest,
            &fx.custody_digest,
            &fx.closure_digest,
            &fx.closed_identity,
            &fx.closed_observation,
            Some(0),
            true,
            &fx.checks,
        )
        .expect("record in catalog_lookup");
    {
        let cat = catalog_lookup.clone();
        let digest = fx.custody_digest.clone();
        catalog_lookup.set_in_flight_hook_for_test(move || {
            assert_eq!(
                cat.revoke_custody_digest(&digest, "contended revoke inside lookup"),
                Err(NativeEvidenceCatalogError::Unavailable)
            );
        });
    }
    assert_eq!(
        catalog_lookup.lookup_by_custody_digest(&fx.custody_digest),
        Err(NativeEvidenceCatalogError::Unavailable)
    );
    assert_eq!(
        catalog_lookup.lookup_by_task_attempt(
            &fx.task.contract.task_id,
            fx.task.attempt_id.as_ref().expect("attempt"),
        ),
        Err(NativeEvidenceCatalogError::Unavailable)
    );
}
