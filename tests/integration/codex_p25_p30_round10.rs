//! Round 8 Item 2: deterministic cross-packet qualification.
//!
//! The current main tree exposes read-only workspace custody and recovery
//! proposal APIs. This fixture composes them without invoking a webhook,
//! provider, executor, or release installer.

#[path = "support/recovery.rs"]
mod recovery_fixture;

use admiralty_core::{ProjectId, RootId};
use admiralty_runtime::{
    recovery::{Reconciliation, plan_recovery},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use std::{fs, path::PathBuf, process::Command};

#[test]
fn p25_p30_round8_p09_webhook_orchestrator_qualification() {
    let temp = tempfile::tempdir().expect("temporary qualification workspace");
    let workspace = fs::canonicalize(temp.path()).expect("canonical workspace");
    fs::create_dir(workspace.join("src")).expect("owned source directory");
    fs::write(workspace.join("src/lib.rs"), b"qualification fixture\n").expect("source fixture");

    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round8-node").expect("valid node ID"),
        root_id: RootId::new("round8-root").expect("valid root ID"),
        project_id: ProjectId::new("round8-project").expect("valid project ID"),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("secrets")],
    };
    let validated = registration
        .validate()
        .expect("workspace registration validates");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert_eq!(
        registration.validate().expect("repeat validation"),
        validated
    );

    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let proposal = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("complete deterministic evidence produces a proposal");
    let replay = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("replaying identical evidence produces the same proposal");
    assert_eq!(proposal, replay);
    assert!(proposal.resume_proposed);
    assert!(!proposal.restore_authorized());
    assert!(!proposal.effects_authorized());

    let mut uncertain = observation;
    uncertain.unresolved_effects = Reconciliation::Unknown;
    let blocked = plan_recovery(&manifest, &uncertain, Some(&fence))
        .expect("uncertainty is returned as a blocked proposal");
    assert!(!blocked.resume_proposed);
    assert!(!blocked.restore_authorized());
    assert!(!blocked.effects_authorized());

    assert_eq!(
        fs::read(workspace.join("src/lib.rs")).expect("fixture remains unchanged"),
        b"qualification fixture\n"
    );
}

#[test]
fn p25_p30_round10_item3_workspace_gate_audit() {
    use admiralty_runtime::recovery::{Blocker, plan_recovery};

    let (manifest, observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    let first = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("complete P25 recovery evidence is bounded and valid");
    let replay = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("identical P25 evidence replays deterministically");
    assert_eq!(first, replay);
    assert!(first.resume_proposed);
    assert!(!first.restore_authorized());
    assert!(!first.effects_authorized());

    fence.old_executor_fenced = Some(false);
    let blocked = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("unsafe handoff remains a blocked proposal");
    assert!(!blocked.resume_proposed);
    assert!(blocked.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!blocked.restore_authorized());
    assert!(!blocked.effects_authorized());

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves");
    let output = Command::new("python3")
        .args(["scripts/release/package.py", "--self-test"])
        .current_dir(workspace)
        .output()
        .expect("offline P30 package self-test starts");
    assert!(
        output.status.success(),
        "P30 package self-test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Ran 10 tests"),
        "P30 package self-test did not report its expected deterministic suite"
    );
}
