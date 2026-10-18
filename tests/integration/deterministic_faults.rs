//! Offline deterministic fixtures exercise actual mailbox/decoder/usage code.
//! They do not qualify a real forge, provider, transport, clock or task acceptance.
use admiralty_core::{
    CoordinationMessage, MessageId, MessagePayload, PrincipalId, content_digest,
    coordination::mailbox::{BoundedMailbox, DeliveryOutcome},
};
use admiralty_runtime::providers::{
    observation::{JsonlDecoder, ObservationError},
    usage::{CounterKind, UsageAccumulator, UsageError, UsageLimits},
};
use admiralty_testkit::{
    clock::FakeClock,
    fake_provider::{FakeMode, FakeProviderSpec},
    faults::{FaultEffect, FaultPlan, FaultRule},
    forge::{Fault, ForgeError, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator},
    task_fixture,
};

fn assignment(id: &str) -> CoordinationMessage {
    let task = task_fixture("fault-task");
    CoordinationMessage {
        schema_version: 1,
        message_id: MessageId::new(id).expect("valid deterministic fixture operation"),
        project_id: task.project_id.clone(),
        sender: PrincipalId::new("commodore").expect("valid deterministic fixture operation"),
        recipient: PrincipalId::new("worker").expect("valid deterministic fixture operation"),
        causation_id: None,
        context_epoch: "synthetic-epoch".into(),
        authority_ref: task.authority_ref.clone(),
        payload: MessagePayload::TaskAssigned {
            task_id: task.task_id.clone(),
            revision: task.revision,
            contract_digest: content_digest(&task).expect("valid deterministic fixture operation"),
        },
    }
}

#[test]
fn scripted_loss_reorder_duplicate_and_delay_preserve_mailbox_evidence() {
    let first = assignment("first");
    let second = assignment("second");
    let mut mailbox = BoundedMailbox::new(first.project_id.clone(), first.recipient.clone(), 2)
        .expect("valid deterministic fixture operation");
    let mut plan = FaultPlan::new(vec![
        FaultRule {
            point: "send-first".into(),
            occurrence: 1,
            effect: FaultEffect::Drop,
        },
        FaultRule {
            point: "send-first".into(),
            occurrence: 2,
            effect: FaultEffect::Duplicate,
        },
        FaultRule {
            point: "retry".into(),
            occurrence: 1,
            effect: FaultEffect::DelayMs(10),
        },
    ])
    .expect("valid deterministic fixture operation");
    let mut clock = FakeClock::new(100);
    // The harness interprets symbolic faults; the plan itself cannot execute them.
    assert_eq!(
        plan.hit("send-first")
            .expect("valid deterministic fixture operation"),
        Some(FaultEffect::Drop)
    );
    assert_eq!(mailbox.retained_len(), 0);
    assert_eq!(
        mailbox.route_sequenced(2, second.clone()),
        DeliveryOutcome::BufferedOutOfOrder {
            seq: 2,
            contiguous_seq: 0
        }
    );
    assert!(mailbox.deliver_contiguous_batch(128).is_empty());
    let Some(FaultEffect::DelayMs(delay)) = plan
        .hit("retry")
        .expect("valid deterministic fixture operation")
    else {
        panic!("delay fixture")
    };
    let deadline = clock
        .deadline_after_ms(delay)
        .expect("valid deterministic fixture operation");
    clock
        .advance_ms(delay - 1)
        .expect("valid deterministic fixture operation");
    assert!(!deadline.is_due(clock.now_ms()));
    assert!(!mailbox.ack_delivery(2), "a gap cannot be locally ACKed");
    clock
        .advance_ms(1)
        .expect("valid deterministic fixture operation");
    assert!(deadline.is_due(clock.now_ms()));
    assert_eq!(
        plan.hit("send-first")
            .expect("valid deterministic fixture operation"),
        Some(FaultEffect::Duplicate)
    );
    assert_eq!(
        mailbox.route_sequenced(1, first.clone()),
        DeliveryOutcome::Enqueued { seq: 1 }
    );
    assert_eq!(
        mailbox.route_sequenced(1, first.clone()),
        DeliveryOutcome::Duplicate { seq: 1 }
    );
    assert_eq!(mailbox.contiguous_seq(), 2);
    assert_eq!(
        mailbox.deliver_contiguous_batch(128),
        vec![(1, first.clone()), (2, second)]
    );
    assert!(mailbox.ack_delivery(1));
    assert!(!mailbox.ack_delivery(1));
    let before = mailbox.clone();
    let mut collision = first.clone();
    collision.context_epoch = "changed-epoch".into();
    assert!(matches!(
        mailbox.route_sequenced(1, collision),
        DeliveryOutcome::MessageIdCollision { .. }
    ));
    assert_eq!(mailbox, before);
    assert_eq!(
        mailbox.route_sequenced(1, first),
        DeliveryOutcome::Duplicate { seq: 1 }
    );
    assert!(matches!(
        mailbox.route_sequenced(3, assignment("third")),
        DeliveryOutcome::RequiresResnapshot { .. }
    ));
    assert_eq!(
        mailbox.retained_len(),
        2,
        "ACK does not erase replay evidence"
    );
    assert!(mailbox.resnapshot_required());
    assert_eq!(
        plan.hits("send-first")
            .expect("valid deterministic fixture operation"),
        2
    );
}

#[test]
fn split_utf8_fixture_remains_provisional_until_complete_jsonl_frame() {
    for seed in [0, 42, u64::MAX] {
        let spec = FakeProviderSpec::new(FakeMode::Split, seed);
        assert_eq!(
            spec.jsonl_bytes(),
            FakeProviderSpec::new(FakeMode::Success, seed).jsonl_bytes()
        );
        let chunks = spec.chunks();
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].last(), Some(&0xc3));
        assert_eq!(chunks[1], vec![0xa9]);
        let mut decoder = JsonlDecoder::new();
        for chunk in &chunks[..3] {
            assert!(
                decoder
                    .push(chunk)
                    .expect("valid deterministic fixture operation")
                    .is_empty()
            );
        }
        let events = decoder
            .push(&chunks[3])
            .expect("valid deterministic fixture operation");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["text"], "hello café");
        assert_eq!(events[0]["simulated"], true);
        decoder
            .finish()
            .expect("valid deterministic fixture operation");
        assert_eq!(decoder.push(b"\n"), Err(ObservationError::Closed));
        let mut truncated = JsonlDecoder::new();
        for chunk in &chunks[..3] {
            truncated
                .push(chunk)
                .expect("valid deterministic fixture operation");
        }
        assert_eq!(truncated.finish(), Err(ObservationError::Framing));
        assert_eq!(truncated.push(b"\n"), Err(ObservationError::Framing));
    }
}

#[test]
fn malformed_fixture_never_recovers_by_appending_valid_output() {
    let mut decoder = JsonlDecoder::new();
    let mut chunks = FakeProviderSpec::new(FakeMode::Malformed, 42)
        .chunks()
        .into_iter();
    // The valid first frame is provisional until the complete capture finishes.
    assert_eq!(
        decoder
            .push(
                &chunks
                    .next()
                    .expect("valid deterministic fixture operation")
            )
            .expect("valid deterministic fixture operation")
            .len(),
        1
    );
    for chunk in chunks.by_ref().take(2) {
        assert!(
            decoder
                .push(&chunk)
                .expect("valid deterministic fixture operation")
                .is_empty()
        );
    }
    assert_eq!(
        decoder.push(
            &chunks
                .next()
                .expect("valid deterministic fixture operation")
        ),
        Err(ObservationError::Json)
    );
    assert_eq!(decoder.finish(), Err(ObservationError::Json));
    assert_eq!(
        decoder.push(&FakeProviderSpec::new(FakeMode::Success, 42).jsonl_bytes()),
        Err(ObservationError::Json)
    );
}

#[test]
fn reset_fixture_preserves_unknown_opening_duplicates_and_model_segments() {
    for seed in [0, 42, u64::MAX] {
        let spec = FakeProviderSpec::new(FakeMode::CumulativeReset, seed);
        assert_eq!(spec.chunks(), spec.chunks());
        let samples = spec.cumulative_samples();
        let mut decoder = JsonlDecoder::new();
        let mut values = Vec::new();
        // One-byte reads also split every multibyte and framing boundary.
        for byte in spec.jsonl_bytes() {
            values.extend(
                decoder
                    .push(&[byte])
                    .expect("valid deterministic fixture operation"),
            );
        }
        decoder
            .finish()
            .expect("valid deterministic fixture operation");
        assert_eq!(values.len(), samples.len());
        for (index, (value, sample)) in values.iter().zip(&samples).enumerate() {
            assert_eq!(value["type"], "cumulative_usage");
            assert_eq!(value["source"], "fake-provider");
            assert_eq!(value["source_sequence"], index + 1);
            assert_eq!(value["source_event_id"], sample.source_event_id);
            assert_eq!(value["session_id"], sample.session_id);
            assert_eq!(value["segment_id"], sample.segment_id);
            assert_eq!(value["model"], sample.model);
            assert_eq!(value["counter_kind"], "input");
            assert_eq!(
                value["cumulative_value"],
                serde_json::to_value(sample.cumulative_value.map(|v| v.to_string()))
                    .expect("valid deterministic fixture operation")
            );
            assert_eq!(
                value["reset_baseline"],
                serde_json::to_value(sample.reset_baseline.map(|v| v.to_string()))
                    .expect("valid deterministic fixture operation")
            );
            assert_eq!(value["simulated"], true);
        }
        let mut usage = UsageAccumulator::new(UsageLimits::new(1, 2, 6))
            .expect("valid deterministic fixture operation");
        let deltas: Vec<_> = samples
            .iter()
            .cloned()
            .map(|s| {
                usage
                    .observe(s)
                    .expect("valid deterministic fixture operation")
            })
            .collect();
        assert_eq!(
            deltas.iter().map(|d| d.delta).collect::<Vec<_>>(),
            vec![None, None, Some(2), None, None, Some(3), Some(2), None]
        );
        assert_eq!(
            deltas.iter().map(|d| d.duplicate).collect::<Vec<_>>(),
            vec![false, false, false, true, false, false, false, true]
        );
        let session = &samples[0].session_id;
        let opening = usage
            .segment(session, "opening")
            .expect("valid deterministic fixture operation");
        let reset = usage
            .segment(session, "reset")
            .expect("valid deterministic fixture operation");
        assert_ne!(opening.model, reset.model);
        assert_eq!(
            opening
                .counter(CounterKind::Input)
                .expect("valid deterministic fixture operation")
                .total,
            None
        );
        assert_eq!(
            opening
                .counter(CounterKind::Input)
                .expect("valid deterministic fixture operation")
                .cumulative_value,
            Some(12 + seed % 7)
        );
        assert_eq!(
            reset
                .counter(CounterKind::Input)
                .expect("valid deterministic fixture operation")
                .total,
            Some(5)
        );
        let mut decreased = samples[6].clone();
        decreased.source_event_id = "decreased".into();
        decreased.cumulative_value = Some(1);
        assert!(matches!(
            usage.observe(decreased),
            Err(UsageError::CounterDecreased { .. })
        ));
        let mut collision = samples[6].clone();
        collision.cumulative_value = Some(6);
        assert_eq!(usage.observe(collision), Err(UsageError::EventIdCollision));
        assert_eq!(usage.segment(session, "opening"), Some(opening));
        assert_eq!(usage.segment(session, "reset"), Some(reset));
        // A known delta sum is not a complete session total or measured savings.
    }
}

fn forge_request() -> ForgeRequest {
    ForgeRequest {
        id: "request-1".into(),
        key: ForgeKey {
            repository: "synthetic-repo".into(),
            target: "main".into(),
        },
        expected_head: "a".repeat(40),
        new_head: "b".repeat(40),
    }
}

#[test]
fn lost_forge_reply_requires_reconciliation_and_never_grants_acceptance() {
    for fault in [
        Fault::LoseReplyBeforeMutation,
        Fault::LoseReplyAfterMutation,
    ] {
        let request = forge_request();
        let mut forge = ForgeSimulator::new(1, 2).expect("valid deterministic fixture operation");
        forge
            .seed_ref(request.key.clone(), request.expected_head.clone())
            .expect("valid deterministic fixture operation");
        let reply = forge
            .submit(request.clone(), fault)
            .expect("valid deterministic fixture operation");
        assert_eq!(reply, ForgeReply::OutcomeUnknown);
        assert!(reply.simulated());
        assert!(!reply.acceptance_granted());
        assert!(!reply.execution_granted());
        let fact = forge
            .reconcile(&request.id)
            .expect("valid deterministic fixture operation")
            .clone();
        let expected = if fault == Fault::LoseReplyBeforeMutation {
            ForgeOutcome::NotApplied
        } else {
            ForgeOutcome::Applied {
                previous_head: request.expected_head.clone(),
                new_head: request.new_head.clone(),
            }
        };
        assert_eq!(fact, expected);
        assert!(!fact.acceptance_granted());
        assert!(!fact.execution_granted());
        let before = forge.clone();
        assert_eq!(
            forge
                .submit(request.clone(), Fault::None)
                .expect("valid deterministic fixture operation"),
            ForgeReply::Observed(fact)
        );
        assert_eq!(
            forge, before,
            "retry returns a historical fact, never repeats mutation"
        );
        let mut changed = request.clone();
        changed.new_head = "c".repeat(40);
        assert_eq!(
            forge.submit(changed, Fault::None),
            Err(ForgeError::RequestCollision)
        );
        assert_eq!(forge, before);
        assert_eq!(
            forge.applied_mutations(),
            u64::from(fault == Fault::LoseReplyAfterMutation)
        );
        assert_eq!(forge.reconcile("unknown-request"), None);
    }
}
