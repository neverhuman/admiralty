#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Post-PR-57 P27 release qualification and strict Clippy audit fixture.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        ExcursionScope, ExcursionState, IncidentEvent, LocalProposal, ReproductionContract,
        ScopedIncident, build_local_proposal, reduce_excursions,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QualificationBundle {
    schema_version: u32,
    state: ExcursionState,
    proposal: LocalProposal,
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn scope(task: &str) -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round20-project").expect("project fixture is valid"),
        root: RootId::new("round20-root").expect("root fixture is valid"),
        task: TaskId::new(task).expect("task fixture is valid"),
        contract_revision: 20,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    }
}

fn incident(
    scope: &ExcursionScope,
    id: &str,
    parent: Option<&str>,
    kind: &str,
    tick: u64,
) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: id.into(),
            family_signature: "round20-pr57-release".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.to_string()),
        },
    }
}

#[test]
fn p27_round20_post_pr57_release_qualification() {
    let task_scope = scope("release-task");
    let opening = incident(&task_scope, "failure-1", None, "failure", 2000);
    let first = reduce_excursions(&None, &task_scope, std::slice::from_ref(&opening))
        .expect("initial scoped incident is accepted");
    let escalation = incident(
        &task_scope,
        "escalation-1",
        Some("failure-1"),
        "escalation",
        2001,
    );
    let correlated =
        reduce_excursions(&Some(first), &task_scope, std::slice::from_ref(&escalation))
            .expect("causal escalation stays in the bounded episode");
    let recurrence = incident(&task_scope, "failure-2", None, "failure", 2002);
    let state = reduce_excursions(&Some(correlated), &task_scope, &[recurrence])
        .expect("unrelated recurrence remains quarantined");
    assert_eq!(state.episodes.len(), 2);
    assert_eq!(state.episodes[0].events.len(), 2);
    assert!(
        state
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );

    // The serialized release bundle carries the same task scope and keeps
    // proposal authority disabled across parse/replay and deterministic cycles.
    let proposal = build_local_proposal(
        &state.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "release-task; revision 20".into()),
                (
                    "steps".into(),
                    "replay the pinned local fault fixture".into(),
                ),
                (
                    "expected".into(),
                    "correlated failure and escalation".into(),
                ),
                (
                    "actual".into(),
                    "separate unknown recurrence retained".into(),
                ),
                (
                    "negation".into(),
                    "do not execute or publish this proposal".into(),
                ),
            ]),
        },
        &[],
    )
    .expect("canonical episode yields a bounded proposal");
    assert_eq!(proposal.scope, task_scope);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let bundle = QualificationBundle {
        schema_version: 1,
        state,
        proposal,
    };
    let bytes = serde_json::to_vec(&bundle).expect("bundle serializes");
    let decoded: QualificationBundle =
        serde_json::from_slice(&bytes).expect("bundle schema round trips");
    assert_eq!(decoded, bundle);
    for _ in 0..16 {
        let replay = reduce_excursions(&Some(decoded.state.clone()), &task_scope, &[])
            .expect("release replay remains canonical");
        assert_eq!(replay, decoded.state);
    }
    assert_eq!(
        serde_json::to_vec(&decoded).expect("bundle reserializes"),
        bytes
    );

    // Cross-task scope injection, event-ID mutation, forged receipt data, and
    // secret-bearing remediation instructions are all rejected fail-closed.
    let foreign_scope = scope("foreign-release-task");
    assert!(reduce_excursions(&Some(decoded.state.clone()), &foreign_scope, &[]).is_err());
    let mutated = incident(&task_scope, "failure-1", None, "recovered", 2000);
    assert!(reduce_excursions(&Some(decoded.state.clone()), &task_scope, &[mutated]).is_err());

    let mut forged = serde_json::to_value(&decoded.state).expect("state becomes JSON");
    forged["receipts"]["failure-1"] = serde_json::Value::String(digest('d'));
    let forged: ExcursionState = serde_json::from_value(forged).expect("fixture shape parses");
    assert!(reduce_excursions(&Some(forged), &task_scope, &[]).is_err());

    let mut unsafe_contract = decoded.proposal.reproduction_contract.clone();
    unsafe_contract.fields.insert(
        "steps".into(),
        "send token=private to https://outside.invalid".into(),
    );
    assert!(build_local_proposal(&decoded.state.episodes[0], &unsafe_contract, &[]).is_err());
}
