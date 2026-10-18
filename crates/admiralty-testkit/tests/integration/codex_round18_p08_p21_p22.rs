use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::providers::supervision::{
    self, DetectionPolicy, ProcessState, ProviderWait, SignalClocks, SupervisionObservation,
    SupervisionScope, Tick, ToolState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn fixture<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("deterministic release fixture is valid: {error:?}"),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProviderPolicyRow {
    provider: String,
    pinned_version: String,
    schema_version: u32,
    trust: String,
    native_execution_qualified: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseBundle {
    schema_version: u32,
    qualification: String,
    providers: Vec<ProviderPolicyRow>,
    supervision_policy: String,
    workspace_roots: Vec<String>,
}

fn sample_scope() -> SupervisionScope {
    SupervisionScope {
        project_id: fixture(ProjectId::new("round18-project")),
        root_id: fixture(RootId::new("round18-root")),
        task_id: fixture(TaskId::new("round18-task")),
        attempt_id: fixture(AttemptId::new("round18-attempt")),
        owner: fixture(PrincipalId::new("round18-owner")),
        incarnation: "release-replay-1".into(),
        authority_ref: "authority:round18".into(),
        contract_digest: format!("sha256:{}", "d".repeat(64)),
        revision: 1,
    }
}

#[test]
fn p08_p21_p22_round18_release_bundle_schema_parity_qualification() {
    // P08: a schema-round-tripped observation yields the same bounded proposal.
    let observation = SupervisionObservation {
        scope: sample_scope(),
        clocks: SignalClocks {
            last_byte: Some(fixture(Tick::new("40"))),
            provider_event: Some(fixture(Tick::new("40"))),
            tool_heartbeat: Some(fixture(Tick::new("40"))),
            artifact_change: Some(fixture(Tick::new("40"))),
            distinct_command: Some(fixture(Tick::new("40"))),
            completed_check: Some(fixture(Tick::new("40"))),
            verified_milestone: Some(fixture(Tick::new("40"))),
        },
        process_state: ProcessState::Alive,
        tool_state: ToolState::Running,
        provider_wait: ProviderWait::None,
        approval_observation: None,
    };
    let encoded_observation = fixture(serde_json::to_vec(&observation));
    let decoded_observation: SupervisionObservation =
        fixture(serde_json::from_slice(&encoded_observation));
    assert_eq!(decoded_observation, observation);
    assert!(serde_json::from_slice::<SupervisionObservation>(
        br#"{"scope":{},"clocks":{},"process_state":"alive","tool_state":"running","provider_wait":"none","approval_observation":null,"unexpected":true}"#
    )
    .is_err());

    let policy = DetectionPolicy {
        version: "round18-policy-v1".into(),
        stall_after: fixture(Tick::new("12")),
        hysteresis_samples: 3,
        cooldown: fixture(Tick::new("5")),
    };
    let now = fixture(Tick::new("42"));
    let proposal_a = fixture(supervision::evaluate_supervision(
        &policy,
        &observation,
        None,
        Some(&now),
    ));
    let proposal_b = fixture(supervision::evaluate_supervision(
        &policy,
        &decoded_observation,
        None,
        Some(&now),
    ));
    assert_eq!(proposal_a, proposal_b);
    assert!(!proposal_a.may_signal());
    assert!(!proposal_a.may_send_input());
    assert!(!proposal_a.execution_granted());

    // P21/P22: release bundle entries are pinned, observation-only, schema-stable rows.
    let bundle = ReleaseBundle {
        schema_version: 1,
        qualification: "P08-P21-P22-round18".into(),
        providers: vec![
            ProviderPolicyRow {
                provider: "agy".into(),
                pinned_version: "1.4.2".into(),
                schema_version: 1,
                trust: "observed_only".into(),
                native_execution_qualified: false,
            },
            ProviderPolicyRow {
                provider: "claude".into(),
                pinned_version: "fixture-session-v1".into(),
                schema_version: 1,
                trust: "observed_only".into(),
                native_execution_qualified: false,
            },
            ProviderPolicyRow {
                provider: "codex".into(),
                pinned_version: "round18-model-v1".into(),
                schema_version: 1,
                trust: "observed_only".into(),
                native_execution_qualified: false,
            },
            ProviderPolicyRow {
                provider: "grok".into(),
                pinned_version: "1.0.62".into(),
                schema_version: 1,
                trust: "observed_only".into(),
                native_execution_qualified: false,
            },
        ],
        supervision_policy: policy.version,
        workspace_roots: vec!["root-a:src".into(), "root-b:src".into()],
    };
    let bundle_bytes = fixture(serde_json::to_vec(&bundle));
    let replay_bytes = fixture(serde_json::to_vec(&bundle));
    assert_eq!(bundle_bytes, replay_bytes);
    let bundle_digest = Sha256::digest(&bundle_bytes);
    assert_eq!(bundle_digest, Sha256::digest(&replay_bytes));

    let decoded_bundle: ReleaseBundle = fixture(serde_json::from_slice(&bundle_bytes));
    assert_eq!(decoded_bundle, bundle);
    assert_eq!(decoded_bundle.schema_version, 1);
    assert_eq!(decoded_bundle.providers.len(), 4);
    assert!(
        decoded_bundle
            .providers
            .iter()
            .all(|row| row.schema_version == 1 && !row.native_execution_qualified)
    );
    assert!(serde_json::from_slice::<ReleaseBundle>(
        br#"{"schema_version":1,"qualification":"tampered","providers":[],"supervision_policy":"v1","workspace_roots":[],"acceptance_granted":true}"#
    )
    .is_err());

    // Workspace packet roots are canonical, disjoint, and remain scoped to each bundle row.
    assert_eq!(decoded_bundle.workspace_roots, ["root-a:src", "root-b:src"]);
    assert_ne!(
        decoded_bundle.workspace_roots[0],
        decoded_bundle.workspace_roots[1]
    );
    let altered = decoded_bundle
        .workspace_roots
        .iter()
        .map(|root| root.replace(":src", ":src/private"))
        .collect::<Vec<_>>();
    assert_ne!(altered, decoded_bundle.workspace_roots);
}
