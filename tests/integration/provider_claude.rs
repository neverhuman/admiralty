//! Offline, synthetic counterexamples plus retained imported fixture vectors.
use admiralty_runtime::{
    providers::claude::{
        CURRENT_CLI_VERSION, ClaudeError, ClaudeLaunch, Effort, EventScope, MAX_EVENT_BYTES,
        MAX_EVENTS, MAX_STREAM_BYTES, OBSERVED_CLI_VERSION, TurnOutcome, Usage, UsageSource,
        parse_stream, prepare_launch, qualification,
    },
    session::{LaunchSpec, MAX_INPUT_BYTES},
};
use admiralty_testkit::faults::{FaultEffect, FaultPlan, FaultRule};
use serde_json::{Value, json};
use std::path::PathBuf;

const TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");
const RESUME: &[u8] = include_bytes!("../fixtures/providers/claude/turn-boundary-resume.jsonl");
const UUID: &str = "aee5e41e-8913-4c03-acef-e4fb73978921";

#[test]
fn real_303_continuity_preserves_session_model_and_terminal_usage() {
    let fresh = include_bytes!("../fixtures/providers/claude/2.1.303/fresh.jsonl");
    let resumed = include_bytes!("../fixtures/providers/claude/2.1.303/resume.jsonl");
    let first = parse_stream(fresh, None).expect("retained bounded real first turn");
    let second = parse_stream(resumed, first.session_id.as_deref()).expect("same real session");
    for out in [&first, &second] {
        assert_eq!(out.version.as_deref(), Some(CURRENT_CLI_VERSION));
        assert_eq!(out.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(out.outcome, TurnOutcome::Succeeded);
        assert_eq!(out.usage_source, UsageSource::ParentTerminal);
        assert!(out.usage.cache_read_input_tokens.is_some());
        assert!(!out.native_execution_qualified());
        assert!(!out.trusted_acceptance());
    }
    let result: Value = serde_json::from_slice(
        resumed
            .split(|b| *b == b'\n')
            .rfind(|l| !l.is_empty())
            .expect("terminal result"),
    )
    .expect("JSON result");
    assert_eq!(result["result"], "ADM-P21-20261019");
}

#[test]
fn complete_launch_binding_rejects_capture_and_envelope_drift() {
    let mut profile = launch();
    profile.model = "claude-sonnet-5-5".into();
    let envelope = LaunchSpec {
        run_id: "bound-claude".into(),
        program: profile.program.clone(),
        args: vec![],
        cwd: profile.cwd.clone(),
        spool_dir: "/workspace/spool".into(),
    };
    let prepared = prepare_launch(
        profile.clone(),
        envelope.clone(),
        "Exact task\ncontext",
        OBSERVED_CLI_VERSION,
    )
    .expect("bound proposal");
    let mut captured = events();
    captured[0]["tools"] = json!([]);
    captured[0]["mcp_servers"] = json!([]);
    assert!(prepared.parse_capture(&wire(&captured)).is_ok());
    assert!(
        prepared.parse_capture(TURN).is_err(),
        "missing tool configuration is unknown"
    );
    for field in ["tools", "mcp_servers"] {
        let mut changed = captured.clone();
        changed[0][field] = json!(["unexpected-tool"]);
        assert!(prepared.parse_capture(&wire(&changed)).is_err());
    }
    assert!(prepared.spec().args.contains(&"--verbose".into()));
    assert!(prepared.spec().args.contains(&"--safe-mode".into()));
    assert!(
        prepared
            .spec()
            .args
            .windows(2)
            .any(|pair| pair == ["--mcp-config", "{\"mcpServers\":{}}"])
    );
    assert_eq!(
        serde_json::from_slice::<Value>(prepared.input()).expect("input")["message"]["content"],
        "Exact task\ncontext"
    );
    let changed = prepare_launch(
        profile.clone(),
        envelope.clone(),
        "Changed task",
        OBSERVED_CLI_VERSION,
    )
    .expect("new binding");
    assert_ne!(prepared.binding_digest(), changed.binding_digest());
    for field in [
        "session_id",
        "model",
        "permissionMode",
        "claude_code_version",
    ] {
        let mut source = events();
        source[0][field] = json!(if field == "claude_code_version" {
            CURRENT_CLI_VERSION
        } else {
            "foreign"
        });
        assert!(prepared.parse_capture(&wire(&source)).is_err(), "{field}");
    }
    for field in ["program", "cwd", "args"] {
        let mut bad = envelope.clone();
        match field {
            "program" => bad.program = "/other/claude".into(),
            "cwd" => bad.cwd = "/other/work".into(),
            _ => bad.args.push("--unbound".into()),
        }
        assert!(prepare_launch(profile.clone(), bad, "Task", OBSERVED_CLI_VERSION).is_err());
    }
    profile.model = "sonnet".into();
    assert!(prepare_launch(profile, envelope, "Task", OBSERVED_CLI_VERSION).is_err());
}

fn events() -> Vec<Value> {
    TURN.split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("trusted checked-in synthetic event"))
        .collect()
}
fn wire(events: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for event in events {
        bytes.extend(serde_json::to_vec(event).expect("trusted synthetic event encoding"));
        bytes.push(b'\n');
    }
    bytes
}
fn launch() -> ClaudeLaunch {
    ClaudeLaunch {
        program: PathBuf::from("/usr/local/bin/claude"),
        cwd: PathBuf::from("/workspace/project"),
        model: "sonnet".into(),
        effort: Effort::Low,
        max_budget_usd: "0.1".into(),
        permission_mode: "dontAsk".into(),
        session_id: Some("fixture-session".into()),
        resume: true,
    }
}

#[test]
fn retained_complete_turn_preserves_terminal_native_categories_without_double_count() {
    let out = parse_stream(TURN, Some("fixture-session")).expect("valid complete checked-in turn");
    assert_eq!(out.session_id.as_deref(), Some("fixture-session"));
    assert_eq!(out.version.as_deref(), Some(OBSERVED_CLI_VERSION));
    assert_eq!(out.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(out.usage.cache_read_input_tokens, Some(649));
    assert_eq!(out.usage.cache_creation_input_tokens, Some(1991));
    assert_eq!(out.usage.output_tokens, Some(20));
    assert_eq!(out.usage.thinking_tokens, Some(0));
    assert_eq!(out.total_cost_usd, Some(0.0082329));
    assert_eq!(out.outcome, TurnOutcome::Succeeded);
    assert_eq!(out.usage_source, UsageSource::ParentTerminal);
    assert!(out.permission_denials_known);
    assert!(!out.native_execution_qualified());
    assert!(!out.trusted_acceptance());
}

#[test]
fn retained_resume_vector_preserves_missing_error_flag_as_unknown() {
    let out = parse_stream(RESUME, Some("fixture-session"))
        .expect("complete retained resume observation");
    assert_eq!(out.result_subtype.as_deref(), Some("success"));
    assert_eq!(out.outcome, TurnOutcome::Unknown);
    assert_eq!(out.is_error, None);
    assert_eq!(out.usage.output_tokens, Some(22));
    assert_eq!(out.usage.thinking_tokens, None);
    assert!(!out.trusted_acceptance());
    assert_eq!(
        parse_stream(RESUME, Some("foreign")),
        Err(ClaudeError::ResumeMismatch)
    );
}

#[test]
fn retained_incomplete_vectors_fail_instead_of_granting_authority() {
    for bytes in [
        &include_bytes!("../fixtures/providers/claude/approval-denial.jsonl")[..],
        &include_bytes!("../fixtures/providers/claude/negative-invalid.jsonl")[..],
    ] {
        assert!(parse_stream(bytes, None).is_err());
    }
}

#[test]
fn exactly_one_parent_init_and_result_are_required_in_order() {
    let source = events();
    for bad in [
        vec![],
        vec![source[0].clone()],
        vec![source[1].clone(), source[2].clone()],
        vec![source[2].clone()],
        vec![source[0].clone(), source[0].clone(), source[2].clone()],
        vec![source[0].clone(), source[2].clone(), source[2].clone()],
        vec![source[0].clone(), source[2].clone(), source[1].clone()],
    ] {
        assert!(parse_stream(&wire(&bad), None).is_err());
    }
    let mut child_init = source.clone();
    child_init[0]["parent_tool_use_id"] = json!("opaque-child");
    assert!(parse_stream(&wire(&child_init), None).is_err());
    let mut unknown_first = source;
    unknown_first.insert(0, json!({"type":"unknown"}));
    assert!(parse_stream(&wire(&unknown_first), None).is_err());
}

#[test]
fn complete_lf_framing_accepts_crlf_but_never_partial_eof_or_extra_terminal() {
    assert_eq!(
        parse_stream(&TURN[..TURN.len() - 1], None),
        Err(ClaudeError::Framing)
    );
    let crlf = String::from_utf8(TURN.to_vec())
        .expect("fixture UTF-8")
        .replace('\n', "\r\n");
    assert_eq!(
        parse_stream(crlf.as_bytes(), None)
            .expect("complete CRLF turn")
            .usage
            .output_tokens,
        Some(20)
    );
    let mut partial = TURN.to_vec();
    partial.extend_from_slice(b" ");
    assert_eq!(parse_stream(&partial, None), Err(ClaudeError::Framing));
    let mut following = events();
    following.push(json!({"type":"system","subtype":"init","session_id":"other"}));
    assert!(parse_stream(&wire(&following), None).is_err());
}

#[test]
fn required_init_fields_and_supported_dialect_are_checked() {
    for key in [
        "session_id",
        "model",
        "permissionMode",
        "claude_code_version",
    ] {
        for replacement in [None, Some(json!(null)), Some(json!("")), Some(json!(false))] {
            let mut source = events();
            if let Some(value) = replacement {
                source[0][key] = value;
            } else {
                source[0].as_object_mut().expect("init object").remove(key);
            }
            assert!(parse_stream(&wire(&source), None).is_err(), "field {key}");
        }
    }
    let mut changed = events();
    changed[0]["claude_code_version"] = json!("99.0.0");
    assert_eq!(
        parse_stream(&wire(&changed), None),
        Err(ClaudeError::VersionMismatch)
    );
}

#[test]
fn every_supplied_session_is_bound_and_terminal_identity_is_mandatory() {
    for index in [0, 1, 2] {
        let mut source = events();
        source[index]["session_id"] = json!("foreign");
        assert_eq!(
            parse_stream(&wire(&source), Some("fixture-session")),
            Err(ClaudeError::ResumeMismatch)
        );
    }
    let mut source = events();
    source[2]
        .as_object_mut()
        .expect("terminal object")
        .remove("session_id");
    assert!(parse_stream(&wire(&source), None).is_err());
    for expected in ["", "space id", "bad\nid"] {
        assert!(parse_stream(TURN, Some(expected)).is_err());
    }
}

#[test]
fn recursive_duplicate_fields_scalar_events_and_invalid_utf8_are_rejected() {
    assert_eq!(
        parse_stream(
            include_bytes!("../fixtures/providers/claude/synthetic-duplicate-usage.jsonl"),
            None
        ),
        Err(ClaudeError::Json)
    );
    for raw in [
        &b"{\"type\":\"result\",\"is_error\":true,\"is_error\":false}\n"[..],
        b"{\"type\":\"result\",\"permission_denials\":[{}],\"permission_denials\":[]}\n",
        b"{\"type\":\"assistant\",\"message\":{\"usage\":{\"output_tokens\":100,\"output_tokens\":1}}}\n",
        b"{\"type\":\"x\",\"array\":[{\"a\":1,\"a\":2}]}\n",
        b"false\n", b"null\n", b"[]\n", b"1\n", b"{} {}\n", b"{\"type\":\"\xff\"}\n",
    ] { assert_eq!(parse_stream(raw, None), Err(ClaudeError::Json)); }
}

#[test]
fn malformed_terminal_usage_never_falls_back_to_valid_assistant_usage() {
    for bad in [
        json!(null),
        json!(false),
        json!(123),
        json!("usage"),
        json!([]),
        json!({"input_tokens":100,"output_tokens":-1}),
        json!({"input_tokens":"2"}),
        json!({"cache_read_input_tokens":null}),
        json!({"cache_creation_input_tokens":true}),
        json!({"output_tokens":1.5}),
        json!({"output_tokens_details":null}),
        json!({"output_tokens_details":{"thinking_tokens":-1}}),
        json!({"output_tokens_details":{"thinking_tokens":"1"}}),
    ] {
        let mut source = events();
        source[2]["usage"] = bad;
        assert!(parse_stream(&wire(&source), None).is_err());
    }
    let mut bytes = wire(&events()[..2]);
    bytes.extend_from_slice(b"{\"type\":\"result\",\"session_id\":\"fixture-session\",\"subtype\":\"success\",\"is_error\":false,\"usage\":{\"output_tokens\":18446744073709551616}}\n");
    assert!(parse_stream(&bytes, None).is_err());
}

#[test]
fn malformed_parent_or_opaque_child_assistant_usage_is_not_ignored() {
    for child in [false, true] {
        let mut source = events();
        source[1]["message"]["usage"] = json!({"input_tokens":"invalid"});
        if child {
            source[1]["parent_tool_use_id"] = json!("opaque-child");
        }
        assert!(parse_stream(&wire(&source), None).is_err());
    }
    for malformed in [json!(null), json!(1), json!(false)] {
        let mut source = events();
        source[1]["message"] = malformed;
        assert!(parse_stream(&wire(&source), None).is_err());
    }
}

#[test]
fn missing_parent_usage_preserves_unknown_and_child_usage_has_separate_scope() {
    let out = parse_stream(
        include_bytes!("../fixtures/providers/claude/synthetic-opaque-child-no-parent-usage.jsonl"),
        None,
    )
    .expect("synthetic scope-separated complete turn");
    assert!(out.opaque_subagents);
    assert_eq!(out.usage, Usage::default());
    assert_eq!(out.usage_source, UsageSource::Unknown);
    assert_eq!(out.events.len(), 5);
    assert_eq!(out.events[1].scope, EventScope::Parent);
    assert_eq!(
        out.events[1]
            .usage
            .as_ref()
            .expect("observed parent message usage")
            .input_tokens,
        Some(100)
    );
    for index in [2, 3] {
        assert_eq!(out.events[index].scope, EventScope::OpaqueChild);
        assert_eq!(
            out.events[index]
                .usage
                .as_ref()
                .expect("observed child usage")
                .input_tokens,
            Some(1)
        );
    }
    assert_eq!(out.total_cost_usd, None);
    assert!(!out.permission_denials_known);
    assert!(!out.trusted_acceptance());
}

#[test]
fn partial_terminal_categories_and_maximum_native_counter_remain_exact() {
    let mut source = events();
    source[2]["usage"] = json!({"input_tokens":u64::MAX});
    let out = parse_stream(&wire(&source), None).expect("valid partial native usage");
    assert_eq!(out.usage.input_tokens, Some(u64::MAX));
    assert_eq!(out.usage.output_tokens, None);
    assert_eq!(out.usage.cache_read_input_tokens, None);
    assert_eq!(out.usage.thinking_tokens, None);
    assert_eq!(out.usage_source, UsageSource::ParentTerminal);
}

#[test]
fn opaque_child_result_does_not_complete_or_replace_parent_result() {
    let mut source = events();
    source[2]["parent_tool_use_id"] = json!("opaque-child");
    assert!(parse_stream(&wire(&source), None).is_err());
    let mut parent = events().remove(2);
    parent["usage"] = json!({"input_tokens":200,"output_tokens":30});
    source.push(parent);
    let out = parse_stream(&wire(&source), None).expect("child result followed by parent result");
    assert_eq!(out.usage.input_tokens, Some(200));
    assert_eq!(out.usage.output_tokens, Some(30));
    assert_eq!(out.events[2].scope, EventScope::OpaqueChild);
    assert_eq!(out.outcome, TurnOutcome::Succeeded);
    assert_eq!(
        out.events
            .iter()
            .filter(|event| event.event_type == "result" && event.scope == EventScope::Parent)
            .count(),
        1
    );
    source[2]["session_id"] = json!("foreign");
    assert_eq!(
        parse_stream(&wire(&source), None),
        Err(ClaudeError::ResumeMismatch)
    );
}

#[test]
fn terminal_failure_denials_unknown_and_conflicts_remain_distinct() {
    let failed = parse_stream(
        include_bytes!("../fixtures/providers/claude/synthetic-failed-turn.jsonl"),
        None,
    )
    .expect("explicit synthetic provider failure");
    assert_eq!(failed.outcome, TurnOutcome::Failed);
    assert_eq!(failed.is_error, Some(true));
    let denial = parse_stream(
        include_bytes!("../fixtures/providers/claude/synthetic-bound-approval-denial.jsonl"),
        None,
    )
    .expect("session-bound synthetic denial observation");
    assert_eq!(denial.permission_denials, 1);
    assert!(!denial.trusted_acceptance());
    let mut source = events();
    source[2]["subtype"] = json!("future_terminal");
    let unknown = parse_stream(&wire(&source), None).expect("unknown outcome retained");
    assert_eq!(unknown.outcome, TurnOutcome::Unknown);
    assert!(unknown.protocol_unknown);
    for (subtype, flag) in [("success", true), ("error_max_turns", false)] {
        source[2]["subtype"] = json!(subtype);
        source[2]["is_error"] = json!(flag);
        assert!(parse_stream(&wire(&source), None).is_err());
    }
}

#[test]
fn malformed_error_cost_and_permission_denials_cannot_disappear() {
    for (key, bad) in [
        ("is_error", json!(null)),
        ("is_error", json!("false")),
        ("permission_denials", json!(null)),
        ("permission_denials", json!(true)),
        ("permission_denials", json!([false])),
        ("total_cost_usd", json!(-1)),
        ("total_cost_usd", json!("0")),
        ("total_cost_usd", json!(null)),
    ] {
        let mut source = events();
        source[2][key] = bad;
        assert!(parse_stream(&wire(&source), None).is_err(), "field {key}");
    }
}

#[test]
fn unknown_events_preserve_order_and_never_become_success_or_authority() {
    let mut source = events();
    source.insert(
        1,
        json!({"type":"TaskAccepted","session_id":"fixture-session","accepted":true}),
    );
    let bytes = wire(&source);
    let out = parse_stream(&bytes, None).expect("unknown protocol record remains an observation");
    assert!(out.protocol_unknown);
    assert_eq!(out.outcome, TurnOutcome::Unknown);
    assert_eq!(out.events[1].event_type, "TaskAccepted");
    for (index, event) in out.events.iter().enumerate() {
        assert_eq!(event.ordinal, index + 1);
        assert!(event.event_digest.starts_with("sha256:"));
    }
    assert_eq!(
        out,
        parse_stream(&bytes, None).expect("deterministic observation replay")
    );
    assert!(!out.trusted_acceptance());
    assert!(!out.native_execution_qualified());
}

#[test]
fn line_stream_event_and_metadata_limits_fail_closed() {
    assert_eq!(
        parse_stream(&vec![b' '; MAX_STREAM_BYTES + 1], None),
        Err(ClaudeError::Bounds)
    );
    let huge = format!(
        "{{\"type\":\"x\",\"body\":\"{}\"}}\n",
        "x".repeat(MAX_EVENT_BYTES)
    );
    assert_eq!(
        parse_stream(huge.as_bytes(), None),
        Err(ClaudeError::Bounds)
    );
    let mut source = events();
    let mut boundary_event = json!({"type":"unknown","body":""});
    let encoded_empty = serde_json::to_vec(&boundary_event).expect("empty boundary event");
    let padding = MAX_EVENT_BYTES - encoded_empty.len();
    boundary_event["body"] = json!("x".repeat(padding));
    source.insert(1, boundary_event);
    let boundary_stream = wire(&source);
    assert_eq!(
        boundary_stream
            .split(|byte| *byte == b'\n')
            .nth(1)
            .expect("boundary event is the second JSONL frame")
            .len(),
        MAX_EVENT_BYTES
    );
    assert!(parse_stream(&boundary_stream, None).is_ok());
    assert_eq!(
        parse_stream(&b"{}\n".repeat(MAX_EVENTS + 1), None),
        Err(ClaudeError::Bounds)
    );
    for (index, key, size) in [
        (0, "session_id", 129),
        (0, "model", 257),
        (0, "permissionMode", 65),
        (1, "type", 129),
    ] {
        let mut source = events();
        source[index][key] = json!("x".repeat(size));
        assert!(parse_stream(&wire(&source), None).is_err());
    }
}

#[test]
fn input_frame_round_trips_multiline_unicode_as_one_bounded_record() {
    let prompt = "Goal: preserve NOT.\nScope: src/task.rs\r\nCheck:\tcargo test 🦀";
    let bytes = ClaudeLaunch::input_frame(prompt).expect("supported exact multiline capsule");
    assert!(bytes.len() <= MAX_INPUT_BYTES);
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
    let decoded: Value = serde_json::from_slice(&bytes).expect("one JSONL user record");
    assert_eq!(decoded["message"]["content"], prompt);
    assert_eq!(decoded["message"]["role"], "user");
    for prompt in [
        "",
        " \r\n\t",
        "bad\0prompt",
        "bad\u{7f}prompt",
        "bad\u{1b}prompt",
    ] {
        assert!(ClaudeLaunch::input_frame(prompt).is_err());
    }
}

#[test]
fn input_budget_includes_json_escaping_and_final_lf_without_truncation() {
    let overhead = ClaudeLaunch::input_frame("x")
        .expect("one-byte prompt")
        .len()
        - 1;
    let exact = "x".repeat(MAX_INPUT_BYTES - overhead);
    assert_eq!(
        ClaudeLaunch::input_frame(&exact)
            .expect("exact encoded limit")
            .len(),
        MAX_INPUT_BYTES
    );
    assert_eq!(
        ClaudeLaunch::input_frame(&(exact + "x")),
        Err(ClaudeError::Bounds)
    );
    assert_eq!(
        ClaudeLaunch::input_frame(&"\"".repeat(MAX_INPUT_BYTES / 2)),
        Err(ClaudeError::Bounds)
    );
    assert_eq!(
        ClaudeLaunch::input_frame(&"x".repeat(MAX_INPUT_BYTES + 1)),
        Err(ClaudeError::Bounds)
    );
}

#[test]
fn argv_is_unadmitted_print_only_with_captured_permission_subset_and_default_omission() {
    for mode in ["acceptEdits", "dontAsk", "manual", "plan", "default"] {
        let mut value = launch();
        value.permission_mode = mode.into();
        let args = value.args().expect("supported unadmitted proposal");
        assert!(args.contains(&"--print".into()));
        assert!(args.contains(&"--restricted".into()));
        assert!(args.windows(2).any(|pair| pair == ["--model", "sonnet"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--resume", "fixture-session"])
        );
        assert_eq!(
            args.iter().any(|arg| arg == "--permission-mode"),
            mode != "default"
        );
        if mode != "default" {
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--permission-mode", mode])
            );
        }
        assert!(!value.is_admitted());
    }
    for mode in ["bypassPermissions", "auto", "invented"] {
        let mut value = launch();
        value.permission_mode = mode.into();
        assert!(value.args().is_err());
    }
    let help = include_str!("../fixtures/providers/claude/claude-2.1.302-help.txt");
    assert!(help.contains("--print"));
    assert!(help.contains("refuses bypassPermissions"));
}

#[test]
fn fresh_explicit_uuid_and_automatic_fresh_session_are_reachable_without_resume() {
    let mut value = launch();
    value.resume = false;
    value.session_id = Some(UUID.into());
    let args = value.args().expect("fresh UUID proposal");
    assert!(args.windows(2).any(|pair| pair == ["--session-id", UUID]));
    assert!(!args.contains(&"--resume".into()));
    value.session_id = None;
    assert!(
        !value
            .args()
            .expect("automatic fresh session proposal")
            .contains(&"--session-id".into())
    );
    value.resume = true;
    assert!(value.args().is_err());
    value.resume = false;
    for id in [
        "fixture-session",
        "bad uuid",
        "aee5e41e89134c03acefe4fb73978921",
    ] {
        value.session_id = Some(id.into());
        assert!(value.args().is_err());
    }
}

#[test]
fn lexical_launch_bounds_and_untrusted_flags_never_grant_admission() {
    let mut option_like = launch();
    option_like.model = "--help".into();
    assert!(option_like.args().is_err());
    option_like = launch();
    option_like.session_id = Some("--help".into());
    assert!(option_like.args().is_err());
    let mut value = launch();
    value.cwd = PathBuf::from("/workspace/../escape");
    assert!(value.args().is_err());
    value = launch();
    value.program = PathBuf::from(format!("/{}", "x".repeat(4096)));
    assert!(value.args().is_err());
    for budget in ["NaN", "inf", "0", "-1", "0.10", "1e100"] {
        value = launch();
        value.max_budget_usd = budget.into();
        assert!(value.args().is_err());
    }
    let mut forged = serde_json::to_value(launch()).expect("proposal JSON");
    forged["native_execution_qualified"] = json!(true);
    assert!(serde_json::from_value::<ClaudeLaunch>(forged).is_err());
    let capabilities = qualification();
    for key in [
        "structured_stream",
        "launch_input_resume",
        "configuration_bound",
        "runtime_activation",
        "native_execution_qualified",
        "trusted_acceptance",
        "parent_child_usage_deduplicated",
    ] {
        assert_eq!(capabilities[key], false, "unsupported claim {key}");
    }
    assert_eq!(capabilities["admission"], "unadmitted_offline_proposal");
    assert_eq!(capabilities["context_floor"], "unknown");
    let floor: Value = serde_json::from_str(include_str!(
        "../fixtures/providers/claude/context-floor.json"
    ))
    .expect("retained context metadata fixture");
    assert!(
        floor["context_floor"]
            .as_str()
            .expect("qualification caveat")
            .starts_with("unknown")
    );
}

#[test]
fn oversized_claude_capture_is_rejected_before_parsing() {
    let oversized = vec![b' '; MAX_STREAM_BYTES + 1];
    assert_eq!(parse_stream(&oversized, None), Err(ClaudeError::Bounds));
    assert!(ClaudeLaunch::input_frame("embedded\0nul").is_err());
}

#[test]
fn replaying_a_complete_claude_capture_preserves_events_and_usage() {
    let first = parse_stream(TURN, Some("fixture-session")).expect("complete fixture turn");
    let replay = parse_stream(TURN, Some("fixture-session")).expect("replayed fixture turn");
    assert_eq!(replay.events, first.events);
    assert_eq!(replay.usage, first.usage);
    assert!(!replay.trusted_acceptance());
}

#[test]
fn injected_loss_of_terminal_claude_event_cannot_complete_the_turn() {
    let records = TURN
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    assert!(records.len() >= 2);
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "claude-event".into(),
        occurrence: records.len() as u64,
        effect: FaultEffect::Drop,
    }])
    .expect("bounded deterministic fault schedule");
    let mut observed = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let effect = faults.hit("claude-event").expect("registered fault point");
        if index + 1 == records.len() {
            assert_eq!(effect, Some(FaultEffect::Drop));
        } else {
            assert_eq!(effect, None);
            observed.extend_from_slice(record);
        }
    }
    assert!(parse_stream(&observed, None).is_err());
}
