use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        ExcursionScope, IncidentEvent, ReproductionContract, ScopedIncident, build_local_proposal,
        reduce_excursions,
    },
};
use std::collections::BTreeMap;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn scope() -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round14-project").expect("valid project fixture"),
        root: RootId::new("round14-root").expect("valid root fixture"),
        task: TaskId::new("round14-task").expect("valid task fixture"),
        contract_revision: 1,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    }
}

fn incident(
    scope: &ExcursionScope,
    event_id: &str,
    parent: Option<&str>,
    kind: &str,
    tick: &str,
) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: event_id.into(),
            family_signature: "round14-timeout".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.into()),
        },
    }
}

#[test]
fn codex_round14_p27() {
    let scope = scope();
    let failure = incident(&scope, "failure-1", None, "failure", "100");
    let initial = reduce_excursions(&None, &scope, std::slice::from_ref(&failure))
        .expect("scoped failure is recorded");
    let replay = reduce_excursions(
        &Some(initial.clone()),
        &scope,
        std::slice::from_ref(&failure),
    )
    .expect("deterministic replay is idempotent");
    assert_eq!(initial, replay);
    assert_eq!(
        serde_json::to_vec(&initial).expect("canonical state serializes"),
        serde_json::to_vec(&replay).expect("replayed state serializes")
    );

    let escalation = incident(
        &scope,
        "escalation-1",
        Some("failure-1"),
        "escalation",
        "101",
    );
    let correlated = reduce_excursions(&Some(initial.clone()), &scope, &[escalation])
        .expect("causal escalation is correlated");
    assert_eq!(correlated.episodes.len(), 1);
    assert_eq!(correlated.episodes[0].events.len(), 2);

    let changed_id = incident(&scope, "failure-1", None, "recovered", "100");
    assert!(reduce_excursions(&Some(initial.clone()), &scope, &[changed_id]).is_err());
    let foreign_scope = ExcursionScope {
        task: TaskId::new("foreign-task").expect("foreign task fixture"),
        ..scope.clone()
    };
    assert!(reduce_excursions(&Some(initial.clone()), &foreign_scope, &[]).is_err());
    let predating_child = incident(
        &scope,
        "child-before-parent",
        Some("failure-1"),
        "escalation",
        "99",
    );
    assert!(reduce_excursions(&Some(initial), &scope, &[predating_child]).is_err());

    let proposal = build_local_proposal(
        &correlated.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "round14-task".into()),
                ("steps".into(), "replay the fixed fixture".into()),
                ("expected".into(), "one correlated episode".into()),
                (
                    "actual".into(),
                    "failure and escalation were recorded".into(),
                ),
                ("negation".into(), "do not infer an unobserved cause".into()),
            ]),
        },
        &[],
    )
    .expect("canonical episode creates a bounded local proposal");
    assert_eq!(proposal.scope, scope);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);
    assert_eq!(proposal.proposed_investigator_max_steps, 8);
    assert_eq!(proposal.proposed_investigator_max_model_turns, 1);

    let secret_contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "round14-task".into()),
            (
                "steps".into(),
                "send token=private to https://private.invalid".into(),
            ),
            ("expected".into(), "redacted".into()),
            ("actual".into(), "secret was rejected".into()),
        ]),
    };
    assert!(build_local_proposal(&correlated.episodes[0], &secret_contract, &[]).is_err());
}
