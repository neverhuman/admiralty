//! Round 8 Item 2 cross-packet qualification for P09 and P20.

use admiralty_core::placement::decode_input;
use admiralty_core::{
    AttemptId,
    conflicts::{
        CandidateChange, CandidateConstraints, FileChange, InterfaceRevision, probe_cache_key,
    },
    economics::{
        self, BoundedId, CohortSpec, DecimalU64 as EconU64, DecimalU128, ObservedAmount,
        ParentInclusion, Rational, RootBinding, RootOutcome, RootStatus, UsageRecord,
    },
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::session::{OutputChunk, OutputStream};
use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceSpec};
use admiralty_runtime::{
    providers::codex::{CodexEffort, CodexNdjsonStream, CodexTermination},
    recovery::plan_recovery,
    session::{LaunchSpec, SessionError, validate_launch_envelope},
};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use std::{fs, path::PathBuf};

#[path = "support/recovery.rs"]
mod recovery_fixture;

fn observation(binding: &AssetBinding, registered: bool, at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: (at - 10).to_string(),
        clock_id: "round8-p09-p20".into(),
        observed_at: at.to_string(),
        decision_at: (at + 1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round8-qualification".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round8_p09_orchestrator_retirement_qualification() {
    let fixture = tempfile::tempdir().expect("temporary owned fixture");
    let root = fs::canonicalize(fixture.path()).expect("canonical fixture root");
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("asset.txt"), b"fixture asset").expect("write asset");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round8-node").expect("node id"),
        root_id: RootId::new("round8-root").expect("root id"),
        project_id: ProjectId::new("round8-project").expect("project id"),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("asset.txt")],
        forbidden_paths: Vec::new(),
    };
    let validated = spec.validate().expect("P09 workspace registration");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(&PathBuf::from("asset.txt")));

    let binding = AssetBinding {
        project_id: ProjectId::new("round8-project").expect("project id"),
        root_id: RootId::new("round8-root").expect("root id"),
        task_id: TaskId::new("round8-task").expect("task id"),
        asset_id: "asset-1".into(),
        owner_id: PrincipalId::new("round8-owner").expect("owner id"),
        sha256: format!("sha256:{}", "b".repeat(64)),
        generation: "1".into(),
    };
    let mut db = Connection::open_in_memory().expect("open deterministic custody db");
    custody::initialize(&db).expect("initialize custody db");
    custody::register_asset(
        &mut db,
        RegisterAssetRequest {
            command_id: "register".into(),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation(&binding, false, 10),
        },
    )
    .expect("register orchestrated asset");

    let pin = AcquirePinRequest {
        command_id: "pin".into(),
        binding: binding.clone(),
        pin_id: "active-orchestrator".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone()).expect("acquire active pin");
    assert_eq!(
        custody::acquire_pin(&mut db, pin).expect("idempotent pin replay"),
        pin_receipt
    );

    let blocked = ClaimRetirementRequest {
        command_id: "retire-blocked".into(),
        binding: binding.clone(),
        claim_id: "retirement-1".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, 12),
    };
    assert!(matches!(
        custody::claim_retirement(&mut db, blocked),
        Err(CustodyError::ActivePins)
    ));
    custody::release_pin(
        &mut db,
        ReleasePinRequest {
            command_id: "unpin".into(),
            binding: binding.clone(),
            pin_id: "active-orchestrator".into(),
            expected_revision: "2".into(),
            observation: observation(&binding, true, 12),
        },
    )
    .expect("release orchestrator pin");
    custody::claim_retirement(
        &mut db,
        ClaimRetirementRequest {
            command_id: "retire".into(),
            binding: binding.clone(),
            claim_id: "retirement-1".into(),
            expected_revision: "3".into(),
            observation: observation(&binding, true, 13),
        },
    )
    .expect("retirement claim after release");
    assert_eq!(
        fs::read(workspace.join("asset.txt")).expect("asset remains"),
        b"fixture asset"
    );
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

#[test]
fn p20_p24_round9_item3_telegram_retirement_qualification() {
    // P04/P20: retirement is blocked by active custody; Round 8 already covers
    // the successful release path. Here, foreign scope remains a hard denial.
    let binding = AssetBinding {
        project_id: ProjectId::new("round9-project").expect("project id"),
        root_id: RootId::new("round9-root").expect("root id"),
        task_id: TaskId::new("round9-task").expect("task id"),
        asset_id: "round9-asset".into(),
        owner_id: PrincipalId::new("round9-owner").expect("owner id"),
        sha256: digest('a'),
        generation: "1".into(),
    };
    let mut db = Connection::open_in_memory().expect("open deterministic custody db");
    custody::initialize(&db).expect("initialize custody db");
    custody::register_asset(
        &mut db,
        RegisterAssetRequest {
            command_id: "r9-register".into(),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation(&binding, false, 10),
        },
    )
    .expect("register asset");
    let mut foreign = binding.clone();
    foreign.root_id = RootId::new("round9-foreign-root").expect("foreign root");
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "r9-foreign-retire".into(),
                binding: foreign.clone(),
                claim_id: "r9-claim".into(),
                expected_revision: "1".into(),
                observation: observation(&foreign, true, 11),
            }
        ),
        Err(CustodyError::ForeignAsset)
    ));

    // P07: malformed/unsafe process launch envelopes fail before execution.
    let launch = LaunchSpec {
        run_id: "round9".into(),
        program: PathBuf::from("relative-program"),
        args: Vec::new(),
        cwd: PathBuf::from("/fixture"),
        spool_dir: PathBuf::from("/fixture/spool"),
    };
    assert!(matches!(
        validate_launch_envelope(&launch),
        Err(SessionError::InvalidSpec(_))
    ));

    // P18: ancestry changes invalidate a conflict-probe cache key even when
    // the candidate tree is otherwise unchanged.
    let make_candidate = |ancestry: char| CandidateChange {
        project_id: ProjectId::new("round9-project").expect("project id"),
        root_id: RootId::new("round9-root").expect("root id"),
        task_id: TaskId::new("round9-task").expect("task id"),
        candidate_id: "candidate".into(),
        base_sha: "b".repeat(40),
        head_sha: "c".repeat(40),
        tree_sha: "d".repeat(40),
        ancestry_digest: Some(digest(ancestry)),
        files: vec![FileChange {
            path: "src/lib.rs".into(),
            hunks: None,
        }],
        constraints: Some(CandidateConstraints {
            contract_revision: 1,
            contract_digest: digest('e'),
            owned_paths: vec!["src/lib.rs".into()],
            denied_paths: Vec::new(),
            interfaces: vec![InterfaceRevision {
                interface_id: "api".into(),
                revision: 1,
            }],
        }),
    };
    assert_ne!(
        probe_cache_key(&[make_candidate('f')], "policy").expect("cache key"),
        probe_cache_key(&[make_candidate('9')], "policy").expect("cache key")
    );

    // P21: a partial provider capture remains unknown and grants no acceptance.
    let mut stream = CodexNdjsonStream::new("fixture-model", CodexEffort::Low).expect("parser");
    stream
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n".to_vec(),
        })
        .expect("parse start frame");
    let summary = stream.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.trusted_acceptance());

    // P24: placement input rejects unrecognized fields instead of guessing.
    assert!(decode_input(br#"{"request":{},"offers":[],"policy":{},"now":null,"previous_tick":null,"telegram_override":true}"#).is_err());

    // P25: recovery without an external old-executor fence stays non-authorizing.
    let (manifest, recovery_observation, _) = recovery_fixture::valid_recovery_evidence();
    let proposal = plan_recovery(&manifest, &recovery_observation, None).expect("blocked proposal");
    assert!(!proposal.restore_authorized());
    assert!(!proposal.effects_authorized());

    // P26: unknown valuation is retained as null with Unknown coverage.
    let root = RootId::new("round9-econ-root").expect("root id");
    let binding = RootBinding {
        root_id: root.clone(),
        task_id: TaskId::new("round9-econ-task").expect("task id"),
        contract_revision: 1,
        contract_digest: digest('b'),
        protected_oracle_digest: digest('c'),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round9-cohort").expect("cohort id"),
        project_id: ProjectId::new("round9-econ-project").expect("project id"),
        fixed_assignment_roots: vec![root.clone()],
        required_delivery_stage: BoundedId::new("accepted").expect("stage"),
        valuation_digest: digest('d'),
        assigned_at: EconU64(1),
        bindings: vec![binding.clone()],
    };
    let usage = UsageRecord {
        project_id: cohort.project_id.clone(),
        binding: binding.clone(),
        observation_id: BoundedId::new("round9-unknown-cost").expect("observation id"),
        revision: EconU64(1),
        root_scope: root.clone(),
        role: BoundedId::new("worker").expect("role"),
        purpose: BoundedId::new("retirement").expect("purpose"),
        provider: BoundedId::new("fixture").expect("provider"),
        unit: BoundedId::new("usd_micros").expect("unit"),
        amount: ObservedAmount::Unknown {
            reason: "unpriced".into(),
        },
        parent_inclusion: ParentInclusion::Standalone,
    };
    let outcome = RootOutcome {
        project_id: cohort.project_id.clone(),
        binding,
        acceptance_receipt_digest: None,
        root_id: root,
        status: RootStatus::Failed,
        accepted_stage: None,
        maturity: EconU64(10),
    };
    let summary = economics::summarize_cohort(&cohort, &[usage], &[outcome], 10).expect("summary");
    assert_eq!(summary.units[0].amount, None);
    assert_eq!(summary.units[0].coverage, economics::Coverage::Unknown);
    assert_eq!(
        DecimalU128(9_007_199_254_740_993).as_u128(),
        9_007_199_254_740_993
    );
    assert!(Rational::new(1, 0).is_none());
    let _ = AttemptId::new("round9-attempt").expect("typed attempt edge fixture");
}
