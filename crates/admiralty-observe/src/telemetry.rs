//! State-change telemetry for the observer.
//!
//! Each cycle emits one `obs.cycle` event (identical cycles coalesce into a
//! single `x=N` line per minute) plus events only when something changed:
//! a host warning appeared or cleared, a project's Git health state moved,
//! or a captain alert was raised. Steady state therefore costs about one
//! line a minute, and every line is something an agent can act on.
use admiralty_telemetry::{Level, digest::template, event};
use serde_json::Value;
use std::collections::BTreeMap;

/// Stable identity for a warning: numbers change every cycle, the condition
/// does not. `"build / 86.0% used, 14 GB free"` → `"build / #.#% used, # GB free"`.
fn warning_identity(warning: &Value) -> Option<(String, String)> {
    let text = warning["text"].as_str()?;
    let level = warning["level"].as_str().unwrap_or("unknown").to_owned();
    Some((format!("{level}|{}", template(text)), text.to_owned()))
}

fn warnings(snapshot: Option<&Value>) -> BTreeMap<String, (String, String)> {
    snapshot
        .map(|s| &s["global_resources"]["warnings"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let (identity, text) = warning_identity(w)?;
            let level = w["level"].as_str().unwrap_or("unknown").to_owned();
            Some((identity, (level, text)))
        })
        .collect()
}

fn state(snapshot: Option<&Value>, project: &str) -> Option<String> {
    snapshot?["git_health"][project]["state"]
        .as_str()
        .map(str::to_owned)
}

/// Emit cycle, warning and Git-health transition events.
pub fn changes(previous: Option<&Value>, current: &Value) {
    let hosts = current["global_resources"]["hosts"].as_array();
    let reachable = hosts
        .into_iter()
        .flatten()
        .filter(|h| h["reachable"] == true)
        .count();
    let mut cycle = event(Level::Info, "obs.cycle", "cycle.published")
        .field("projects", current["order"].as_array().map_or(0, Vec::len))
        .field("hosts", hosts.map_or(0, Vec::len))
        .field("reachable", reachable)
        .volatile(
            "ms",
            current["collector"]["duration_ms"].as_u64().unwrap_or(0),
        );
    if cycle.active() {
        for (name, ms) in current["collector"]["phases_ms"]
            .as_object()
            .into_iter()
            .flatten()
        {
            cycle = cycle.volatile(&format!("{name}_ms"), ms.as_u64().unwrap_or(0));
        }
    }
    cycle.emit();
    for key in ["usage_error", "history_error"] {
        if let Some(detail) = current[key].as_str() {
            event(Level::Warn, "obs.cycle", key)
                .field("detail", detail)
                .emit();
        }
    }

    let before = warnings(previous);
    let after = warnings(Some(current));
    for (identity, (level, text)) in &after {
        if !before.contains_key(identity) {
            let severity = if level == "red" {
                Level::Error
            } else {
                Level::Warn
            };
            event(severity, "obs.host", "host.warning")
                .field("level", level.as_str())
                .field("text", text.as_str())
                .emit();
        }
    }
    // A first cycle has no baseline: absence there is not a recovery.
    if previous.is_some() {
        for (identity, (level, text)) in &before {
            if !after.contains_key(identity) {
                event(Level::Info, "obs.host", "host.recovered")
                    .field("level", level.as_str())
                    .field("text", text.as_str())
                    .emit();
            }
        }
    }

    for (project, report) in current["git_health"].as_object().into_iter().flatten() {
        let now = report["state"].as_str().unwrap_or("unknown");
        let then = state(previous, project);
        if then.as_deref() != Some(now) {
            let level = match now {
                "attention" | "unknown" => Level::Warn,
                _ => Level::Info,
            };
            let codes = report["issues"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i["code"].as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            event(level, "obs.git", "git_health.changed")
                .field("project", project.as_str())
                .field("from", then.unwrap_or_else(|| "none".into()))
                .field("to", now)
                .field_opt("issues", (!codes.is_empty()).then_some(codes))
                .emit();
        }
    }
    for alert in current["git_health_alerts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        event(Level::Warn, "obs.git", "git_health.alert")
            .field("project", alert["project_id"].as_str().unwrap_or("unknown"))
            .field("state", alert["state"].as_str().unwrap_or("unknown"))
            .field(
                "issues",
                alert["issue_codes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            )
            .emit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn warning_identity_ignores_changing_numbers_but_keeps_level() {
        let a = warning_identity(&json!({"level":"amber","text":"build / 86.0% used, 14 GB free"}));
        let b = warning_identity(&json!({"level":"amber","text":"build / 87.5% used, 12 GB free"}));
        let c = warning_identity(&json!({"level":"red","text":"build / 93.0% used, 7 GB free"}));
        assert_eq!(a.as_ref().map(|x| &x.0), b.as_ref().map(|x| &x.0));
        assert_ne!(a.map(|x| x.0), c.map(|x| x.0));
        assert!(warning_identity(&json!({"level":"red"})).is_none());
    }

    #[test]
    fn transitions_are_detected_from_previous_snapshot() {
        let previous = json!({"global_resources":{"warnings":[{"level":"amber","text":"h memory availability is low"}]},
            "git_health":{"p":{"state":"healthy"}}});
        let current = json!({"order":["p"],"global_resources":{"hosts":[],"warnings":[]},
            "git_health":{"p":{"state":"attention","issues":[{"code":"merge_conflicts"}]}},
            "git_health_alerts":[{"project_id":"p","state":"attention","issue_codes":["merge_conflicts"]}],
            "collector":{"duration_ms":12,"phases_ms":{"git":3}}});
        // Emission goes to the process logger; this checks state extraction.
        assert_eq!(warnings(Some(&previous)).len(), 1);
        assert!(warnings(Some(&current)).is_empty());
        assert_eq!(state(Some(&previous), "p").as_deref(), Some("healthy"));
        assert_eq!(state(None, "p"), None);
        changes(Some(&previous), &current);
        changes(None, &current);
    }
}
