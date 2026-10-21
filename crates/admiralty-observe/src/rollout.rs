//! Observe packet manifests without turning receipt strings into authority.
use crate::{config::ObserveConfig, io, now_utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
mod legacy_parity;
const MAX_BYTES: u64 = 1024 * 1024;
const STAGES: [&str; 8] = [
    "planned",
    "ready",
    "claimed",
    "in_progress",
    "blocked",
    "review",
    "verified",
    "integrated",
];
fn text(value: &Value, max: usize) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(max)
        .collect()
}
fn reference(value: &Value, keys: &[&str]) -> bool {
    keys.iter().all(|key| !text(&value[key], 1000).is_empty())
        && value["observed_utc"]
            .as_str()
            .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
}
/// Only absolute http(s) links with a host and no embedded credentials are
/// published; anything else (script/data URLs, scheme-relative or userinfo
/// links) becomes empty text rather than a clickable or credential-bearing URL.
fn safe_url(value: &Value) -> String {
    let url = text(value, 2048);
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return String::new();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    };
    if host.is_empty()
        || authority.contains('@')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return String::new();
    }
    url
}
/// A screen the fleet observer already attributed to another configured
/// project cannot be adopted by manifest text.
fn foreign_project(screen: &Value, project: &str) -> bool {
    screen["project"]
        .as_str()
        .is_some_and(|owner| owner != "unassigned" && owner != project)
}
fn identity_pins_match(value: &Value, keys: &[&str], observed: Option<u64>) -> bool {
    observed.is_some_and(|pid| {
        pid > 1
            && keys.iter().all(|key| {
                value
                    .get(key)
                    .is_none_or(|pin| pin.is_null() || pin.as_u64() == Some(pid))
            })
    })
}
pub fn collect(config: &ObserveConfig, screens: &Value) -> Option<Value> {
    let path = config.rollout_manifest.as_ref()?;
    let mut source = json!({"filename":path.file_name().and_then(|s|s.to_str()),"loaded_utc":now_utc(),"mtime_utc":io::mtime_utc(path)});
    let result = (|| -> crate::Result<Value> {
        let bytes = io::read_bytes(path, MAX_BYTES)?;
        source["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        let mut raw: Value = serde_json::from_slice(&bytes)?;
        // Assigning a field on a non-object JSON value would panic.
        if !raw.is_object() {
            return Err("rollout manifest must be a JSON object".into());
        }
        if let Some(lanes_file) = &config.rollout_lanes_file {
            let lanes = io::read_json(lanes_file, MAX_BYTES)?;
            if !lanes["lanes"].is_array() {
                return Err("configured lane assignments are invalid".into());
            }
            raw["lanes"] = lanes["lanes"].clone();
        }
        normalize(
            raw,
            source.clone(),
            screens,
            &config.node_id,
            chrono::Utc::now(),
        )
    })();
    Some(result.unwrap_or_else(|_|json!({"state":"invalid","read_only":true,"source":source,"error":"Configured rollout manifest is unavailable or invalid; no packet state inferred."})))
}
fn normalize(
    raw: Value,
    source: Value,
    screens: &Value,
    node: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> crate::Result<Value> {
    if raw["schema_version"] != 1 || text(&raw["project"]["id"], 100).is_empty() {
        return Err("invalid manifest identity".into());
    }
    let project_id = text(&raw["project"]["id"], 100);
    let packets = raw["packets"].as_array().ok_or("missing packets")?;
    // Absent lanes are optional; a present non-list value is a type error.
    let lanes = match raw.get("lanes") {
        None => Vec::new(),
        Some(Value::Array(lanes)) => lanes.clone(),
        Some(_) => return Err("lanes must be a list".into()),
    };
    if packets.len() > 2000 || lanes.len() > 200 {
        return Err("manifest bounds exceeded".into());
    }
    let updated = chrono::DateTime::parse_from_rfc3339(
        raw["updated_utc"]
            .as_str()
            .ok_or("missing observation timestamp")?,
    )?;
    let age = now.signed_duration_since(updated).num_seconds();
    // Booleans, floats and strings are type errors, never the default.
    let threshold = match raw.get("stale_after_s") {
        None => 300,
        Some(value) => value.as_i64().ok_or("invalid stale threshold")?,
    };
    if !(30..=86400).contains(&threshold) {
        return Err("invalid stale threshold".into());
    }
    let mut counts: BTreeMap<&str, u64> = STAGES.iter().map(|s| (*s, 0)).collect();
    let mut normalized = Vec::new();
    let mut ids = BTreeSet::new();
    for packet in packets {
        let id = text(&packet["id"], 100);
        if id.is_empty() || !ids.insert(id.clone()) {
            return Err("invalid packet identity".into());
        }
        let reported = text(&packet["status"], 40);
        let mut stage = if reported == "active" {
            "in_progress"
        } else {
            reported.as_str()
        };
        if !STAGES.contains(&stage) {
            return Err("invalid packet stage".into());
        }
        let verified = reference(
            &packet["verification"],
            &["receipt", "revision", "verifier", "observed_utc"],
        );
        let integrated = reference(
            &packet["integration"],
            &["receipt", "revision", "target_ref", "observed_utc"],
        );
        let mut warnings = Vec::new();
        if matches!(stage, "verified" | "integrated") && !verified {
            stage = "claimed";
            warnings.push("Verification provenance missing; displayed as claimed.");
        }
        if stage == "integrated" && !integrated {
            stage = "verified";
            warnings.push("Integration provenance missing; displayed as verification recorded.");
        }
        if verified
            && packet["candidate_revision"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && packet["candidate_revision"] != packet["verification"]["revision"]
        {
            if matches!(stage, "verified" | "integrated") {
                stage = "claimed"
            }
            warnings.push("Verification revision differs from candidate revision.");
        }
        if verified
            && integrated
            && packet["verification"]["revision"] != packet["integration"]["revision"]
        {
            if stage == "integrated" {
                stage = "verified"
            }
            warnings.push("Integration revision differs from verification revision.");
        }
        if verified
            && packet["claim"]["owner"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && packet["claim"]["owner"] == packet["verification"]["verifier"]
        {
            if matches!(stage, "verified" | "integrated") {
                stage = "claimed"
            }
            warnings.push("Verifier matches claimant; independent acceptance is not established.");
        }
        *counts.get_mut(stage).ok_or("unsupported stage")? += 1;
        let mut p = json!({"id":id,"title":text(&packet["title"],500),"status":stage,"reported_status":reported,"warnings":warnings});
        for key in [
            "depends_on",
            "claim",
            "candidate_revision",
            "evidence",
            "capsule_path",
            "lane",
        ] {
            p[key] = packet[key].clone();
        }
        p["verification"] = if verified {
            packet["verification"].clone()
        } else {
            Value::Null
        };
        p["integration"] = if integrated {
            packet["integration"].clone()
        } else {
            Value::Null
        };
        normalized.push(p);
    }
    let inventory = screens.as_array().cloned().unwrap_or_default();
    let mut observed_lanes = Vec::new();
    let mut adopted = BTreeMap::<String, usize>::new();
    let mut lane_ids = BTreeSet::new();
    for lane in &lanes {
        let id = text(&lane["id"], 200);
        if id.is_empty() || !lane_ids.insert(id.clone()) {
            return Err("invalid lane identity".into());
        }
        let screen = text(&lane["screen"], 200);
        let host = text(&lane["node"], 100);
        let candidates = inventory
            .iter()
            .filter(|s| {
                (host.is_empty() || host == node)
                    && s["host"] == node
                    && (s["id"] == screen || s["name"] == screen)
            })
            .collect::<Vec<_>>();
        // Raw lane reports carry `updated_utc`; observer-merged manifests carry
        // `source_updated_utc`. Either is reported source time, as text only.
        let source_time = ["updated_utc", "source_updated_utc"]
            .iter()
            .map(|key| text(&lane[*key], 200))
            .find(|value| !value.is_empty())
            .map_or(Value::Null, Value::from);
        let base = json!({"id":id,"screen":screen,"node":host,"branch":text(&lane["branch"],200),"packet_id":text(&lane["packet_id"],200),"source_updated_utc":source_time,
            "status":"unknown","reported_status":text(&lane["status"],40),"model":"unknown","effort":null,"tier":null,"identity_state":"not_observed","progress_observed":false});
        let mut result = base.clone();
        let mut adopted_id = None;
        if candidates.len() == 1 {
            let found = candidates[0];
            let pid = found["pid"].as_u64();
            if foreign_project(found, &project_id) {
                result["identity_state"] = json!("producer_collision");
            } else if identity_pins_match(lane, &["screen_pid", "reported_screen_pid"], pid) {
                result["screen"] = found["name"].clone();
                result["screen_id"] = found["id"].clone();
                result["screen_pid"] = found["pid"].clone();
                result["identity_state"] = json!("observed");
                for key in ["model", "effort", "model_observed_utc", "agent_running"] {
                    result[key] = found[key].clone();
                }
                let screen_id = found["id"].to_string();
                *adopted.entry(screen_id.clone()).or_default() += 1;
                adopted_id = Some(screen_id);
            } else {
                result["identity_state"] = json!("identity_mismatch");
            }
        }
        observed_lanes.push((base, adopted_id, result));
    }
    // One observed screen cannot be two lanes: aliases adopt nothing.
    let observed_lanes = observed_lanes
        .into_iter()
        .map(|(mut base, adopted_id, result)| {
            if adopted_id.is_some_and(|id| adopted.get(&id).is_some_and(|n| *n > 1)) {
                base["identity_state"] = json!("aliased");
                base
            } else {
                result
            }
        })
        .collect::<Vec<_>>();
    let manager = text(&raw["project"]["captain_screen"], 200);
    let matches = inventory
        .iter()
        .filter(|s| s["host"] == node && (s["id"] == manager || s["name"] == manager))
        .collect::<Vec<_>>();
    let mut captain = Value::Null;
    if matches.len() == 1 {
        let observed = matches[0];
        let pid = observed["pid"].as_u64();
        if !foreign_project(observed, &project_id)
            && identity_pins_match(
                &raw["project"],
                &["captain_screen_pid", "reported_captain_screen_pid"],
                pid,
            )
        {
            captain = observed.clone();
            captain["identity_state"] = json!("observed");
        }
    }
    let mut project = raw["project"].clone();
    if project.get("repository_url").is_some() {
        project["repository_url"] = json!(safe_url(&project["repository_url"]));
    }
    let mut pr = raw["pr"].clone();
    if let Some(fields) = pr.as_object_mut()
        && let Some(url) = fields.get_mut("url")
    {
        *url = json!(safe_url(url));
    }
    Ok(
        json!({"schema_version":1,"state":"ready","read_only":true,"project":project,"updated_utc":raw["updated_utc"],"producer":raw["producer"],"source":source,
        "freshness":{"state":if age < -30{"future"}else if age>threshold{"stale"}else{"fresh"},"age_s":age,"stale_after_s":threshold},
        "packets":normalized,"lanes":observed_lanes,"counts":counts,"total":packets.len(),"pr":pr,
        "live_observation":{"source":"rust_screen_process_observation/v1","captain":captain,"observed_utc":now_utc()},
        "assurance":"Manifest observation only. Receipt references are not independently validated by this collector."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integration_claims_require_actual_provenance_and_never_become_accepted() {
        let raw = json!({"schema_version":1,"project":{"id":"example"},"updated_utc":now_utc(),"packets":[{"id":"P0","status":"integrated","verification":{"receipt":"path","revision":"a","verifier":"reviewer","observed_utc":now_utc()}}]});
        let v =
            normalize(raw, json!({}), &json!([]), "node", chrono::Utc::now()).expect("normalize");
        assert_eq!(v["counts"]["verified"], 1);
        assert_eq!(v["counts"]["integrated"], 0);
        assert_eq!(
            v["packets"][0]["warnings"]
                .as_array()
                .expect("warnings")
                .len(),
            1
        );
    }
    #[test]
    fn source_time_and_unknown_captain_are_preserved() {
        let raw = json!({"schema_version":1,"project":{"id":"example","captain_screen":"manager"},"updated_utc":"2020-01-01T00:00:00Z","packets":[]});
        let v =
            normalize(raw, json!({}), &json!([]), "node", chrono::Utc::now()).expect("normalize");
        assert_eq!(v["freshness"]["state"], "stale");
        assert_eq!(v["updated_utc"], "2020-01-01T00:00:00Z");
        assert!(v["live_observation"]["captain"].is_null());
    }
    #[test]
    fn lane_claims_never_override_observed_models_or_reused_screen_identity() {
        let mut raw = json!({"schema_version":1,"project":{"id":"example"},"updated_utc":now_utc(),"packets":[],
            "lanes":[{"id":"worker","screen":"worker","node":"node","screen_pid":123,"model":"claimed-model","status":"done"}]});
        let screens = json!([{"host":"node","id":"123.worker","name":"worker","pid":123,"model":"observed-model","agent_running":true,"model_observed_utc":"2026-10-18T22:00:00Z"}]);
        let observed = normalize(raw.clone(), json!({}), &screens, "node", chrono::Utc::now())
            .expect("valid fixture");
        assert_eq!(observed["lanes"][0]["model"], "observed-model");
        assert_eq!(observed["lanes"][0]["status"], "unknown");
        assert_eq!(observed["lanes"][0]["progress_observed"], false);
        raw["lanes"][0]["screen_pid"] = json!(122);
        let reused = normalize(raw.clone(), json!({}), &screens, "node", chrono::Utc::now())
            .expect("valid fixture");
        assert_eq!(reused["lanes"][0]["identity_state"], "identity_mismatch");
        assert_eq!(reused["lanes"][0]["model"], "unknown");
        raw["project"]["captain_screen"] = json!("worker");
        for pin in [json!("123"), json!(-1), json!(123.5), json!({"pid":123})] {
            raw["lanes"][0]["screen_pid"] = pin.clone();
            raw["project"]["captain_screen_pid"] = pin;
            let malformed = normalize(raw.clone(), json!({}), &screens, "node", chrono::Utc::now())
                .expect("valid fixture");
            assert_eq!(malformed["lanes"][0]["identity_state"], "identity_mismatch");
            assert_eq!(malformed["lanes"][0]["model"], "unknown");
            assert!(malformed["live_observation"]["captain"].is_null());
        }
    }
}
