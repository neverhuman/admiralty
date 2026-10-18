use admiralty_runtime::{
    providers::codex::{
        CodexEffort, CodexError, CodexNdjsonStream, CodexTermination, MAX_STREAM_BYTES,
    },
    session::{OutputChunk, OutputStream},
};
use admiralty_testkit::faults::{FaultEffect, FaultPlan, FaultRule};

const TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");

#[test]
fn fragmented_codex_turn_preserves_usage_and_never_claims_acceptance() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Medium)
        .expect("explicit model accepted");
    let mut observations = Vec::new();
    let mut offset = 0_u64;

    for bytes in TURN.chunks(7) {
        observations.extend(
            parser
                .push_chunk(&OutputChunk {
                    offset,
                    stream: OutputStream::Stdout,
                    bytes: bytes.to_vec(),
                })
                .expect("fragment is accepted into the contiguous journal"),
        );
        offset += bytes.len() as u64;
    }

    let completed = observations
        .iter()
        .find(|observation| observation.event_type == "turn.completed")
        .expect("completion event observed");
    let usage = completed
        .usage
        .as_ref()
        .expect("native turn usage observed");
    assert_eq!(usage.input_tokens.as_deref(), Some("100"));
    assert_eq!(usage.cached_input_tokens.as_deref(), Some("90"));
    assert_eq!(usage.output_tokens.as_deref(), Some("3"));
    assert!(observations.iter().all(|event| !event.trusted_acceptance()));

    let summary = parser.finish(Some(0), true);
    assert_eq!(summary.termination, CodexTermination::ProviderTurnCompleted);
    assert_eq!(summary.thread_id.as_deref(), Some("fixture-thread"));
    assert_eq!(summary.raw_bytes, TURN.len().to_string());
    assert!(!summary.trusted_acceptance());
    assert!(!summary.native_execution_qualified());
}

#[test]
fn incomplete_codex_capture_stays_unknown() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{\"type\":\"thread.started\",\"thread_id\":\"fixture-thread\"}\n".to_vec(),
        })
        .expect("start event parses");

    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.capture_complete);
    assert!(!summary.trusted_acceptance());
}

#[test]
fn journal_gap_poisoning_requires_reconciliation() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let gap = parser.push_chunk(&OutputChunk {
        offset: 1,
        stream: OutputStream::Stdout,
        bytes: b"{}\n".to_vec(),
    });
    assert!(matches!(gap, Err(CodexError::JournalGap)));

    let retry = parser.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{}\n".to_vec(),
    });
    assert!(matches!(retry, Err(CodexError::Failed)));
}

#[test]
fn malformed_codex_json_yields_no_observation_and_poisoning_is_sticky() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let malformed = parser.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{broken}\n".to_vec(),
    });
    assert!(matches!(malformed, Err(CodexError::Json(_))));
    assert!(parser.observations().is_empty());
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(CodexError::Failed)
    ));
}

#[test]
fn codex_stream_limit_rejects_first_byte_over_boundary() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let result = parser.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: vec![b' '; MAX_STREAM_BYTES + 1],
    });
    assert!(matches!(result, Err(CodexError::Bounds)));
    assert!(parser.observations().is_empty());
}

#[test]
fn replaying_a_codex_journal_chunk_cannot_duplicate_observations() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let chunk = OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{\"type\":\"thread.started\",\"thread_id\":\"fixture-thread\"}\n".to_vec(),
    };
    let first = parser.push_chunk(&chunk).expect("initial journal chunk");
    assert_eq!(first.len(), 1);
    assert!(matches!(
        parser.push_chunk(&chunk),
        Err(CodexError::JournalGap)
    ));
    assert_eq!(parser.observations().len(), 1);
}

#[test]
fn invalid_utf8_in_a_fragmented_codex_event_is_rejected_without_observation() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let corrupted = b"{\"type\":\"thread.started\",\"thread_id\":\"\xc3(\"}\n";
    let split = corrupted
        .windows(2)
        .position(|pair| pair == b"\xc3(")
        .expect("corrupt UTF-8 sequence");
    assert!(
        parser
            .push_chunk(&OutputChunk {
                offset: 0,
                stream: OutputStream::Stdout,
                bytes: corrupted[..split + 1].to_vec(),
            })
            .is_ok()
    );
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: (split + 1) as u64,
            stream: OutputStream::Stdout,
            bytes: corrupted[split + 1..].to_vec(),
        }),
        Err(CodexError::Json(_))
    ));
    assert!(parser.observations().is_empty());
}

#[test]
fn completed_provider_turn_stays_unknown_without_complete_capture() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN.to_vec(),
        })
        .expect("provider turn parses");
    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.capture_complete);
    assert!(!summary.trusted_acceptance());
}

#[test]
fn injected_codex_tail_loss_keeps_capture_unqualified() {
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "codex-read".into(),
        occurrence: 2,
        effect: FaultEffect::Drop,
    }])
    .expect("bounded deterministic fault schedule");
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let split = TURN.len() / 2;
    assert_eq!(faults.hit("codex-read"), Ok(None));
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN[..split].to_vec(),
        })
        .expect("first provider read");
    assert_eq!(faults.hit("codex-read"), Ok(Some(FaultEffect::Drop)));

    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.capture_complete);
    assert!(!summary.trusted_acceptance());
}

#[test]
fn p24_p29_round8_p08_telegram_worktree_qualification() {
    // The deterministic Codex transcript may report a completed turn, but the
    // worktree qualification remains false until the capture is complete and
    // independently accepted by the surrounding workflow.
    let mut parser = CodexNdjsonStream::new("round8-fixture", CodexEffort::Medium)
        .expect("explicit fixture model accepted");
    let observations = parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN.to_vec(),
        })
        .expect("fixture transcript parses deterministically");

    assert!(
        observations
            .iter()
            .any(|event| event.event_type == "turn.completed")
    );
    assert!(observations.iter().all(|event| !event.trusted_acceptance()));

    let incomplete = parser.finish(Some(0), false);
    assert_eq!(incomplete.termination, CodexTermination::Unknown);
    assert!(!incomplete.capture_complete);
    assert!(!incomplete.trusted_acceptance());
    assert!(!incomplete.native_execution_qualified());
}
