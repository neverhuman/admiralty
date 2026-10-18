#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Hermetic Round 21 release qualification for P27 cross-packet containment.

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
struct HermeticReleaseBundle {
    schema_version: u32,
    state: ExcursionState,
    proposal: LocalProposal,
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn scope(task: &str) -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round21-project").expect("static project fixture is valid"),
        root: RootId::new("round21-root").expect("static root fixture is valid"),
        task: TaskId::new(task).expect("static task fixture is valid"),
        contract_revision: 21,
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
            family_signature: "round21-hermetic-release".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some(tick.to_string()),
        },
    }
}

#[test]
fn p27_round21_hermetic_cross_packet_release_qualification() {
    // No clock, filesystem, environment, provider, or network state is read:
    // fixed fixtures exercise the reducer and release serialization directly.
    let packet_a = scope("packet-a");
    let packet_b = scope("packet-b");

    let root_failure = incident(&packet_a, "a-failure-1", None, "failure", 2100);
    let initial = reduce_excursions(&None, &packet_a, std::slice::from_ref(&root_failure))
        .expect("fixed packet A event is valid");
    let linked_escalation = incident(
        &packet_a,
        "a-escalation-1",
        Some("a-failure-1"),
        "escalation",
        2101,
    );
    let correlated = reduce_excursions(
        &Some(initial),
        &packet_a,
        std::slice::from_ref(&linked_escalation),
    )
    .expect("explicit parent keeps escalation in packet A episode");
    assert_eq!(correlated.episodes.len(), 1);
    assert_eq!(correlated.episodes[0].events.len(), 2);

    let foreign_event = incident(&packet_b, "b-failure-1", None, "failure", 2100);
    assert!(
        reduce_excursions(
            &Some(correlated.clone()),
            &packet_a,
            std::slice::from_ref(&foreign_event),
        )
        .is_err()
    );
    let isolated_b = reduce_excursions(&None, &packet_b, &[foreign_event])
        .expect("packet B starts an independent bounded state");
    assert_eq!(isolated_b.scope, packet_b);
    assert_eq!(isolated_b.episodes.len(), 1);

    let recurrence = incident(&packet_a, "a-failure-2", None, "failure", 2102);
    let quarantined = reduce_excursions(&Some(correlated), &packet_a, &[recurrence])
        .expect("unlinked recurrence remains quarantined");
    assert_eq!(quarantined.episodes.len(), 2);
    assert!(
        quarantined
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );

    let proposal = build_local_proposal(
        &quarantined.episodes[0],
        &ReproductionContract {
            fields: BTreeMap::from([
                ("scope".into(), "packet-a; revision 21".into()),
                (
                    "steps".into(),
                    "replay deterministic in-memory fixture".into(),
                ),
                (
                    "expected".into(),
                    "same scoped episode after every replay".into(),
                ),
                (
                    "actual".into(),
                    "correlated chain and isolated recurrence".into(),
                ),
                (
                    "negation".into(),
                    "do not execute or publish the proposal".into(),
                ),
            ]),
        },
        &[],
    )
    .expect("safe local proposal is constructed");
    assert_eq!(proposal.scope, packet_a);
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);

    let bundle = HermeticReleaseBundle {
        schema_version: 1,
        state: quarantined,
        proposal,
    };
    let bytes = serde_json::to_vec(&bundle).expect("release bundle serializes");
    let decoded: HermeticReleaseBundle =
        serde_json::from_slice(&bytes).expect("release bundle parses under strict schema");
    assert_eq!(decoded, bundle);
    for _ in 0..21 {
        let replay = reduce_excursions(&Some(decoded.state.clone()), &packet_a, &[])
            .expect("idempotent release replay stays canonical");
        assert_eq!(replay, decoded.state);
    }
    assert_eq!(
        serde_json::to_vec(&decoded).expect("release bundle deterministically reserializes"),
        bytes
    );

    let mut unexpected = serde_json::to_value(&decoded).expect("bundle becomes JSON");
    unexpected["execution_authorized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<HermeticReleaseBundle>(unexpected).is_err());

    let foreign_scope = scope("packet-c");
    assert!(reduce_excursions(&Some(decoded.state.clone()), &foreign_scope, &[]).is_err());
    assert_eq!(decoded.state.scope, packet_a);
}
