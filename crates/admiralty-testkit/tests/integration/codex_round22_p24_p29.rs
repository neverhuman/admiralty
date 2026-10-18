#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Round 22 canary and release package qualification for placement evidence.

use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
    PlacementInput, PlacementPolicy, PlacementRequest, decode_input, propose_placement,
};
use admiralty_core::{ProjectId, RootId, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn d(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round22-project").expect("project fixture"),
        root: RootId::new("round22-root").expect("root fixture"),
        task: TaskId::new("round22-task").expect("task fixture"),
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "round22-authority".into(),
        required_capabilities: vec!["rust".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "round22-pool".into(),
            amount: d(5),
        },
    }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy {
        max_observation_age: d(5),
        maximum_pressure: d(4),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(22)),
    }
}

fn offer(id: &str, slots: u64, revision: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new(id).expect("node fixture"),
        boot_id: "round22-boot".into(),
        authority_generation: d(3),
        visible_scopes: Observation {
            value: vec!["round22-project".into()],
            observed_at: d(220),
        },
        capabilities: Observation {
            value: vec!["rust".into()],
            observed_at: d(220),
        },
        account_pool: Observation {
            value: "round22-pool".into(),
            observed_at: d(220),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: d(slots),
                    budget_remaining: d(10),
                },
            },
            observed_at: d(220),
        },
        current_pressure: Observation {
            value: Known::Known { value: d(1) },
            observed_at: d(220),
        },
        spool_free: Observation {
            value: Known::Known { value: d(8) },
            observed_at: d(220),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(d(220)),
        ownership_revision: Some(d(revision)),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseCanaryBundle {
    schema_version: u32,
    input: PlacementInput,
    proposal: admiralty_core::placement::PlacementProposal,
}

fn assert_proposal_is_non_authorizing(
    proposal: &admiralty_core::placement::PlacementProposal,
) {
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

#[test]
fn codex_round22_p24_p29_canary_smoke_and_release_package_qualification() {
    let input = PlacementInput {
        request: request(),
        offers: vec![offer("node-b", 1, 22), offer("node-a", 2, 22)],
        policy: policy(),
        now: Some(d(220)),
        previous_tick: Some(d(219)),
    };

    // A canary replay from the same release evidence is byte stable and ranked.
    let encoded = serde_json::to_vec(&input).expect("serialize release package input");
    let decoded = decode_input(&encoded).expect("bounded canary input decoder accepts bundle");
    assert_eq!(decoded, input);
    let proposal = propose_placement(&decoded.request, &decoded.offers, &decoded.policy, 220)
        .expect("valid canary package produces a placement proposal");
    let replay = propose_placement(&input.request, &input.offers, &input.policy, 220)
        .expect("same release evidence replays");
    assert_eq!(proposal, replay);
    assert_eq!(proposal.candidates.len(), 2);
    assert_eq!(proposal.candidates[0].node_id.as_str(), "node-a");
    assert_proposal_is_non_authorizing(&proposal);

    let package = ReleaseCanaryBundle {
        schema_version: 1,
        input: input.clone(),
        proposal,
    };
    let package_bytes = serde_json::to_vec(&package).expect("serialize canary release bundle");
    let restored: ReleaseCanaryBundle =
        serde_json::from_slice(&package_bytes).expect("restore canary release bundle");
    assert_eq!(restored, package);
    assert_proposal_is_non_authorizing(&restored.proposal);

    // Unknown capacity cannot turn a package replay into a canary candidate.
    let mut uncertain = offer("node-uncertain", 1, 22);
    uncertain.capacity.value = Known::Unknown {
        reason: "canary capacity observation unavailable".into(),
    };
    let closed = propose_placement(&input.request, &[uncertain], &input.policy, 220)
        .expect("unknown capacity is a fail-closed proposal result");
    assert!(closed.candidates.is_empty());
    assert_eq!(closed.rejected.len(), 1);
    assert_proposal_is_non_authorizing(&closed);

    // The release decoder rejects unrecognized fields before a canary can use them.
    let mut tampered: Value = serde_json::from_slice(&encoded).expect("parse package input");
    tampered["unreviewed_release_field"] = Value::Bool(true);
    let tampered_bytes = serde_json::to_vec(&tampered).expect("serialize tampered package input");
    assert!(decode_input(&tampered_bytes).is_err());
}
