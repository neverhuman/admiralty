//! Small allowlisted records for agents and log shippers. Never copy source tails.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The digest describes health evidence, not freshness. Timestamps and timings
/// remain in every heartbeat, so unchanged evidence never appears newly verified.
pub fn published(snapshot: &Value, previous_digest: Option<&str>) -> Value {
    let reports = snapshot["git_health"].as_object();
    let mut states = BTreeMap::<&str, usize>::new();
    let mut fingerprints = BTreeMap::new();
    if let Some(reports) = reports {
        for (project, report) in reports {
            let state = report["state"]
                .as_str()
                .filter(|s| matches!(*s, "healthy" | "attention" | "unknown" | "disabled"))
                .unwrap_or("unknown");
            *states.entry(state).or_default() += 1;
            fingerprints.insert(
                project,
                (&report["instance_id"], &report["fingerprint"], state),
            );
        }
    }
    let digest = reports.map(|_| {
        // A BTreeMap canonicalizes order. Do not hash/render raw free-form logs.
        let encoded = serde_json::to_vec(&fingerprints).expect("health identities serialize");
        format!("sha256:{:x}", Sha256::digest(encoded))
    });
    let collector = &snapshot["collector"];
    let usage_available = if snapshot.get("usage_error").is_some() {
        Some(false)
    } else {
        snapshot.get("token_efficiency").map(|_| true)
    };
    let history_available = if snapshot.get("history_error").is_some() {
        Some(false)
    } else {
        snapshot.get("history").map(|_| true)
    };
    let degraded = reports.is_none()
        || states.get("unknown").copied().unwrap_or(0) > 0
        || states.get("attention").copied().unwrap_or(0) > 0
        || usage_available != Some(true)
        || history_available != Some(true);
    json!({
        "schema_version":1,"event_name":"observer.snapshot_published",
        "severity_text":if degraded {"WARN"} else {"INFO"},
        "source_observed_utc":snapshot["generated_utc"],
        "observed_utc":collector["observed_utc"],
        "duration_ms":collector["duration_ms"],"stages_ms":collector["stages_ms"],
        "interval_s":snapshot["interval_s"],
        "projects":snapshot["project_catalog"].as_array().map(Vec::len),
        "git_reports":reports.map(serde_json::Map::len),
        "git_states":if reports.is_some() {json!(states)} else {Value::Null},
        "health_digest":digest,
        "health_changed":digest.as_deref().map(|value| Some(value) != previous_digest),
        "usage_available":usage_available,
        "history_available":history_available,
        "evidence_file":"status.json","read_only":true,"model_turns":0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_failures_and_clocks_without_copying_private_source_context() {
        let source = json!({"generated_utc":"original-clock", "project_catalog":[{},{}],
            "collector":{"observed_utc":"collection-clock","duration_ms":12,"stages_ms":{"git":3}},
            "git_health":{"private-project":{"instance_id":"repo","fingerprint":"one","state":"unknown"}},
            "usage_error":"private path", "tail":"private transcript", "token":"private credential"});
        let record = published(&source, None);
        assert_eq!(record["severity_text"], "WARN");
        assert_eq!(record["source_observed_utc"], "original-clock");
        assert_eq!(record["observed_utc"], "collection-clock");
        assert_eq!(record["git_states"]["unknown"], 1);
        assert_eq!(record["usage_available"], false);
        assert_eq!(record["health_changed"], true);
        assert!(!record.to_string().contains("private"));
    }

    #[test]
    fn health_identity_ignores_clock_churn_but_detects_changed_or_removed_reports() {
        let mut source =
            json!({"git_health":{"p":{"instance_id":"a","fingerprint":"one","state":"healthy"}}});
        let first = published(&source, None);
        let digest = first["health_digest"].as_str();
        source["generated_utc"] = json!("later");
        source["collector"] = json!({"duration_ms":900});
        assert_eq!(published(&source, digest)["health_changed"], false);
        source["git_health"]["p"]["instance_id"] = json!("new-instance");
        assert_eq!(published(&source, digest)["health_changed"], true);
        source["git_health"] = json!({});
        assert_eq!(published(&source, digest)["health_changed"], true);
        assert!(published(&json!({}), digest)["health_digest"].is_null());
        assert!(published(&json!({}), digest)["health_changed"].is_null());
        assert!(published(&json!({}), digest)["usage_available"].is_null());
        assert!(published(&json!({}), digest)["history_available"].is_null());
    }
}
