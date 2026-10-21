//! Synthetic legacy regression cases (`tests/fixtures/legacy/cases/*.json`)
//! shared by the observer parity tests, plus the fixture hygiene checks that
//! the retired `tests/fixtures/legacy/verify.py` performed. The cases are
//! synthetic reconstructions; they never certify historical acceptance.
use serde_json::Value;

const CASES: [(&str, &str); 11] = [
    (
        "affected_test_omission",
        include_str!("../../../tests/fixtures/legacy/cases/affected_test_omission.json"),
    ),
    (
        "coverage_accumulator",
        include_str!("../../../tests/fixtures/legacy/cases/coverage_accumulator.json"),
    ),
    (
        "docs_only_implementation",
        include_str!("../../../tests/fixtures/legacy/cases/docs_only_implementation.json"),
    ),
    (
        "foreign_cleanup",
        include_str!("../../../tests/fixtures/legacy/cases/foreign_cleanup.json"),
    ),
    (
        "lifetime_window_leakage",
        include_str!("../../../tests/fixtures/legacy/cases/lifetime_window_leakage.json"),
    ),
    (
        "missing_scope",
        include_str!("../../../tests/fixtures/legacy/cases/missing_scope.json"),
    ),
    (
        "mixed_model_attribution",
        include_str!("../../../tests/fixtures/legacy/cases/mixed_model_attribution.json"),
    ),
    (
        "quoted_approval",
        include_str!("../../../tests/fixtures/legacy/cases/quoted_approval.json"),
    ),
    (
        "stale_capture",
        include_str!("../../../tests/fixtures/legacy/cases/stale_capture.json"),
    ),
    (
        "unknown_price_zero_fill",
        include_str!("../../../tests/fixtures/legacy/cases/unknown_price_zero_fill.json"),
    ),
    (
        "wrong_checkout_pass",
        include_str!("../../../tests/fixtures/legacy/cases/wrong_checkout_pass.json"),
    ),
];
/// Historical inventory of the pinned legacy baseline. Its source digests are
/// immutable Git history at `repository.base_sha`; they are no longer
/// re-verified against the working tree because the pinned sources are retired.
const SOURCE_MANIFEST: &str = include_str!("../../../tests/fixtures/legacy/source-manifest.json");

pub(crate) fn case(name: &str) -> Value {
    let text = CASES
        .iter()
        .find(|(id, _)| *id == name)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("unknown legacy case {name}"));
    serde_json::from_str(text).expect("legacy case fixture is valid JSON")
}

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Broader (never narrower) re-expression of `tools/export_history.py`
/// `SECRET_RES`, which `verify.py` applied to every fixture: the token prefixes
/// match without their minimum-length suffixes, and keyword assignments match
/// regardless of the assigned value's length.
pub(crate) fn credential_like(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let word_start = |i: usize| !lower[..i].chars().next_back().is_some_and(word);
    let found = |needle: &str, at_start: bool| {
        lower
            .match_indices(needle)
            .any(|(i, _)| !at_start || word_start(i))
    };
    // `gh[pousr]_` and `github_pat_` and the PEM header have no leading `\b`.
    let anywhere = [
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "-----begin",
        "authorization:",
    ];
    // `\bsk-`, `\bAKIA`, `\bxox[abprs]-`, `\bAIza`, `\beyJ` (case-folded here).
    let at_word_start = [
        "sk-", "akia", "xoxa-", "xoxb-", "xoxp-", "xoxr-", "xoxs-", "aiza", "eyj",
    ];
    if anywhere.iter().any(|needle| found(needle, false))
        || at_word_start.iter().any(|needle| found(needle, true))
    {
        return true;
    }
    [
        "api_key", "api-key", "apikey", "secret", "passwd", "password", "bearer",
    ]
    .iter()
    .any(|keyword| {
        lower.match_indices(keyword).any(|(i, _)| {
            let rest = &lower[i + keyword.len()..];
            if !word_start(i) || rest.chars().next().is_some_and(word) {
                return false;
            }
            let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest);
            rest.trim_start().starts_with([':', '='])
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Parity with tests/fixtures/legacy/verify.py (fixture checks): every
    /// fixture is valid JSON without credential-like text or absolute home
    /// paths, and exactly 11 uniquely identified synthetic cases exist. The
    /// pinned-source Git digests and history mirror checks are retired with the
    /// pinned legacy sources; `source-manifest.json` remains historical data.
    #[test]
    fn fixture_hygiene_replaces_retired_verify_py() {
        let mut ids = BTreeSet::new();
        for (name, text) in CASES
            .iter()
            .chain([("source-manifest", SOURCE_MANIFEST)].iter())
        {
            let value: Value = serde_json::from_str(text).expect("fixture is valid JSON");
            assert!(!credential_like(text), "credential-like text in {name}");
            assert!(
                !text.contains("/home/") && !text.contains("/Users/"),
                "absolute home path in {name}"
            );
            if *name == "source-manifest" {
                assert_eq!(
                    value["purpose"],
                    "Pinned read-only legacy evidence, not runtime authority"
                );
                continue;
            }
            assert_eq!(value["id"], *name);
            assert_eq!(value["schema_version"], 1);
            assert_eq!(value["provenance"]["kind"], "synthetic_reconstruction");
            assert!(value["provenance"]["historical_exact_attempts"].is_null());
            assert!(value["provenance"]["historical_exact_acceptances"].is_null());
            assert!(ids.insert(value["id"].as_str().expect("case id").to_owned()));
        }
        assert_eq!(ids.len(), 11);
    }

    /// Each exporter `SECRET_RES` pattern's canonical form is still detected by
    /// the regex-free scanner, while ordinary fixture prose is not.
    #[test]
    fn credential_scan_covers_every_exporter_pattern() {
        let a = |n: usize| "a".repeat(n);
        for text in [
            format!("ghp_{}", a(36)),
            format!("xgho_{}", a(20)),
            format!("github_pat_{}", a(22)),
            format!("key sk-{}", a(20)),
            "AKIAABCDEFGHIJKLMNOP".to_owned(),
            format!("xoxb-{}", a(10)),
            "-----BEGIN RSA PRIVATE KEY-----".to_owned(),
            format!("AIza{}", a(35)),
            "api_key = abcdefghijklmnop1234".to_owned(),
            "API-KEY: abcdefghijklmnop1234".to_owned(),
            r#"{"password": "abcdefghijklmnopq"}"#.to_owned(),
            "secret='abcdefghijklmnopqrst'".to_owned(),
            "Bearer = abcdefghijklmnopqrst".to_owned(),
            format!("eyJ{}.{}.", a(20), a(20)),
            "Authorization: Bearer abcdefghijkl".to_owned(),
        ] {
            assert!(credential_like(&text), "{text}");
        }
        for text in [
            "task-123 risk-free disk-usage",
            "A successful narrow check cannot close a changed shared interface",
            "the secret of the passwords table",
            "Synthetic idle prompt >",
        ] {
            assert!(!credential_like(text), "{text}");
        }
    }
}
