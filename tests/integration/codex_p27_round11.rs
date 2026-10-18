//! Round 11 final P27 gate: scoped incident lineage through safe local proposal.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        EvidenceRef, ExcursionScope, IncidentEvent, ReproductionContract, ScopedIncident,
        build_local_proposal, reduce_excursions,
    },
};
use std::collections::BTreeMap;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn make_scope(task: &str) -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round11-project").expect("project fixture"),
        root: RootId::new("round11-root").expect("root fixture"),
        task: TaskId::new(task).expect("task fixture"),
        contract_revision: 7,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    }
}

fn incident(
    scope: &ExcursionScope,
    id: &str,
    parent: Option<&str>,
    kind: &str,
    tick: &str,
    evidence: Vec<String>,
) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: id.into(),
            family_signature: "round11-storage-timeout".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: evidence,
            observed_tick: Some(tick.into()),
        },
    }
}

#[test]
fn p27_round11_item4_final_validation_gate() {
    let scope = make_scope("round11-task");
    let evidence_digest = digest('c');
    let failure = incident(
        &scope,
        "failure-1",
        None,
        "failure",
        "100",
        vec![evidence_digest.clone()],
    );

    // Exact replay preserves one event and one canonical episode.
    let first = reduce_excursions(&None, &scope, std::slice::from_ref(&failure))
        .expect("scoped failure is admitted");
    assert_eq!(first.episodes.len(), 1);
    let replay = reduce_excursions(&Some(first.clone()), &scope, std::slice::from_ref(&failure))
        .expect("exact replay is idempotent");
    assert_eq!(replay, first);

    // A causally linked escalation joins the episode; independent recurrence
    // remains separate and retains unknown attribution.
    let escalation = incident(
        &scope,
        "escalation-1",
        Some("failure-1"),
        "escalation",
        "101",
        vec![],
    );
    let correlated = reduce_excursions(&Some(first), &scope, std::slice::from_ref(&escalation))
        .expect("causal escalation is correlated");
    assert_eq!(correlated.episodes.len(), 1);
    assert_eq!(correlated.episodes[0].events.len(), 2);
    assert_eq!(
        reduce_excursions(
            &Some(correlated.clone()),
            &scope,
            std::slice::from_ref(&escalation)
        )
        .expect("escalation replay is idempotent"),
        correlated
    );

    let recurrence = incident(&scope, "failure-2", None, "failure", "102", vec![]);
    let recurrent = reduce_excursions(&Some(correlated.clone()), &scope, &[recurrence])
        .expect("independent recurrence is kept separately");
    assert_eq!(recurrent.episodes.len(), 2);
    assert!(
        recurrent
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );

    // Scope substitution, event-ID byte mutation, malformed clocks, and a child
    // observed before its parent fail closed without changing canonical state.
    let foreign_scope = make_scope("foreign-task");
    assert!(reduce_excursions(&Some(correlated.clone()), &foreign_scope, &[]).is_err());
    let changed = incident(&scope, "failure-1", None, "recovered", "100", vec![]);
    assert!(reduce_excursions(&Some(correlated.clone()), &scope, &[changed]).is_err());
    let malformed = incident(&scope, "malformed", None, "failure", "0103", vec![]);
    assert!(reduce_excursions(&None, &scope, &[malformed]).is_err());
    let predating_child = incident(
        &scope,
        "predating-child",
        Some("failure-1"),
        "escalation",
        "99",
        vec![],
    );
    assert!(reduce_excursions(&Some(correlated.clone()), &scope, &[predating_child]).is_err());
    assert_eq!(
        correlated.episodes.len(),
        1,
        "rejections do not mutate prior state"
    );

    // Local proposal construction preserves provenance and uncertainty, enforces
    // investigator bounds, and grants no execution or external publication.
    let contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "round11-task; contract revision 7".into()),
            ("steps".into(), "replay fixed incident evidence".into()),
            ("expected".into(), "failure and escalation correlate".into()),
            (
                "actual".into(),
                "a separate recurrence remains unknown".into(),
            ),
            (
                "negation".into(),
                "do not infer an unobserved root cause".into(),
            ),
        ]),
    };
    let proposal = build_local_proposal(
        &correlated.episodes[0],
        &contract,
        &[EvidenceRef {
            digest: evidence_digest.clone(),
            media_type: "application/json".into(),
        }],
    )
    .expect("bounded local proposal is built from canonical episode evidence");
    assert_eq!(proposal.scope, scope);
    assert_eq!(proposal.evidence_digests, vec![evidence_digest]);
    assert!(proposal.uncertainty.contains("unknown"));
    assert_eq!(proposal.proposed_investigator_max_steps, 8);
    assert_eq!(proposal.proposed_investigator_max_model_turns, 1);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let mut secret_contract = contract.clone();
    secret_contract.fields.insert(
        "steps".into(),
        "post token=private to https://external.invalid".into(),
    );
    assert!(build_local_proposal(&correlated.episodes[0], &secret_contract, &[]).is_err());

    // Rebuilding from the same inputs gives byte-identical local export data.
    let proposal_replay = build_local_proposal(
        &correlated.episodes[0],
        &contract,
        &[EvidenceRef {
            digest: digest('c'),
            media_type: "application/json".into(),
        }],
    )
    .expect("identical proposal inputs replay");
    assert_eq!(proposal, proposal_replay);
    let export = serde_json::to_vec(&(&correlated, &proposal)).expect("local export serializes");
    let replay_export =
        serde_json::to_vec(&(&correlated, &proposal_replay)).expect("replay export serializes");
    assert_eq!(export, replay_export);
}
