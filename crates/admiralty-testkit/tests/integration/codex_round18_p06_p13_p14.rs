#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Round 18 release bundle determinism and Rust-schema output parity audit.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ScheduledTask, SchedulerError, SchedulerSnapshot,
        SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VALIDATORS: &str = include_str!("../../../../packages/contracts/generated/validators.mjs");
const TASK_SCHEMA: &[u8] = include_bytes!("../../../../schemas/task-contract.schema.json");
const COORDINATION_SCHEMA: &[u8] =
    include_bytes!("../../../../schemas/coordination-message.schema.json");
const EVENT_SCHEMA: &[u8] = include_bytes!("../../../../schemas/domain-event.schema.json");

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn assert_schema_fingerprint(name: &str, bytes: &[u8], title: &str) -> String {
    let schema: Value = serde_json::from_slice(bytes).expect("parse checked-in JSON schema");
    assert_eq!(schema["title"], title);
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    if schema["type"] == "object" {
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["schema_version"]["type"], "integer");
    } else {
        let variants = schema["oneOf"].as_array().expect("event schema variants");
        assert!(!variants.is_empty());
        assert!(variants.iter().all(|variant| {
            variant["type"] == "object" && variant["additionalProperties"] == false
        }));
    }
    let digest = sha256(bytes);
    let generated_fingerprint = format!("\"{name}\": \"{digest}\"");
    assert!(
        VALIDATORS.contains(&generated_fingerprint),
        "generated validator fingerprint must match {name} schema bytes"
    );
    digest
}

#[test]
fn codex_round18_p06_p13_p14() {
    let schema_digests = [
        (
            "task-contract",
            assert_schema_fingerprint("task-contract", TASK_SCHEMA, "TaskContract"),
        ),
        (
            "coordination-message",
            assert_schema_fingerprint(
                "coordination-message",
                COORDINATION_SCHEMA,
                "CoordinationMessage",
            ),
        ),
        (
            "domain-event",
            assert_schema_fingerprint("domain-event", EVENT_SCHEMA, "DomainEvent"),
        ),
    ];

    let mut intake = task_fixture("ROUND18-INTAKE");
    intake.objective = "Origin: intake://round18/release; preserve review rationale".into();
    let mut release = task_fixture("ROUND18-RELEASE");
    release.dependencies = vec![intake.task_id.clone()];
    let project_id = intake.project_id.clone();
    let intake_id = intake.task_id.clone();
    let release_id = release.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round18-release-bundle".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![release, intake],
    };
    let plan_source = serde_json::to_vec(&plan).expect("serialize release workplan");
    let preview = preview_workplan(&plan_source).expect("preview release workplan");

    let mut root_builder = tempfile::Builder::new();
    root_builder.prefix("admiralty-round18-bundle-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        root_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = root_builder
        .tempdir()
        .expect("create private bundle workspace");
    let store = Store::open(root.path().join("release.sqlite")).expect("open release registry");
    let request = RegisterWorkplanRequest {
        command_id: "round18-bundle-register".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("digest release plan"),
        source: plan_source.clone(),
    };
    let receipt = store
        .register_workplan(request.clone())
        .expect("record release checkpoint");
    assert_eq!(
        receipt,
        store
            .register_workplan(request)
            .expect("release checkpoint replay")
    );

    let registered = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read release checkpoint")
        .expect("release checkpoint exists");
    assert_eq!(registered.plan, plan);
    let intake_contract = registered
        .plan
        .tasks
        .iter()
        .find(|contract| contract.task_id == intake_id)
        .expect("intake contract retained");
    let release_contract = registered
        .plan
        .tasks
        .iter()
        .find(|contract| contract.task_id == release_id)
        .expect("release contract retained");
    let intake_capsule = compile_task_capsule(intake_contract, "round18-release", 4096)
        .expect("compile intake capsule");
    let release_capsule = compile_task_capsule(release_contract, "round18-release", 4096)
        .expect("compile release capsule");
    assert!(intake_capsule.text.contains("intake://round18/release"));

    let scope = SchedulingScope {
        project_id,
        root_id: intake_contract.root_id.clone(),
        revision: 1,
    };
    let snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![
            ScheduledTask {
                contract: intake_contract.clone(),
                contract_digest: intake_capsule.contract_digest.clone(),
                observed_contract_revision: intake_contract.revision,
                state: TaskSchedulingState::Pending,
                lease: LeaseObservation::Absent,
            },
            ScheduledTask {
                contract: release_contract.clone(),
                contract_digest: release_capsule.contract_digest.clone(),
                observed_contract_revision: release_contract.revision,
                state: TaskSchedulingState::Pending,
                lease: LeaseObservation::Absent,
            },
        ],
        available_quota: Knowledge::Known(2),
        available_capacity: Knowledge::Known(2),
    };
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("release readiness proposal");
    assert_eq!(proposal.ready.as_slice(), std::slice::from_ref(&intake_id));
    assert!(!proposal.execution_granted);

    let receipt_value = serde_json::to_value(&registered.receipt).expect("receipt JSON");
    assert_eq!(receipt_value["recorded_only"], true);
    assert_eq!(receipt_value["task_events_emitted"], false);
    assert_eq!(receipt_value["execution_granted"], false);
    assert_eq!(receipt_value["acceptance_granted"], false);
    let bundle = json!({
        "schema_version": 1,
        "plan_source_sha256": sha256(&plan_source),
        "plan_digest": content_digest(&registered.plan).expect("digest registered plan"),
        "task_order": preview.task_order,
        "task_digests": preview.task_digests,
        "schema_digests": schema_digests,
        "capsule_digests": [intake_capsule.contract_digest, release_capsule.contract_digest],
        "checkpoint_proposal": proposal,
        "registration_receipt": receipt_value,
    });
    let bundle_bytes = serde_json::to_vec(&bundle).expect("serialize release bundle");
    let replay_bytes = serde_json::to_vec(&bundle).expect("replay bundle serialization");
    assert_eq!(bundle_bytes, replay_bytes);
    let bundle_digest = sha256(&bundle_bytes);
    let bundle_path = root.path().join("release-bundle.json");
    std::fs::write(&bundle_path, &bundle_bytes).expect("write release bundle artifact");
    let reopened_bundle = std::fs::read(&bundle_path).expect("read release bundle artifact");
    assert_eq!(sha256(&reopened_bundle), bundle_digest);

    let mut forged = snapshot;
    forged.tasks[1].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&forged, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == release_id.as_str()
    ));
    assert!(
        store
            .events_after(0, 10)
            .expect("bundle build emits no events")
            .is_empty()
    );
}
