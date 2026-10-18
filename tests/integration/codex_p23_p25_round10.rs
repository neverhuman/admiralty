//! Round 10 workspace gate and recovery audit qualification.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeId, NodeIdentity, NodeRegistry, NodeScope,
    decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    MANAGED_EXECUTION_QUALIFIED,
    recovery::{Blocker, plan_recovery},
    workspace::registry::{Custody, NodeRef, WorkspaceError, WorkspaceSpec},
};
use std::{fs, path::PathBuf};

#[path = "support/recovery.rs"]
mod recovery_fixture;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

#[test]
fn p23_p25_round10_item3_workspace_gate_audit() {
    // A workspace gate is bound to the enrolled node identity and remains read only.
    let fixture = tempfile::tempdir().expect("temporary workspace fixture");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace directory");
    fs::create_dir(workspace.join("src")).expect("owned source directory");
    fs::write(workspace.join("src/node.rs"), b"fixture").expect("owned source");
    fs::create_dir(workspace.join(".git")).expect("forbidden metadata directory");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round10-node").expect("node ID"),
        root_id: RootId::new("round10-root").expect("root ID"),
        project_id: ProjectId::new("round10-project").expect("project ID"),
        workspace: fs::canonicalize(&workspace).expect("canonical workspace"),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let gate = spec.validate().expect("registered workspace validates");
    assert_eq!(gate.custody, Custody::RegisteredOnly);
    assert!(gate.owns(std::path::Path::new("src/node.rs")));
    assert!(!gate.owns(std::path::Path::new(".git/config")));
    assert!(!gate.owns(std::path::Path::new("../outside")));
    let mut traversal = spec.clone();
    traversal.owned_paths.push(PathBuf::from("../outside"));
    assert_eq!(
        traversal.validate().expect_err("traversal denied"),
        WorkspaceError::InvalidRelativePath
    );

    // P23 enrollment ingestion must match the workspace node, scope and generation.
    let scope = NodeScope {
        project_id: ProjectId::new("round10-project").expect("project ID"),
        root_id: RootId::new("round10-root").expect("root ID"),
        task_id: TaskId::new("round10-task").expect("task ID"),
        authority: PrincipalId::new("round10-authority").expect("authority"),
    };
    let actor = scope.authority.clone();
    let token = EnrollmentToken {
        token_id: "round10-token".into(),
        token_digest: digest('a'),
        scope: scope.clone(),
        expires_tick: DecimalU64::new(20),
        consumed_binding: None,
    };
    let issued = reduce_enrollment_for(
        &NodeRegistry::default(),
        &EnrollmentEvent::TokenIssued {
            event_id: "issue-round10".into(),
            token,
        },
        &scope,
        &actor,
        Some(1),
    )
    .expect("scoped enrollment token");
    let enrolled_event = EnrollmentEvent::Enrolled {
        event_id: "enroll-round10".into(),
        token_id: "round10-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: NodeId::new("round10-node").expect("node ID"),
            boot_id: "boot-1".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let bytes = serde_json::to_vec(&enrolled_event).expect("encode enrollment event");
    let decoded = decode_event(&bytes).expect("strict event decode");
    let enrolled = reduce_enrollment_for(&issued, &decoded, &scope, &actor, Some(2))
        .expect("node enrollment accepted");
    let enrolled_node = NodeId::new("round10-node").expect("node ID");
    assert!(enrolled.nodes.contains_key(&enrolled_node));
    assert_eq!(
        gate.node_id.as_str(),
        enrolled_event_identity(&enrolled_event)
    );
    let foreign = PrincipalId::new("foreign-round10-authority").expect("foreign authority");
    assert!(reduce_enrollment_for(&issued, &decoded, &scope, &foreign, Some(2)).is_err());

    // P25 recovery audit remains gated on an external fence and never authorizes effects.
    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let blocked = plan_recovery(&manifest, &observation, None).expect("recovery audit");
    assert!(blocked.blockers.contains(&Blocker::MissingExternalFence));
    assert!(!blocked.resume_proposed);
    let cleared = plan_recovery(&manifest, &observation, Some(&fence)).expect("fenced audit");
    assert!(cleared.resume_proposed);
    assert!(!cleared.restore_authorized());
    assert!(!cleared.effects_authorized());
    const { assert!(!MANAGED_EXECUTION_QUALIFIED); }
}

fn enrolled_event_identity(event: &EnrollmentEvent) -> &str {
    match event {
        EnrollmentEvent::Enrolled { identity, .. } => identity.node_id.as_str(),
        _ => unreachable!("fixture is enrollment"),
    }
}
