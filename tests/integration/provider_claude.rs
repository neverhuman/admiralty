use admiralty_runtime::providers::claude::{
    ClaudeError, ClaudeLaunch, Effort, parse_stream, qualification,
};
use std::path::PathBuf;

const TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");

#[test]
fn parses_structured_usage_without_parent_message_double_count() {
    let o = parse_stream(TURN, None).expect("valid fixture or launch");
    assert_eq!(o.session_id.as_deref(), Some("fixture-session"));
    assert_eq!(o.version.as_deref(), Some("2.1.302"));
    assert_eq!(o.usage.cache_read_input_tokens, Some(649));
    assert_eq!(o.usage.cache_creation_input_tokens, Some(1991));
    assert_eq!(o.usage.output_tokens, Some(20));
    assert_eq!(o.total_cost_usd, Some(0.0082329));
}

#[test]
fn rejects_malformed_usage_and_wrong_resume_identity() {
    let bad = include_bytes!("../fixtures/providers/claude/negative-invalid.jsonl");
    assert_eq!(
        parse_stream(bad, None),
        Err(ClaudeError::Invalid("stream has no usage-bearing event"))
    );
    assert_eq!(
        parse_stream(TURN, Some("different")),
        Err(ClaudeError::ResumeMismatch)
    );
    assert_eq!(
        parse_stream(TURN, Some("fixture-session"))
            .expect("valid fixture or launch")
            .result_subtype
            .as_deref(),
        Some("success")
    );
}

#[test]
fn records_approval_denial_and_opaque_native_children() {
    let mut input = include_bytes!("../fixtures/providers/claude/approval-denial.jsonl").to_vec();
    let text = String::from_utf8(input.clone()).expect("valid fixture or launch");
    assert_eq!(
        parse_stream(text.as_bytes(), None)
            .expect("valid fixture or launch")
            .permission_denials,
        1
    );
    input.extend_from_slice(b"\n{\"type\":\"assistant\",\"parent_tool_use_id\":\"native-agent\",\"message\":{\"usage\":{\"input_tokens\":1}}}");
    assert!(
        parse_stream(&input, None)
            .expect("valid fixture or launch")
            .opaque_subagents
    );
}

#[test]
fn launch_is_bound_and_shell_free_and_capability_claims_are_limited() {
    let launch = ClaudeLaunch {
        program: PathBuf::from("/usr/local/bin/claude"),
        cwd: PathBuf::from("/workspace/project"),
        model: "sonnet".into(),
        effort: Effort::Low,
        max_budget_usd: "0.1".into(),
        permission_mode: "dontAsk".into(),
        session_id: Some("fixture-session".into()),
        resume: true,
    };
    let args = launch.args().expect("valid fixture or launch");
    assert!(
        args.windows(2)
            .any(|p| p == ["--resume", "fixture-session"])
    );
    assert!(args.windows(2).any(|p| p == ["--model", "sonnet"]));
    assert_eq!(
        qualification()["subagents"],
        "opaque_native_not_managed_peers"
    );
    assert_eq!(qualification()["coordination_tools"], "unqualified");
    let mut invalid = launch;
    invalid.cwd = PathBuf::from("/workspace/../escape");
    assert!(invalid.validate().is_err());
}
