//! Round 12 red-team audit of P27's serialized episode and replay receipts.

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

fn scope() -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("round12-project").expect("project fixture"),
        root: RootId::new("round12-root").expect("root fixture"),
        task: TaskId::new("round12-task").expect("task fixture"),
        contract_revision: 2,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    }
}

fn event(id: &str, kind: &str) -> IncidentEvent {
    IncidentEvent {
        event_id: id.into(),
        family_signature: "round12-fixture-failure".into(),
        cause_parent: None,
        kind: kind.into(),
        evidence_digests: vec![digest('c')],
        observed_tick: Some("17".into()),
    }
}

fn scoped(scope: &ExcursionScope, event: IncidentEvent) -> ScopedIncident {
    ScopedIncident {
        scope: scope.clone(),
        event,
    }
}

#[test]
fn p27_round12_item5_redteam_audit_gate() {
    let scope = scope();
    let original = scoped(&scope, event("event-1", "failure"));
    let canonical = reduce_excursions(&None, &scope, std::slice::from_ref(&original))
        .expect("canonical incident state");

    // The event ID's immutable receipt binds exact bytes. A changed payload with
    // the same ID is rejected, while exact replay retains the same state.
    assert_eq!(
        reduce_excursions(
            &Some(canonical.clone()),
            &scope,
            std::slice::from_ref(&original)
        )
        .expect("exact event replay"),
        canonical
    );
    let changed = scoped(&scope, event("event-1", "recovered"));
    assert!(reduce_excursions(&Some(canonical.clone()), &scope, &[changed]).is_err());

    // A forged serialized receipt and a forged episode projection are both
    // rejected when the reducer reconstructs canonical state from event history.
    let mut forged_receipt = serde_json::to_value(&canonical).expect("state serializes");
    forged_receipt["receipts"]["event-1"] = serde_json::Value::String(digest('d'));
    let forged_receipt: admiralty_core::excursions::ExcursionState =
        serde_json::from_value(forged_receipt).expect("receipt shape remains valid");
    assert!(reduce_excursions(&Some(forged_receipt), &scope, &[]).is_err());

    let mut forged_projection = serde_json::to_value(&canonical).expect("state serializes");
    forged_projection["episodes"][0]["events"][0]["kind"] =
        serde_json::Value::String("success".into());
    let forged_projection: admiralty_core::excursions::ExcursionState =
        serde_json::from_value(forged_projection).expect("projection shape remains valid");
    assert!(reduce_excursions(&Some(forged_projection), &scope, &[]).is_err());

    // Tampered scope and noncanonical clock input fail before any proposal can
    // be formed; a valid proposal retains provenance and carries no authority.
    let foreign = ExcursionScope {
        task: TaskId::new("other-task").expect("foreign task fixture"),
        ..scope.clone()
    };
    assert!(reduce_excursions(&Some(canonical.clone()), &foreign, &[]).is_err());
    let malformed = scoped(
        &scope,
        IncidentEvent {
            observed_tick: Some("017".into()),
            ..event("malformed", "failure")
        },
    );
    assert!(reduce_excursions(&None, &scope, &[malformed]).is_err());

    let contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "round12-task".into()),
            ("steps".into(), "replay immutable fixture".into()),
            ("expected".into(), "integrity check passes".into()),
            ("actual".into(), "one failure was recorded".into()),
            ("negation".into(), "do not infer cause".into()),
        ]),
    };
    let proposal = build_local_proposal(
        &canonical.episodes[0],
        &contract,
        &[EvidenceRef {
            digest: digest('c'),
            media_type: "application/json".into(),
        }],
    )
    .expect("canonical evidence forms local proposal");
    assert!(!proposal.execution_granted);
    assert!(!proposal.external_publication_allowed);
    assert_eq!(proposal.evidence_digests, vec![digest('c')]);

    let mut secret = contract;
    secret.fields.insert(
        "steps".into(),
        "send token=private to https://private.invalid".into(),
    );
    assert!(build_local_proposal(&canonical.episodes[0], &secret, &[]).is_err());
}
