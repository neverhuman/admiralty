//! Deterministic cross-packet export qualification for P27, P24, and P25.
//! These public proposal APIs are composed offline; the test invokes no
//! Telegram/webhook, node, provider, executor, restore, or publication effect.

#[path = "support/recovery.rs"]
mod recovery_fixture;

use admiralty_core::{
    ProjectId, RootId, TaskId,
    excursions::{
        EvidenceRef, ExcursionScope, IncidentEvent, ReproductionContract, ScopedIncident,
        build_local_proposal, reduce_excursions,
    },
    placement::{
        BudgetRequirement, Capacity, DecimalU64 as PlacementU64, Known, NodeId, NodeOffer,
        Observation, PlacementPolicy, PlacementRequest, propose_placement,
    },
};
use admiralty_runtime::recovery::{Reconciliation, plan_recovery};
use std::collections::BTreeMap;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn d(value: u64) -> PlacementU64 {
    PlacementU64::new(value)
}

fn offer(id: &str) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(id).expect("fixture node ID"),
        boot_id: "boot-fixed".into(),
        authority_generation: d(1),
        visible_scopes: Observation {
            value: vec!["project-round10".into()],
            observed_at: d(10),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: d(10),
        },
        account_pool: Observation {
            value: "pool-round10".into(),
            observed_at: d(10),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: d(1),
                    budget_remaining: d(10),
                },
            },
            observed_at: d(10),
        },
        current_pressure: Observation {
            value: Known::Known { value: d(1) },
            observed_at: d(10),
        },
        spool_free: Observation {
            value: Known::Known { value: d(10) },
            observed_at: d(10),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(d(10)),
        ownership_revision: Some(d(1)),
    }
}

#[test]
fn p27_round9_item3_export_telegram_webhook_qualification() {
    // P24: sorted placement is a deterministic proposal and grants no execution.
    let request = PlacementRequest {
        project: ProjectId::new("project-round10").expect("project fixture"),
        root: RootId::new("root-round10").expect("root fixture"),
        task: TaskId::new("task-round10").expect("task fixture"),
        contract_digest: digest('a'),
        authority_ref: "grant-fixture".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "pool-round10".into(),
            amount: d(5),
        },
    };
    let policy = PlacementPolicy {
        max_observation_age: d(10),
        maximum_pressure: d(5),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(1)),
    };
    let placement = propose_placement(&request, &[offer("node-b"), offer("node-a")], &policy, 10)
        .expect("bounded fleet placement proposal");
    let placement_replay =
        propose_placement(&request, &[offer("node-a"), offer("node-b")], &policy, 10)
            .expect("permutation replay");
    assert_eq!(placement, placement_replay);
    assert_eq!(placement.candidates[0].node_id.as_str(), "node-a");
    assert!(!placement.execution_granted);
    assert!(!placement.authority_granted);

    // P25: complete fixed recovery evidence proposes resume without authorizing it.
    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let recovery = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("fixed recovery evidence is accepted");
    assert!(recovery.resume_proposed);
    assert!(!recovery.restore_authorized());
    assert!(!recovery.effects_authorized());
    let mut uncertain = observation;
    uncertain.unresolved_effects = Reconciliation::Unknown;
    let blocked = plan_recovery(&manifest, &uncertain, Some(&fence))
        .expect("unknown external effect remains a typed blocked proposal");
    assert!(!blocked.resume_proposed);

    // P27: reduce a bounded incident and produce a sanitized local proposal.
    let scope = ExcursionScope {
        project: ProjectId::new("project-round10").expect("project fixture"),
        root: RootId::new("root-round10").expect("root fixture"),
        task: TaskId::new("task-round10").expect("task fixture"),
        contract_revision: 1,
        contract_digest: digest('a'),
        protected_oracle_digest: digest('b'),
    };
    let incident = ScopedIncident {
        scope: scope.clone(),
        event: IncidentEvent {
            event_id: "incident-round10".into(),
            family_signature: "export-delivery-timeout".into(),
            cause_parent: None,
            kind: "failure".into(),
            evidence_digests: vec![digest('c')],
            observed_tick: Some("42".into()),
        },
    };
    let state = reduce_excursions(&None, &scope, std::slice::from_ref(&incident))
        .expect("scoped incident reduces");
    let contract = ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "task-round10".into()),
            ("steps".into(), "replay fixed delivery fixture".into()),
            ("expected".into(), "proposal is stable".into()),
            ("actual".into(), "timeout was observed".into()),
            ("negation".into(), "do not contact external services".into()),
        ]),
    };
    let proposal = build_local_proposal(
        &state.episodes[0],
        &contract,
        &[EvidenceRef {
            digest: digest('c'),
            media_type: "application/json".into(),
        }],
    )
    .expect("sanitized local proposal");
    assert_eq!(proposal.scope, scope);
    assert_eq!(
        proposal,
        build_local_proposal(
            &state.episodes[0],
            &contract,
            &[EvidenceRef {
                digest: digest('c'),
                media_type: "application/json".into()
            }],
        )
        .expect("identical proposal replay")
    );

    // Stable serialized artifacts are suitable for local export/inspection only.
    let export_a = serde_json::to_vec(&(&placement, &recovery, &proposal))
        .expect("proposal export serializes");
    let export_b = serde_json::to_vec(&(&placement_replay, &recovery, &proposal))
        .expect("replayed proposal export serializes");
    assert_eq!(export_a, export_b);
}
