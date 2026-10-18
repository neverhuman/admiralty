//! Round 17 post-item-5 audit of cross-packet P27 containment invariants.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        ExcursionScope, ExcursionState, IncidentEvent, MAX_CAUSAL_DEPTH, ReproductionContract,
        ScopedIncident, build_local_proposal, reduce_excursions,
    },
};
use std::{collections::BTreeMap, error::Error};

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn scope(task: &str) -> Result<ExcursionScope, Box<dyn Error>> {
    Ok(ExcursionScope {
        project: ProjectId::new("round17-project")?,
        root: RootId::new("round17-root")?,
        task: TaskId::new(task)?,
        contract_revision: 17,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    })
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
            family_signature: "round17-release-audit".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.to_string()),
        },
    }
}

#[test]
fn p27_round17_post_item5_release_audit() -> Result<(), Box<dyn Error>> {
    let task_a = scope("packet-a")?;
    let task_b = scope("packet-b")?;
    let failure = incident(&task_a, "failure-a1", None, "failure", 1700);
    let base = reduce_excursions(&None, &task_a, std::slice::from_ref(&failure))?;

    let escalation = incident(
        &task_a,
        "escalation-a1",
        Some("failure-a1"),
        "escalation",
        1701,
    );
    let correlated = reduce_excursions(
        &Some(base.clone()),
        &task_a,
        std::slice::from_ref(&escalation),
    )?;
    assert_eq!(correlated.episodes.len(), 1);
    assert_eq!(correlated.episodes[0].events.len(), 2);

    // Cross-packet replay preserves the task A projection while task B is
    // independently reduced and quarantined with its own scope and lineage.
    let packet_b_event = incident(&task_b, "failure-b1", None, "failure", 1700);
    assert!(
        reduce_excursions(
            &Some(correlated.clone()),
            &task_a,
            std::slice::from_ref(&packet_b_event),
        )
        .is_err()
    );
    let isolated_b = reduce_excursions(&None, &task_b, &[packet_b_event])?;
    assert_eq!(isolated_b.episodes.len(), 1);
    assert_eq!(isolated_b.episodes[0].scope, task_b);
    assert_eq!(
        correlated.episodes.len(),
        1,
        "foreign input leaves prior state intact"
    );

    let recurrence = incident(&task_a, "failure-a2", None, "failure", 1702);
    let quarantined = reduce_excursions(&Some(correlated), &task_a, &[recurrence])?;
    assert_eq!(quarantined.episodes.len(), 2);
    assert!(
        quarantined
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );
    let canonical = serde_json::to_vec(&quarantined)?;

    // Receipt/projection corruption, event byte substitution, a causal child
    // before its parent, and an over-depth chain must all fail closed.
    let mut forged_receipt = serde_json::to_value(&quarantined)?;
    forged_receipt["receipts"]["failure-a1"] = serde_json::Value::String(digest('d'));
    let forged_receipt: ExcursionState = serde_json::from_value(forged_receipt)?;
    assert!(reduce_excursions(&Some(forged_receipt), &task_a, &[]).is_err());

    let mut forged_projection = serde_json::to_value(&quarantined)?;
    forged_projection["episodes"][0]["events"][0]["kind"] =
        serde_json::Value::String("success".into());
    let forged_projection: ExcursionState = serde_json::from_value(forged_projection)?;
    assert!(reduce_excursions(&Some(forged_projection), &task_a, &[]).is_err());

    let substituted = incident(&task_a, "failure-a1", None, "recovered", 1700);
    assert!(reduce_excursions(&Some(quarantined.clone()), &task_a, &[substituted]).is_err());
    let predating_child = incident(
        &task_a,
        "late-child",
        Some("failure-a1"),
        "escalation",
        1699,
    );
    assert!(reduce_excursions(&Some(quarantined.clone()), &task_a, &[predating_child]).is_err());
    assert_eq!(serde_json::to_vec(&quarantined)?, canonical);

    let mut bounded_chain = None;
    for depth in 0..=MAX_CAUSAL_DEPTH {
        let id = format!("chain-{depth}");
        let parent = (depth > 0).then(|| format!("chain-{}", depth - 1));
        let link = incident(
            &task_a,
            &id,
            parent.as_deref(),
            "failure",
            1800 + depth as u64,
        );
        bounded_chain = Some(reduce_excursions(
            &bounded_chain,
            &task_a,
            std::slice::from_ref(&link),
        )?);
    }
    let overflow = incident(
        &task_a,
        &format!("chain-{}", MAX_CAUSAL_DEPTH + 1),
        Some(&format!("chain-{MAX_CAUSAL_DEPTH}")),
        "failure",
        1800 + MAX_CAUSAL_DEPTH as u64 + 1,
    );
    assert!(reduce_excursions(&bounded_chain, &task_a, &[overflow]).is_err());

    let proposal = build_local_proposal(
        &quarantined.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "packet-a; contract revision 17".into()),
                (
                    "steps".into(),
                    "replay only the immutable local fixture".into(),
                ),
                (
                    "expected".into(),
                    "correlated escalation and isolated recurrence".into(),
                ),
                (
                    "actual".into(),
                    "two scoped episodes with unknown attribution".into(),
                ),
                (
                    "negation".into(),
                    "no execution, publication, or inferred cause".into(),
                ),
            ]),
        },
        &[],
    )?;
    assert_eq!(proposal.scope, task_a);
    assert!(proposal.uncertainty.contains("unknown"));
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let unsafe_contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "packet-a".into()),
            (
                "steps".into(),
                "send token=secret to https://private.invalid".into(),
            ),
            ("expected".into(), "blocked".into()),
            ("actual".into(), "fail closed".into()),
        ]),
    };
    assert!(build_local_proposal(&quarantined.episodes[0], &unsafe_contract, &[]).is_err());
    Ok(())
}
