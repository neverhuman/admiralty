#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Hermetic Round 22 canary smoke and release package qualification.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        ExcursionScope, ExcursionState, IncidentEvent, ScopedIncident, reduce_excursions,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanaryPackage {
    schema_version: u32,
    channel: String,
    state: ExcursionState,
    smoke_status: String,
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn scope(task: &str) -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round22-project").expect("static project fixture is valid"),
        root: RootId::new("round22-root").expect("static root fixture is valid"),
        task: TaskId::new(task).expect("static task fixture is valid"),
        contract_revision: 22,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    }
}

fn event(scope: &ExcursionScope, id: &str, parent: Option<&str>, kind: &str) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: id.into(),
            family_signature: "round22-canary-smoke".into(),
            cause_parent: parent.map(str::to_owned),
            kind: kind.into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some("2200".into()),
        },
    }
}

#[test]
fn p27_round22_canary_smoke_release_package_qualification() {
    // Fixed in-memory inputs make smoke replay independent of host paths,
    // environment, wall clock, network, and provider fixtures.
    let packet = scope("canary-packet");
    let foreign_packet = scope("foreign-packet");
    let opening = event(&packet, "canary-failure", None, "failure");
    let opened = reduce_excursions(&None, &packet, std::slice::from_ref(&opening))
        .expect("fixed canary opening is accepted");
    let escalation = event(
        &packet,
        "canary-escalation",
        Some("canary-failure"),
        "escalation",
    );
    let contained = reduce_excursions(&Some(opened), &packet, std::slice::from_ref(&escalation))
        .expect("linked canary escalation stays within its episode");
    assert_eq!(contained.episodes.len(), 1);
    assert_eq!(contained.episodes[0].events.len(), 2);

    let foreign = event(&foreign_packet, "foreign-failure", None, "failure");
    assert!(
        reduce_excursions(
            &Some(contained.clone()),
            &packet,
            std::slice::from_ref(&foreign),
        )
        .is_err()
    );

    let recurrence = event(&packet, "canary-recurrence", None, "failure");
    let quarantined = reduce_excursions(&Some(contained), &packet, &[recurrence])
        .expect("unlinked recurrence is retained as uncertain evidence");
    assert_eq!(quarantined.episodes.len(), 2);
    assert!(
        quarantined
            .episodes
            .iter()
            .all(|episode| episode.unknown_attribution)
    );

    let package = CanaryPackage {
        schema_version: 1,
        channel: "canary".into(),
        state: quarantined,
        smoke_status: "contained; execution disabled".into(),
    };
    let bytes = serde_json::to_vec(&package).expect("package encoding succeeds");
    let replay: CanaryPackage =
        serde_json::from_slice(&bytes).expect("strict package schema accepts its own output");
    assert_eq!(replay, package);
    assert_eq!(
        serde_json::to_vec(&replay).expect("package re-encodes"),
        bytes
    );

    let mut unauthorized = serde_json::to_value(&replay).expect("package becomes JSON");
    unauthorized["execution_authorized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<CanaryPackage>(unauthorized).is_err());
    assert!(
        reduce_excursions(&Some(replay.state.clone()), &foreign_packet, &[]).is_err(),
        "a canary package cannot be replayed under another packet scope"
    );
    assert_eq!(replay.state.scope, packet);
}
