//! Offline Round 9 Item 3 cross-provider qualification for P08, P21 and P22.

use admiralty_runtime::providers::{agy, claude, codex, grok};
use admiralty_runtime::session::{OutputChunk, OutputStream};
use serde_json::Value;

const CODEX_TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
const CLAUDE_TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");
const AGY_CAPABILITIES: &str = include_str!("../fixtures/providers/agy/1.4.2/capabilities.json");
const GROK_CAPABILITIES: &str = include_str!("../fixtures/providers/grok/1.0.62/capabilities.json");

fn help_evidence(fixture: &str) -> String {
    let value: Value = serde_json::from_str(fixture).expect("checked-in provider fixture");
    value["help_evidence"]
        .as_array()
        .expect("help evidence array")
        .iter()
        .map(|entry| entry.as_str().expect("help evidence string"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn p08_p21_p22_round9_item3_claude_provider_matrix_qualification() {
    // P08: fragmented Codex events retain usage facts but never confer acceptance.
    let mut codex = codex::CodexNdjsonStream::new("fixture-model", codex::CodexEffort::Medium)
        .expect("Codex fixture model");
    let mut offset = 0_u64;
    let mut codex_usage = None;
    for chunk in CODEX_TURN.chunks(7) {
        let observations = codex
            .push_chunk(&OutputChunk {
                offset,
                stream: OutputStream::Stdout,
                bytes: chunk.to_vec(),
            })
            .expect("contiguous Codex fixture fragment");
        offset += chunk.len() as u64;
        codex_usage = observations
            .into_iter()
            .find_map(|event| event.usage)
            .or(codex_usage);
    }
    let codex_usage = codex_usage.expect("Codex fixture includes usage");
    assert_eq!(codex_usage.input_tokens.as_deref(), Some("100"));
    assert_eq!(codex_usage.cached_input_tokens.as_deref(), Some("90"));
    assert_eq!(codex_usage.output_tokens.as_deref(), Some("3"));
    let codex_summary = codex.finish(Some(0), true);
    assert!(!codex_summary.trusted_acceptance());
    assert!(!codex_summary.native_execution_qualified());

    // P21: the retained Claude turn reports native cache categories without trust.
    let claude = claude::parse_stream(CLAUDE_TURN, Some("fixture-session"))
        .expect("complete Claude fixture");
    assert_eq!(claude.usage.input_tokens, Some(2));
    assert_eq!(claude.usage.cache_read_input_tokens, Some(649));
    assert_eq!(claude.usage.cache_creation_input_tokens, Some(1991));
    assert_eq!(claude.usage.output_tokens, Some(20));
    assert!(!claude.trusted_acceptance());
    assert!(!claude.native_execution_qualified());

    // P22: the provider matrix distinguishes advertised syntax from qualified behavior.
    let agy_caps = agy::qualify("1.4.2", &help_evidence(AGY_CAPABILITIES))
        .expect("known agy capability fixture");
    assert_eq!(agy_caps.stream_json, agy::Capability::Advertised);
    assert!(!agy_caps.native_execution_qualified());
    let grok_caps = grok::qualify(
        "grok 1.0.62 (1474f9cbdd91) [stable]",
        &help_evidence(GROK_CAPABILITIES),
    )
    .expect("known grok capability fixture");
    assert_eq!(grok_caps.acp_stream, grok::Capability::Advertised);
    assert!(!grok_caps.native_execution_qualified());
    assert!(agy::qualify("1.4.3", &help_evidence(AGY_CAPABILITIES)).is_err());
    assert!(
        grok::qualify(
            "grok 1.0.63 (next) [stable]",
            &help_evidence(GROK_CAPABILITIES)
        )
        .is_err()
    );
}
