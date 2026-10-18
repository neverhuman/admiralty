//! Round 8 Item 2 cross-packet qualification for P08, P09, and P27.
//! The fixture is fully local: provider output is parsed, workspace custody is
//! checked, and the resulting scoped incident is reduced without side effects.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{ExcursionScope, IncidentEvent, ScopedIncident, reduce_excursions},
};
use admiralty_runtime::{
    providers::codex::{CodexEffort, CodexNdjsonStream},
    session::{OutputChunk, OutputStream},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use std::{fs, path::PathBuf};

#[test]
fn p27_round8_p08_p09_export_orchestrator_qualification() {
    // P08: the checked-in transcript deterministically yields a completed turn,
    // while remaining explicitly unqualified for native execution/acceptance.
    let transcript = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
    let mut parser = CodexNdjsonStream::new("fixture-model", CodexEffort::Medium)
        .expect("fixture model is valid");
    let observations = parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: transcript.to_vec(),
        })
        .expect("fixture transcript parses");
    assert!(
        observations
            .iter()
            .any(|event| event.event_type == "turn.completed")
    );
    let summary = parser.finish(Some(0), true);
    assert!(!summary.native_execution_qualified());
    assert!(!summary.trusted_acceptance());

    // P09: the orchestrator accepts only the explicitly owned workspace scope.
    let temp = tempfile::tempdir().expect("temporary workspace fixture");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace source directory");
    fs::write(workspace.join("src/main.rs"), b"fixture").expect("owned source fixture");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-round8").expect("node fixture"),
        root_id: RootId::new("root-round8").expect("root fixture"),
        project_id: ProjectId::new("project-round8").expect("project fixture"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let validated = spec.validate().expect("workspace scope validates");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/main.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));

    // P27: deterministic scoped incident export/reduction is repeatable and
    // does not turn local evidence into execution authority.
    let scope = ExcursionScope {
        project: ProjectId::new("project-round8").expect("project fixture"),
        root: RootId::new("root-round8").expect("root fixture"),
        task: TaskId::new("task-round8").expect("task fixture"),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let incident = ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: "incident-round8".into(),
            family_signature: "fixture-provider-completion".into(),
            cause_parent: None,
            kind: "observation".into(),
            evidence_digests: vec![],
            observed_tick: Some("42".into()),
        },
    };
    let first = reduce_excursions(&None, &scope, std::slice::from_ref(&incident))
        .expect("scoped observation reduces");
    let replay = reduce_excursions(&Some(first.clone()), &scope, &[incident])
        .expect("deterministic export replay is accepted");
    assert_eq!(first, replay);
    assert_eq!(replay.episodes.len(), 1);
    assert!(replay.episodes[0].unknown_attribution);
}
