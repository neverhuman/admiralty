use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::workplans::{
    MAX_WORKPLAN_BYTES, Workplan, WorkplanError, preview_workplan, preview_workplan_with_existing,
};
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, MessageId, PrincipalId, ProjectId, RootId, TaskContract, TaskId,
};
use std::error::Error;

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

fn task(task_id: &str, project_id: ProjectId) -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id,
        root_id: RootId::new("round15-root")?,
        task_id: TaskId::new(task_id)?,
        revision: 1,
        title: format!("Release smoke {task_id}"),
        objective: "Run deterministic release checks".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "smoke".into(),
            text: "The declared release check passes".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "grant:round15".into(),
        max_attempts: 1,
        simulated: true,
    })
}

#[test]
fn p07_p17_round15_release_smoke_multistep_stress() -> Result<(), Box<dyn Error>> {
    let project = ProjectId::new("round15-project")?;
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round15-smoke-plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![task("release", project.clone())?, task("verify", project)?],
    };
    let source = serde_json::to_vec(&plan)?;
    let preview = preview_workplan(&source)?;
    let replay = preview_workplan_with_existing(&source, &preview.task_digests)?;
    assert_eq!(
        preview, replay,
        "work-order preview API replay must be stable"
    );
    assert_eq!(preview.task_order.len(), 2);
    assert_eq!(
        preview_workplan(&vec![b' '; MAX_WORKPLAN_BYTES + 1]),
        Err(WorkplanError::TooLarge),
        "oversized release input must fail before parsing"
    );

    let scope = InterfaceScope {
        project_id: ProjectId::new("round15-project")?,
        root_id: RootId::new("round15-root")?,
        task_id: TaskId::new("round15-task")?,
        authority_ref: "grant:round15".into(),
    };
    let producer = principal("round15-producer")?;
    let captain = principal("round15-captain")?;
    let owner = principal("round15-owner")?;
    let interface_id = InterfaceId::new("round15-release-api")?;
    let mut state = None;

    // Exercise repeated publish/ack/replay transitions across distinct revisions.
    for revision in 1_u64..=32 {
        let artifact_digest = format!("sha256:{:064x}", revision);
        let schema_digest = format!("sha256:{:064x}", revision + 100);
        let contract = InterfaceContract {
            interface_id: interface_id.clone(),
            scope: scope.clone(),
            producer: producer.clone(),
            consumers: vec![captain.clone()],
            revision: DecimalU64::new(revision),
            artifact_digest: artifact_digest.clone(),
            schema_digest: schema_digest.clone(),
        };
        let publish = InterfaceEvent::Publish {
            event_id: MessageId::new(format!("round15-publish-{revision}"))?,
            contract,
            actor: producer.clone(),
            authority_owner: owner.clone(),
            recipient_epochs: vec![RecipientEpoch {
                recipient: captain.clone(),
                epoch: format!("captain-epoch-{revision}"),
            }],
            expected_state_revision: state.as_ref().map(
                |current: &admiralty_core::interfaces::InterfaceState| current.revision.clone(),
            ),
        };
        let encoded = serde_json::to_vec(&publish)?;
        let via_json_api: InterfaceEvent = serde_json::from_slice(&encoded)?;
        let via_schema_api = decode_event(&encoded)?;
        assert_eq!(via_json_api, via_schema_api, "decode APIs must agree");
        let published = reduce_interface_for(&state, &via_schema_api, &scope, &producer, &owner)?;
        let replayed_publish = reduce_interface_for(
            &Some(published.state.clone()),
            &via_schema_api,
            &scope,
            &producer,
            &owner,
        )?;
        assert!(replayed_publish.replayed);
        assert_eq!(replayed_publish.state, published.state);
        state = Some(published.state);

        let ack = InterfaceEvent::Acknowledge {
            event_id: MessageId::new(format!("round15-ack-{revision}"))?,
            interface_id: interface_id.clone(),
            scope: scope.clone(),
            actor: captain.clone(),
            recipient_epoch: format!("captain-epoch-{revision}"),
            interface_revision: DecimalU64::new(revision),
            artifact_digest,
            schema_digest,
            expected_state_revision: state
                .as_ref()
                .map(|current| current.revision.clone())
                .ok_or("publish did not produce interface state")?,
            authority_ref: scope.authority_ref.clone(),
        };
        let acknowledged = reduce_interface_for(&state, &ack, &scope, &captain, &owner)?;
        let replayed_ack = reduce_interface_for(
            &Some(acknowledged.state.clone()),
            &ack,
            &scope,
            &captain,
            &owner,
        )?;
        assert!(replayed_ack.replayed);
        assert_eq!(replayed_ack.state, acknowledged.state);
        state = Some(acknowledged.state);
    }

    let mut forged: serde_json::Value =
        serde_json::from_slice(&serde_json::to_vec(&InterfaceEvent::Acknowledge {
            event_id: MessageId::new("round15-forged-ack")?,
            interface_id,
            scope: scope.clone(),
            actor: captain.clone(),
            recipient_epoch: "captain-epoch-32".into(),
            interface_revision: DecimalU64::new(32),
            artifact_digest: format!("sha256:{:064x}", 32_u64),
            schema_digest: format!("sha256:{:064x}", 132_u64),
            expected_state_revision: state
                .as_ref()
                .map(|current| current.revision.clone())
                .ok_or("missing final state")?,
            authority_ref: scope.authority_ref.clone(),
        })?)?;
    forged["authority_granted"] = serde_json::Value::Bool(true);
    assert!(decode_event(&serde_json::to_vec(&forged)?).is_err());

    let mut foreign_scope = scope.clone();
    foreign_scope.task_id = TaskId::new("foreign-task")?;
    let final_state = state.as_ref().ok_or("missing final state")?;
    let unauthorized = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round15-foreign-ack")?,
        interface_id: InterfaceId::new("round15-release-api")?,
        scope: foreign_scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-epoch-32".into(),
        interface_revision: DecimalU64::new(32),
        artifact_digest: format!("sha256:{:064x}", 32_u64),
        schema_digest: format!("sha256:{:064x}", 132_u64),
        expected_state_revision: final_state.revision.clone(),
        authority_ref: foreign_scope.authority_ref.clone(),
    };
    assert!(reduce_interface_for(&state, &unauthorized, &scope, &captain, &owner,).is_err());
    Ok(())
}
