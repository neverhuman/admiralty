use admiralty_runtime::providers::{agy, grok, process};
use std::{fs, path::PathBuf};

fn fixture(path: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests")
            .join(path),
    )
    .expect("fixture readable")
}

#[test]
fn agy_142_capability_canary_and_stream_bounds() {
    let cap: serde_json::Value =
        serde_json::from_str(&fixture("fixtures/providers/agy/1.4.2/capabilities.json"))
            .expect("valid test value");
    let ev = cap["help_evidence"]
        .as_array()
        .expect("valid test value")
        .iter()
        .map(|v| v.as_str().expect("valid test value"))
        .collect::<Vec<_>>()
        .join(" ");
    let qualified = agy::qualify("1.4.2", &ev).expect("valid test value");
    assert_eq!(qualified.stream_json, agy::Capability::Advertised);
    assert!(matches!(
        qualified.auth_status,
        agy::Capability::Unsupported(_)
    ));
    assert!(matches!(qualified.usage, agy::Capability::Unsupported(_)));
    assert!(matches!(qualified.resume, agy::Capability::Unsupported(_)));
    assert!(matches!(
        qualified.permission_semantics,
        agy::Capability::Unsupported(_)
    ));
    assert!(agy::qualify("1.4.3", &ev).is_err());
    let stream = fixture("fixtures/providers/agy/1.4.2/stream.jsonl");
    let events = agy::parse_stream(stream.as_bytes()).expect("valid test value");
    assert_eq!(
        events
            .iter()
            .map(|e| e.event_type.as_deref())
            .collect::<Vec<_>>(),
        [Some("assistant"), Some("result")]
    );
    assert!(agy::parse_stream(b"{broken}\n").is_err());
    let launch = agy::launch(PathBuf::from("/opt/agy"), Some("gemini-2.5-pro"), "hello")
        .expect("valid test value");
    assert!(
        launch
            .args
            .windows(2)
            .any(|w| w == ["--output-format", "stream-json"])
    );
    assert!(!launch.args.iter().any(|arg| arg == "--input-format"));
    assert_eq!(launch.usage, None);
}

#[test]
fn grok_1062_acp_canary_keeps_provider_protocol_separate() {
    let cap: serde_json::Value =
        serde_json::from_str(&fixture("fixtures/providers/grok/1.0.62/capabilities.json"))
            .expect("valid test value");
    let ev = cap["help_evidence"]
        .as_array()
        .expect("valid test value")
        .iter()
        .map(|v| v.as_str().expect("valid test value"))
        .collect::<Vec<_>>()
        .join(" ");
    let qualified =
        grok::qualify("grok 1.0.62 (1474f9cbdd91) [stable]", &ev).expect("valid test value");
    assert_eq!(qualified.acp_stream, grok::Capability::Advertised);
    assert!(matches!(
        qualified.auth_status,
        grok::Capability::Unsupported(_)
    ));
    assert!(matches!(qualified.usage, grok::Capability::Unsupported(_)));
    assert!(matches!(qualified.resume, grok::Capability::Unsupported(_)));
    assert!(matches!(
        qualified.permission_semantics,
        grok::Capability::Unsupported(_)
    ));
    assert!(grok::qualify("grok 1.0.63 (next) [stable]", &ev).is_err());
    let stream = fixture("fixtures/providers/grok/1.0.62/stream.jsonl");
    let events = grok::parse_stream(stream.as_bytes()).expect("valid test value");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].method.as_deref(), Some("session/update"));
    assert_eq!(events[0].raw["jsonrpc"], "2.0");
    let launch = grok::launch(PathBuf::from("/opt/grok"), Some("grok-3"), "hello")
        .expect("valid test value");
    assert!(
        launch
            .args
            .windows(2)
            .any(|w| w == ["--output-format", "streaming-json"])
    );
    assert_eq!(launch.usage, None);
}

#[test]
fn generic_process_and_legacy_identity_stay_conservative() {
    let launch = process::Launch::new(
        PathBuf::from("/usr/bin/tool"),
        vec!["--model".into(), "x".into()],
        PathBuf::from("/work"),
    )
    .expect("valid test value");
    assert_eq!(launch.args[0], "--model");
    assert!(process::Launch::new(PathBuf::from("tool"), vec![], PathBuf::from("/work")).is_err());
    let obs =
        process::observe(process::Lifecycle::Exited(0), b"result", 3).expect("valid test value");
    assert!(obs.output_truncated);
    assert_eq!(obs.usage, None);
    assert_eq!(obs.provider, None);
    for lifecycle in [
        process::Lifecycle::Running,
        process::Lifecycle::Signaled,
        process::Lifecycle::OutcomeUnknown,
    ] {
        assert_eq!(
            process::observe(lifecycle, b"", 1)
                .expect("valid test value")
                .lifecycle,
            lifecycle
        );
    }

    for line in fixture("fixtures/providers/process/legacy/argv.jsonl").lines() {
        let row: serde_json::Value = serde_json::from_str(line).expect("valid test value");
        let argv = row["argv"]
            .as_array()
            .expect("valid test value")
            .iter()
            .map(|s| s.as_str().expect("valid test value").to_string())
            .collect::<Vec<_>>();
        let id = process::classify_legacy(&argv, None, None);
        assert_eq!(id.provider.as_deref(), row["expected_provider"].as_str());
        assert_eq!(id.model.as_deref(), row["expected_model"].as_str());
        assert!(id.observed_only);
    }
    let grok_screen = process::classify_legacy(&[], Some("old-grok-session"), Some("Grok 3"));
    assert_eq!(grok_screen.provider.as_deref(), Some("grok"));
    assert!(grok_screen.observed_only);
}
