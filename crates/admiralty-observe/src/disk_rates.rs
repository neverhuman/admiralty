//! Disk growth and time-to-full from the observer's own bounded samples.
//!
//! Native parity for the retired `dashboard/globalres.py` estimate: each disk's
//! used bytes are sampled every cycle and kept for six hours in a private state
//! file. The rate uses the oldest sample that is between 20 minutes and 3 hours
//! old, so a single noisy cycle cannot produce a projection. Without such a
//! baseline every derived field stays null rather than zero.
//!
//! The projection is linear over the observed window. It is advisory resource
//! information, not a reservation, enforcement decision or capacity guarantee.
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub const STATE_FILE: &str = "disk-rates-v1.json";
const STATE_BYTES: u64 = 1024 * 1024;
const RETAIN_S: f64 = 6.0 * 3600.0;
const MIN_BASE_S: f64 = 20.0 * 60.0;
const MAX_BASE_S: f64 = 3.0 * 3600.0;
const MAX_SERIES: usize = 256;
const MAX_SAMPLES: usize = 2048;
const RED_HOURS: f64 = 12.0;
const AMBER_HOURS: f64 = 24.0;

type Series = BTreeMap<String, Vec<(f64, u64)>>;

fn load(path: &Path) -> Series {
    let Ok(value) = crate::io::read_json(path, STATE_BYTES) else {
        return Series::new();
    };
    let mut series = Series::new();
    for (key, samples) in value["series"].as_object().into_iter().flatten() {
        let parsed = samples
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| Some((s.get(0)?.as_f64()?, s.get(1)?.as_u64()?)))
            .filter(|(at, _)| at.is_finite())
            .collect::<Vec<_>>();
        series.insert(key.clone(), parsed);
    }
    series
}

fn level(pct: f64, hours_to_full: Option<f64>) -> &'static str {
    if pct >= 92.0 || hours_to_full.is_some_and(|h| h < RED_HOURS) {
        "red"
    } else if pct >= 85.0 || hours_to_full.is_some_and(|h| h < AMBER_HOURS) {
        "amber"
    } else {
        "ok"
    }
}

/// Update every reachable host's disks in place and persist the samples.
/// A state read failure starts a new window; a write failure only means the
/// next cycle cannot reuse this sample. Neither invents a rate.
pub fn apply(output_dir: &Path, hosts: &mut [Value], now_s: f64) -> bool {
    let path = output_dir.join(STATE_FILE);
    let mut series = load(&path);
    let mut seen = Vec::new();
    for host in hosts.iter_mut() {
        let Some(name) = host["host"].as_str().map(str::to_owned) else {
            continue;
        };
        let mut disk_warnings = Vec::new();
        for disk in host["disks"].as_array_mut().into_iter().flatten() {
            let (Some(mount), Some(used), Some(avail), Some(pct)) = (
                disk["mount"].as_str().map(str::to_owned),
                disk["used"].as_u64(),
                disk["avail"].as_u64(),
                disk["pct"].as_f64(),
            ) else {
                continue;
            };
            let key = format!("{name}:{mount}");
            let samples = series.entry(key.clone()).or_default();
            samples.retain(|(at, _)| now_s - at < RETAIN_S && *at <= now_s);
            samples.push((now_s, used));
            if samples.len() > MAX_SAMPLES {
                let excess = samples.len() - MAX_SAMPLES;
                *samples = samples.split_off(excess);
            }
            seen.push(key);
            let base = samples
                .iter()
                .find(|(at, _)| (MIN_BASE_S..=MAX_BASE_S).contains(&(now_s - at)))
                .copied();
            let rate =
                base.map(|(at, before)| (used as f64 - before as f64) / (now_s - at) * 3600.0);
            let hours_to_full = rate.filter(|r| *r > 0.0).map(|r| avail as f64 / r);
            disk["growth_gb_per_h"] = rate.map_or(Value::Null, |r| json!(r / 1e9));
            disk["hours_to_full"] = hours_to_full.map_or(Value::Null, |h| json!(h));
            disk["rate_window_min"] =
                base.map_or(Value::Null, |(at, _)| json!(((now_s - at) / 60.0).round()));
            let level = level(pct, hours_to_full);
            disk["level"] = json!(level);
            if level != "ok" {
                let projection = hours_to_full
                    .zip(rate)
                    .map(|(h, r)| format!(", ~{h:.1} h to full at {:.2} GB/h", r / 1e9))
                    .unwrap_or_default();
                disk_warnings.push(json!({"level":level,"kind":"disk","text":format!(
                    "{name} disk {mount} {pct:.1}% used{projection}")}));
            }
        }
        if let Some(warnings) = host["warnings"].as_array_mut() {
            warnings.retain(|w| w["kind"] != "disk");
            warnings.extend(disk_warnings);
        }
    }
    // Keep only currently observed series plus the most recently updated rest,
    // bounded so an unbounded mount list cannot grow private state forever.
    series.retain(|key, samples| {
        !samples.is_empty()
            && (seen.contains(key) || samples.last().is_some_and(|(at, _)| now_s - at < RETAIN_S))
    });
    while series.len() > MAX_SERIES {
        let oldest = series
            .iter()
            .min_by(|a, b| {
                let a = a.1.last().map_or(f64::MIN, |s| s.0);
                let b = b.1.last().map_or(f64::MIN, |s| s.0);
                a.total_cmp(&b)
            })
            .map(|(k, _)| k.clone());
        match oldest {
            Some(key) => {
                series.remove(&key);
            }
            None => break,
        }
    }
    let encoded = series
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                json!(
                    v.into_iter()
                        .map(|(at, used)| json!([at, used]))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect::<Map<_, _>>();
    crate::io::atomic_json(&path, &json!({"schema_version":1,"series":encoded})).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(used: u64, avail: u64) -> Value {
        let pct = 100.0 * used as f64 / (used + avail) as f64;
        json!({"host":"node","disks":[{"mount":"/","size":used+avail,"used":used,"avail":avail,"pct":pct,
            "growth_gb_per_h":null,"hours_to_full":null,"rate_window_min":null,"level":"ok"}],
            "warnings":[{"level":"amber","kind":"disk","text":"stale"},{"level":"amber","text":"node CPU load is high"}]})
    }

    #[test]
    fn rate_needs_a_twenty_minute_baseline_then_projects_time_to_full() {
        let dir = tempfile::tempdir().expect("dir");
        let gb = 1_000_000_000_u64;
        let mut hosts = vec![host(100 * gb, 400 * gb)];
        assert!(apply(dir.path(), &mut hosts, 1_000.0));
        let disk = &hosts[0]["disks"][0];
        assert!(disk["growth_gb_per_h"].is_null() && disk["hours_to_full"].is_null());
        // Ten minutes later: still no baseline old enough.
        let mut hosts = vec![host(105 * gb, 395 * gb)];
        apply(dir.path(), &mut hosts, 1_600.0);
        assert!(hosts[0]["disks"][0]["growth_gb_per_h"].is_null());
        // One hour after the first sample: 30 GB/h, 370 GB free -> ~12.33 h.
        let mut hosts = vec![host(130 * gb, 370 * gb)];
        apply(dir.path(), &mut hosts, 4_600.0);
        let disk = &hosts[0]["disks"][0];
        assert!((disk["growth_gb_per_h"].as_f64().expect("rate") - 30.0).abs() < 1e-9);
        assert!((disk["hours_to_full"].as_f64().expect("eta") - 370.0 / 30.0).abs() < 1e-9);
        assert_eq!(disk["rate_window_min"], 60.0);
        // Under 24 h to full raises amber even though the disk is only 26% used.
        assert_eq!(disk["level"], "amber");
        let warnings = hosts[0]["warnings"].as_array().expect("warnings");
        assert_eq!(warnings.len(), 2, "stale disk warning replaced, CPU kept");
        assert!(
            warnings
                .iter()
                .any(|w| w["text"].as_str().is_some_and(|t| t.contains("h to full")))
        );
    }

    #[test]
    fn shrinking_usage_reports_negative_growth_without_projection() {
        let dir = tempfile::tempdir().expect("dir");
        let gb = 1_000_000_000_u64;
        apply(dir.path(), &mut [host(200 * gb, 300 * gb)], 0.0);
        let mut hosts = vec![host(190 * gb, 310 * gb)];
        apply(dir.path(), &mut hosts, 3_600.0);
        let disk = &hosts[0]["disks"][0];
        assert!(disk["growth_gb_per_h"].as_f64().expect("rate") < 0.0);
        assert!(disk["hours_to_full"].is_null());
        assert_eq!(disk["level"], "ok");
        assert_eq!(hosts[0]["warnings"].as_array().expect("w").len(), 1);
    }

    #[test]
    fn samples_older_than_three_hours_are_not_a_baseline_and_corrupt_state_restarts() {
        let dir = tempfile::tempdir().expect("dir");
        let gb = 1_000_000_000_u64;
        apply(dir.path(), &mut [host(100 * gb, 400 * gb)], 0.0);
        let mut hosts = vec![host(150 * gb, 350 * gb)];
        apply(dir.path(), &mut hosts, 4.0 * 3600.0);
        assert!(hosts[0]["disks"][0]["growth_gb_per_h"].is_null());
        std::fs::write(dir.path().join(STATE_FILE), b"not json").expect("corrupt");
        let mut hosts = vec![host(150 * gb, 350 * gb)];
        apply(dir.path(), &mut hosts, 5.0 * 3600.0);
        assert!(hosts[0]["disks"][0]["growth_gb_per_h"].is_null());
    }
}
