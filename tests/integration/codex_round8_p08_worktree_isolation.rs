//! Round 8 Item 2: deterministic cross-packet worktree isolation qualification.

use admiralty_core::{ProjectId, RootId};
use admiralty_runtime::{
    providers::codex::{CodexEffort, CodexError, CodexNdjsonStream},
    recovery::{Blocker, SessionCustody, plan_recovery},
    session::{LaunchSpec, validate_launch_envelope},
    workspace::registry::{Custody, GitMetadata, NodeRef, WorkspaceSpec},
};
use std::{fs, path::PathBuf};

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn p04_p10_round8_p08_worktree_isolation_qualification() {
    let temp = tempfile::tempdir().expect("temporary worktree fixture");
    let worktrees = temp.path().join("worktrees");
    let first_root = worktrees.join("worker-a");
    let second_root = worktrees.join("worker-b");
    fs::create_dir_all(worktrees.join(".git/worktrees/worker-a")).expect("first worktree metadata");
    fs::create_dir_all(worktrees.join(".git/worktrees/worker-b"))
        .expect("second worktree metadata");
    fs::create_dir_all(first_root.join("src")).expect("first worktree source");
    fs::create_dir_all(second_root.join("src")).expect("second worktree source");
    fs::write(first_root.join("src/owned.rs"), b"worker-a").expect("first source");
    fs::write(second_root.join("src/owned.rs"), b"worker-b").expect("second source");
    // Worktree metadata is observed as a file, without resolving or invoking Git.
    fs::write(
        first_root.join(".git"),
        b"gitdir: ../.git/worktrees/worker-a\n",
    )
    .expect("first git pointer");
    fs::write(
        second_root.join(".git"),
        b"gitdir: ../.git/worktrees/worker-b\n",
    )
    .expect("second git pointer");
    let first_root = fs::canonicalize(first_root).expect("canonical first worktree");
    let second_root = fs::canonicalize(second_root).expect("canonical second worktree");

    let spec = |node: &str, root: &str, project: &str, workspace: PathBuf| WorkspaceSpec {
        node_id: NodeRef::new(node).expect("valid node ID"),
        root_id: RootId::new(root).expect("valid root ID"),
        project_id: ProjectId::new(project).expect("valid project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("secrets")],
    };
    let first_spec = spec("worker-a", "root-a", "project-a", first_root.clone());
    let second_spec = spec("worker-b", "root-b", "project-b", second_root.clone());
    let first = first_spec.validate().expect("first worktree validates");
    let second = second_spec.validate().expect("second worktree validates");

    assert_eq!(first.custody, Custody::RegisteredOnly);
    assert_eq!(second.custody, Custody::RegisteredOnly);
    assert_eq!(first.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(second.git_metadata, GitMetadata::WorktreeFile);
    assert!(first.owns(std::path::Path::new("src/owned.rs")));
    assert!(second.owns(std::path::Path::new("src/owned.rs")));
    assert!(!first.owns(std::path::Path::new("../worker-b/src/owned.rs")));
    assert!(!second.owns(std::path::Path::new("../worker-a/src/owned.rs")));
    assert!(!first.owns(std::path::Path::new(".git")));
    assert!(!second.owns(std::path::Path::new(".git")));

    assert_eq!(
        first_spec.validate().expect("repeat first observation"),
        first
    );
    assert_eq!(
        second_spec.validate().expect("repeat second observation"),
        second
    );
    assert_eq!(
        fs::read(first_root.join("src/owned.rs")).expect("read first owned file"),
        b"worker-a"
    );
    assert_eq!(
        fs::read(second_root.join("src/owned.rs")).expect("read second owned file"),
        b"worker-b"
    );
}

#[test]
fn p04_p10_round9_item3_lane_spawner_recovery_qualification() {
    use admiralty_core::{
        AttemptId, PrincipalId, TaskId,
        placement::{
            BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
            PlacementPolicy, PlacementRequest, propose_placement,
        },
    };
    use admiralty_runtime::reservations::{
        AccountId, CurrencyUnit, MinorUnits, ReservationLedger, ReservationScope, reserve_budget,
    };

    // P04: registration scope is lexical, and traversal never owns a sibling.
    let temp = tempfile::tempdir().expect("isolated lane fixture");
    let lane = temp.path().join("lane");
    fs::create_dir_all(lane.join("src")).expect("lane source");
    fs::write(lane.join("src/main.rs"), b"lane-owned").expect("lane file");
    let workspace = fs::canonicalize(&lane).expect("canonical lane");
    let registered = WorkspaceSpec {
        node_id: NodeRef::new("lane-spawner").expect("node"),
        root_id: RootId::new("lane-root").expect("root"),
        project_id: ProjectId::new("lane-project").expect("project"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    }
    .validate()
    .expect("workspace registration");
    assert!(registered.owns(std::path::Path::new("src/main.rs")));
    assert!(!registered.owns(std::path::Path::new("src/private/token")));
    assert!(!registered.owns(std::path::Path::new("../other/src/main.rs")));

    // P07: malformed spawner input is rejected before process launch.
    let launch = LaunchSpec {
        run_id: "../reused-run".into(),
        program: PathBuf::from("/bin/true"),
        args: vec![],
        cwd: lane.clone(),
        spool_dir: temp.path().join("spool"),
    };
    assert!(validate_launch_envelope(&launch).is_err());

    // P18: unknown spend limits fail closed without mutating the ledger.
    let scope = ReservationScope {
        project_id: ProjectId::new("lane-project").expect("project"),
        root_id: RootId::new("lane-root").expect("root"),
        task_id: TaskId::new("lane-task").expect("task"),
        attempt_id: AttemptId::new("lane-attempt").expect("attempt"),
        owner: PrincipalId::new("lane-owner").expect("owner"),
        incarnation: "boot-1".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "grant-lane".into(),
        resource_claims_digest: format!("sha256:{}", "b".repeat(64)),
        account_id: AccountId::new("lane-account").expect("account"),
        unit: CurrencyUnit::new("USD", 2).expect("currency"),
    };
    let ledger = ReservationLedger::new();
    assert!(
        reserve_budget(
            &ledger,
            "event",
            "reservation",
            scope,
            "correlation",
            MinorUnits::new(1)
        )
        .is_err()
    );
    assert_eq!(ledger.revision, 0);
    assert!(ledger.reservations.is_empty());

    // P21/P26: a journal gap poisons a provider stream; replay cannot repair it.
    let mut stream = CodexNdjsonStream::new("model", CodexEffort::Low).expect("stream");
    assert!(matches!(
        stream.push_chunk(&admiralty_runtime::session::OutputChunk {
            offset: 1,
            stream: admiralty_runtime::session::OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(CodexError::JournalGap)
    ));
    assert!(matches!(
        stream.push_chunk(&admiralty_runtime::session::OutputChunk {
            offset: 0,
            stream: admiralty_runtime::session::OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(CodexError::Failed)
    ));

    // P24: disconnected nodes cannot start ordinary work without a bound permit.
    let now = DecimalU64::new(10);
    let request = PlacementRequest {
        project: ProjectId::new("p").expect("project"),
        root: RootId::new("r").expect("root"),
        task: TaskId::new("t").expect("task"),
        contract_digest: format!("sha256:{}", "c".repeat(64)),
        authority_ref: "grant".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "pool".into(),
            amount: now.clone(),
        },
    };
    let node = NodeOffer {
        node_id: NodeId::new("node").expect("node"),
        boot_id: "boot".into(),
        authority_generation: DecimalU64::new(1),
        visible_scopes: Observation {
            value: vec!["p".into()],
            observed_at: now.clone(),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: now.clone(),
        },
        account_pool: Observation {
            value: "pool".into(),
            observed_at: now.clone(),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: DecimalU64::new(1),
                    budget_remaining: DecimalU64::new(50),
                },
            },
            observed_at: now.clone(),
        },
        current_pressure: Observation {
            value: Known::Known {
                value: DecimalU64::new(0),
            },
            observed_at: now.clone(),
        },
        spool_free: Observation {
            value: Known::Known {
                value: DecimalU64::new(50),
            },
            observed_at: now.clone(),
        },
        existing_active_roots: vec![],
        controller_connected: false,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(now.clone()),
        ownership_revision: Some(DecimalU64::new(1)),
    };
    let policy = PlacementPolicy {
        max_observation_age: DecimalU64::new(10),
        maximum_pressure: DecimalU64::new(5),
        minimum_spool_reserve: DecimalU64::new(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(DecimalU64::new(1)),
    };
    let placement = propose_placement(&request, &[node], &policy, 10).expect("placement proposal");
    assert!(placement.candidates.is_empty());
    assert!(!placement.execution_granted && !placement.authority_granted);

    // P25: unknown session custody blocks recovery even when artifacts match.
    let (manifest, mut observation, fence) = recovery_fixture::valid_recovery_evidence();
    observation.session_custody = SessionCustody::Unknown;
    let recovery =
        plan_recovery(&manifest, &observation, Some(&fence)).expect("safe recovery plan");
    assert!(!recovery.restore_authorized());
    assert!(
        recovery
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, Blocker::UnknownSessionCustody))
    );
}
