use admiralty_core::{
    ProjectId, RootId, TaskId,
    conflicts::{CandidateChange, FileChange, analyze_conflicts},
    economics::ObservedAmount,
    placement::decode_input,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, PublicationBlock,
        PublicationScope, PublicationSnapshot, propose_publication, publication_request_digest,
    },
};
use admiralty_runtime::{
    providers::codex::{CodexEffort, CodexError, CodexNdjsonStream},
    recovery::{Compatibility, plan_recovery},
    session::{LaunchSpec, OutputChunk, OutputStream, validate_launch_envelope},
};
use admiralty_store::artifacts::ArtifactStore;

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn p26_p31_round9_item3_hitl_approval_cutover_qualification() {
    // P04: exact artifact replay is stable and the stored bytes verify.
    let dir = tempfile::tempdir().expect("artifact fixture directory");
    let artifact_root = dir.path().join("objects");
    std::fs::create_dir(&artifact_root).expect("create artifact root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o700))
            .expect("make artifact root private");
    }
    let store = ArtifactStore::open(&artifact_root, 128).expect("open artifact store");
    let artifact = store
        .publish_bytes(b"reviewed evidence")
        .expect("publish evidence");
    assert_eq!(
        store.publish_bytes(b"reviewed evidence").expect("replay"),
        artifact
    );
    assert_eq!(
        store.read_verified(&artifact).expect("verify bytes"),
        b"reviewed evidence"
    );
    assert!(store.publish_bytes(&[0; 129]).is_err());

    // P07: launch admission rejects an invalid run identity and never broadens argv.
    let mut launch = LaunchSpec {
        run_id: "round9-cutover".into(),
        program: "/usr/bin/codex".into(),
        args: vec!["exec".into()],
        cwd: "/workspace/project".into(),
        spool_dir: "/workspace/project/.spool".into(),
    };
    assert!(validate_launch_envelope(&launch).is_ok());
    launch.run_id = "../escape".into();
    assert!(validate_launch_envelope(&launch).is_err());

    // P18: traversal in an observed candidate cannot enter a frozen generation.
    let candidate = CandidateChange {
        project_id: ProjectId::new("round9-project").expect("project ID"),
        root_id: RootId::new("round9-root").expect("root ID"),
        task_id: TaskId::new("round9-task").expect("task ID"),
        candidate_id: "candidate-a".into(),
        base_sha: format!("sha256:{}", "a".repeat(64)),
        head_sha: format!("sha256:{}", "b".repeat(64)),
        tree_sha: format!("sha256:{}", "c".repeat(64)),
        ancestry_digest: None,
        files: vec![FileChange {
            path: "../outside.rs".into(),
            hunks: None,
        }],
        constraints: None,
    };
    assert!(analyze_conflicts(&[candidate], "round9-policy").is_err());

    // P21: provider output with a journal gap is retained as an error, not approval.
    let mut parser = CodexNdjsonStream::new("review-model", CodexEffort::Low).expect("parser");
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: 1,
            stream: OutputStream::Stdout,
            bytes: b"{}\\n".to_vec(),
        }),
        Err(CodexError::JournalGap)
    ));

    // P24: incomplete placement input is rejected before a placement proposal.
    assert!(decode_input(b"{}").is_err());

    // P25: incompatible recovery evidence cannot reopen effect authority.
    let (manifest, mut observation, fence) = recovery_fixture::valid_recovery_evidence();
    observation.compatibility = Compatibility::Incompatible;
    let recovery = plan_recovery(&manifest, &observation, Some(&fence)).expect("blocked proposal");
    assert!(!recovery.resume_proposed);
    assert!(!recovery.restore_authorized());
    assert!(!recovery.effects_authorized());

    // P26 + HITL cutover: evidence must match the current generation/head;
    // even valid reviewer evidence yields only a proposal, never execution.
    let scope = PublicationScope {
        project_id: ProjectId::new("round9-project").expect("project ID"),
        root_id: RootId::new("round9-root").expect("root ID"),
        task_id: TaskId::new("round9-task").expect("task ID"),
        target_ref: "refs/heads/main".into(),
        authority_generation: DecimalU64::new(4),
    };
    let mut snapshot = PublicationSnapshot {
        current_head: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        current_base: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
        expected_head: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        expected_base: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
        required_reviewers: vec!["owner".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "owner".into(),
            head: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            dismissed: false,
            provenance_ref: "fixture:approval".into(),
        }],
        required_checks: vec!["ci".into()],
        checks: vec![CheckEvidence {
            check_id: "ci".into(),
            head: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            state: CheckState::Passed,
            provenance_ref: "fixture:check".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let key = "round9-cutover";
    let digest = publication_request_digest(&scope, &snapshot, key, &scope.authority_generation)
        .expect("digest");
    let valid = propose_publication(&scope, &snapshot, key, &digest, &scope.authority_generation);
    assert!(valid.eligible);
    assert!(!valid.execution_granted());
    snapshot.approvals[0].dismissed = true;
    let dismissed_digest =
        publication_request_digest(&scope, &snapshot, key, &scope.authority_generation)
            .expect("updated digest");
    let dismissed = propose_publication(
        &scope,
        &snapshot,
        key,
        &dismissed_digest,
        &scope.authority_generation,
    );
    assert!(!dismissed.eligible);
    assert!(
        dismissed
            .reasons
            .contains(&PublicationBlock::DismissedApproval)
    );
    assert!(!dismissed.execution_granted());

    let unknown_cost = ObservedAmount::Unknown {
        reason: "provider interval unavailable".into(),
    };
    let encoded = serde_json::to_string(&unknown_cost).expect("serialize unknown cost");
    assert_eq!(
        serde_json::from_str::<ObservedAmount>(&encoded).expect("roundtrip unknown"),
        unknown_cost
    );
}
