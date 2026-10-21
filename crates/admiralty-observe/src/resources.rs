//! Disk growth and time-to-full for observed hosts.
//!
//! Rust parity for the retained `dashboard/globalres.py` sampler: a private,
//! bounded sample file keeps `(time, used bytes)` per host mount for six
//! hours. The growth rate uses the oldest sample between 20 minutes and three
//! hours old, exactly as the Python helper did, so short spikes do not swing
//! the estimate and stale samples cannot dominate it.
//!
//! Levels match the retained thresholds: red at >= 92% used or < 12 hours to
//! full; amber at >= 85% used or < 24 hours to full. A rate needs real
//! samples: a fresh observer reports `growth_gb_per_h: null` until its first
//! sample is 20 minutes old. Nothing here writes outside the output directory.
use crate::io;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, path::Path};

pub const SAMPLE_FILE: &str = ".rust-disk-samples.json";
/// Samples older than this are discarded.
pub const RETAIN_SECONDS: i64 = 6 * 3600;
/// A new sample is stored at most this often per mount.
pub const SAMPLE_SPACING_SECONDS: i64 = 300;
const MIN_BASELINE_SECONDS: i64 = 20 * 60;
const MAX_BASELINE_SECONDS: i64 = 3 * 3600;
const MAX_SAMPLES_PER_MOUNT: usize = 96;
const MAX_MOUNTS: usize = 512;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const THRESHOLDS: &str = "disk red >=92% used or <12h to full; amber >=85% or <24h; growth from the oldest sample 20min-3h old";

type Samples = BTreeMap<String, Vec<(i64, u64)>>;

fn load(path: &Path) -> Samples {
    let Ok(value) = io::read_json(path, MAX_FILE_BYTES) else {
        return Samples::new();
    };
    if value["schema_version"] != 1 {
        return Samples::new();
    }
    value["samples"]
        .as_object()
        .into_iter()
        .flatten()
        .take(MAX_MOUNTS)
        .map(|(key, rows)| {
            let rows = rows
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| Some((row.get(0)?.as_i64()?, row.get(1)?.as_u64()?)))
                .take(MAX_SAMPLES_PER_MOUNT)
                .collect::<Vec<_>>();
            (key.clone(), rows)
        })
        .collect()
}

/// Growth estimate for one mount from its retained samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Growth {
    pub gb_per_hour: f64,
    pub hours_to_full: Option<f64>,
    pub window_minutes: i64,
}

pub fn growth(samples: &[(i64, u64)], now: i64, used: u64, avail: u64) -> Option<Growth> {
    let (at, base) = samples.iter().copied().find(|(at, _)| {
        let age = now - at;
        (MIN_BASELINE_SECONDS..=MAX_BASELINE_SECONDS).contains(&age)
    })?;
    let seconds = now - at;
    if seconds <= 0 {
        return None;
    }
    let bytes_per_hour = (used as f64 - base as f64) / seconds as f64 * 3600.0;
    Some(Growth {
        gb_per_hour: round2(bytes_per_hour / 1e9),
        hours_to_full: (bytes_per_hour > 0.0).then(|| round1(avail as f64 / bytes_per_hour)),
        window_minutes: seconds / 60,
    })
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub fn level(pct: f64, hours_to_full: Option<f64>) -> &'static str {
    if pct >= 92.0 || hours_to_full.is_some_and(|h| h < 12.0) {
        "red"
    } else if pct >= 85.0 || hours_to_full.is_some_and(|h| h < 24.0) {
        "amber"
    } else {
        "ok"
    }
}

/// Update `global_resources` in place: record samples, set
/// `growth_gb_per_h`/`hours_to_full`/`rate_window_min`/`level` on every disk,
/// rebuild disk warnings and persist the bounded sample file.
pub fn annotate(output_dir: &Path, global: &mut Value, now: i64) -> crate::Result<()> {
    let path = output_dir.join(SAMPLE_FILE);
    let mut samples = load(&path);
    let mut seen = Vec::new();
    for host in global["hosts"].as_array_mut().into_iter().flatten() {
        if host["reachable"] != true {
            continue;
        }
        let id = host["host"].as_str().unwrap_or("unknown").to_owned();
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
            let key = format!("{id}:{mount}");
            let rows = samples.entry(key.clone()).or_default();
            rows.retain(|(at, _)| now - *at <= RETAIN_SECONDS && *at <= now);
            let estimate = growth(rows, now, used, avail);
            if rows
                .last()
                .is_none_or(|(at, _)| now - at >= SAMPLE_SPACING_SECONDS)
            {
                rows.push((now, used));
            }
            if rows.len() > MAX_SAMPLES_PER_MOUNT {
                let excess = rows.len() - MAX_SAMPLES_PER_MOUNT;
                rows.drain(..excess);
            }
            seen.push(key);
            let hours = estimate.as_ref().and_then(|g| g.hours_to_full);
            let level = level(pct, hours);
            disk["growth_gb_per_h"] = estimate
                .as_ref()
                .map_or(Value::Null, |g| json!(g.gb_per_hour));
            disk["hours_to_full"] = hours.map_or(Value::Null, |h| json!(h));
            disk["rate_window_min"] = estimate
                .as_ref()
                .map_or(Value::Null, |g| json!(g.window_minutes));
            disk["level"] = json!(level);
            if level != "ok" {
                let mut text = format!(
                    "{id} {mount} {pct:.1}% used, {:.0} GB free",
                    avail as f64 / 1e9
                );
                if let (Some(hours), Some(g)) = (hours, &estimate) {
                    text.push_str(&format!(
                        ", ~{hours:.1} h to full at {:.1} GB/h",
                        g.gb_per_hour
                    ));
                }
                disk_warnings.push(json!({"level":level,"kind":"disk","text":text}));
            }
        }
        // Replace the probe's static disk warnings with growth-aware ones;
        // memory, load and pressure warnings are kept as observed.
        let mut warnings = host["warnings"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| {
                w["kind"] != "disk"
                    && !w["text"]
                        .as_str()
                        .is_some_and(|t| t.starts_with(&format!("{id} disk ")))
            })
            .collect::<Vec<_>>();
        warnings.extend(disk_warnings);
        host["warnings"] = json!(warnings);
    }
    // Forget mounts that disappeared, then bound the total key count.
    samples.retain(|key, rows| {
        seen.contains(key) || rows.iter().any(|(at, _)| now - at <= RETAIN_SECONDS)
    });
    while samples.len() > MAX_MOUNTS {
        let oldest = samples
            .iter()
            .min_by_key(|(_, rows)| rows.last().map_or(i64::MIN, |r| r.0))
            .map(|(key, _)| key.clone());
        match oldest {
            Some(key) => {
                samples.remove(&key);
            }
            None => break,
        }
    }
    let rebuilt = global["hosts"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|h| h["warnings"].as_array().cloned().unwrap_or_default())
        .collect::<Vec<_>>();
    global["warnings"] = json!(rebuilt);
    global["thresholds"] = json!(THRESHOLDS);
    let encoded = samples
        .into_iter()
        .map(|(key, rows)| {
            (
                key,
                json!(
                    rows.into_iter()
                        .map(|(at, used)| json!([at, used]))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect::<Map<_, _>>();
    io::atomic_json(&path, &json!({"schema_version":1,"samples":encoded}))
}

fn number(value: &Value, digits: usize) -> String {
    value
        .as_f64()
        .map_or_else(|| "?".into(), |n| format!("{n:.digits$}"))
}

/// Operator table for `admiralty-observe --resources` (replaces
/// `python3 globalres.py`). Unknown values print as `?`, never zero.
pub fn table(global: &Value, generated_utc: &str) -> String {
    let mut out = format!("Host resources observed {generated_utc}\n");
    for host in global["hosts"].as_array().into_iter().flatten() {
        let name = host["host"].as_str().unwrap_or("unknown");
        if host["reachable"] != true {
            out.push_str(&format!(
                "== {name} unreachable: {}\n",
                host["error"].as_str().unwrap_or("unknown")
            ));
            continue;
        }
        let mem = &host["mem"];
        out.push_str(&format!(
            "== {name} load {}/{} RAM avail {}/{} GB swap {}/{} GB D-state {}\n",
            number(&host["load"][0], 1),
            host["nproc"].as_u64().map_or("?".into(), |n| n.to_string()),
            number(&mem["avail_gb"], 1),
            number(&mem["total_gb"], 1),
            number(&mem["swap_used_gb"], 1),
            number(&mem["swap_total_gb"], 1),
            host["d_state_procs"]
                .as_u64()
                .map_or("?".into(), |n| n.to_string()),
        ));
        for disk in host["disks"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "   {:<32} {:>5}% used {:>7} GB free  growth {} GB/h  to-full {} h  [{}]\n",
                disk["mount"].as_str().unwrap_or("?"),
                number(&disk["pct"], 1),
                disk["avail"]
                    .as_f64()
                    .map_or("?".into(), |n| format!("{:.0}", n / 1e9)),
                number(&disk["growth_gb_per_h"], 2),
                number(&disk["hours_to_full"], 1),
                disk["level"].as_str().unwrap_or("?"),
            ));
        }
        for gpu in host["gpus"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "   GPU{} {} util {}% mem {}/{} MiB {} C\n",
                gpu["idx"]
                    .as_u64()
                    .or_else(|| gpu["index"].as_u64())
                    .map_or("?".into(), |n| n.to_string()),
                gpu["name"].as_str().unwrap_or("?"),
                number(&gpu["util"], 0),
                number(&gpu["mem_used_mib"], 0),
                number(&gpu["mem_total_mib"], 0),
                number(&gpu["temp_c"], 0),
            ));
        }
        if host["pressure"].is_object() {
            let p = &host["pressure"];
            out.push_str(&format!(
                "   pressure avg60: cpu some {}%  memory full {}%  io full {}%\n",
                number(&p["cpu"]["some_avg60"], 1),
                number(&p["memory"]["full_avg60"], 1),
                number(&p["io"]["full_avg60"], 1),
            ));
        }
    }
    out.push_str("WARNINGS:\n");
    let warnings = global["warnings"].as_array().cloned().unwrap_or_default();
    if warnings.is_empty() {
        out.push_str("  none\n");
    }
    for warning in warnings {
        out.push_str(&format!(
            "  [{}] {}\n",
            warning["level"].as_str().unwrap_or("?"),
            warning["text"].as_str().unwrap_or("?")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_prints_unknowns_as_question_marks_and_lists_warnings() {
        let mut value = global(88_000_000_000, 12_000_000_000, 88.0);
        value["hosts"][0]["nproc"] = json!(8);
        value["hosts"][0]["load"] = json!([2.5, 2.0, 1.0]);
        value["warnings"] = json!([{"level":"amber","text":"build / 88.0% used"}]);
        let text = table(&value, "2027-01-15T08:00:00Z");
        assert!(text.starts_with("Host resources observed 2027-01-15T08:00:00Z\n"));
        assert!(text.contains("== build load 2.5/8 RAM avail ?/? GB swap ?/? GB D-state ?"));
        assert!(
            text.contains("88.0% used      12 GB free  growth ? GB/h  to-full ? h  [ok]"),
            "{text}"
        );
        assert!(text.contains("== offline unreachable: unknown"));
        assert!(text.ends_with("WARNINGS:\n  [amber] build / 88.0% used\n"));
    }

    fn global(used: u64, avail: u64, pct: f64) -> Value {
        json!({"hosts":[{"host":"build","reachable":true,
            "disks":[{"mount":"/","size":used+avail,"used":used,"avail":avail,"pct":pct,"level":"ok"}],
            "warnings":[{"level":"amber","text":"build memory availability is low"},
                        {"level":"amber","text":"build disk / 86.0% used"}]},
            {"host":"offline","reachable":false,"disks":[],"warnings":[]}],"warnings":[]})
    }

    #[test]
    fn growth_uses_oldest_sample_between_twenty_minutes_and_three_hours() {
        let now = 100_000;
        let samples = vec![
            (now - 4 * 3600, 0), // too old for a baseline
            (now - 2 * 3600, 1_000_000_000),
            (now - 600, 50_000_000_000), // too recent
        ];
        let estimate = growth(&samples, now, 3_000_000_000, 10_000_000_000).expect("growth");
        assert_eq!(estimate.gb_per_hour, 1.0);
        assert_eq!(estimate.hours_to_full, Some(10.0));
        assert_eq!(estimate.window_minutes, 120);
        assert_eq!(growth(&samples[2..], now, 1, 1), None, "no baseline yet");
        let shrinking = growth(&samples[1..2], now, 0, 5).expect("negative rate");
        assert!(shrinking.gb_per_hour < 0.0);
        assert_eq!(shrinking.hours_to_full, None, "never fills while shrinking");
    }

    #[test]
    fn levels_match_retained_python_thresholds() {
        assert_eq!(level(50.0, None), "ok");
        assert_eq!(level(85.0, None), "amber");
        assert_eq!(level(92.0, None), "red");
        assert_eq!(level(10.0, Some(23.9)), "amber");
        assert_eq!(level(10.0, Some(11.9)), "red");
        assert_eq!(level(10.0, Some(48.0)), "ok");
    }

    #[test]
    fn annotate_persists_bounded_samples_and_rewrites_disk_warnings() {
        let directory = tempfile::tempdir().expect("tempdir");
        let start = 1_800_000_000;
        // First cycle: no baseline, static level kept from pct.
        let mut first = global(86_000_000_000, 14_000_000_000, 86.0);
        annotate(directory.path(), &mut first, start).expect("first");
        let disk = &first["hosts"][0]["disks"][0];
        assert_eq!(disk["growth_gb_per_h"], Value::Null);
        assert_eq!(disk["level"], "amber");
        let warnings = first["warnings"].as_array().expect("warnings");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[1]["text"]
                .as_str()
                .expect("text")
                .starts_with("build / 86.0% used, 14 GB free")
        );
        // Thirty minutes later the disk grew 2 GB: 4 GB/h, 3 hours to full.
        let mut later = global(88_000_000_000, 12_000_000_000, 88.0);
        annotate(directory.path(), &mut later, start + 1800).expect("later");
        let disk = &later["hosts"][0]["disks"][0];
        assert_eq!(disk["growth_gb_per_h"], 4.0);
        assert_eq!(disk["hours_to_full"], 3.0);
        assert_eq!(disk["rate_window_min"], 30);
        assert_eq!(disk["level"], "red");
        let text = later["warnings"][1]["text"].as_str().expect("text");
        assert!(text.ends_with("~3.0 h to full at 4.0 GB/h"), "{text}");
        assert_eq!(later["thresholds"], THRESHOLDS);
        // Samples are private, bounded and spaced.
        let stored =
            io::read_json(&directory.path().join(SAMPLE_FILE), MAX_FILE_BYTES).expect("samples");
        assert_eq!(
            stored["samples"]["build:/"].as_array().expect("rows").len(),
            2
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join(SAMPLE_FILE))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
        let mut soon = global(88_000_000_000, 12_000_000_000, 88.0);
        annotate(directory.path(), &mut soon, start + 1860).expect("soon");
        let stored =
            io::read_json(&directory.path().join(SAMPLE_FILE), MAX_FILE_BYTES).expect("samples");
        assert_eq!(
            stored["samples"]["build:/"].as_array().expect("rows").len(),
            2,
            "spacing"
        );
    }

    #[test]
    fn corrupt_or_foreign_sample_files_restart_cleanly() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join(SAMPLE_FILE), b"{not json").expect("write");
        let mut value = global(1, 99, 1.0);
        annotate(directory.path(), &mut value, 1_800_000_000).expect("recovers");
        assert_eq!(value["hosts"][0]["disks"][0]["level"], "ok");
        std::fs::write(
            directory.path().join(SAMPLE_FILE),
            br#"{"schema_version":2,"samples":{"x:/":[[1,2]]}}"#,
        )
        .expect("write");
        assert!(load(&directory.path().join(SAMPLE_FILE)).is_empty());
    }
}
