use admiralty_core::excursions::{
    EvidenceRef, ExcursionScope, IncidentEvent, ReproductionContract, ScopedIncident,
    build_local_proposal, reduce_excursions,
};
use admiralty_core::{ProjectId, RootId, TaskId};
use std::collections::BTreeMap;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn scope(task: &str) -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("harbor").expect("fixture project"),
        root: RootId::new("root-a").expect("fixture root"),
        task: TaskId::new(task).expect("fixture task"),
        contract_revision: 3,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    }
}

fn event(id: &str, parent: Option<&str>, kind: &str) -> IncidentEvent {
    IncidentEvent {
        event_id: id.into(),
        family_signature: "storage-timeout".into(),
        cause_parent: parent.map(str::to_owned),
        kind: kind.into(),
        evidence_digests: vec![],
        observed_tick: Some("42".into()),
    }
}

fn input(scope: &ExcursionScope, event: IncidentEvent) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event,
    }
}

#[test]
fn exact_replay_is_idempotent_and_correlated_failure_escalation_is_one_episode() {
    let scope = scope("task-lineage");
    let failure = input(&scope, event("failure-1", None, "failure"));
    let first = reduce_excursions(&None, &scope, std::slice::from_ref(&failure))
        .expect("first failure is recorded");
    let replay = reduce_excursions(&Some(first.clone()), &scope, std::slice::from_ref(&failure))
        .expect("byte-identical replay is accepted");
    assert_eq!(first, replay, "replay must not duplicate an incident");

    let escalation = input(
        &scope,
        event("escalation-1", Some("failure-1"), "escalation"),
    );
    let correlated = reduce_excursions(&Some(first), &scope, &[escalation])
        .expect("explicit causal parent joins escalation to failure");
    assert_eq!(correlated.episodes.len(), 1);
    assert_eq!(correlated.episodes[0].events.len(), 2);
}

#[test]
fn independent_recurrence_remains_separate_and_foreign_or_cyclic_causes_fail() {
    let task_scope = scope("task-recurrence");
    let initial = input(&task_scope, event("failure-1", None, "failure"));
    let first = reduce_excursions(&None, &task_scope, &[initial]).expect("initial failure");
    let recurrence = input(&task_scope, event("failure-2", None, "failure"));
    let recurrent = reduce_excursions(&Some(first), &task_scope, &[recurrence])
        .expect("independent recurrence is retained");
    assert_eq!(recurrent.episodes.len(), 2);
    assert_ne!(
        recurrent.episodes[0].episode_id,
        recurrent.episodes[1].episode_id
    );
    assert!(
        recurrent
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );

    let cycle = input(&task_scope, event("cycle-1", Some("cycle-1"), "failure"));
    assert!(reduce_excursions(&None, &task_scope, &[cycle]).is_err());
    let foreign_parent = input(
        &task_scope,
        event("child-1", Some("other-scope-parent"), "failure"),
    );
    assert!(reduce_excursions(&None, &task_scope, &[foreign_parent]).is_err());
    let foreign_scope = input(
        &scope("task-elsewhere"),
        event("foreign-1", None, "failure"),
    );
    assert!(reduce_excursions(&None, &task_scope, &[foreign_scope]).is_err());
}

#[test]
fn local_proposal_keeps_unknowns_and_scope_without_granting_effects() {
    let scope = scope("task-proposal");
    let evidence_digest = digest('c');
    let mut observed = event("incident-1", None, "failure");
    observed.evidence_digests.push(evidence_digest.clone());
    let reduced = reduce_excursions(&None, &scope, &[input(&scope, observed)])
        .expect("scoped incident reduces");
    let proposal = build_local_proposal(
        &reduced.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "task-proposal; contract revision 3".into()),
                ("steps".into(), "replay the recorded fixture".into()),
                ("expected".into(), "incident is contained".into()),
                (
                    "actual".into(),
                    "failure and escalation were observed".into(),
                ),
                ("negation".into(), "do not infer a root cause".into()),
            ]),
        },
        &[EvidenceRef {
            digest: evidence_digest.clone(),
            media_type: "application/json".into(),
        }],
    )
    .expect("bounded local proposal");

    assert_eq!(proposal.scope, scope);
    assert!(proposal.uncertainty.contains("unknown"));
    assert!(!proposal.external_publication_allowed);
    assert!(!proposal.execution_granted);
    assert_eq!(proposal.proposed_investigator_max_steps, 8);
    assert_eq!(proposal.proposed_investigator_max_model_turns, 1);
    assert_eq!(proposal.evidence_digests, vec![evidence_digest]);
    assert_eq!(
        proposal.reproduction_contract.fields["negation"],
        "do not infer a root cause"
    );

    let mut secret_bearing = ReproductionContract {
        fields: proposal.reproduction_contract.fields.clone(),
    };
    secret_bearing.fields.insert(
        "steps".into(),
        "send token=private to https://private.invalid".into(),
    );
    assert!(build_local_proposal(&reduced.episodes[0], &secret_bearing, &[]).is_err());
}
