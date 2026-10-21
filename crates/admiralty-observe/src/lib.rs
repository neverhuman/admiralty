//! Native read-only fleet ingestion. No agent turns, Python, provider launches,
//! source checkout mutations or acceptance authority are part of observation.
pub mod config;
pub mod disk_rates;
pub mod fleet;
pub mod git;
pub mod git_health;
pub mod history;
pub mod io;
pub mod logging;
pub mod operations_telemetry;
pub mod projects;
pub mod resources;
pub mod rollout;
pub mod telemetry;
pub mod usage;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub async fn cycle(config: &config::ObserveConfig) -> Result<serde_json::Value> {
    use serde_json::json;
    use std::time::Instant;
    let started = Instant::now();
    // Read the last published snapshot once: Git uses it to retain the last
    // successful facts and health uses it for alert deduplication.
    let previous = io::read_json(&config.output_dir.join("status.json"), 32 * 1024 * 1024).ok();
    // Git scans do not depend on Screen/fleet observation, so both run
    // concurrently; the cycle now costs roughly the slower branch, not the sum.
    let fleet_and_projects = async {
        let at = Instant::now();
        let observed = fleet::collect(config).await?;
        let fleet_ms = at.elapsed().as_millis();
        let screens = observed
            .get("screens")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let mut feeds = config.clone();
        // Rollout project entries classify actual workers/managers without
        // manufacturing a second task ledger from their assignment metadata.
        feeds.projects.retain(|p| p.tracker["kind"] != "rollout");
        let at = Instant::now();
        let producer_projects =
            projects::collect_with_previous(&feeds, &screens, previous.as_ref()).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            observed,
            screens,
            producer_projects,
            fleet_ms,
            at.elapsed().as_millis(),
        ))
    };
    let git_scan = async {
        let at = Instant::now();
        let repositories = git::collect_with_previous(config, previous.as_ref()).await;
        (repositories, at.elapsed().as_millis())
    };
    let (fleet_and_projects, (repositories, git_ms)) = tokio::join!(fleet_and_projects, git_scan);
    let (observed, screens, producer_projects, fleet_ms, projects_ms) = fleet_and_projects?;
    let mut snapshot = json!({"generated_utc":now_utc(),"interval_s":config.interval_seconds,
        "host":config.node_id,"projects":producer_projects,
        "order":config.projects.iter().map(|p|p.id.clone()).collect::<Vec<_>>()});
    if let Some(fields) = observed.as_object() {
        for (key, value) in fields {
            if key != "screens" {
                snapshot[key] = value.clone();
            }
        }
    }
    if let Some(rollout) = rollout::collect(config, &screens) {
        snapshot["rollout"] = rollout;
    }
    attach_hierarchy(config, &mut snapshot);
    snapshot["project_catalog"] = json!(
        config
            .projects
            .iter()
            .map(|p| json!({"id":p.id,"name":p.name}))
            .collect::<Vec<_>>()
    );
    // Disk growth needs retained samples; a failure keeps growth unknown.
    if resources::annotate(
        &config.output_dir,
        &mut snapshot["global_resources"],
        chrono::Utc::now().timestamp(),
    )
    .is_err()
        && let Some(warnings) = snapshot["global_resources"]["observation_warnings"].as_array_mut()
    {
        warnings.push(json!(
            "Disk growth samples were not persisted; growth stays unknown."
        ));
    }
    let at = Instant::now();
    let (git_health, git_health_alerts) =
        git_health::collect(config, &repositories, previous.as_ref());
    let git_metrics = git_health::metrics(&repositories, &git_health);
    let git_health_ms = at.elapsed().as_millis();
    snapshot["git"] = serde_json::to_value(repositories)?;
    snapshot["git_health"] = serde_json::to_value(git_health)?;
    snapshot["git_metrics"] = serde_json::to_value(git_metrics)?;
    snapshot["git_health_alerts"] = serde_json::to_value(git_health_alerts)?;
    let at = Instant::now();
    match usage::collect(config, &snapshot).await {
        Ok(fields) => {
            if let Some(fields) = fields.as_object() {
                for (key, value) in fields {
                    snapshot[key] = value.clone();
                }
            }
        }
        Err(_) => {
            snapshot["usage_error"] =
                json!("Native usage observation unavailable; no measurements inferred.")
        }
    }
    let usage_ms = at.elapsed().as_millis();
    let at = Instant::now();
    match history::update(config, &snapshot) {
        Ok(value) => snapshot["history"] = value,
        Err(_) => {
            snapshot["history_error"] =
                json!("Native history observation unavailable; prior history preserved.")
        }
    }
    let history_ms = at.elapsed().as_millis();
    // Fleet/projects and Git overlap, so stage times can sum past duration_ms.
    snapshot["collector"] = json!({"implementation":"rust","version":env!("CARGO_PKG_VERSION"),
        "observed_utc":now_utc(),"duration_ms":started.elapsed().as_millis(),"read_only_sources":true,
        "model_turns":0,"acceptance_authority":false,
        "stages_ms":{"fleet":fleet_ms,"projects":projects_ms,"git":git_ms,
            "git_health":git_health_ms,"usage":usage_ms,"history":history_ms},
        "stages_note":"Fleet+projects and Git scans run concurrently; stage durations overlap."});
    snapshot["collector"]["phases_ms"] = snapshot["collector"]["stages_ms"].clone();
    let summary = operations_telemetry::published(
        &snapshot,
        previous
            .as_ref()
            .and_then(|p| p["collector"]["health_digest"].as_str()),
    );
    snapshot["collector"]["health_digest"] = summary["health_digest"].clone();
    snapshot["collector"]["health_changed"] = summary["health_changed"].clone();
    let snapshot = io::public_value(snapshot);
    io::atomic_json(&config.output_dir.join("status.json"), &snapshot)?;
    telemetry::changes(previous.as_ref(), &snapshot);
    Ok(snapshot)
}

fn attach_hierarchy(config: &config::ObserveConfig, snapshot: &mut serde_json::Value) {
    use serde_json::{Value, json};
    let mut captains = Vec::new();
    for project in &config.projects {
        let rollout = &snapshot["rollout"];
        let from_rollout = snapshot["projects"].get(&project.id).is_none()
            && rollout["project"]["id"] == project.id;
        let p = if from_rollout {
            rollout
        } else {
            &snapshot["projects"][&project.id]
        };
        if !p.is_object() {
            continue;
        }
        let manager = if from_rollout {
            &p["live_observation"]["captain"]
        } else {
            &p["captain"]
        };
        let state = if manager["agent_running"] == true {
            "active"
        } else if manager["alive"] == true {
            "idle"
        } else {
            "unknown"
        };
        let mut workers = p[if from_rollout { "lanes" } else { "workers" }]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for remote in snapshot["remote_screens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["project"] == project.id)
        {
            let mut remote = remote.clone();
            remote["screen"] = remote["name"].clone();
            workers.push(remote);
        }
        // Keep all independently observed manager provenance. Rebuilding a
        // short summary used to discard effort, node and observation age.
        let mut captain = Value::Object(manager.as_object().cloned().unwrap_or_default());
        captain["key"] = json!(project.id);
        captain["name"] = json!(project.name);
        captain["screen"] = manager
            .get("id")
            .or_else(|| manager.get("name"))
            .cloned()
            .unwrap_or(Value::Null);
        captain["state"] = json!(state);
        captain["counts"] = p["counts"].clone();
        captain["done"] = p["done"].clone();
        captain["total"] = p["total"].clone();
        captain["workers"] = json!(workers);
        captains.push(captain);
    }
    let admiral = snapshot["super_captain"]["screen"].clone();
    let mut unassigned = snapshot["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["project"] == "unassigned")
        .cloned()
        .collect::<Vec<Value>>();
    unassigned.extend(
        snapshot["remote_screens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["project"] == "unassigned")
            .cloned(),
    );
    snapshot["hierarchy"] = json!({"owner":config.owner_name.as_deref().unwrap_or("Owner"),"grokbot":{"name":"Grok Bot","routines":[],"routines_observation":"unknown; routines are not measured by this collector"},"super_captain":admiral,"captains":captains,"unassigned":unassigned});
}

#[cfg(test)]
mod hierarchy_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hierarchy_retains_observed_manager_fields_and_joins_manifest_projects() {
        let config = serde_json::from_value(json!({"schema_version":1,"node_id":"local","output_dir":"/owned",
            "projects":[{"id":"ordinary","name":"Ordinary","repository":"/repo","tracker":{}},
                {"id":"release","name":"Release","repository":"/repo","tracker":{"kind":"rollout"}}]})).expect("config");
        let mut snapshot = json!({"projects":{"ordinary":{"captain":{"id":"10.manager","name":"manager","host":"local","model":"observed-model","effort":"medium","model_source":"footer","model_observed_utc":"2026-10-18T10:00:00Z","agent_running":true},"counts":{"in_progress":1},"total":2,"done":0}},
            "rollout":{"project":{"id":"release"},"total":32,"counts":{"verified":32,"in_progress":0},"live_observation":{"captain":{"id":"20.release-manager","host":"local","model":"observed-other","effort":"high","model_source":"footer","model_observed_utc":"2026-10-18T10:01:00Z","agent_running":true}},"lanes":[{"id":"one","screen":"worker","model":null,"identity_state":"not_observed"}]}});
        attach_hierarchy(&config, &mut snapshot);
        let captain = &snapshot["hierarchy"]["captains"][0];
        assert_eq!(captain["effort"], "medium");
        assert_eq!(captain["host"], "local");
        assert_eq!(captain["model_source"], "footer");
        assert_eq!(captain["model_observed_utc"], "2026-10-18T10:00:00Z");
        assert_eq!(captain["screen"], "10.manager");
        let release = &snapshot["hierarchy"]["captains"][1];
        assert_eq!(release["key"], "release");
        assert_eq!(release["model"], "observed-other");
        assert_eq!(release["effort"], "high");
        assert_eq!(release["state"], "active");
        assert_eq!(release["counts"]["verified"], 32);
        assert!(release["done"].is_null());
        assert!(release["workers"][0]["model"].is_null());
    }
}

#[cfg(test)]
mod compatibility_tests {
    #[test]
    fn retained_hierarchy_has_array_shape_without_inventing_watcher_routines() {
        let config = serde_json::from_value(serde_json::json!({
            "schema_version":1,"node_id":"n","output_dir":"/owned",
            "owner_name":"Configured Owner"
        }))
        .expect("configuration fixture");
        let mut snapshot = serde_json::json!({"projects":{},"agents":[]});
        super::attach_hierarchy(&config, &mut snapshot);
        let watcher = &snapshot["hierarchy"]["grokbot"];
        assert_eq!(watcher["name"], "Grok Bot");
        assert!(
            watcher["routines"]
                .as_array()
                .expect("retained view requires array")
                .is_empty()
        );
        assert!(
            watcher["routines_observation"]
                .as_str()
                .expect("uncertainty")
                .starts_with("unknown")
        );
        assert_eq!(snapshot["hierarchy"]["owner"], "Configured Owner");
    }
}
