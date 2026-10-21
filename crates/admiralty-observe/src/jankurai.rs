//! Jankurai score ingestion: publish each project's latest `jankurai audit`
//! report (`.jankurai/repo-score.json`) as a compact, bounded summary.
//!
//! The observer never runs the auditor, never writes into a repository and
//! never turns a missing or malformed report into a score. A project without
//! a report is published as `missing`; an unreadable or out-of-schema report
//! as `invalid`. Scores keep the auditor's own decision, minimum and caps, so
//! the console can show "56, advisory, minimum 85" instead of a bare number.
use crate::{config::ObserveConfig, io};
use serde_json::{Map, Value, json};
use std::path::Path;

/// Default report location written by `jankurai audit` (relative to the repository).
pub const REPORT_PATH: &str = ".jankurai/repo-score.json";
/// Reports for large repositories are a few hundred KiB; refuse anything absurd.
pub const MAX_REPORT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CAPS: usize = 32;
const MAX_DIMENSIONS: usize = 24;
const SEVERITIES: [&str; 5] = ["critical", "high", "medium", "low", "info"];

/// Summaries for every configured project, keyed by project id.
pub fn collect(config: &ObserveConfig) -> Value {
    let mut projects = Map::new();
    for project in &config.projects {
        projects.insert(
            project.id.clone(),
            observe(&project.repository.join(REPORT_PATH)),
        );
    }
    json!({
        "report_path": REPORT_PATH,
        "projects": projects,
        "note": "Latest committed or locally written jankurai report per repository; the observer does not run audits. Missing reports stay missing, never zero."
    })
}

/// Observe one report path.
pub fn observe(path: &Path) -> Value {
    let modified = std::fs::symlink_metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(|time| {
            chrono::DateTime::<chrono::Utc>::from(time)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
    match io::read_json(path, MAX_REPORT_BYTES) {
        Ok(report) => summarize(&report, modified),
        Err(error) => {
            let missing = std::fs::symlink_metadata(path).is_err();
            json!({
                "status": if missing { "missing" } else { "invalid" },
                "reason": if missing { "no report; run `jankurai audit` in the repository".to_owned() } else { io::clean(&error.to_string()) },
            })
        }
    }
}

fn bounded_number(value: &Value) -> Value {
    value
        .as_f64()
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
        .map_or(Value::Null, |n| json!(n))
}

fn short_text(value: &Value, max: usize) -> Value {
    value
        .as_str()
        .map(|text| json!(io::clean(&text.chars().take(max).collect::<String>())))
        .unwrap_or(Value::Null)
}

/// Unix-seconds strings (the auditor's `generated_at`) become RFC 3339.
fn generated_at(value: &Value) -> Value {
    let seconds = match value {
        Value::String(text) => text.parse::<i64>().ok(),
        Value::Number(number) => number.as_i64(),
        _ => None,
    };
    seconds
        .and_then(|s| chrono::DateTime::<chrono::Utc>::from_timestamp(s, 0))
        .map_or_else(
            || short_text(value, 64),
            |time| json!(time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        )
}

/// Reduce a full report to the facts the console and agents need.
pub fn summarize(report: &Value, modified_utc: Option<String>) -> Value {
    if report.get("standard").and_then(Value::as_str) != Some("jankurai")
        || !report.get("score").is_some_and(Value::is_number)
    {
        return json!({"status": "invalid", "reason": "not a jankurai repo-score report"});
    }
    let decision = &report["decision"];
    let mut findings = Map::new();
    let mut total = 0_u64;
    for severity in SEVERITIES {
        findings.insert(severity.to_owned(), json!(0));
    }
    let listed = report["findings"].as_array();
    for finding in listed.into_iter().flatten() {
        if let Some(severity) = finding["severity"]
            .as_str()
            .filter(|s| SEVERITIES.contains(s))
        {
            let count = findings[severity].as_u64().unwrap_or(0) + 1;
            findings.insert(severity.to_owned(), json!(count));
            total += 1;
        }
    }
    findings.insert("total".into(), json!(total));
    let caps: Vec<Value> = report["caps_applied"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_CAPS)
        .map(|cap| short_text(cap, 96))
        .filter(|cap| !cap.is_null())
        .collect();
    let dimensions: Vec<Value> = report["dimensions"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_DIMENSIONS)
        .map(|dimension| {
            json!({
                "name": short_text(&dimension["name"], 96),
                "score": bounded_number(&dimension["score"]),
                "weight": dimension["weight"].as_u64(),
            })
        })
        .collect();
    let lowest = dimensions
        .iter()
        .filter(|d| d["score"].is_number())
        .min_by(|a, b| {
            a["score"]
                .as_f64()
                .partial_cmp(&b["score"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|d| d["name"].clone())
        .unwrap_or(Value::Null);
    json!({
        "status": "observed",
        "score": bounded_number(&report["score"]),
        "raw_score": bounded_number(&report["raw_score"]),
        "decision": short_text(&decision["status"], 32),
        "minimum_score": bounded_number(&decision["minimum_score"]),
        "passed": decision["passed"].as_bool(),
        "hard_findings": decision["hard_findings"].as_u64(),
        "soft_findings": decision["soft_findings"].as_u64(),
        "findings": findings,
        "caps_applied": caps,
        "caps_total": report["caps_applied"].as_array().map(Vec::len),
        "dimensions": dimensions,
        "weakest_dimension": lowest,
        "conformance": {
            "claimed": short_text(&report["claimed_conformance_level"], 16),
            "observed": short_text(&report["observed_conformance_level"], 16),
            "decision": short_text(&report["conformance_decision"], 32),
        },
        "auditor_version": short_text(&report["auditor_version"], 32),
        "standard_version": short_text(&report["standard_version"], 32),
        "audited_head": short_text(&report["git"]["head"], 64),
        "audited_dirty": report["dirty_worktree"].as_bool(),
        "scope": short_text(&report["scope"]["mode"], 32),
        "generated_utc": generated_at(&report["generated_at"]),
        "report_modified_utc": modified_utc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Value {
        json!({
            "standard": "jankurai", "standard_version": "0.9.0", "auditor_version": "1.7.5",
            "schema_version": "1.9.0", "score": 56, "raw_score": 56,
            "generated_at": "1792590948", "dirty_worktree": false,
            "scope": {"mode": "full", "paths": []},
            "git": {"head": "2f5b93157c3b0a2194929f08d166d18c1abaad43"},
            "claimed_conformance_level": "HL3", "observed_conformance_level": "HL1",
            "conformance_decision": "review",
            "decision": {"status": "advisory", "minimum_score": 85, "passed": false, "hard_findings": 53, "soft_findings": 27},
            "caps_applied": ["no-one-command-setup-or-validation", "python-direct-product-truth-or-db-ownership"],
            "dimensions": [
                {"name": "Code shape", "weight": 12, "score": 0, "weighted_points": 0, "evidence": [], "notes": []},
                {"name": "Security", "weight": 12, "score": 88, "weighted_points": 10.56, "evidence": [], "notes": []}
            ],
            "findings": [
                {"severity": "high", "problem": "x"}, {"severity": "high"}, {"severity": "medium"},
                {"severity": "critical"}, {"severity": "bogus"}
            ]
        })
    }

    #[test]
    fn summary_keeps_decision_caps_and_counts_without_inventing_values() {
        let summary = summarize(&report(), Some("2026-10-21T13:55:00Z".into()));
        assert_eq!(summary["status"], "observed");
        assert_eq!(summary["score"], json!(56.0));
        assert_eq!(summary["decision"], "advisory");
        assert_eq!(summary["minimum_score"], json!(85.0));
        assert_eq!(summary["passed"], false);
        assert_eq!(summary["findings"]["high"], 2);
        assert_eq!(summary["findings"]["critical"], 1);
        assert_eq!(
            summary["findings"]["total"], 4,
            "unknown severities are not counted"
        );
        assert_eq!(summary["caps_total"], 2);
        assert_eq!(summary["weakest_dimension"], "Code shape");
        assert_eq!(summary["generated_utc"], "2026-10-21T13:55:48Z");
        assert_eq!(
            summary["audited_head"],
            "2f5b93157c3b0a2194929f08d166d18c1abaad43"
        );
    }

    #[test]
    fn foreign_or_scoreless_documents_are_invalid_not_zero() {
        assert_eq!(summarize(&json!({"score": 99}), None)["status"], "invalid");
        assert_eq!(
            summarize(&json!({"standard": "jankurai", "score": "high"}), None)["status"],
            "invalid"
        );
        let mut out_of_range = report();
        out_of_range["score"] = json!(140);
        assert!(summarize(&out_of_range, None)["score"].is_null());
    }

    #[test]
    fn missing_symlinked_and_malformed_reports_stay_visible() -> crate::Result<()> {
        let directory = tempfile::tempdir()?;
        let missing = observe(&directory.path().join(REPORT_PATH));
        assert_eq!(missing["status"], "missing");
        let report_dir = directory.path().join(".jankurai");
        std::fs::create_dir(&report_dir)?;
        let path = report_dir.join("repo-score.json");
        std::fs::write(&path, b"{not json")?;
        assert_eq!(observe(&path)["status"], "invalid");
        std::fs::remove_file(&path)?;
        let target = directory.path().join("elsewhere.json");
        std::fs::write(&target, serde_json::to_vec(&report())?)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &path)?;
            assert_eq!(
                observe(&path)["status"],
                "invalid",
                "symlinks are not followed"
            );
            std::fs::remove_file(&path)?;
        }
        std::fs::write(&path, serde_json::to_vec(&report())?)?;
        let observed = observe(&path);
        assert_eq!(observed["status"], "observed");
        assert!(observed["report_modified_utc"].is_string());
        Ok(())
    }
}
