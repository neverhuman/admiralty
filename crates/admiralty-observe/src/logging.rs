//! Small allowlisted JSONL events for log processors and agents. No source prose.
use serde_json::{Value, json};

pub fn published(snapshot: &Value, output_bytes: Option<u64>) -> Value {
    let mut states = [0usize; 4];
    for report in snapshot["git_health"]
        .as_object()
        .into_iter()
        .flat_map(|r| r.values())
    {
        states[match report["state"].as_str() {
            Some("healthy") => 0,
            Some("attention") => 1,
            Some("disabled") => 2,
            _ => 3,
        }] += 1;
    }
    json!({"schema_version":1,"event":"observation.published","level":"info",
        "observed_utc":snapshot["collector"]["observed_utc"].as_str(),
        "duration_ms":snapshot["collector"]["duration_ms"].as_u64(),
        "output_bytes":output_bytes,
        "project_count":snapshot["projects"].as_object().map(|p|p.len()),
        "git":{"reports":states.iter().sum::<usize>(),"healthy":states[0],"attention":states[1],"disabled":states[2],"unknown":states[3]},
        "changed_git_alerts":snapshot["git_health_alerts"].as_array().map(|a|a.len()),
        "usage_error":!snapshot["usage_error"].is_null(),
        "history_error":!snapshot["history_error"].is_null()})
}

pub fn failure(event: &'static str) -> Value {
    json!({"schema_version":1,"event":event,"level":"error","observed_utc":crate::now_utc(),
        "code":"observation_failed","detail":"Inspect the local observer inputs; prior published facts are retained."})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_counters_and_fixed_codes_without_importing_private_text() {
        let event = published(
            &json!({"projects":{"p":{"tail":"secret"}},
            "git_health":{"p":{"state":"attention","detail":"secret"}},
            "usage_error":"secret","collector":{"duration_ms":42},"git_health_alerts":[]}),
            Some(1234),
        );
        assert_eq!(event["git"]["attention"], 1);
        assert_eq!(event["usage_error"], true);
        assert!(event["observed_utc"].is_null());
        assert!(!event.to_string().contains("secret"));
        assert_eq!(event["output_bytes"], 1234);
    }
}
