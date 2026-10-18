//! Offline Round 9 Item 3 cross-provider qualification for P08, P21 and P22.

use admiralty_runtime::providers::{agy, claude, codex, grok, process};
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

#[test]
fn p08_p21_p22_round11_item4_final_validation_gate() {
    // P08: terminal Codex facts are reproducible while trust and native qualification stay false.
    let parse_codex = || {
        let mut parser = codex::CodexNdjsonStream::new("fixture-model", codex::CodexEffort::Medium)
            .expect("Codex fixture model");
        let mut offset = 0_u64;
        let mut observed = Vec::new();
        for bytes in CODEX_TURN.chunks(11) {
            observed.extend(
                parser
                    .push_chunk(&OutputChunk {
                        offset,
                        stream: OutputStream::Stdout,
                        bytes: bytes.to_vec(),
                    })
                    .expect("ordered Codex fragments"),
            );
            offset += bytes.len() as u64;
        }
        let summary = parser.finish(Some(0), true);
        (observed, summary)
    };
    let (codex_observations, codex_summary) = parse_codex();
    let (codex_replay, codex_replay_summary) = parse_codex();
    assert_eq!(codex_observations, codex_replay);
    assert_eq!(codex_summary, codex_replay_summary);
    assert_eq!(
        codex_summary.termination,
        codex::CodexTermination::ProviderTurnCompleted
    );
    assert!(!codex_summary.trusted_acceptance());
    assert!(!codex_summary.native_execution_qualified());

    // P21: Claude's terminal usage is preserved exactly and remains observation only.
    let claude = claude::parse_stream(CLAUDE_TURN, Some("fixture-session"))
        .expect("complete Claude terminal turn");
    assert_eq!(claude.outcome, claude::TurnOutcome::Succeeded);
    assert_eq!(claude.usage.cache_read_input_tokens, Some(649));
    assert_eq!(claude.usage.cache_creation_input_tokens, Some(1991));
    assert_eq!(claude.usage.output_tokens, Some(20));
    assert!(!claude.trusted_acceptance());
    assert!(!claude.native_execution_qualified());

    // P22: matrix evidence advertises protocols but leaves auth, usage and execution unqualified.
    let agy_caps =
        agy::qualify("1.4.2", &help_evidence(AGY_CAPABILITIES)).expect("pinned agy matrix row");
    assert_eq!(agy_caps.stream_json, agy::Capability::Advertised);
    assert!(matches!(
        agy_caps.auth_status,
        agy::Capability::Unsupported(_)
    ));
    assert!(matches!(agy_caps.usage, agy::Capability::Unsupported(_)));
    assert!(!agy_caps.native_execution_qualified());

    let grok_caps = grok::qualify(
        "grok 1.0.62 (1474f9cbdd91) [stable]",
        &help_evidence(GROK_CAPABILITIES),
    )
    .expect("pinned Grok matrix row");
    assert_eq!(grok_caps.acp_stream, grok::Capability::Advertised);
    assert!(matches!(grok_caps.usage, grok::Capability::Unsupported(_)));
    assert!(!grok_caps.native_execution_qualified());

    let legacy = process::classify_legacy(
        &["/opt/agy".into(), "--model".into(), "fixture-model".into()],
        Some("recycled-session-name"),
        Some("quoted provider transcript"),
    )
    .expect("bounded legacy process hints");
    assert_eq!(legacy.executable_hint.as_deref(), Some("agy"));
    assert_eq!(legacy.requested_model.as_deref(), Some("fixture-model"));
    assert!(legacy.observed_only);
    assert_eq!(legacy.observed_provider, None);
    assert_eq!(legacy.observed_model, None);
}

#[test]
fn p08_p21_p22_round12_item5_redteam_audit_gate() {
    // P08: a complete provider turn remains observation evidence, not an acceptance.
    let mut parser = codex::CodexNdjsonStream::new("fixture-model", codex::CodexEffort::Medium)
        .expect("Codex fixture model");
    let mut offset = 0_u64;
    let mut events = Vec::new();
    for bytes in CODEX_TURN.chunks(13) {
        events.extend(
            parser
                .push_chunk(&OutputChunk {
                    offset,
                    stream: OutputStream::Stdout,
                    bytes: bytes.to_vec(),
                })
                .expect("ordered Codex fixture chunks"),
        );
        offset += bytes.len() as u64;
    }
    let summary = parser.finish(Some(0), true);
    assert!(events.iter().any(|event| event.usage.is_some()));
    assert_eq!(
        summary.termination,
        codex::CodexTermination::ProviderTurnCompleted
    );
    assert!(!summary.trusted_acceptance());
    assert!(!summary.native_execution_qualified());

    // P21: Claude usage categories are retained independently; parsing is not trust.
    let claude = claude::parse_stream(CLAUDE_TURN, Some("fixture-session"))
        .expect("complete Claude fixture");
    assert_eq!(claude.usage.input_tokens, Some(2));
    assert_eq!(claude.usage.cache_read_input_tokens, Some(649));
    assert_eq!(claude.usage.cache_creation_input_tokens, Some(1991));
    assert_eq!(claude.usage.output_tokens, Some(20));
    assert!(!claude.trusted_acceptance());
    assert!(!claude.native_execution_qualified());

    // P22: advertised protocols cannot be promoted to qualified provider behavior.
    let agy =
        agy::qualify("1.4.2", &help_evidence(AGY_CAPABILITIES)).expect("pinned agy fixture row");
    assert_eq!(agy.stream_json, agy::Capability::Advertised);
    assert!(matches!(agy.auth_status, agy::Capability::Unsupported(_)));
    assert!(matches!(agy.usage, agy::Capability::Unsupported(_)));
    assert!(!agy.native_execution_qualified());
    let grok = grok::qualify(
        "grok 1.0.62 (1474f9cbdd91) [stable]",
        &help_evidence(GROK_CAPABILITIES),
    )
    .expect("pinned Grok fixture row");
    assert_eq!(grok.acp_stream, grok::Capability::Advertised);
    assert!(matches!(grok.usage, grok::Capability::Unsupported(_)));
    assert!(!grok.native_execution_qualified());

    // Red-team the release boundary: a merged commit or imported status is only a hint.
    // These checked-in work-order requirements keep delivery evidence stage-specific.
    let work_order = include_str!("../../docs/plans/admiralty-v2/WORK_ORDER_REVIEW.md");
    let plan = include_str!("../../docs/plans/admiralty-v2/MASTER_PLAN.md");
    assert!(work_order.contains("Tracking trailers"));
    assert!(work_order.contains("never acceptance, model identity or release evidence"));
    assert!(plan.contains("Never promote an imported `done`, a tracking trailer, merged PR or available package into deployed/released status."));
}
