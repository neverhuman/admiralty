//! Native read-only fleet ingestion. No agent turns, Python, provider launches,
//! source checkout mutations or acceptance authority are part of observation.
pub mod cli;
pub mod config;
pub mod fleet;
pub mod git;
pub mod git_health;
pub mod history;
pub mod io;
#[cfg(test)]
mod legacy_cases;
pub mod projects;
pub mod rollout;
pub mod usage;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub async fn cycle(config: &config::ObserveConfig) -> Result<serde_json::Value> {
    use serde_json::json;
    let started = std::time::Instant::now();
    let observed = fleet::collect(config).await?;
    let screens = observed
        .get("screens")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let mut feeds = config.clone();
    // Rollout project entries classify actual workers/managers without
    // manufacturing a second task ledger from their assignment metadata.
    feeds.projects.retain(|p| p.tracker["kind"] != "rollout");
    let producer_projects = projects::collect(&feeds, &screens).await?;
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
    let previous = io::read_json(&config.output_dir.join("status.json"), 32 * 1024 * 1024).ok();
    let repositories = git::collect(config).await;
    let (git_health, git_health_alerts) =
        git_health::collect(config, &repositories, previous.as_ref());
    snapshot["git"] = serde_json::to_value(repositories)?;
    snapshot["git_health"] = serde_json::to_value(git_health)?;
    snapshot["git_health_alerts"] = serde_json::to_value(git_health_alerts)?;
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
    match history::update(config, &snapshot) {
        Ok(value) => snapshot["history"] = value,
        Err(_) => {
            snapshot["history_error"] =
                json!("Native history observation unavailable; prior history preserved.")
        }
    }
    snapshot["collector"] = json!({"implementation":"rust","version":env!("CARGO_PKG_VERSION"),
        "observed_utc":now_utc(),"duration_ms":started.elapsed().as_millis(),"read_only_sources":true,
        "model_turns":0,"acceptance_authority":false});
    let snapshot = io::public_value(snapshot);
    io::atomic_json(&config.output_dir.join("status.json"), &snapshot)?;
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
mod legacy_parity_tests {
    use super::*;
    use serde_json::json;

    fn config(projects: serde_json::Value) -> config::ObserveConfig {
        serde_json::from_value(
            json!({"schema_version":1,"node_id":"local","output_dir":"/owned","projects":projects}),
        )
        .expect("observer configuration fixture")
    }

    /// Parity with tests/legacy/test_rollout_live_workers.py::test_extras_merge_keeps_producer_records_and_actual_worker_state
    /// (adapted): a rollout manifest sharing a producer project's id never
    /// replaces the producer's manager, workers or counts. The worker_state
    /// half does not apply: the Rust observer publishes `state: "unknown"`
    /// instead of inferring active/idle/finished from screen changes.
    #[test]
    fn extras_merge_keeps_producer_records_and_actual_worker_state() {
        let config = config(
            json!([{"id":"custom-build","name":"Custom build","repository":"/repo","tracker":{}}]),
        );
        let mut snapshot = json!({"projects":{"custom-build":{"captain":{"id":"10.captain_custom","name":"captain_custom","agent_running":true},
                "counts":{"done":32},"total":40,"done":32,"workers":[{"id":"11.worker","name":"worker"}]}},
            "rollout":{"project":{"id":"custom-build"},"counts":{"claimed":1},"total":1,
                "live_observation":{"captain":{"id":"201.custom_captain","agent_running":true}},
                "lanes":[{"id":"worker-one","screen":"custom_worker"}]}});
        let producer = snapshot["projects"].clone();
        attach_hierarchy(&config, &mut snapshot);
        let captain = &snapshot["hierarchy"]["captains"][0];
        assert_eq!(captain["screen"], "10.captain_custom");
        assert_eq!(captain["state"], "active");
        assert_eq!(captain["counts"]["done"], 32);
        assert_eq!(
            captain["workers"],
            json!([{"id":"11.worker","name":"worker"}])
        );
        assert_eq!(snapshot["projects"], producer);
    }

    /// Parity with tests/legacy/test_rollout_live_workers.py::test_cycle_loads_manifest_and_joins_before_extras_without_adding_projects
    /// (adapted): joining the manifest adds a hierarchy entry for its configured
    /// project but never adds or rewrites producer projects or their counts.
    #[test]
    fn cycle_loads_manifest_and_joins_before_extras_without_adding_projects() {
        let mut projects = ["alpha", "beta", "gamma", "delta"]
            .iter()
            .map(|id| json!({"id":id,"name":id,"repository":"/repo","tracker":{}}))
            .collect::<Vec<_>>();
        projects.push(json!({"id":"custom-build","name":"Custom build","repository":"/repo","tracker":{"kind":"rollout"}}));
        let config = config(json!(projects));
        let producers = ["alpha", "beta", "gamma", "delta"]
            .iter()
            .enumerate()
            .map(|(index, id)| {
                (
                    (*id).to_owned(),
                    json!({"captain":{"id":format!("{}.captain_{id}", index + 10)},"counts":{"done":32}}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let rollout_counts = json!({"claimed":1,"verified":0});
        let mut snapshot = json!({"projects":producers,"rollout":{"project":{"id":"custom-build"},
            "counts":rollout_counts,"total":1,"live_observation":{"captain":null},"lanes":[]}});
        let before = snapshot["projects"].clone();
        attach_hierarchy(&config, &mut snapshot);
        assert_eq!(snapshot["projects"], before);
        let captains = snapshot["hierarchy"]["captains"]
            .as_array()
            .expect("hierarchy captains");
        assert_eq!(captains.len(), 5);
        for captain in &captains[..4] {
            assert_eq!(captain["counts"]["done"], 32);
        }
        assert_eq!(captains[4]["key"], "custom-build");
        assert_eq!(captains[4]["counts"], rollout_counts);
        assert_eq!(captains[4]["state"], "unknown");
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
