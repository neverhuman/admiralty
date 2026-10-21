//! One compact log event per observation cycle, built for agent consumption.
//!
//! A healthy unchanged fleet produces identical events (durations excluded),
//! which the caller's [`admiralty_core::agent_log::Rollup`] collapses into one
//! line per window. Any change in Git health, alerts, resource warnings or
//! source errors is a different event and is written immediately.
use admiralty_core::agent_log::{Event, Level};
use serde_json::{Value, json};
use std::time::Duration;

/// Fields that vary every cycle without changing the fleet's condition.
pub const VOLATILE: &[&str] = &[
    "ms",
    "fleet_ms",
    "projects_ms",
    "git_ms",
    "git_health_ms",
    "usage_ms",
    "history_ms",
];

fn ids_with_state(snapshot: &Value, state: &str) -> Vec<String> {
    snapshot["git_health"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, report)| report["state"] == state)
        .map(|(id, _)| id.clone())
        .collect()
}

pub fn cycle_event(outcome: &crate::Result<Value>, elapsed: Duration) -> Event {
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(snapshot) => {
            let warnings = snapshot["global_resources"]["warnings"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let level_count = |level: &str| warnings.iter().filter(|w| w["level"] == level).count();
            let attention = ids_with_state(snapshot, "attention");
            let unknown = ids_with_state(snapshot, "unknown");
            let source_errors = ["usage_error", "history_error"]
                .into_iter()
                .filter(|key| snapshot[*key].is_string())
                .collect::<Vec<_>>();
            let degraded =
                !attention.is_empty() || level_count("red") > 0 || !source_errors.is_empty();
            let stages = &snapshot["collector"]["stages_ms"];
            let mut event = Event::new(
                crate::now_utc(),
                if degraded { Level::Warn } else { Level::Info },
                "observe.cycle",
            )
            .field("projects", snapshot["order"].as_array().map_or(0, Vec::len))
            .field("ms", ms);
            for stage in ["fleet", "projects", "git", "git_health", "usage", "history"] {
                if let Some(value) = stages[stage].as_u64() {
                    event = event.field(format!("{stage}_ms"), value);
                }
            }
            event
                .field("git_attention", json!(attention))
                .field("git_unknown", json!(unknown))
                .field(
                    "git_alerts",
                    snapshot["git_health_alerts"].as_array().map_or(0, Vec::len),
                )
                .field("res_red", level_count("red"))
                .field("res_amber", level_count("amber"))
                .field("source_errors", json!(source_errors))
        }
        Err(error) => Event::new(crate::now_utc(), Level::Error, "observe.cycle_failed")
            .field("ms", ms)
            .field("err", crate::io::clean(&error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::agent_log::{Format, Rollup};

    fn snapshot(attention: bool, ms: u64) -> Value {
        json!({"order":["a","b"],
            "collector":{"stages_ms":{"fleet":ms,"git":ms+1}},
            "git_health":{"a":{"state":if attention {"attention"} else {"healthy"}},"b":{"state":"unknown"}},
            "git_health_alerts":[],
            "global_resources":{"warnings":[{"level":"amber","text":"disk"}]}})
    }

    #[test]
    fn cycle_event_is_compact_and_unchanged_fleet_collapses() {
        let quiet = cycle_event(&Ok(snapshot(false, 10)), Duration::from_millis(1834));
        let line = quiet.render(Format::Logfmt);
        assert!(
            line.contains(" info observe.cycle projects=2 ms=1834 fleet_ms=10 git_ms=11 "),
            "{line}"
        );
        assert!(
            line.contains(
                "git_attention=[] git_unknown=b git_alerts=0 res_red=0 res_amber=1 source_errors=[]"
            ),
            "{line}"
        );
        let mut rollup = Rollup::new(600, VOLATILE);
        assert_eq!(rollup.admit(quiet, 0).len(), 1);
        // Only timings changed: collapsed.
        let again = cycle_event(&Ok(snapshot(false, 99)), Duration::from_millis(2500));
        assert!(rollup.admit(again, 15).is_empty());
        // A new Git attention state is written immediately as a warning.
        let changed = cycle_event(&Ok(snapshot(true, 10)), Duration::from_millis(1800));
        let out = rollup.admit(changed, 30);
        assert_eq!(out.len(), 2, "pending repeat summary, then the change");
        assert_eq!(out[1].level, Level::Warn);
        assert!(out[1].render(Format::Logfmt).contains("git_attention=a"));
    }

    #[test]
    fn failures_are_errors_with_scrubbed_single_line_text() {
        let failed: crate::Result<Value> = Err("scan failed\nsecond line".into());
        let event = cycle_event(&failed, Duration::from_millis(5));
        assert_eq!(event.level, Level::Error);
        let line = event.render(Format::Json);
        let parsed: Value = serde_json::from_str(&line).expect("ndjson");
        assert_eq!(parsed["ev"], "observe.cycle_failed");
        assert_eq!(parsed["ms"], 5);
        assert!(!line.contains('\n'));
    }
}
