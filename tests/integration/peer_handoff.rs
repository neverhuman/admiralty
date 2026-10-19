use admiralty_core::interfaces::{
    CompatibilityEvidence, DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId,
    InterfaceScope, RecipientEpoch, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_testkit::faults::{FaultEffect, FaultPlan, FaultRule};

fn principal(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("fixture principal")
}

fn scope() -> InterfaceScope {
    InterfaceScope {
        project_id: ProjectId::new("project").expect("fixture project"),
        root_id: RootId::new("root").expect("fixture root"),
        task_id: TaskId::new("producer-task").expect("fixture task"),
        authority_ref: "grant-1".into(),
    }
}

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn publish(event_id: &str, revision: u64, expected_state_revision: Option<u64>) -> InterfaceEvent {
    let consumer = principal("consumer");
    InterfaceEvent::Publish {
        event_id: MessageId::new(event_id).expect("fixture event"),
        contract: InterfaceContract {
            interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
            scope: scope(),
            producer: principal("producer"),
            consumers: vec![consumer.clone()],
            revision: DecimalU64::new(revision),
            artifact_digest: digest(if revision == 1 { 'a' } else { 'c' }),
            schema_digest: digest(if revision == 1 { 'b' } else { 'd' }),
        },
        actor: principal("producer"),
        authority_owner: principal("owner"),
        recipient_epochs: vec![RecipientEpoch {
            recipient: consumer,
            epoch: if revision == 1 { "epoch-1" } else { "epoch-2" }.into(),
        }],
        expected_state_revision: expected_state_revision.map(DecimalU64::new),
    }
}

fn acknowledge() -> InterfaceEvent {
    InterfaceEvent::Acknowledge {
        event_id: MessageId::new("ack-1").expect("fixture event"),
        interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
        scope: scope(),
        actor: principal("consumer"),
        recipient_epoch: "epoch-1".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: digest('a'),
        schema_digest: digest('b'),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: "grant-1".into(),
    }
}

#[test]
fn consumer_ack_is_provisional_and_new_revision_proposes_invalidation() {
    let scope = scope();
    let owner = principal("owner");
    let producer = principal("producer");
    let consumer = principal("consumer");
    let initial = reduce_interface_for(
        &None,
        &publish("publish-1", 1, None),
        &scope,
        &producer,
        &owner,
    )
    .expect("publish interface contract");
    let ack_event = acknowledge();
    let mut corrupted_digest = ack_event.clone();
    if let InterfaceEvent::Acknowledge { schema_digest, .. } = &mut corrupted_digest {
        *schema_digest = "sha256:not-a-digest".into();
    }
    assert!(
        reduce_interface_for(
            &Some(initial.state.clone()),
            &corrupted_digest,
            &scope,
            &consumer,
            &owner,
        )
        .is_err(),
        "cross-boundary acknowledgment with a corrupted schema digest must be rejected"
    );

    let mut foreign_scope = scope.clone();
    foreign_scope.authority_ref = "other-grant".into();
    assert!(
        reduce_interface_for(
            &Some(initial.state.clone()),
            &ack_event,
            &foreign_scope,
            &consumer,
            &owner,
        )
        .is_err(),
        "an acknowledgement bound to a foreign scope must be rejected"
    );
    assert!(
        reduce_interface_for(
            &Some(initial.state.clone()),
            &ack_event,
            &scope,
            &principal("unauthenticated-peer"),
            &owner,
        )
        .is_err(),
        "an acknowledgement from an unauthenticated peer must be rejected"
    );

    let acknowledged =
        reduce_interface_for(&Some(initial.state), &ack_event, &scope, &consumer, &owner)
            .expect("consumer acknowledgment");
    assert!(matches!(
        acknowledged.state.evidence.get(&consumer),
        Some(CompatibilityEvidence::ProvisionalAck { .. })
    ));
    assert!(acknowledged.invalidations.is_empty());

    let replay = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &ack_event,
        &scope,
        &consumer,
        &owner,
    )
    .expect("idempotent acknowledgment replay");
    assert!(replay.replayed);
    assert_eq!(replay.state, acknowledged.state);

    let mut changed_replay = ack_event.clone();
    if let InterfaceEvent::Acknowledge {
        artifact_digest, ..
    } = &mut changed_replay
    {
        *artifact_digest = digest('c');
    }
    let collision = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &changed_replay,
        &scope,
        &consumer,
        &owner,
    )
    .expect_err("a valid changed digest must reach immutable event collision detection");
    assert_eq!(
        collision.0, "event ID reused with different content",
        "reusing an event ID must fail for a collision, not malformed digest syntax"
    );
    assert_eq!(
        acknowledged.state.evidence.get(&consumer),
        Some(&CompatibilityEvidence::ProvisionalAck {
            artifact_digest: digest('a'),
            schema_digest: digest('b'),
        }),
        "rejected replay collision must leave accepted evidence unchanged"
    );

    let revised = reduce_interface_for(
        &Some(acknowledged.state),
        &publish("publish-2", 2, Some(2)),
        &scope,
        &producer,
        &owner,
    )
    .expect("publish revised contract");
    assert_eq!(revised.invalidations.len(), 1);
    assert_eq!(revised.invalidations[0].consumer, consumer);
    assert_eq!(
        revised.invalidations[0].previous_revision,
        DecimalU64::new(1)
    );
    assert_eq!(revised.invalidations[0].new_revision, DecimalU64::new(2));
}

#[test]
fn injected_ack_drop_preserves_unacknowledged_state_until_retry() {
    let scope = scope();
    let owner = principal("owner");
    let producer = principal("producer");
    let consumer = principal("consumer");
    let published = reduce_interface_for(
        &None,
        &publish("fault-publish", 1, None),
        &scope,
        &producer,
        &owner,
    )
    .expect("publish before injected delivery fault");
    let initial_state = published.state;
    let acknowledgement = acknowledge();
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "peer-ack-delivery".into(),
        occurrence: 1,
        effect: FaultEffect::Drop,
    }])
    .expect("bounded deterministic fault schedule");

    assert_eq!(
        faults.hit("peer-ack-delivery"),
        Ok(Some(FaultEffect::Drop)),
        "first acknowledgment delivery is deterministically dropped"
    );
    assert!(!initial_state.evidence.contains_key(&consumer));

    assert_eq!(faults.hit("peer-ack-delivery"), Ok(None));
    let retried = reduce_interface_for(
        &Some(initial_state),
        &acknowledgement,
        &scope,
        &consumer,
        &owner,
    )
    .expect("retry after dropped delivery");
    assert!(matches!(
        retried.state.evidence.get(&consumer),
        Some(CompatibilityEvidence::ProvisionalAck { .. })
    ));
    assert_eq!(faults.hits("peer-ack-delivery"), Ok(2));
}

#[tokio::test]
async fn mcp_and_cli_use_authenticated_durable_peer_inbox_without_manager_relay()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use admiralty_core::{
        CoordinationMessage, DomainEvent, MessagePayload, SourceIdentity, content_digest,
    };
    use admiralty_runtime::agent_tools::{AgentClient, McpSession, ToolCall};
    use admiralty_server::services::{
        agent_routes::{self, AgentRouteState},
        coordination::{
            CoordinationAction, CoordinationCredential, CoordinationCredentialAuthority,
            CoordinationCredentialGrant,
        },
    };
    use admiralty_store::{Store, inbox::InboxScope};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::{RwLock, oneshot};

    #[cfg(unix)]
    let temporary = {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?
    };
    #[cfg(not(unix))]
    let temporary = tempfile::tempdir()?;
    let store = Store::open(temporary.path().join("peers.sqlite"))?;
    let mut task = admiralty_testkit::task_fixture("producer-task");
    task.project_id = scope().project_id;
    task.root_id = scope().root_id;
    let inbox_scope = InboxScope {
        project_id: task.project_id.clone(),
        recipient: principal("consumer"),
        context_epoch: "epoch-1".into(),
    };
    let message_id = MessageId::new("producer-schema-ready")?;
    for (sequence, event) in [
        DomainEvent::TaskSubmitted {
            contract: task.clone(),
        },
        DomainEvent::MessageQueued {
            message: CoordinationMessage {
                schema_version: 1,
                message_id: message_id.clone(),
                project_id: task.project_id.clone(),
                sender: principal("producer"),
                recipient: principal("consumer"),
                causation_id: None,
                context_epoch: "epoch-1".into(),
                authority_ref: task.authority_ref.clone(),
                payload: MessagePayload::ArtifactReady {
                    task_id: task.task_id.clone(),
                    artifact_ref: digest('b'),
                    interface_revision: "1".into(),
                },
            },
        },
    ]
    .into_iter()
    .enumerate()
    {
        store.append(
            SourceIdentity {
                producer_id: "peer-seed".into(),
                incarnation: "test".into(),
                sequence: (sequence + 1).to_string(),
            },
            event,
            "2026-10-19T14:00:00Z".into(),
        )?;
    }
    let ack_source = SourceIdentity {
        producer_id: "consumer".into(),
        incarnation: "peer-test".into(),
        sequence: "1".into(),
    };
    let token = "peer-consumer-token-with-sufficient-entropy".to_owned();
    let authority = CoordinationCredentialAuthority::new(vec![CoordinationCredential::configured(
        "peer-consumer".into(),
        token.clone(),
        principal("consumer"),
        vec![
            CoordinationCredentialGrant {
                scope: inbox_scope.clone(),
                action: CoordinationAction::ReadPending,
                message_id: None,
                source: None,
            },
            CoordinationCredentialGrant {
                scope: inbox_scope,
                action: CoordinationAction::AcknowledgeApplication,
                message_id: Some(message_id.clone()),
                source: Some(ack_source),
            },
        ],
    )])
    .expect("valid scoped peer credentials");
    let credentials = Arc::new(RwLock::new(authority));
    let router = axum::Router::new().nest(
        "/v2/agents/inbox",
        agent_routes::router(AgentRouteState::new(store.clone(), credentials.clone())),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let cursor_before = store.snapshot()?.cursor;
    let expected_digest = content_digest(&task)?;
    tokio::task::spawn_blocking(move || -> Result<(),Box<dyn std::error::Error + Send + Sync>> {
        let client = AgentClient::new(address,token)?;
        let mut call = ToolCall { name: "task_context_get".into(), arguments: json!({"project_id":"project","task_id":"producer-task","context_epoch":"epoch-1"}) };
        let context = client.call(&call)?;
        assert_eq!(context["contract_digest"],expected_digest);
        assert_eq!(context["artifact_refs"],json!([digest('b')]));
        assert!(context["capsule"].is_object());
        call.arguments["known_contract_digest"] = context["contract_digest"].clone();
        let mut mcp = McpSession::default();
        mcp.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"peer-fixture","version":"1"}}}), |_| unreachable!());
        mcp.handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), |_| unreachable!());
        let compact = mcp.handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":call}), |call| client.call(call)).expect("MCP result");
        assert_eq!(compact["result"]["isError"],false);
        assert!(compact["result"]["structuredContent"]["capsule"].is_null());
        assert_eq!(compact["result"]["structuredContent"]["contract_digest"],expected_digest);
        assert_eq!(compact["result"]["structuredContent"]["artifact_refs"],json!([digest('b')]));
        assert_eq!(store.snapshot()?.cursor,cursor_before,"context retrieval is read-only");
        call.arguments["context_epoch"] = json!("stale-epoch");
        assert!(client.call(&call).is_err(),"compacted or foreign epochs need fresh grants");
        let ack = ToolCall { name: "inbox_application_ack".into(), arguments: json!({"project_id":"project","recipient":"consumer","context_epoch":"epoch-1","message_id":"producer-schema-ready","producer_id":"consumer","incarnation":"peer-test","sequence":"1","observed_utc":"2026-10-19T14:00:01Z"}) };
        client.call(&ack)?;
        let after_ack = store.snapshot()?.cursor;
        client.call(&ack)?;
        assert_eq!(store.snapshot()?.cursor,after_ack,"exact ACK retry cannot duplicate a durable receipt");
        let pending = client.call(&ToolCall { name: "inbox_pending".into(), arguments: json!({"project_id":"project","recipient":"consumer","context_epoch":"epoch-1"}) })?;
        assert_eq!(pending["messages"],json!([]));
        Ok(())
    }).await??;
    credentials
        .write()
        .await
        .revoke("peer-consumer")
        .expect("revoke test-owned credential");
    let _ = stop.send(());
    server.await??;
    Ok(())
}
