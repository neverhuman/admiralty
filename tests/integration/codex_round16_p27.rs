//! Round 16 end-to-end cutover and fault-containment qualification for P27.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        ExcursionScope, IncidentEvent, ReproductionContract, ScopedIncident, build_local_proposal,
        reduce_excursions,
    },
};
use std::{collections::BTreeMap, error::Error};

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn make_scope(task: &str) -> Result<ExcursionScope, Box<dyn Error>> {
    Ok(ExcursionScope {
        project: ProjectId::new("round16-project")?,
        root: RootId::new("round16-root")?,
        task: TaskId::new(task)?,
        contract_revision: 16,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    })
}

fn event(
    scope: &ExcursionScope,
    id: &str,
    parent: Option<&str>,
    kind: &str,
    tick: &str,
) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: id.into(),
            family_signature: "round16-cutover-fault".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.into()),
        },
    }
}

#[test]
fn p27_round16_end_to_end_cutover_fault_containment() -> Result<(), Box<dyn Error>> {
    let scope = make_scope("cutover-task")?;
    let first_event = event(&scope, "failure-1", None, "failure", "1600");
    let initial = reduce_excursions(&None, &scope, std::slice::from_ref(&first_event))?;

    // Replaying the same cutover history through many deterministic steps must
    // preserve the canonical projection and its serialized bytes.
    let mut replay = initial.clone();
    for _ in 0..64 {
        replay = reduce_excursions(&Some(replay), &scope, std::slice::from_ref(&first_event))?;
    }
    assert_eq!(replay, initial);
    let canonical_bytes = serde_json::to_vec(&initial)?;
    assert_eq!(serde_json::to_vec(&replay)?, canonical_bytes);

    let escalation = event(
        &scope,
        "escalation-1",
        Some("failure-1"),
        "escalation",
        "1601",
    );
    let correlated = reduce_excursions(
        &Some(initial.clone()),
        &scope,
        std::slice::from_ref(&escalation),
    )?;
    assert_eq!(correlated.episodes.len(), 1);
    assert_eq!(correlated.episodes[0].events.len(), 2);
    let replayed_escalation = reduce_excursions(
        &Some(correlated.clone()),
        &scope,
        std::slice::from_ref(&escalation),
    )?;
    assert_eq!(replayed_escalation, correlated);

    // Independent recurrence stays isolated and unattributed; all malformed,
    // foreign, or conflicting inputs are rejected without mutating prior state.
    let recurrence = event(&scope, "failure-2", None, "failure", "1602");
    let quarantined = reduce_excursions(
        &Some(replayed_escalation),
        &scope,
        std::slice::from_ref(&recurrence),
    )?;
    assert_eq!(quarantined.episodes.len(), 2);
    assert!(
        quarantined
            .episodes
            .iter()
            .all(|row| row.unknown_attribution)
    );
    assert_ne!(
        quarantined.episodes[0].episode_id,
        quarantined.episodes[1].episode_id
    );
    let quarantine_bytes = serde_json::to_vec(&quarantined)?;

    let foreign_scope = make_scope("foreign-task")?;
    assert!(reduce_excursions(&Some(quarantined.clone()), &foreign_scope, &[]).is_err());
    let changed_replay = event(&scope, "failure-1", None, "recovered", "1600");
    assert!(reduce_excursions(&Some(quarantined.clone()), &scope, &[changed_replay]).is_err());
    let predating_child = event(
        &scope,
        "invalid-child",
        Some("failure-1"),
        "escalation",
        "1599",
    );
    assert!(reduce_excursions(&Some(quarantined.clone()), &scope, &[predating_child]).is_err());
    let malformed_clock = event(&scope, "invalid-clock", None, "failure", "01603");
    assert!(reduce_excursions(&Some(quarantined.clone()), &scope, &[malformed_clock]).is_err());
    assert_eq!(serde_json::to_vec(&quarantined)?, quarantine_bytes);

    let proposal = build_local_proposal(
        &quarantined.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "round16-task; revision 16".into()),
                ("steps".into(), "replay the local fault fixture".into()),
                ("expected".into(), "bounded isolated recurrence".into()),
                (
                    "actual".into(),
                    "two separately quarantined episodes".into(),
                ),
                (
                    "negation".into(),
                    "no external execution or publication".into(),
                ),
            ]),
        },
        &[],
    )?;
    assert_eq!(proposal.scope, scope);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let unsafe_contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "round16-task".into()),
            (
                "steps".into(),
                "publish token=secret to https://private.invalid".into(),
            ),
            ("expected".into(), "redacted".into()),
            ("actual".into(), "must fail closed".into()),
        ]),
    };
    assert!(build_local_proposal(&quarantined.episodes[0], &unsafe_contract, &[]).is_err());

    Ok(())
}
