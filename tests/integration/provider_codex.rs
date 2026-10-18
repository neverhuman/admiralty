use admiralty_runtime::{
    providers::codex::{CodexEffort, CodexError, CodexNdjsonStream, CodexTermination},
    session::{OutputChunk, OutputStream},
};

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
