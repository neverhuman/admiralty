//! Offline only. These tests never launch a provider, read credentials or grant acceptance.
use admiralty_runtime::{
    providers::{
        agy, grok,
        observation::{MAX_EVENT_BYTES, MAX_EVENTS, MAX_STREAM_BYTES, ObservationError},
        process,
    },
    session::{
        LaunchSpec, MAX_LAUNCH_ARG_BYTES, MAX_LAUNCH_ARGS, MAX_LAUNCH_ARGV_BYTES,
        validate_launch_envelope,
    },
};
use std::{fs, path::PathBuf};

fn fixture(path: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests")
            .join(path),
    )
    .expect("checked-in repository fixture")
}
fn envelope(program: &str) -> LaunchSpec {
    LaunchSpec {
        run_id: "offline-fixture".into(),
        program: program.into(),
        args: vec![],
        cwd: "/fixture/work".into(),
        spool_dir: "/fixture/spool".into(),
    }
}
fn help(path: &str) -> String {
    let value: serde_json::Value =
        serde_json::from_str(&fixture(path)).expect("capability fixture");
    value["help_evidence"]
        .as_array()
        .expect("help array")
        .iter()
        .map(|v| v.as_str().expect("help string"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn help_canaries_preserve_unsupported_capabilities_and_no_native_qualification() {
    let text = help("fixtures/providers/agy/1.4.2/capabilities.json");
    let cap = agy::qualify("1.4.2", &text).expect("known help");
    assert_eq!(cap.stream_json, agy::Capability::Advertised);
    assert!(!cap.native_execution_qualified());
    for v in [
        cap.auth_status,
        cap.usage,
        cap.resume,
        cap.permission_semantics,
    ] {
        assert!(matches!(v, agy::Capability::Unsupported(_)));
    }
    assert!(agy::qualify("1.4.3", &text).is_err());
    assert!(matches!(
        agy::qualify("1.4.2", "")
            .expect("missing syntax")
            .stream_json,
        agy::Capability::Unsupported(_)
    ));
    assert!(agy::qualify("1.4.2", &"x".repeat(agy::MAX_HELP_BYTES + 1)).is_err());
    let text = help("fixtures/providers/grok/1.0.62/capabilities.json");
    let cap = grok::qualify("grok 1.0.62 (1474f9cbdd91) [stable]", &text).expect("known help");
    assert_eq!(cap.acp_stream, grok::Capability::Advertised);
    assert!(!cap.native_execution_qualified());
    for v in [
        cap.auth_status,
        cap.usage,
        cap.resume,
        cap.permission_semantics,
    ] {
        assert!(matches!(v, grok::Capability::Unsupported(_)));
    }
    assert!(grok::qualify("grok 1.0.63 (next) [stable]", &text).is_err());
    assert!(
        grok::qualify(
            "grok 1.0.62 (1474f9cbdd91) [stable]",
            &"x".repeat(grok::MAX_HELP_BYTES + 1)
        )
        .is_err()
    );
}

#[test]
fn provider_launch_data_is_exact_shell_free_and_explicitly_unadmitted() {
    let prompt = "Implement task; $(touch /not-executed)\nsecond line";
    let a =
        agy::launch(envelope("/opt/agy"), Some("requested-gemini"), prompt).expect("bounded data");
    assert_eq!(
        a.spec().args,
        [
            "--print",
            "--output-format",
            "stream-json",
            "--model",
            "requested-gemini",
            prompt
        ]
    );
    assert_eq!(a.requested_model.as_deref(), Some("requested-gemini"));
    assert_eq!(a.observed_model, None);
    assert_eq!(a.usage, None);
    assert!(!a.native_execution_qualified());
    assert!(!a.parent_acceptance());
    validate_launch_envelope(a.spec()).expect("same host and daemon envelope");
    assert_eq!(a.into_spec().cwd, PathBuf::from("/fixture/work"));
    let g =
        grok::launch(envelope("/opt/grok"), Some("requested-grok"), prompt).expect("bounded data");
    assert_eq!(
        g.spec().args,
        [
            "--single",
            prompt,
            "--output-format",
            "streaming-json",
            "--model",
            "requested-grok"
        ]
    );
    assert_eq!(g.requested_model.as_deref(), Some("requested-grok"));
    assert_eq!(g.observed_model, None);
    assert_eq!(g.usage, None);
    assert!(!g.native_execution_qualified());
    assert!(!g.parent_acceptance());
    validate_launch_envelope(g.spec()).expect("same host and daemon envelope");
    assert_eq!(g.into_spec().spool_dir, PathBuf::from("/fixture/spool"));
}

#[test]
fn provider_builders_reject_option_like_data_and_serialized_prompt_overflow() {
    for p in ["", " ", "--resume", "bad\0prompt"] {
        assert!(agy::launch(envelope("/opt/agy"), None, p).is_err());
        assert!(grok::launch(envelope("/opt/grok"), None, p).is_err());
    }
    for m in ["", " ", "--unsafe-option", "bad\nmodel"] {
        assert!(agy::launch(envelope("/opt/agy"), Some(m), "task").is_err());
        assert!(grok::launch(envelope("/opt/grok"), Some(m), "task").is_err());
    }
    let mut supplied = envelope("/opt/agy");
    supplied.args.push("--unreviewed".into());
    assert!(agy::launch(supplied.clone(), None, "task").is_err());
    assert!(grok::launch(supplied, None, "task").is_err());
    let escaped = "\u{1}".repeat(MAX_LAUNCH_ARG_BYTES);
    assert!(agy::launch(envelope("/opt/agy"), None, &escaped).is_err());
    assert!(grok::launch(envelope("/opt/grok"), None, &escaped).is_err());
    let ordinary = "x".repeat(MAX_LAUNCH_ARG_BYTES);
    assert!(agy::launch(envelope("/opt/agy"), None, &ordinary).is_ok());
    assert!(grok::launch(envelope("/opt/grok"), None, &ordinary).is_ok());
}

#[test]
fn generic_launch_checks_combined_argv_wire_bytes_and_every_path() {
    let mut s = envelope("/fixture/tool");
    s.args = vec!["x".repeat(MAX_LAUNCH_ARG_BYTES); 2];
    let launch = process::Launch::new(s.clone()).expect("128KiB raw argv fits");
    assert!(!launch.native_execution_qualified());
    assert!(!launch.parent_acceptance());
    assert_eq!(launch.spec().args.len(), 2);
    assert_eq!(launch.into_spec().run_id, "offline-fixture");
    s.args.push("x".into());
    assert!(process::Launch::new(s).is_err());
    for args in [
        vec!["x".repeat(MAX_LAUNCH_ARG_BYTES + 1)],
        vec![String::new(); MAX_LAUNCH_ARGS + 1],
        vec!["bad\0arg".into()],
    ] {
        let mut s = envelope("/fixture/tool");
        s.args = args;
        assert!(process::Launch::new(s).is_err());
    }
    let mut s = envelope("/fixture/tool");
    s.args = vec!["\\".repeat(MAX_LAUNCH_ARG_BYTES); 2];
    assert_eq!(
        s.args.iter().map(String::len).sum::<usize>(),
        MAX_LAUNCH_ARGV_BYTES
    );
    assert!(process::Launch::new(s).is_err());
    for path in [
        "relative",
        "/fixture/../other",
        "/fixture/./other",
        "/fixture//other",
        "/fixture/other/",
        "/fixture/bad\npath",
    ] {
        for field in 0..3 {
            let mut s = envelope("/fixture/tool");
            match field {
                0 => s.program = path.into(),
                1 => s.cwd = path.into(),
                _ => s.spool_dir = path.into(),
            }
            assert!(process::Launch::new(s).is_err(), "field {field}: {path:?}");
        }
    }
    let mut s = envelope("/fixture/tool");
    s.run_id = "../foreign".into();
    assert!(process::Launch::new(s).is_err());
}

#[test]
fn legacy_executable_and_requested_model_are_only_hints() {
    for line in fixture("fixtures/providers/process/legacy/argv.jsonl").lines() {
        let row: serde_json::Value = serde_json::from_str(line).expect("argv fixture");
        let argv = row["argv"]
            .as_array()
            .expect("argv array")
            .iter()
            .map(|v| v.as_str().expect("argv string").to_owned())
            .collect::<Vec<_>>();
        let id = process::classify_legacy(&argv, None, None).expect("bounded argv");
        assert_eq!(
            id.executable_hint.as_deref(),
            row["executable_hint"].as_str()
        );
        assert_eq!(
            id.requested_model.as_deref(),
            row["requested_model"].as_str()
        );
        assert_eq!(id.observed_provider, None);
        assert_eq!(id.observed_model, None);
        assert!(id.observed_only);
    }
    for argv in [
        vec![],
        vec!["/usr/bin/other".into()],
        vec!["/opt/agy".into(), "--model".into(), "requested".into()],
    ] {
        let id = process::classify_legacy(
            &argv,
            Some("misleading-grok-session"),
            Some("quoted Grok banner"),
        )
        .expect("screen text ignored");
        assert_eq!(id.observed_provider, None);
        assert_eq!(id.observed_model, None);
        assert_eq!(
            id.executable_hint.as_deref(),
            if argv.first().is_some_and(|v| v == "/opt/agy") {
                Some("agy")
            } else {
                None
            }
        );
    }
    let argv = vec![
        "/opt/agy".into(),
        "--".into(),
        "--model".into(),
        "prompt-data".into(),
    ];
    assert_eq!(
        process::classify_legacy(&argv, None, None)
            .expect("bounded data")
            .requested_model,
        None
    );
    let argv = vec!["/opt/agy".into(), "--model=requested".into()];
    assert_eq!(
        process::classify_legacy(&argv, None, None)
            .expect("bounded flag")
            .requested_model
            .as_deref(),
        Some("requested")
    );
}

#[test]
fn ambiguous_or_oversized_argv_is_not_silently_attributed() {
    for args in [
        vec!["tool", "--model"],
        vec!["tool", "--model="],
        vec!["tool", "-m", "--unsafe"],
        vec!["tool", "-m", "one", "--model", "two"],
        vec!["tool", "--model", "bad\nmodel"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(process::classify_legacy(&args, None, None).is_err());
    }
    assert!(process::classify_legacy(&vec!["x".into(); MAX_LAUNCH_ARGS + 1], None, None).is_err());
    assert!(
        process::classify_legacy(&vec!["x".repeat(MAX_LAUNCH_ARG_BYTES); 3], None, None).is_err()
    );
    assert!(process::classify_legacy(&["bad\0argv".into()], None, None).is_err());
}

#[test]
fn caller_supplied_lifecycle_and_pass_text_never_grant_acceptance() {
    for state in [
        process::Lifecycle::Running,
        process::Lifecycle::Exited(0),
        process::Lifecycle::Exited(1),
        process::Lifecycle::Signaled,
        process::Lifecycle::OutcomeUnknown,
    ] {
        let o = process::observe(state, b"PASS verified accepted", 3).expect("bounded observation");
        assert_eq!(o.lifecycle, state);
        assert!(o.output_truncated);
        assert_eq!(o.provider, None);
        assert_eq!(o.usage, None);
        assert!(!o.native_execution_qualified());
        assert!(!o.parent_acceptance());
    }
    assert!(process::observe(process::Lifecycle::Running, b"", 0).is_err());
    assert!(process::observe(process::Lifecycle::Running, b"", MAX_STREAM_BYTES + 1).is_err());
}

#[test]
fn golden_streams_are_identical_at_every_fragment_boundary() {
    let a = fixture("fixtures/providers/agy/1.4.2/stream.jsonl");
    let g = fixture("fixtures/providers/grok/1.0.62/stream.jsonl");
    let expected_a = agy::parse_stream(a.as_bytes()).expect("synthetic AGY objects");
    let expected_g = grok::parse_stream(g.as_bytes()).expect("synthetic ACP objects");
    assert_eq!(
        expected_a
            .iter()
            .map(|o| o.event_type.as_str())
            .collect::<Vec<_>>(),
        ["assistant", "result"]
    );
    assert_eq!(expected_g[0].method, "session/update");
    assert_eq!(expected_g[0].raw["jsonrpc"], "2.0");
    for split in 0..=a.len() {
        let mut d = agy::StreamDecoder::new();
        let mut out = d.push(&a.as_bytes()[..split]).expect("prefix");
        out.extend(d.push(&a.as_bytes()[split..]).expect("suffix"));
        d.finish().expect("framing complete");
        assert_eq!(out, expected_a);
    }
    for split in 0..=g.len() {
        let mut d = grok::StreamDecoder::new();
        let mut out = d.push(&g.as_bytes()[..split]).expect("prefix");
        out.extend(d.push(&g.as_bytes()[split..]).expect("suffix"));
        d.finish().expect("framing complete");
        assert_eq!(out, expected_g);
    }
}

#[test]
fn duplicate_scalar_invalid_and_unterminated_jsonl_fail_conservatively() {
    for bytes in [
        &b"null\n"[..],
        b"true\n",
        b"42\n",
        b"\"event\"\n",
        b"[]\n",
        b"{}\n",
        b"{\"type\":\"result\",\"type\":\"assistant\"}\n",
        b"{\"type\":\"result\",\"usage\":{\"x\":1,\"x\":2}}\n",
        b"{\"type\":\"result\",\"items\":[{\"x\":1,\"x\":2}]}\n",
        b"{\"type\":\"result\"}",
        b"{\"type\":\"result\"}\n ",
        b"{\"type\":\"result\"} {\"type\":\"result\"}\n",
        b"{\"type\":\"\xff\"}\n",
        b"{broken}\n",
    ] {
        assert!(agy::parse_stream(bytes).is_err(), "AGY accepted {bytes:?}");
        assert!(
            grok::parse_stream(bytes).is_err(),
            "Grok accepted {bytes:?}"
        );
    }
    for bytes in [
        &b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"method\":\"forged\"}\n"[..],
        b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"x\":1,\"x\":2}}\n",
        b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\"}",
        b"{\"jsonrpc\":\"wrong\",\"method\":\"session/update\"}\n",
        b"{\"jsonrpc\":\"2.0\",\"method\":false}\n",
        b"{\"jsonrpc\":\"2.0\",\"method\":\"\"}\n",
        b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":[]}\n",
    ] {
        assert!(grok::parse_stream(bytes).is_err());
    }
}

#[test]
fn semantic_errors_and_partial_frames_stay_failed_after_provisional_events() {
    let mut a = agy::StreamDecoder::new();
    assert_eq!(
        a.push(b"{\"type\":\"assistant\"}\n")
            .expect("provisional event")
            .len(),
        1
    );
    assert!(matches!(
        a.push(b"{\"type\":false}\n"),
        Err(agy::AgyError::Stream(ObservationError::Json))
    ));
    assert!(matches!(
        a.push(b"{\"type\":\"result\"}\n"),
        Err(agy::AgyError::Stream(ObservationError::Json))
    ));
    assert!(matches!(
        a.finish(),
        Err(agy::AgyError::Stream(ObservationError::Json))
    ));
    let mut g = grok::StreamDecoder::new();
    g.push(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\"}\n")
        .expect("provisional event");
    g.push(b"{\"jsonrpc\":\"2.0\"").expect("partial frame");
    assert!(matches!(
        g.finish(),
        Err(grok::GrokError::Stream(ObservationError::Framing))
    ));
    assert!(matches!(
        g.push(b"}\n"),
        Err(grok::GrokError::Stream(ObservationError::Framing))
    ));
    let mut a = agy::StreamDecoder::new();
    a.push(b"{\"type\":\"result\"}\n").expect("object");
    a.finish().expect("close");
    assert!(matches!(
        a.push(b"\n"),
        Err(agy::AgyError::Stream(ObservationError::Closed))
    ));
}

#[test]
fn unicode_crlf_unknown_events_and_provider_pass_remain_raw_observations() {
    let bytes="\r\n{\"type\":\"future_event\",\"text\":\"π🦀\",\"model\":\"forged\",\"accepted\":true}\r\n".as_bytes();
    for split in 0..=bytes.len() {
        let mut d = agy::StreamDecoder::new();
        let mut out = d.push(&bytes[..split]).expect("split UTF8 prefix");
        out.extend(d.push(&bytes[split..]).expect("suffix"));
        d.finish().expect("framing only");
        assert_eq!(out[0].event_type, "future_event");
        assert_eq!(out[0].raw["accepted"], true);
    }
    let out = grok::parse_stream(
        b"{\"jsonrpc\":\"2.0\",\"method\":\"future/event\",\"params\":{\"status\":\"PASS\"}}\r\n",
    )
    .expect("unknown method observation");
    assert_eq!(out[0].method, "future/event");
    assert_eq!(out[0].raw["params"]["status"], "PASS");
}

#[test]
fn shared_line_stream_and_event_caps_apply_to_both_providers() {
    let line = vec![b'x'; MAX_EVENT_BYTES + 1];
    assert!(matches!(
        agy::parse_stream(&line),
        Err(agy::AgyError::Stream(ObservationError::Bounds))
    ));
    assert!(matches!(
        grok::parse_stream(&line),
        Err(grok::GrokError::Stream(ObservationError::Bounds))
    ));
    let stream = vec![b'\n'; MAX_STREAM_BYTES + 1];
    assert!(matches!(
        agy::parse_stream(&stream),
        Err(agy::AgyError::Stream(ObservationError::Bounds))
    ));
    assert!(matches!(
        grok::parse_stream(&stream),
        Err(grok::GrokError::Stream(ObservationError::Bounds))
    ));
    let a = b"{\"type\":\"assistant\"}\n".repeat(MAX_EVENTS + 1);
    let g = b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\"}\n".repeat(MAX_EVENTS + 1);
    assert!(matches!(
        agy::parse_stream(&a),
        Err(agy::AgyError::Stream(ObservationError::Bounds))
    ));
    assert!(matches!(
        grok::parse_stream(&g),
        Err(grok::GrokError::Stream(ObservationError::Bounds))
    ));
}

#[test]
fn generic_process_rejects_an_oversized_argument_without_launching() {
    let oversized_arg = "x".repeat(process::MAX_ARG_BYTES + 1);
    let mut spec = envelope("/usr/bin/tool");
    spec.args = vec![oversized_arg];
    let rejected = process::Launch::new(spec);
    assert!(rejected.is_err());
}
