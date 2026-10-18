//! Offline legacy evidence conformance. These checks validate synthetic reference
//! cases and the pinned input inventory; they do not claim replacement-runtime
//! acceptance or contact the operating fleet.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn fixture(name: &str) -> Value {
    let text = match name {
        "lifetime_window_leakage" => {
            include_str!("../fixtures/legacy/cases/lifetime_window_leakage.json")
        }
        "mixed_model_attribution" => {
            include_str!("../fixtures/legacy/cases/mixed_model_attribution.json")
        }
        "unknown_price_zero_fill" => {
            include_str!("../fixtures/legacy/cases/unknown_price_zero_fill.json")
        }
        "coverage_accumulator" => {
            include_str!("../fixtures/legacy/cases/coverage_accumulator.json")
        }
        "stale_capture" => include_str!("../fixtures/legacy/cases/stale_capture.json"),
        "quoted_approval" => include_str!("../fixtures/legacy/cases/quoted_approval.json"),
        "missing_scope" => include_str!("../fixtures/legacy/cases/missing_scope.json"),
        "docs_only_implementation" => {
            include_str!("../fixtures/legacy/cases/docs_only_implementation.json")
        }
        "affected_test_omission" => {
            include_str!("../fixtures/legacy/cases/affected_test_omission.json")
        }
        "wrong_checkout_pass" => include_str!("../fixtures/legacy/cases/wrong_checkout_pass.json"),
        "foreign_cleanup" => include_str!("../fixtures/legacy/cases/foreign_cleanup.json"),
        _ => panic!("unknown fixture: {name}"),
    };
    serde_json::from_str(text).expect("valid legacy fixture")
}

fn number(value: &Value) -> u64 {
    value.as_u64().expect("nonnegative integer")
}

#[test]
fn legacy_sources_have_a_pinned_identity_and_distinct_historical_claims() {
    let manifest: Value =
        serde_json::from_str(include_str!("../fixtures/legacy/source-manifest.json"))
            .expect("pinned legacy source manifest must contain valid JSON");
    assert_eq!(
        manifest["repository"]["base_sha"],
        "1e8ed17eff27ed3ab80dc6860fc8eda301406aee"
    );
    assert_eq!(
        manifest["older_proposal_baseline"]["verification"],
        "unverified_proposal_claims_only"
    );
    assert!(manifest["older_proposal_baseline"]["sha256"].is_null());
    assert!(manifest["historical_attempt_acceptance_counts"].is_null());
    let sources = manifest["sources"]
        .as_array()
        .expect("source manifest must inventory source records");
    let ledger = sources
        .iter()
        .find(|entry| entry["path"] == "data/history/ledger/model_usage_2026-10-18.jsonl")
        .expect("source manifest must include the canonical routing ledger");
    assert_eq!(ledger["records"], 503);
    assert_eq!(
        manifest["older_proposal_baseline"]["claimed_records"]["routing_ledger"],
        449
    );
    let paths: BTreeSet<_> = sources
        .iter()
        .map(|entry| {
            let digest = entry["sha256"]
                .as_str()
                .expect("source entry must carry a SHA-256 string");
            assert_eq!(digest.len(), 64);
            assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
            entry["path"]
                .as_str()
                .expect("source entry must identify its repository-relative path")
        })
        .collect();
    assert_eq!(paths.len(), sources.len());
}

#[test]
fn synthetic_cases_never_claim_historical_attempts_or_acceptances() {
    let names = [
        "lifetime_window_leakage",
        "mixed_model_attribution",
        "unknown_price_zero_fill",
        "coverage_accumulator",
        "stale_capture",
        "quoted_approval",
        "missing_scope",
        "docs_only_implementation",
        "affected_test_omission",
        "wrong_checkout_pass",
        "foreign_cleanup",
    ];
    for name in names {
        let case = fixture(name);
        assert_eq!(case["id"], name);
        assert_eq!(case["provenance"]["kind"], "synthetic_reconstruction");
        assert!(case["provenance"]["historical_exact_attempts"].is_null());
        assert!(case["provenance"]["historical_exact_acceptances"].is_null());
    }
}

#[test]
fn window_consumption_subtracts_the_opening_balance() {
    let case = fixture("lifetime_window_leakage");
    let samples = case["input"]["samples"]
        .as_array()
        .expect("window fixture must include cumulative opening and closing samples");
    let opening = &samples[0];
    let closing = &samples[1];
    assert!(number(&opening["epoch"]) <= number(&case["input"]["window_start_epoch"]));
    let input = number(&closing["input"]) - number(&opening["input"]);
    let output = number(&closing["output"]) - number(&opening["output"]);
    assert_eq!(input, number(&case["expected"]["window_input"]));
    assert_eq!(output, number(&case["expected"]["window_output"]));
    assert_eq!(input + output, number(&case["expected"]["window_tokens"]));
    assert_ne!(
        input + output,
        number(&case["expected"]["legacy_lifetime_tokens"])
    );
}

#[test]
fn model_segments_conserve_usage_without_relabeling_history() {
    let case = fixture("mixed_model_attribution");
    let mut by_model: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for segment in case["input"]["segments"]
        .as_array()
        .expect("mixed-model fixture must contain native usage segments")
    {
        let entry = by_model
            .entry(
                segment["model"]
                    .as_str()
                    .expect("usage segment must name its actual model"),
            )
            .or_default();
        entry.0 += number(&segment["input"]);
        entry.1 += number(&segment["output"]);
    }
    assert_eq!(by_model.len(), 2);
    for (model, (input, output)) in &by_model {
        assert_eq!(
            *input,
            number(&case["expected"]["by_model"][*model]["input"])
        );
        assert_eq!(
            *output,
            number(&case["expected"]["by_model"][*model]["output"])
        );
    }
    assert_eq!(
        by_model.values().map(|value| value.0).sum::<u64>(),
        number(&case["expected"]["total_input"])
    );
    assert_eq!(
        by_model.values().map(|value| value.1).sum::<u64>(),
        number(&case["expected"]["total_output"])
    );
}

#[test]
fn missing_prices_leave_total_valuation_unavailable() {
    let case = fixture("unknown_price_zero_fill");
    let segments = case["input"]["segments"]
        .as_array()
        .expect("price fixture must include both priced and unpriced segments");
    let known: f64 = segments
        .iter()
        .filter_map(|value| value["credits_est"].as_f64())
        .sum();
    let priced: u64 = segments
        .iter()
        .filter(|value| !value["credits_est"].is_null())
        .map(|value| number(&value["tokens"]))
        .sum();
    let total: u64 = segments.iter().map(|value| number(&value["tokens"])).sum();
    let complete_total = segments
        .iter()
        .map(|value| value["credits_est"].as_f64())
        .sum::<Option<f64>>();
    assert_eq!(
        known,
        case["expected"]["known_credits_est"]
            .as_f64()
            .expect("price fixture must specify the known valuation subtotal")
    );
    assert_eq!(priced, number(&case["expected"]["priced_tokens"]));
    assert_eq!(total - priced, number(&case["expected"]["unpriced_tokens"]));
    assert!(complete_total.is_none());
    assert!(case["expected"]["total_credits_est"].is_null());
    assert!(
        (100.0 * priced as f64 / total as f64
            - case["expected"]["priced_coverage_pct"]
                .as_f64()
                .expect("price fixture must specify priced token coverage"))
        .abs()
            < 1e-10
    );
}

#[test]
fn coverage_numerator_survives_each_grouping() {
    let case = fixture("coverage_accumulator");
    let segments = case["input"]["segments"]
        .as_array()
        .expect("coverage fixture must contain attributed and unattributed segments");
    let total: u64 = segments.iter().map(|value| number(&value["tokens"])).sum();
    let attributed: u64 = segments
        .iter()
        .filter(|value| value["attributed"] == true)
        .map(|value| number(&value["tokens"]))
        .sum();
    assert_eq!(
        attributed,
        number(&case["expected"]["project_attributed_tokens"])
    );
    assert_eq!(
        attributed,
        number(&case["expected"]["tier_attributed_tokens"])
    );
    assert_eq!(total, number(&case["expected"]["total_tokens"]));
    assert_eq!(
        100 * attributed / total,
        number(&case["expected"]["coverage_pct"])
    );
}

#[test]
fn stale_or_failed_capture_cannot_authorize_input() {
    let case = fixture("stale_capture");
    let input = &case["input"];
    let capture = &input["capture"];
    let fresh = number(&input["current_epoch"]).saturating_sub(number(&capture["observed_epoch"]))
        <= number(&input["max_age_s"]);
    let allowed = fresh
        && capture["command_exit"] == 0
        && capture["process_identity"] == input["current_process_identity"];
    assert_eq!(
        allowed,
        case["expected"]["input_allowed"]
            .as_bool()
            .expect("stale-capture fixture must specify whether input is allowed")
    );
}

#[test]
fn quoted_permission_words_do_not_authorize_a_response() {
    let case = fixture("quoted_approval");
    let input = &case["input"];
    assert!(
        input["output_lines"]
            .as_array()
            .expect("approval fixture must contain synthetic output lines")
            .iter()
            .any(|line| line
                .as_str()
                .expect("synthetic terminal line must be text")
                .contains("(y/n)"))
    );
    let allowed = !input["structured_permission_request"].is_null()
        && input["turn_state"] == "awaiting_permission";
    assert_eq!(
        allowed,
        case["expected"]["input_allowed"]
            .as_bool()
            .expect("quoted-approval fixture must specify whether input is allowed")
    );
    assert_eq!(input["turn_state"], case["expected"]["state"]);
}

#[test]
fn missing_write_scope_blocks_admission() {
    let case = fixture("missing_scope");
    let scoped = case["input"]["task"]["write_scope"]
        .as_array()
        .is_some_and(|scope| !scope.is_empty());
    assert!(!scoped);
    assert_eq!(case["expected"]["admission"], "reject");
}

#[test]
fn documentation_and_self_report_do_not_close_runtime_behavior() {
    let case = fixture("docs_only_implementation");
    let input = &case["input"];
    let code_changed = input["changed_paths"]
        .as_array()
        .expect("docs-only fixture must enumerate changed paths")
        .iter()
        .any(|path| {
            !path
                .as_str()
                .expect("changed path must be a string")
                .starts_with("docs/")
        });
    let ran_behavior = input["verification"]["commands"]
        .as_array()
        .is_some_and(|commands| !commands.is_empty());
    assert_eq!(
        code_changed && ran_behavior,
        case["expected"]["accepted"]
            .as_bool()
            .expect("docs-only fixture must specify its acceptance result")
    );
}

#[test]
fn affected_test_evidence_must_include_consumers() {
    let case = fixture("affected_test_omission");
    let input = &case["input"];
    let passed: BTreeSet<_> = input["executed_tests"]
        .as_array()
        .expect("affected-test fixture must enumerate executed checks")
        .iter()
        .filter(|test| test["exit_code"] == 0)
        .map(|test| {
            test["name"]
                .as_str()
                .expect("executed check must have a name")
        })
        .collect();
    let missing: Vec<_> = input["affected_tests"]
        .as_array()
        .expect("affected-test fixture must enumerate affected consumer tests")
        .iter()
        .filter(|test| {
            !passed.contains(
                test.as_str()
                    .expect("affected consumer test must have a name"),
            )
        })
        .cloned()
        .collect();
    assert_eq!(
        serde_json::json!(missing),
        case["expected"]["missing_tests"]
    );
    assert_eq!(
        missing.is_empty(),
        case["expected"]["accepted"]
            .as_bool()
            .expect("affected-test fixture must specify its acceptance result")
    );
}

#[test]
fn pass_receipt_must_name_the_tested_checkout() {
    let case = fixture("wrong_checkout_pass");
    let accepts = |receipt: &Value| {
        let sha = receipt["sha"].as_str().unwrap_or_default();
        receipt["status"] == "PASS"
            && receipt["command_exit"] == 0
            && sha.len() == 40
            && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            && receipt["sha"] == receipt["checkout_sha"]
    };
    assert_eq!(
        accepts(&case["input"]["receipt"]),
        case["expected"]["accepted"]
            .as_bool()
            .expect("wrong-checkout fixture must specify mismatch acceptance")
    );
    assert_eq!(
        accepts(&case["input"]["matching_receipt"]),
        case["expected"]["matching_accepted"]
            .as_bool()
            .expect("wrong-checkout fixture must specify matching-checkout acceptance")
    );
}

#[test]
fn unknown_custody_permits_inventory_only() {
    let case = fixture("foreign_cleanup");
    let input = &case["input"];
    let asset = &input["asset"];
    let owned = !asset["registered_owner"].is_null()
        && asset["registered_owner"] == input["requesting_owner"];
    let safe = owned
        && asset["dirty"] == false
        && asset["symlink"] == false
        && asset["active_processes"]
            .as_array()
            .expect("custody fixture must enumerate active process references")
            .is_empty();
    assert_eq!(
        safe,
        case["expected"]["cleanup_allowed"]
            .as_bool()
            .expect("custody fixture must specify whether cleanup is allowed")
    );
    assert_eq!(case["expected"]["action"], "inventory_only");
}
