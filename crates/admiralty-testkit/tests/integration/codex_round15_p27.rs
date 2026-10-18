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

fn make_scope(task: &str) -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round15-project").expect("static project ID is valid"),
        root: RootId::new("round15-root").expect("static root ID is valid"),
        task: TaskId::new(task).expect("fixture task ID is valid"),
        contract_revision: 15,
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
            family_signature: "p27-excursion-stress".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.to_string()),
        },
    }
}

#[test]
fn codex_round15_p27() {
    let scope = make_scope("round15-task");
    let failure = incident(&scope, "failure-1", None, "failure", 100);
    let first = reduce_excursions(&None, &scope, std::slice::from_ref(&failure))
        .expect("initial excursion is recorded");

    // Repeated release-smoke replay must remain byte-for-byte stable and bounded.
    let mut replay = first.clone();
    for _ in 0..32 {
        replay = reduce_excursions(&Some(replay), &scope, std::slice::from_ref(&failure))
            .expect("identical replay is idempotent");
    }
    assert_eq!(replay, first);
    assert_eq!(
        serde_json::to_vec(&replay).expect("state serializes deterministically"),
        serde_json::to_vec(&first).expect("baseline serializes deterministically")
    );

    let escalation = incident(&scope, "escalation-1", Some("failure-1"), "escalation", 101);
    let correlated = reduce_excursions(
        &Some(first.clone()),
        &scope,
        std::slice::from_ref(&escalation),
    )
    .expect("causal escalation remains within its incident");
    assert_eq!(
        correlated.episodes.len(),
        1,
        "blast radius stays in one episode"
    );
    assert_eq!(correlated.episodes[0].events.len(), 2);
    let repeated = reduce_excursions(&Some(correlated.clone()), &scope, &[escalation])
        .expect("multi-step replay does not duplicate escalation");
    assert_eq!(repeated, correlated);

    // An unrelated recurrence is quarantined as a distinct, unknown-attribution episode.
    let recurrence = incident(&scope, "failure-2", None, "failure", 102);
    let quarantined = reduce_excursions(&Some(repeated), &scope, &[recurrence])
        .expect("independent recurrence is retained without attribution");
    assert_eq!(quarantined.episodes.len(), 2);
    assert!(
        quarantined
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );
    assert_ne!(
        quarantined.episodes[0].episode_id,
        quarantined.episodes[1].episode_id
    );

    // Fail closed on cross-task replay, changed event bytes, and unsafe proposal content.
    let foreign = make_scope("foreign-task");
    assert!(reduce_excursions(&Some(quarantined.clone()), &foreign, &[]).is_err());
    let changed = incident(&scope, "failure-1", None, "recovered", 100);
    assert!(reduce_excursions(&Some(quarantined.clone()), &scope, &[changed]).is_err());

    let safe_contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "round15-task".into()),
            ("steps".into(), "replay the fixed local fixture".into()),
            (
                "expected".into(),
                "one bounded episode per recurrence".into(),
            ),
            ("actual".into(), "two isolated episodes".into()),
            (
                "negation".into(),
                "no external execution or publication".into(),
            ),
        ]),
    };
    let proposal = build_local_proposal(&quarantined.episodes[0], &safe_contract, &[])
        .expect("safe fixture builds local proposal");
    assert_eq!(proposal.scope, scope);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let unsafe_contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "round15-task".into()),
            (
                "steps".into(),
                "publish token=private to https://private.invalid".into(),
            ),
            ("expected".into(), "redacted".into()),
            ("actual".into(), "blocked".into()),
        ]),
    };
    assert!(build_local_proposal(&quarantined.episodes[0], &unsafe_contract, &[]).is_err());
}
