//! Usage-side parity: the synthetic token-accounting legacy cases (sources in
//! the retired `dashboard/token_yield.py` and `dashboard/extras.py`) and the
//! retired `token_yield.py` SQL-metacharacter hazard test.
use super::*;

fn number(value: &Value) -> u64 {
    value.as_u64().expect("nonnegative fixture integer")
}
fn sample(at: i64, model: &str, input: u64, output: u64) -> Sample {
    Sample {
        at,
        model: model.into(),
        tokens: Tokens {
            input,
            cached: Some(0),
            output,
        },
    }
}
fn session(id: &str, cwd: &str, samples: Vec<Sample>) -> Session {
    Session {
        id: id.into(),
        family: "codex".into(),
        cwd: cwd.into(),
        samples,
        observed_usage: true,
        ..Session::default()
    }
}
fn token_event(at: i64, input: u64, output: u64) -> Value {
    json!({"type":"event_msg","timestamp":iso(at),"payload":{"type":"token_count",
        "info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":output}}}})
}
fn jsonl(lines: &[Value]) -> String {
    lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
fn observed_config(root: &Path) -> ObserveConfig {
    serde_json::from_value(json!({"schema_version":1,"node_id":"observed-node","output_dir":root}))
        .expect("observer configuration fixture")
}

/// Synthetic legacy case `unknown_price_zero_fill`: an unpriced model leaves
/// the total valuation unknown instead of adding zero, while the priced
/// subtotal and the unpriced tokens stay visible separately.
#[tokio::test]
async fn legacy_case_unknown_price_zero_fill() {
    let case = crate::legacy_cases::case("unknown_price_zero_fill");
    let now = 10_000;
    let mut rates = BTreeMap::new();
    let mut samples = Vec::new();
    for (index, segment) in case["input"]["segments"]
        .as_array()
        .expect("segments")
        .iter()
        .enumerate()
    {
        let model = segment["model"].as_str().expect("model");
        let tokens = number(&segment["tokens"]);
        samples.push(sample(now - 60 + index as i64, model, tokens, 0));
        if let Some(credits) = segment["credits_est"].as_f64() {
            let rate = credits * 1_000_000.0 / tokens as f64;
            rates.insert(
                model.to_owned(),
                Rate {
                    input: rate,
                    cached: rate,
                    output: rate,
                },
            );
        }
    }
    let observed = session("priced-and-unpriced", "/outside", samples);
    let window = &windows(std::slice::from_ref(&observed), &rates, now)["15"];
    let expected = &case["expected"];
    assert_eq!(window["credits_est"], expected["total_credits_est"]);
    assert_eq!(window["pricing_complete"], false);
    assert_eq!(
        window["priced_credits_est"].as_f64(),
        expected["known_credits_est"].as_f64()
    );
    let by_model = window["by_model"].as_object().expect("by model");
    let priced = by_model
        .values()
        .filter(|row| !row["credits"].is_null())
        .map(|row| number(&row["input"]))
        .sum::<u64>();
    let unpriced = by_model
        .values()
        .filter(|row| row["unpriced"] == true)
        .map(|row| number(&row["lower_bound_input"]))
        .sum::<u64>();
    assert_eq!(priced, number(&expected["priced_tokens"]));
    assert_eq!(unpriced, number(&expected["unpriced_tokens"]));
    let coverage = 100.0 * priced as f64 / (priced + unpriced) as f64;
    assert!((coverage - expected["priced_coverage_pct"].as_f64().expect("coverage")).abs() < 1e-10);
    let config = observed_config(Path::new("/owned"));
    let yielded = yield_metrics(&config, &json!({}), &[observed], &rates, now).await;
    assert!(yielded["total"]["credits"].is_null());
    assert_eq!(yielded["total"]["tokens"], priced + unpriced);
}

/// Synthetic legacy case `mixed_model_attribution`: cumulative counters keep
/// each segment with the model that produced it; the latest model never
/// absorbs earlier consumption.
#[test]
fn legacy_case_mixed_model_attribution() {
    let case = crate::legacy_cases::case("mixed_model_attribution");
    let now = 10_000;
    let mut lines = vec![json!({"type":"session_meta","payload":{"id":"mixed","cwd":"/outside"}})];
    let (mut input, mut output) = (0, 0);
    let segments = case["input"]["segments"].as_array().expect("segments");
    for (index, segment) in segments.iter().enumerate() {
        lines.push(json!({"type":"turn_context","payload":{"model":segment["model"]}}));
        input += number(&segment["input"]);
        output += number(&segment["output"]);
        lines.push(token_event(now - 120 + 60 * index as i64, input, output));
    }
    let observed = scan_codex("", &jsonl(&lines), false);
    let window = &windows(&[observed], &BTreeMap::new(), now)["15"];
    let expected = &case["expected"];
    let by_model = window["by_model"].as_object().expect("by model");
    assert_eq!(by_model.len(), segments.len());
    for (model, usage) in expected["by_model"].as_object().expect("expected models") {
        assert_eq!(by_model[model]["input"], usage["input"], "{model}");
        assert_eq!(by_model[model]["output"], usage["output"], "{model}");
    }
    assert_eq!(window["input_tokens"], expected["total_input"]);
    assert_eq!(window["output_tokens"], expected["total_output"]);
    assert_eq!(expected["assign_all_to_latest_model"], false);
}

/// Synthetic legacy case `lifetime_window_leakage`: a session active inside a
/// window contributes only its in-window delta, never lifetime counters.
/// Adapted: the Rust window is the fixed 15 minutes ending at
/// `window_end_epoch`; the opening sample precedes both window starts.
#[test]
fn legacy_case_lifetime_window_leakage() {
    let case = crate::legacy_cases::case("lifetime_window_leakage");
    let input = &case["input"];
    let now = number(&input["window_end_epoch"]) as i64;
    let samples = input["samples"].as_array().expect("samples");
    let mut lines = vec![
        json!({"type":"session_meta","payload":{"id":"lifetime","cwd":"/outside"}}),
        json!({"type":"turn_context","payload":{"model":"synthetic-model"}}),
    ];
    for item in samples {
        lines.push(token_event(
            number(&item["epoch"]) as i64,
            number(&item["input"]),
            number(&item["output"]),
        ));
    }
    let opening = number(&samples[0]["epoch"]) as i64;
    assert!(opening <= now - 900 && opening <= number(&input["window_start_epoch"]) as i64);
    assert!(number(&samples[1]["epoch"]) as i64 > now - 900);
    let observed = scan_codex("", &jsonl(&lines), false);
    let window = &windows(&[observed], &BTreeMap::new(), now)["15"];
    let expected = &case["expected"];
    assert_eq!(window["coverage_complete"], true);
    assert_eq!(window["input_tokens"], expected["window_input"]);
    assert_eq!(window["output_tokens"], expected["window_output"]);
    let total = number(&window["input_tokens"]) + number(&window["output_tokens"]);
    assert_eq!(total, number(&expected["window_tokens"]));
    assert_ne!(total, number(&expected["legacy_lifetime_tokens"]));
}

/// Synthetic legacy case `coverage_accumulator`: the attributed numerator
/// survives every grouping. Adapted: Rust groups token yield by configured
/// project and fleet total (there is no tier grouping); attribution requires
/// a real commit reachable from `origin/main`.
#[tokio::test]
async fn legacy_case_coverage_accumulator() {
    let case = crate::legacy_cases::case("coverage_accumulator");
    let temp = tempfile::tempdir().expect("fixture directory");
    let repository = temp.path().join("repo");
    fs::create_dir(&repository).expect("repository");
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
    ] {
        git(args.into_iter().map(str::to_owned).collect(), &repository)
            .await
            .expect("isolated git fixture");
    }
    fs::write(repository.join("file.txt"), "attributed work\n").expect("source");
    for args in [
        vec!["add", "--", "file.txt"],
        vec!["commit", "-q", "-m", "Attributed work"],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
    ] {
        git(args.into_iter().map(str::to_owned).collect(), &repository)
            .await
            .expect("git fixture");
    }
    let segments = case["input"]["segments"].as_array().expect("segments");
    let project = segments[0]["project"].as_str().expect("project");
    let mut config = observed_config(temp.path());
    config.projects = serde_json::from_value(
        json!([{"id":project,"name":"Sample","repository":repository,"tracker":{}}]),
    )
    .expect("project configuration");
    let now = Utc::now().timestamp();
    let cwd = repository.to_string_lossy().into_owned();
    let sessions = segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            // Unattributed work happened long before any commit exists.
            let at = if segment["attributed"] == true {
                now
            } else {
                now - 40 * 3600
            };
            session(
                &format!("segment-{index}"),
                &cwd,
                vec![sample(at, "synthetic-model", number(&segment["tokens"]), 0)],
            )
        })
        .collect::<Vec<_>>();
    let value = yield_metrics(
        &config,
        &json!({"projects":{}}),
        &sessions,
        &BTreeMap::new(),
        now,
    )
    .await;
    let expected = &case["expected"];
    let coverage = expected["coverage_pct"].as_f64().expect("coverage");
    for row in [&value["projects"][project], &value["total"]] {
        assert_eq!(row["tokens"], expected["total_tokens"]);
        assert_eq!(
            row["attributed_tokens"],
            expected["project_attributed_tokens"]
        );
        assert_eq!(row["attributed_tokens"], expected["tier_attributed_tokens"]);
        assert_eq!(row["coverage_pct"].as_f64(), Some(coverage));
    }
}

/// Parity with tests/integration/python_policy.rs::sc186o_token_yield_parameterized_sql_rejects_injection_metacharacters.
/// The Rust collector keeps no SQL store: it persists numeric usage state as
/// JSON keyed by a SHA-256 of the source path. Session ids, cwds and file
/// names containing SQL metacharacters and quotes are stored verbatim, cannot
/// break parsing or storage, and repeated cycles do not count usage twice.
#[tokio::test]
async fn sc186o_sql_metacharacters_in_session_identity_are_stored_verbatim() {
    let temp = tempfile::tempdir().expect("fixture directory");
    let logs = temp.path().join("sessions/2026/10/19");
    fs::create_dir_all(&logs).expect("session directory");
    let mut config = observed_config(temp.path());
    config.codex_sessions = Some(temp.path().join("sessions"));
    let evil_sid = r#"sid'; DROP TABLE sessions; -- "quoted" \ back"#;
    let evil_cwd = "/tmp/cwd'; DROP TABLE files; --";
    let now = Utc::now().timestamp();
    let body = jsonl(&[
        json!({"type":"session_meta","payload":{"id":evil_sid,"cwd":evil_cwd}}),
        json!({"type":"turn_context","payload":{"model":"gpt-6-luna"}}),
        token_event(now - 5, 100, 10),
    ]);
    let evil_file = logs.join("rollout-evil'; DROP TABLE sessions; --.jsonl");
    fs::write(&evil_file, &body).expect("session log");
    for _ in 0..2 {
        let value = collect(&config, &json!({"agents":[]}))
            .await
            .expect("usage observation");
        let efficiency = &value["token_efficiency"];
        assert_eq!(efficiency["scan"]["sessions"], 1);
        assert_eq!(efficiency["scan"]["state_persisted"], true);
        assert_eq!(efficiency["windows"]["15"]["input_tokens"], 100);
        assert_eq!(value["token_yield"]["total"]["lower_bound_tokens"], 110);
    }
    let text =
        fs::read_to_string(temp.path().join("usage-state-v1.json")).expect("persisted state");
    let state: UsageState = serde_json::from_str(&text).expect("persisted state parses");
    let key = format!(
        "codex:{:x}",
        Sha256::digest(evil_file.as_os_str().to_string_lossy().as_bytes())
    );
    assert_eq!(state.files.keys().collect::<Vec<_>>(), vec![&key]);
    let file = &state.files[&key];
    assert_eq!(file.session.id, evil_sid);
    assert_eq!(file.session.cwd, evil_cwd);
    assert_eq!(file.offset, body.len() as u64);
    assert!(!text.contains("rollout-evil"));
}
