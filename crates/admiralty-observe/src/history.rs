//! Actual timestamped observations for the existing thirteen temporal plots.
use crate::{config::ObserveConfig, io};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

const MAX_ROWS: usize = 4096;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 128 * 1024;
const DISPLAY_ROWS: usize = 256;
const DISPLAY_BYTES: usize = 2 * 1024 * 1024;
const DISPLAY_NODES: usize = 60_000;

fn time(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| {
        DateTime::parse_from_rfc3339(v.as_str()?)
            .ok()
            .map(|t| t.timestamp())
    })
}
fn numeric(v: &Value) -> Value {
    v.as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|_| v.clone())
        .unwrap_or(Value::Null)
}

fn observed_yield(total: &Value) -> Value {
    let mut observed = json!({});
    for (key, source) in [
        ("tok", "tokens"),
        ("int", "integration_tokens"),
        ("main", "main_tokens"),
        ("attributed", "attributed_tokens"),
    ] {
        observed[key] = numeric(&total[format!("lower_bound_{source}")]);
    }
    for (ratio, count) in [("yi", "int"), ("ym", "main"), ("cov", "attributed")] {
        observed[ratio] = observed[count]
            .as_f64()
            .zip(observed["tok"].as_f64())
            .filter(|(n, d)| *d > 0.0 && *n <= *d)
            .map(|(n, d)| json!(100.0 * n / d))
            .unwrap_or(Value::Null);
    }
    observed
}

fn load(path: &Path) -> Vec<Value> {
    if let Ok(v) = io::read_json(path, MAX_BYTES as u64)
        && let Some(rows) = v
            .get("rows")
            .and_then(Value::as_array)
            .or_else(|| v.as_array())
    {
        return rows.iter().rev().take(MAX_ROWS).cloned().collect();
    }
    let Ok(input) = io::read_text_tail(path, MAX_BYTES) else {
        return vec![];
    };
    input
        .lines()
        .filter(|s| s.len() <= MAX_ROW_BYTES)
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|v| v.is_object() && time(&v["ts"]).is_some())
        .collect()
}

/// Retain only the temporal metric contract, never arbitrary source payloads.
fn historical_row(value: &Value) -> Option<Value> {
    let at = time(&value["ts"])?;
    let numeric_map = |value: &Value| {
        value
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), numeric(v)))
                    .collect::<Map<_, _>>()
            })
            .unwrap_or_default()
    };
    let mut result = json!({"ts":at,"cost_basis":value["cost_basis"].as_str().map(io::clean),"cost_observed_basis":value["cost_observed_basis"].as_str().map(io::clean),"hygiene_note":value["hygiene_note"].as_str().map(io::clean),"tiers":numeric_map(&value["tiers"]),"hyg":numeric_map(&value["hyg"])});
    result["hyg_at"] = json!(
        value["hyg_at"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, v)| time(v)
                .filter(|t| *t > 0 && *t <= at + 5)
                .map(|t| (id.clone(), json!(t))))
            .collect::<Map<_, _>>()
    );
    for key in [
        "cost_index",
        "cost_index_observed",
        "quota_pct",
        "credits_h",
        "luna_fail_total",
        "luna_fp_rate",
    ] {
        result[key] = numeric(&value[key]);
    }
    let mut projects = Map::new();
    for (id, p) in value["proj"].as_object().into_iter().flatten() {
        let ci = p["ci"].as_str().filter(|s| {
            matches!(
                *s,
                "success" | "failure" | "pending" | "unknown" | "cancelled" | "skipped"
            )
        });
        projects.insert(id.clone(),json!({"done":numeric(&p["done"]),"total":numeric(&p["total"]),"ip":numeric(&p["ip"]),"pr":numeric(&p["pr"]),"ci":ci,"st":numeric_map(&p["st"])}));
    }
    result["proj"] = json!(projects);
    let hosts = value["hosts"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(id, h)| {
                    (
                        id.clone(),
                        json!({"load":numeric(&h["load"]),"disk":numeric(&h["disk"])}),
                    )
                })
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    result["hosts"] = json!(hosts);
    let mut ty = Map::new();
    for key in ["tok", "out", "cr", "int", "main", "yi", "ym", "cov"] {
        ty.insert(key.into(), numeric(&value["ty"][key]));
    }
    let p = value["ty"]["p"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(id, v)| {
                    (
                        id.clone(),
                        json!((0..4).map(|n| numeric(&v[n])).collect::<Vec<_>>()),
                    )
                })
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    ty.insert("p".into(), json!(p));
    ty.insert(
        "observed".into(),
        json!(
            ["tok", "int", "main", "attributed", "yi", "ym", "cov"]
                .into_iter()
                .map(|key| (key.to_owned(), numeric(&value["ty"]["observed"][key])))
                .collect::<Map<_, _>>()
        ),
    );
    result["ty"] = json!(ty);
    Some(result)
}

fn row(snapshot: &Value, at: i64) -> Value {
    let efficiency = &snapshot["token_efficiency"];
    let stats = &efficiency["ledger_stats"];
    let total = &snapshot["token_yield"]["total"];
    let mut projects = Map::new();
    let mut hosts = Map::new();
    let mut hygiene = Map::new();
    let mut hygiene_at = Map::new();
    let mut tiers = BTreeMap::<String, u64>::new();
    if let Some(source) = snapshot["projects"].as_object() {
        for (id, p) in source {
            projects.insert(id.clone(),json!({"done":numeric(&p["done"]),"total":numeric(&p["total"]),"ip":numeric(&p["counts"]["in_progress"]),"ci":p["pr"]["ci"]["state"],"pr":p["pr"]["number"],"st":p["stages"]}));
            let h = &snapshot["super_captain"]["status"]["captains"][id]["hygiene"]["score"];
            if !numeric(h).is_null() {
                hygiene.insert(id.clone(), numeric(h));
                hygiene_at.insert(id.clone(), hygiene_time(snapshot, id, at));
            }
        }
    }
    // Manifest-backed projects have their own source ledger. Join observations
    // by identity without equating verification/integration records with done.
    let rollout = &snapshot["rollout"];
    if let Some(id) = rollout["project"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
    {
        if !projects.contains_key(id) {
            projects.insert(id.into(),json!({"done":numeric(&rollout["done"]),"total":numeric(&rollout["total"]),"ip":numeric(&rollout["counts"]["in_progress"]),"ci":rollout["pr"]["ci"]["state"],"pr":rollout["pr"]["number"],"st":rollout["counts"]}));
        }
        let h = &snapshot["super_captain"]["status"]["captains"][id]["hygiene"]["score"];
        if !numeric(h).is_null() {
            hygiene.insert(id.into(), numeric(h));
            hygiene_at.insert(id.into(), hygiene_time(snapshot, id, at));
        }
    }
    for host in snapshot["global_resources"]["hosts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(id) = host["host"].as_str() else {
            continue;
        };
        let cpus = host["nproc"].as_f64().filter(|n| n.is_finite() && *n > 0.0);
        let load = host["load"][0]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .zip(cpus)
            .map(|(load, cpus)| 100.0 * load / cpus);
        let disk = host["disks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["mount"] == "/")
            .map(|d| numeric(&d["pct"]))
            .unwrap_or(Value::Null);
        hosts.insert(id.to_owned(), json!({"load":load,"disk":disk}));
    }
    for agent in snapshot["agents"].as_array().into_iter().flatten() {
        let model = agent["model"].as_str().unwrap_or("").to_ascii_lowercase();
        let family = agent["family"].as_str().unwrap_or("");
        let group = if model.contains("luna") {
            "luna"
        } else if model.contains("astra") {
            "astra"
        } else if model.contains("sol") || model.contains("terra") {
            "sol"
        } else if family == "claude" {
            "claude"
        } else if family == "agy" || family == "gemini" {
            "agy"
        } else {
            "other"
        };
        *tiers.entry(group.into()).or_default() += 1;
    }
    let project_yield = snapshot["token_yield"]["projects"]
        .as_object()
        .map(|p| {
            p.iter()
                .map(|(id, v)| {
                    (
                        id.clone(),
                        json!([
                            v["tokens"],
                            v["integration_tokens"],
                            v["main_tokens"],
                            v["credits_per_done_todo"]
                        ]),
                    )
                })
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    let hygiene_note = if hygiene.is_empty() {
        "Not reported by Admiral: current status contains no measured hygiene score. Historical reports retain their original source age."
    } else {
        "Reported Admiral hygiene scores; source timestamps are independent of fleet sampling."
    };
    json!({"ts":at,"cost_index":numeric(&efficiency["cost_index_vs_all_luna"]),"cost_index_observed":numeric(&efficiency["cost_index_observed_vs_all_luna"]),"cost_basis":efficiency["cost_index_note"],"cost_observed_basis":efficiency["cost_index_observed_note"],"tiers":tiers,"quota_pct":numeric(&efficiency["quota"]["used_percent"]),"credits_h":numeric(&efficiency["windows"]["60"]["credits_per_hour_est"]),"luna_fail_total":numeric(&stats["luna_failures_total"]),"luna_fp_rate":numeric(&stats["luna_first_pass_rate"]),"proj":projects,"hosts":hosts,"hyg":hygiene,"hyg_at":hygiene_at,"hygiene_note":hygiene_note,"ty":{"tok":numeric(&total["tokens"]),"out":numeric(&total["output"]),"cr":numeric(&total["credits"]),"int":numeric(&total["integration_tokens"]),"main":numeric(&total["main_tokens"]),"yi":numeric(&total["yield_integration_pct"]),"ym":numeric(&total["yield_main_pct"]),"cov":numeric(&total["coverage_pct"]),"observed":observed_yield(total),"p":project_yield}})
}

fn hygiene_time(snapshot: &Value, id: &str, at: i64) -> Value {
    let admiral = &snapshot["super_captain"];
    time(&admiral["status"]["captains"][id]["hygiene"]["observed_utc"])
        .or_else(|| time(&admiral["status_mtime_utc"]))
        .filter(|t| *t > 0 && *t <= at + 5)
        .map_or(Value::Null, |t| json!(t))
}

fn bounded(mut rows: Vec<Value>) -> Vec<Value> {
    rows.sort_by_key(|v| time(&v["ts"]));
    let mut bytes = 0_usize;
    let mut selected = Vec::new();
    for row in rows.into_iter().rev() {
        let Ok(encoded) = serde_json::to_vec(&row) else {
            continue;
        };
        if encoded.len() > MAX_ROW_BYTES {
            continue;
        }
        if selected.len() == MAX_ROWS || bytes + encoded.len() + 1 > MAX_BYTES {
            break;
        }
        bytes += encoded.len() + 1;
        selected.push(row);
    }
    selected.reverse();
    selected
}

/// The full retained file is larger than the dashboard's whole-response JSON
/// budget. Embed only a recent suffix, reserving room for live project records.
fn displayed(rows: &[Value]) -> Vec<Value> {
    let mut bytes = 0_usize;
    let mut nodes = 0_usize;
    let mut result = Vec::new();
    for row in rows.iter().rev().take(DISPLAY_ROWS) {
        let Ok(encoded) = serde_json::to_vec(row) else {
            break;
        };
        let mut pending = vec![row];
        let mut row_nodes = 0_usize;
        while let Some(value) = pending.pop() {
            row_nodes += 1;
            match value {
                Value::Object(values) => pending.extend(values.values()),
                Value::Array(values) => pending.extend(values),
                _ => {}
            }
        }
        if bytes + encoded.len() > DISPLAY_BYTES || nodes + row_nodes > DISPLAY_NODES {
            break;
        }
        bytes += encoded.len();
        nodes += row_nodes;
        result.push(row.clone());
    }
    result.reverse();
    result
}
fn persist(path: &Path, rows: &[Value]) -> crate::Result<()> {
    let parent = path.parent().ok_or("history output directory is missing")?;
    let temporary = parent.join(format!(".history-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> crate::Result<()> {
        let mut file = options.open(&temporary)?;
        for row in rows {
            serde_json::to_writer(&mut file, row)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn update(config: &ObserveConfig, snapshot: &Value) -> crate::Result<Value> {
    update_at(config, snapshot, Utc::now().timestamp())
}
fn update_at(config: &ObserveConfig, snapshot: &Value, now: i64) -> crate::Result<Value> {
    let destination = config.output_dir.join("history.jsonl");
    let initialize = match fs::symlink_metadata(&destination) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => return Err("history destination metadata is unavailable".into()),
    };
    let mut source_rows = Vec::new();
    // Migration is a one-time seed. An independently running old producer must
    // never move the native sampling clock or overwrite native rows afterward.
    if initialize && let Some(source) = &config.history_source {
        let path = if source.is_dir() {
            source.join("history.jsonl")
        } else {
            source.clone()
        };
        if path != destination {
            source_rows.extend(load(&path));
        }
    }
    source_rows.extend(load(&destination));
    let mut unique = BTreeMap::new();
    for v in source_rows.into_iter().filter_map(|v| historical_row(&v)) {
        if let Some(at) = time(&v["ts"]).filter(|at| *at > 0 && *at <= now + 5) {
            unique.insert(at, v);
        }
    }
    let at = snapshot.get("generated_utc").and_then(time);
    let interval = config.interval_seconds.clamp(110, 86_400);
    let append = at
        .filter(|at| *at > 0 && *at <= now + 5 && now.saturating_sub(*at) <= interval as i64 + 30)
        .filter(|at| {
            unique
                .last_key_value()
                .is_none_or(|(last, _)| at.saturating_sub(*last) >= interval as i64)
        });
    if let Some(at) = append {
        let metric_row =
            historical_row(&row(snapshot, at)).ok_or("invalid fresh history timestamp")?;
        unique.insert(at, metric_row);
    }
    let rows = bounded(unique.into_values().collect());
    // Persist imported timestamps once, and fresh rows only at the bounded
    // interval. Repeated polls and process restarts do not invent observations.
    if append.is_some() || initialize {
        persist(&destination, &rows)?;
    }
    let visible = displayed(&rows);
    Ok(
        json!({"rows":visible,"retained_rows":rows.len(),"displayed_rows":visible.len(),"omitted_rows":rows.len().saturating_sub(visible.len()),"note":"Actual producer timestamps preserved; fresh native observations at >=110-second intervals. The private file retains up to4096 rows/8MiB; the dashboard embeds only the newest suffix, at most256 rows/2MiB/60000 JSON nodes, reserving room for live observations. Null metrics stay unknown. Git token attribution is an estimate, never task acceptance.","file":destination}),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn partial_usage_history_retains_observations_without_claiming_full_totals() {
        let snapshot = json!({"generated_utc":"1970-01-01T00:05:10Z",
            "projects":{"p":{}},
            "token_efficiency":{"cost_index_vs_all_luna":null,"cost_index_observed_vs_all_luna":4.123456789,"cost_index_observed_note":"Observed sample only"},
            "token_yield":{"total":{"tokens":null,"yield_main_pct":null,
                "lower_bound_tokens":300,"lower_bound_integration_tokens":150,
                "lower_bound_main_tokens":100,"lower_bound_attributed_tokens":225}}});
        let v = historical_row(&row(&snapshot, 310)).unwrap();
        assert!(v["cost_index"].is_null());
        assert_eq!(v["cost_index_observed"], 4.123456789);
        assert_eq!(v["cost_observed_basis"], "Observed sample only");
        assert!(v["ty"]["tok"].is_null());
        assert!(v["ty"]["ym"].is_null());
        assert_eq!(v["ty"]["observed"]["tok"], 300);
        assert_eq!(v["ty"]["observed"]["yi"], 50.0);
        assert_eq!(v["ty"]["observed"]["ym"], 100.0 / 3.0);
        assert_eq!(v["ty"]["observed"]["cov"], 75.0);
        assert!(v["hyg"].as_object().unwrap().is_empty());
        assert!(
            v["hygiene_note"]
                .as_str()
                .unwrap()
                .contains("Not reported by Admiral")
        );
    }
    #[test]
    fn observed_yield_zero_missing_and_invalid_denominators_remain_unknown() {
        for total in [
            json!({}),
            json!({"lower_bound_tokens":0,"lower_bound_main_tokens":0}),
            json!({"lower_bound_tokens":10,"lower_bound_main_tokens":11}),
        ] {
            assert!(observed_yield(&total)["ym"].is_null());
        }
        assert_eq!(
            observed_yield(&json!({"lower_bound_tokens":10,"lower_bound_main_tokens":0}))["ym"],
            0.0
        );
    }
    #[test]
    fn hygiene_history_keeps_report_clock_and_does_not_relabel_missing_scores() {
        let snapshot = json!({"projects":{"p":{}},"super_captain":{
            "status_mtime_utc":"1970-01-01T00:03:00Z","status":{"captains":{"p":{
                "hygiene":{"score":87.654321,"observed_utc":"1970-01-01T00:02:00Z"}}}}}});
        let v = historical_row(&row(&snapshot, 310)).unwrap();
        assert_eq!(v["hyg"]["p"], 87.654321);
        assert_eq!(v["hyg_at"]["p"], 120);
        let sanitized = historical_row(
            &json!({"ts":310,"hyg_at":{"p":999},"ty":{"observed":{"tok":5,"private":"secret"}}}),
        )
        .unwrap();
        assert!(sanitized["hyg_at"]["p"].is_null());
        assert!(!sanitized.to_string().contains("secret"));
        let missing = historical_row(&row(&json!({"projects":{"p":{}}}), 420)).unwrap();
        assert!(missing["hyg"]["p"].is_null());
        assert!(missing["ty"]["observed"]["tok"].is_null());
    }
    fn config(root: &Path) -> ObserveConfig {
        serde_json::from_value(json!({"schema_version":1,"node_id":"node","output_dir":root,"interval_seconds":15,"projects":[],"nodes":[],"coordination_dir":null,"rollout_manifest":null,"codex_sessions":null,"claude_projects":null,"rates_dir":null,"history_source":null,"admiral_screen":null,"gh_program":null})).unwrap()
    }
    #[test]
    fn history_preserves_timestamps_nulls_and_restart_dedup() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = config(temp.path());
        let source = temp.path().join("old.jsonl");
        fs::write(
            &source,
            "{\"ts\":100,\"cost_index\":null,\"proj\":{}}\n{\"ts\":200,\"cost_index\":2.5}\n",
        )
        .unwrap();
        c.history_source = Some(source);
        let snapshot = json!({"generated_utc":"1970-01-01T00:05:00Z","projects":{"p":{"total":10}},"agents":[]});
        let first = update_at(&c, &snapshot, 300).unwrap();
        assert_eq!(first["rows"].as_array().unwrap().len(), 2); // Only100sec since200: no fabricated sample.
        let snapshot = json!({"generated_utc":"1970-01-01T00:05:10Z","projects":{"p":{"total":10}},"agents":[]});
        let second = update_at(&c, &snapshot, 310).unwrap();
        assert_eq!(second["rows"].as_array().unwrap().len(), 3);
        assert_eq!(second["rows"][0]["ts"], 100);
        assert!(second["rows"][0]["cost_index"].is_null());
        assert!(second["rows"][2]["proj"]["p"]["done"].is_null());
        assert!(second["rows"][2]["ty"]["main"].is_null());
        assert_eq!(
            update_at(&c, &snapshot, 311).unwrap()["rows"],
            second["rows"]
        );
        assert_eq!(
            update_at(&c, &json!({"generated_utc":"1970-01-01T00:05:10Z"}), 1000).unwrap()["rows"],
            second["rows"]
        );
    }
    #[test]
    fn external_history_is_imported_once_and_cannot_suppress_native_samples_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = config(temp.path());
        let source = temp.path().join("live-old-history.jsonl");
        fs::write(
            &source,
            "{\"ts\":100,\"cost_index\":1}\n{\"ts\":200,\"cost_index\":2}\n",
        )
        .unwrap();
        c.history_source = Some(source.clone());
        let seed = update_at(&c, &json!({"generated_utc":"1970-01-01T00:05:00Z"}), 300).unwrap();
        assert_eq!(seed["rows"].as_array().unwrap().len(), 2);
        assert_eq!(seed["rows"][0]["ts"], 100);
        assert_eq!(seed["rows"][1]["ts"], 200);

        // The old producer keeps writing. Neither its later sample nor a
        // changed earlier metric may enter the already initialized native file.
        fs::write(&source,"{\"ts\":100,\"cost_index\":999}\n{\"ts\":200,\"cost_index\":999}\n{\"ts\":309,\"cost_index\":999}\n").unwrap();
        let native = update_at(&c,&json!({"generated_utc":"1970-01-01T00:05:10Z","token_efficiency":{"cost_index_vs_all_luna":3}}),310).unwrap();
        let rows = native["rows"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|v| v["ts"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![100, 200, 310]
        );
        assert_eq!(rows[0]["cost_index"], 1);
        assert_eq!(rows[2]["cost_index"], 3);

        fs::write(
            &source,
            "{\"ts\":310,\"cost_index\":888}\n{\"ts\":419,\"cost_index\":888}\n",
        )
        .unwrap();
        let mut restarted = config(temp.path());
        restarted.history_source = Some(source);
        let snapshot = json!({"generated_utc":"1970-01-01T00:07:00Z","token_efficiency":{"cost_index_vs_all_luna":4}});
        let after_restart = update_at(&restarted, &snapshot, 420).unwrap();
        let rows = after_restart["rows"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|v| v["ts"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![100, 200, 310, 420]
        );
        assert_eq!(rows[2]["cost_index"], 3);
        assert_eq!(rows[3]["cost_index"], 4);
        assert_eq!(
            update_at(&restarted, &snapshot, 421).unwrap()["rows"],
            after_restart["rows"]
        );
    }
    #[test]
    fn all_plot_contracts_keep_unknown_values_and_bound_rows() {
        let value = row(
            &json!({"projects":{"p":{}},"global_resources":{"hosts":[{"host":"node","load":[2.0],"nproc":4,"disks":[{"mount":"/","pct":35.0}]}]},"agents":[{"family":"codex","model":"gpt-6-luna"}]}),
            42,
        );
        assert_eq!(value["hosts"]["node"]["load"], 50.0);
        assert_eq!(value["hosts"]["node"]["disk"], 35.0);
        assert_eq!(value["tiers"]["luna"], 1);
        assert!(value["quota_pct"].is_null());
        assert!(value["hyg"].as_object().unwrap().is_empty());
        let rows = bounded((1..5000).map(|ts| json!({"ts":ts})).collect());
        assert_eq!(rows.len(), 4096);
        assert_eq!(rows.last().unwrap()["ts"], 4999);
    }
    #[test]
    fn manifest_project_history_keeps_recorded_checks_distinct_from_done() {
        let observed = row(
            &json!({"projects":{},"rollout":{"project":{"id":"release"},"total":32,"counts":{"verified":32,"in_progress":0},"pr":{"number":8,"ci":{"state":"pending"}}}}),
            42,
        );
        assert_eq!(observed["proj"]["release"]["total"], 32);
        assert_eq!(observed["proj"]["release"]["ip"], 0);
        assert_eq!(observed["proj"]["release"]["st"]["verified"], 32);
        assert_eq!(observed["proj"]["release"]["ci"], "pending");
        assert!(observed["proj"]["release"]["done"].is_null());
        let regular = row(
            &json!({"projects":{"release":{"total":47,"done":1}},"rollout":{"project":{"id":"release"},"total":32,"counts":{"verified":32}}}),
            43,
        );
        assert_eq!(regular["proj"]["release"]["total"], 47);
        assert_eq!(regular["proj"]["release"]["done"], 1);
    }
    #[test]
    fn retained_history_uses_bounded_recent_display_without_changing_timestamps() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = config(temp.path());
        let source = temp.path().join("old.jsonl");
        let rows = (1..=4096).map(|ts| json!({"ts":ts,"proj":{"release":{"total":32,"done":null,"ip":0,"st":{"verified":32}}}}).to_string()).collect::<Vec<_>>().join("\n");
        fs::write(&source, rows).unwrap();
        c.history_source = Some(source);
        let result = update_at(&c, &json!({}), 5000).unwrap();
        assert_eq!(result["retained_rows"], 4096);
        assert_eq!(result["displayed_rows"], 256);
        assert_eq!(result["omitted_rows"], 3840);
        assert_eq!(result["rows"][0]["ts"], 3841);
        assert_eq!(result["rows"][255]["ts"], 4096);
        assert_eq!(load(&temp.path().join("history.jsonl")).len(), 4096);
        let wide = vec![json!({"ts":1,"values":vec![0;1000]}); 256];
        assert!(displayed(&wide).len() < 256);
    }
    #[test]
    fn imported_history_preserves_metric_timestamp_but_never_unrelated_payloads() {
        let value=historical_row(&json!({"ts":42,"cost_index":2.0,"prompt":"private prompt","api_key":"private credential","cost_basis":"Authorization: Bearer private","ty":{"tok":100,"prompt":"not a metric"},"proj":{"p":{"done":3,"total":10,"ip":2,"ci":"success","prompt":"not a metric"}}})).unwrap();
        let serialized = value.to_string();
        assert!(!serialized.contains("private prompt"));
        assert!(!serialized.contains("private credential"));
        assert!(!serialized.contains("Bearer private"));
        assert!(!serialized.contains("not a metric"));
        assert_eq!(value["ts"], 42);
        assert_eq!(value["ty"]["tok"], 100);
        assert_eq!(value["proj"]["p"]["done"], 3);
    }
}
