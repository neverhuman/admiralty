#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Round 18 release bundle and serialized-schema parity for placement.

use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation, PlacementInput,
    PlacementPolicy, PlacementRequest, decode_input, propose_placement,
};
use admiralty_core::{ProjectId, RootId, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn decimal(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round18-project").expect("project fixture"),
        root: RootId::new("round18-root").expect("root fixture"),
        task: TaskId::new("round18-task").expect("task fixture"),
        contract_digest: format!("sha256:{}", "e".repeat(64)),
        authority_ref: "round18-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round18-pool".into(),
            amount: decimal(7),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: decimal(6),
        maximum_pressure: decimal(4),
        minimum_spool_reserve: decimal(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(decimal(14)),
    }
}

fn offer(node_id: &str) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(node_id).expect("node fixture"),
        boot_id: "round18-boot".into(),
        authority_generation: decimal(3),
        visible_scopes: Observation {
            value: vec!["round18-project".into()],
            observed_at: decimal(100),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: decimal(100),
        },
        account_pool: Observation {
            value: "round18-pool".into(),
            observed_at: decimal(100),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: decimal(2),
                    budget_remaining: decimal(12),
                },
            },
            observed_at: decimal(100),
        },
        current_pressure: Observation {
            value: Known::Known { value: decimal(1) },
            observed_at: decimal(100),
        },
        spool_free: Observation {
            value: Known::Known { value: decimal(8) },
            observed_at: decimal(100),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(decimal(100)),
        ownership_revision: Some(decimal(14)),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseBundle {
    schema_version: u32,
    input: PlacementInput,
    proposal: admiralty_core::placement::PlacementProposal,
}

#[test]
fn codex_round18_p24_p29_release_bundle_and_schema_parity() {
    let input = PlacementInput {
        request: request(),
        offers: vec![offer("node-a"), offer("node-b")],
        policy: policy(),
        now: Some(decimal(100)),
        previous_tick: Some(decimal(99)),
    };

    let input_value = serde_json::to_value(&input).expect("serialize qualification input");
    let schema_value: Value = serde_json::to_value(schemars::schema_for!(PlacementRequest))
        .expect("serialize generated placement request schema");
    let request_fields = input_value["request"]
        .as_object()
        .expect("serialized placement request is an object");
    let schema_fields = schema_value["properties"]
        .as_object()
        .expect("placement request schema declares object properties");
    assert_eq!(
        request_fields.keys().collect::<Vec<_>>(),
        schema_fields.keys().collect::<Vec<_>>(),
        "serialized placement fields must match the published schema"
    );
    assert_eq!(input_value["now"], "100");
    assert_eq!(input_value["previous_tick"], "99");
    assert_eq!(
        input_value["offers"][0]["capacity"]["value"]["value"]["slots"],
        "2"
    );

    let bytes = serde_json::to_vec(&input).expect("serialize release evidence bundle input");
    let decoded = decode_input(&bytes).expect("bounded decoder accepts schema-conformant input");
    assert_eq!(decoded, input);

    let proposal = propose_placement(&decoded.request, &decoded.offers, &decoded.policy, 100)
        .expect("validated release bundle produces deterministic proposal");
    let replay = propose_placement(&input.request, &input.offers, &input.policy, 100)
        .expect("same release evidence replays");
    assert_eq!(proposal, replay);
    assert_eq!(proposal.candidates.len(), 2);
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);

    let bundle = ReleaseBundle {
        schema_version: 1,
        input,
        proposal,
    };
    let bundle_bytes = serde_json::to_vec(&bundle).expect("serialize complete release bundle");
    let restored: ReleaseBundle =
        serde_json::from_slice(&bundle_bytes).expect("deserialize complete release bundle");
    assert_eq!(restored, bundle);
    assert!(!restored.proposal.execution_granted);
    assert!(!restored.proposal.publication_granted);
    assert!(!restored.proposal.authority_granted);

    let mut tampered: Value = serde_json::from_slice(&bytes).expect("parse fixture JSON");
    tampered["unexpected_schema_field"] = Value::Bool(true);
    let tampered_bytes = serde_json::to_vec(&tampered).expect("serialize tampered input");
    assert!(decode_input(&tampered_bytes).is_err());
}
