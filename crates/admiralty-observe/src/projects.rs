//! Read-only, bounded adapters for configured project ledgers. These are
//! observations, never task execution, publication or native acceptance grants.
use crate::config::{ObserveConfig, ProjectConfig};
use crate::io::{clean, mtime_utc, read_json, read_text_tail, run};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
const SOURCE_LIMIT: u64 = 4 * 1024 * 1024;
const MAX_TASKS: usize = 2048;
const MAX_CASES: usize = 128;
const MAX_LOG_BYTES: usize = 8 * 1024 * 1024;
const TOTAL_LOG_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
mod legacy_parity;
const STAGES: [&str; 7] = [
    "done",
    "on_main",
    "in_pr",
    "implemented",
    "in_progress",
    "blocked",
    "not_started",
];
fn string(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn short(v: &str, n: usize) -> String {
    clean(v).chars().take(n).collect()
}
fn source_path(project: &ProjectConfig, key: &str) -> Option<PathBuf> {
    project
        .tracker
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .map(|s| {
            let p = PathBuf::from(s);
            if p.is_absolute() {
                p
            } else {
                project.repository.join(p)
            }
        })
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn err(message: &str) -> crate::Result<Value> {
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, message).into())
}
fn bucket(status: &str) -> &'static str {
    let status = status.to_ascii_lowercase();
    match status.as_str() {
        "done" | "verified" | "accepted" | "complete" | "completed" => "done",
        "in_progress" | "in-progress" | "review" | "in_review" | "dispatched" | "active"
        | "assigned" | "running" | "claimed" => "in_progress",
        s if s.starts_with("blocked") || ["audit_held", "failed", "fail"].contains(&s) => "blocked",
        "unknown" | "" => "unknown",
        _ => "open",
    }
}
fn count_values(values: impl Iterator<Item = String>) -> Value {
    let mut result = BTreeMap::<String, usize>::new();
    for v in values {
        *result.entry(v).or_default() += 1
    }
    json!(result)
}
fn summarize(statuses: &BTreeMap<String, String>) -> Value {
    let counts = count_values(statuses.values().map(|s| bucket(s).to_owned()));
    let stages = json!({"done":counts["done"].as_u64().unwrap_or(0),"in_progress":counts["in_progress"].as_u64().unwrap_or(0),"blocked":counts["blocked"].as_u64().unwrap_or(0),"not_started":counts["open"].as_u64().unwrap_or(0),"unknown":counts["unknown"].as_u64().unwrap_or(0)});
    json!({"total":statuses.len(),"done":stages["done"],"counts":counts,"stages":stages,"stage_labels":{"done":"done (source ledger; acceptance not independently checked)","not_started":"not started","unknown":"status unknown"},"raw_status_counts":count_values(statuses.values().cloned()),"_statuses":statuses,"blocked_ids":statuses.iter().filter(|(_,s)|bucket(s)=="blocked").map(|(id,_)|id).collect::<Vec<_>>()})
}
fn tasks(value: &Value) -> crate::Result<&Vec<Value>> {
    let values = value
        .as_array()
        .or_else(|| value.get("tasks").and_then(Value::as_array))
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "task array missing")
        })?;
    if values.len() > MAX_TASKS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "task array exceeds bound",
        )
        .into());
    }
    Ok(values)
}
fn statuses_from_tasks(value: &Value) -> crate::Result<BTreeMap<String, String>> {
    let mut statuses = BTreeMap::<String, String>::new();
    for task in tasks(value)? {
        let id = string(&task["id"]);
        if !valid_id(id) || statuses.contains_key(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid or duplicate task ID",
            )
            .into());
        }
        statuses.insert(
            id.to_owned(),
            task["status"].as_str().unwrap_or("unknown").to_owned(),
        );
    }
    Ok(statuses)
}
fn optional_json(project: &ProjectConfig, key: &str) -> Option<Value> {
    source_path(project, key).and_then(|p| read_json(&p, SOURCE_LIMIT).ok())
}
fn task_array(project: &ProjectConfig) -> crate::Result<Value> {
    let path = source_path(project, "path").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "tracker path missing")
    })?;
    let ledger = read_json(&path, SOURCE_LIMIT)?;
    let statuses = statuses_from_tasks(&ledger)?;
    let mut result = summarize(&statuses);
    result["ledger_source"] = json!(path);
    result["ledger_mtime_utc"] = json!(mtime_utc(&path));
    let progress = optional_json(project, "progress_path").unwrap_or(Value::Null);
    if progress.is_null() && source_path(project, "progress_path").is_some() {
        result["flags"] = json!([
            "Configured progress metadata is unavailable; leases and blockers are unknown."
        ]);
    }
    result["ledger_revision"] = progress
        .get("ledger_revision")
        .or_else(|| ledger.get("revision"))
        .cloned()
        .unwrap_or(Value::Null);
    result["ledger_updated_utc"] = progress
        .get("updated_at")
        .or_else(|| ledger.get("updated_at"))
        .cloned()
        .unwrap_or(Value::Null);
    result["blockers"] = json!(
        progress["blockers"]
            .as_array()
            .map(|a| a
                .iter()
                .take(128)
                .map(|v| short(v.to_string().trim_matches('"'), 500))
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
    result["leases"]=json!(progress["active_leases"].as_array().map(|a|a.iter().take(128).map(|v|json!({"owner":v["owner"],"tasks":v["tasks"],"lease":v["lease_id"],"expires_utc":v["expires_utc"]})).collect::<Vec<_>>()).unwrap_or_default());
    if progress.is_null() {
        result["leases"] = Value::Null;
        result["blockers"] = Value::Null;
    }
    result["current_focus"] = json!(short(string(&progress["current_focus"]), 1000));
    let mut planning = (0_u64, 0_u64);
    let mut implementation = (0_u64, 0_u64);
    for task in tasks(&ledger)? {
        let pair = if task["kind"] == "planning" {
            &mut planning
        } else {
            &mut implementation
        };
        pair.1 += 1;
        if bucket(string(&task["status"])) == "done" {
            pair.0 += 1
        }
    }
    result["by_kind"] = json!({"planning_done":planning.0,"planning_total":planning.1,"impl_done":implementation.0,"impl_total":implementation.1});
    let mut recent = Vec::new();
    if let Some(path) = source_path(project, "events_path") {
        match read_text_tail(&path, 128 * 1024) {
            Ok(text) => {
                for line in text.lines().rev().take(128) {
                    if let Ok(e) = serde_json::from_str::<Value>(line) {
                        recent.push(json!({"task":e["task"],"old":e["old_status"],"new":e["status"],"at":e["at"],"source":"events.jsonl","note":short(string(&e["note"]),300)}));
                    }
                }
            }
            Err(_) => {
                result["event_source_error"] = json!("event source unreadable");
            }
        }
    }
    result["recent"] = json!(recent.into_iter().take(20).collect::<Vec<_>>());
    Ok(result)
}
fn audit(text: &str) -> Value {
    let current = text.split("## Previous").next().unwrap_or(text);
    let mut fence = false;
    let mut failures = 0;
    let mut held = BTreeSet::new();
    for line in current.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || line.starts_with('>') {
            continue;
        }
        let plain = line
            .replace("**", "")
            .trim_start_matches(['#', '-', '*', ' '])
            .to_owned();
        if let Some((key, value)) = plain.split_once(':')
            && ["verdict", "verdicts"].contains(&key.trim().to_ascii_lowercase().as_str())
        {
            let value = value.trim().to_ascii_uppercase();
            let words = value.split_whitespace().collect::<Vec<_>>();
            if (value.starts_with("FAIL")
                && !words
                    .iter()
                    .skip(1)
                    .any(|s| s.trim_matches([':', '=']) == "0"))
                || (words.len() > 1
                    && words[0].parse::<u64>().is_ok_and(|n| n > 0)
                    && words[1].starts_with("FAIL"))
            {
                failures += 1
            }
        }
        if line.contains("AUDIT-HELD") {
            for token in line.split_whitespace() {
                let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
                if valid_id(token) && token.contains('-') && token != "AUDIT-HELD" {
                    held.insert(token.to_owned());
                    break;
                }
            }
        }
    }
    let observed = current
        .lines()
        .find_map(|line| {
            line.split_once("Observed:")
                .and_then(|(_, s)| s.split_whitespace().next())
        })
        .map(str::to_owned);
    json!({"observed_utc":observed,"fail_mentions":failures,"verified_mentions":current.matches("VERIFIED").count(),"weak_mentions":current.matches("WEAK").count(),"audit_held":held,"note":"verdict/source observations; mentions are not acceptance proof"})
}
fn packet_progress(project: &ProjectConfig) -> crate::Result<Value> {
    let path = source_path(project, "path").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "tracker path missing")
    })?;
    let dir = source_path(project, "progress_dir").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "progress_dir missing")
    })?;
    let ledger = read_json(&path, SOURCE_LIMIT)?;
    let packets = ledger["packets"]
        .as_array()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "packets missing"))?;
    if packets.len() > 256 {
        return err("packet bound exceeded");
    }
    let mut statuses = BTreeMap::<String, String>::new();
    let mut rows = Vec::new();
    let mut source_errors = Vec::new();
    for packet in packets {
        let id = string(&packet["id"]);
        if !valid_id(id) {
            return err("invalid packet ID");
        }
        let progress_path = dir.join(format!("{id}.json"));
        let progress = match read_json(&progress_path, SOURCE_LIMIT) {
            Ok(v) => v,
            Err(_) => {
                source_errors.push(format!("{id}: progress unavailable; task states unknown"));
                Value::Null
            }
        };
        let task_list = tasks(packet)?;
        let mut done = 0;
        for task in task_list {
            if statuses.len() >= MAX_TASKS {
                return err("task bound exceeded");
            };
            let tid = string(&task["id"]);
            if !valid_id(tid) || statuses.contains_key(tid) {
                return err("invalid or duplicate task ID");
            }
            let observed = &progress["tasks"][tid];
            let status = observed["status"]
                .as_str()
                .or_else(|| observed.as_str())
                .unwrap_or("unknown");
            if bucket(status) == "done" {
                done += 1
            }
            statuses.insert(tid.into(), status.into());
        }
        rows.push(json!({"id":id,"done":done,"total":task_list.len(),"source_updated_utc":mtime_utc(&progress_path)}));
    }
    let original = statuses.clone();
    let mut validator = Value::Null;
    if let Some(p) = source_path(project, "validator_path") {
        match read_text_tail(&p, 256 * 1024) {
            Ok(text) => {
                validator = audit(&text);
                validator["file_mtime_utc"] = json!(mtime_utc(&p));
                for id in validator["audit_held"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if statuses.get(id).is_some_and(|s| bucket(s) != "done") {
                        statuses.insert(id.to_owned(), "audit_held".into());
                    }
                }
            }
            Err(_) => validator = json!({"status":"unknown","error":"validator source unreadable"}),
        }
    }
    let mut result = summarize(&statuses);
    result["raw_status_counts"] = count_values(original.into_values());
    result["ledger_source"] = json!(format!("{} + {}/*.json", path.display(), dir.display()));
    result["ledger_mtime_utc"] = json!(mtime_utc(&path));
    result["packets_total"] = json!(rows.len());
    result["packets_accepted"] = json!(
        rows.iter()
            .filter(|p| p["total"].as_u64().is_some_and(|n| n > 0) && p["done"] == p["total"])
            .count()
    );
    result["packets"] = json!(rows);
    result["validator"] = validator;
    result["flags"] = json!(source_errors);
    if let Some(lease) = optional_json(project, "lease_path") {
        result["lease"] = json!({"expires_utc":lease["expires_utc"],"renewals":lease["renewals"].as_array().map(Vec::len)});
    }
    Ok(result)
}
fn markdown_tasks(text: &str) -> crate::Result<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    let mut header: Option<Vec<String>> = None;
    let mut fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || !trimmed.starts_with('|') {
            header = None;
            continue;
        }
        let cells = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();
        if cells.iter().all(|s| s.chars().all(|c| "-: ".contains(c))) {
            continue;
        }
        if cells.iter().any(|s| s.eq_ignore_ascii_case("status"))
            && cells
                .iter()
                .any(|s| s.eq_ignore_ascii_case("task id") || s.eq_ignore_ascii_case("id"))
        {
            header = Some(cells.iter().map(|s| s.to_ascii_lowercase()).collect());
            continue;
        }
        let Some(h) = &header else { continue };
        let Some(id_column) = h.iter().position(|s| s == "task id" || s == "id") else {
            continue;
        };
        let Some(status_column) = h.iter().position(|s| s == "status") else {
            continue;
        };
        let Some(id) = cells
            .get(id_column)
            .map(|s| s.trim_matches(['`', '*', ' ']))
        else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        let status_index = if cells.len() != h.len() {
            cells.len().checked_sub(h.len() - status_column)
        } else {
            Some(status_column)
        };
        let Some(raw) = status_index.and_then(|i| cells.get(i)) else {
            continue;
        };
        let status = raw
            .trim_matches(['`', '*', ' '])
            .split(|c: char| !c.is_ascii_alphabetic() && c != '_')
            .find(|s| !s.is_empty())
            .unwrap_or("unknown");
        if result.len() >= MAX_TASKS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Markdown task bound exceeded",
            )
            .into());
        }
        if result.contains_key(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "duplicate Markdown task ID",
            )
            .into());
        }
        result.insert(id.into(),json!({"status":status,"status_text":short(raw,300),"title":cells.get(1).map(|s|short(s,200))}));
    }
    Ok(result)
}
fn markdown_table(project: &ProjectConfig) -> crate::Result<Value> {
    let path = source_path(project, "path").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "tracker path missing")
    })?;
    if fs::metadata(&path)?.len() > SOURCE_LIMIT {
        return err("Markdown ledger exceeds byte bound; counts unknown");
    }
    let text = read_text_tail(&path, SOURCE_LIMIT as usize)?;
    let tasks = markdown_tasks(&text)?;
    if tasks.is_empty() {
        return err("no task table found; total unknown");
    }
    let statuses = tasks
        .iter()
        .map(|(id, t)| (id.clone(), string(&t["status"]).to_owned()))
        .collect();
    let mut result = summarize(&statuses);
    result["ledger_source"] = json!(path);
    result["ledger_mtime_utc"] = json!(mtime_utc(&path));
    result["titles"] = json!(
        tasks
            .iter()
            .map(|(k, v)| (k, &v["title"]))
            .collect::<BTreeMap<_, _>>()
    );
    result["blocked_detail"] = json!(
        tasks
            .iter()
            .filter(|(_, v)| bucket(string(&v["status"])) == "blocked")
            .map(|(k, v)| (k, &v["status_text"]))
            .collect::<BTreeMap<_, _>>()
    );
    if let Some(dir) = source_path(project, "waves_dir") {
        result["waves"] = waves(&dir).unwrap_or_else(
            |_| json!({"error":"wave source unavailable","certified_designs_total":null}),
        );
    }
    Ok(result)
}
fn waves(dir: &Path) -> crate::Result<Value> {
    let mut dirs = fs::read_dir(dir)?
        .take(512)
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix('w')
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| (n, e.path(), name))
        })
        .collect::<Vec<_>>();
    dirs.sort_by_key(|(n, _, _)| *n);
    let mut completed = Vec::new();
    let mut running = Vec::new();
    let mut total = 0_u64;
    let mut legacy = Vec::new();
    for (_, dir, name) in dirs {
        let path = dir.join("leaderboard.json");
        if let Ok(board) = read_json(&path, 1024 * 1024) {
            if let Some(n) = board["qualified_count"].as_u64() {
                total = total.checked_add(n).ok_or("wave count overflow")?
            } else if !board["champion"].is_null() {
                legacy.push(name.clone())
            }
            let best = board["designs"].as_array().and_then(|a| {
                a.iter().filter(|d| d["w_cad"].is_number()).max_by(|a, b| {
                    a["w_cad"]
                        .as_f64()
                        .unwrap_or(f64::NEG_INFINITY)
                        .total_cmp(&b["w_cad"].as_f64().unwrap_or(f64::NEG_INFINITY))
                })
            });
            completed.push(json!({"wave":name,"assessed":board["assessed_count"],"qualified":board["qualified_count"],"unverified":board["unverified"],"champion":board["champion"],"mtime_utc":mtime_utc(&path),"best":best}));
        } else if fs::metadata(&dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < Duration::from_secs(6 * 3600))
        {
            running.push(name)
        }
    }
    Ok(
        json!({"latest_completed":completed.last(),"recent_completed":completed.iter().rev().take(4).collect::<Vec<_>>(),"running_or_pending":running.iter().rev().take(3).collect::<Vec<_>>(),"certified_designs_total":total,"legacy_champion_waves":legacy,"scope":dir,"note":"source leaderboard counts; older champion labels do not establish certification"}),
    )
}
fn plain_git() -> PathBuf {
    PathBuf::from("/usr/bin/git")
}
async fn git(repo: &Path, args: Vec<String>, limit: usize) -> crate::Result<String> {
    let mut argv = vec![
        "--no-pager".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "core.attributesFile=/dev/null".into(),
        "-c".into(),
        "protocol.allow=never".into(),
    ];
    argv.extend(args);
    let result = run(&plain_git(), &argv, Some(repo), 5, limit).await?;
    if result.status != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "read-only Git query failed",
        )
        .into());
    }
    Ok(result.stdout.trim().to_owned())
}
fn oid(s: &str) -> bool {
    [40, 64].contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn mentions(text: &str, id: &str) -> bool {
    let expected = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let tokens = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    for i in 0..tokens.len() {
        let mut candidate = String::new();
        for token in tokens.iter().skip(i).take(4) {
            candidate.push_str(token);
            if candidate == expected {
                return true;
            }
            if candidate.len() >= expected.len() {
                break;
            }
        }
    }
    false
}
fn proof_log(path: &Path, max: usize) -> crate::Result<Vec<u8>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "log path must be absolute and normalized",
        )
        .into());
    }
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part);
        if fs::symlink_metadata(&prefix)?.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "log symlink refused",
            )
            .into());
        }
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(
            (rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC)
                .bits() as i32,
        )
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() == 0 || before.len() > max as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "log outside regular byte bound",
        )
        .into());
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() > max
        || bytes.len() as u64 != before.len()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "log changed during read",
        )
        .into());
    }
    Ok(bytes)
}
// Receipt claims are not allowed to exploit JSON's duplicate-key ambiguity.
fn unique_json(bytes: &[u8]) -> crate::Result<Value> {
    use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    struct Unique(Value);
    struct UniqueVisitor;
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            d.deserialize_any(UniqueVisitor)
        }
    }
    impl<'de> Visitor<'de> for UniqueVisitor {
        type Value = Unique;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unambiguous JSON")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Unique, A::Error> {
            let mut m = Map::new();
            while let Some(k) = a.next_key::<String>()? {
                if m.contains_key(&k) {
                    return Err(de::Error::custom("duplicate receipt field"));
                }
                m.insert(k, a.next_value::<Unique>()?.0);
            }
            Ok(Unique(Value::Object(m)))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Unique, A::Error> {
            let mut v = Vec::new();
            while let Some(item) = a.next_element::<Unique>()? {
                v.push(item.0);
            }
            Ok(Unique(Value::Array(v)))
        }
        fn visit_str<E: de::Error>(self, s: &str) -> std::result::Result<Unique, E> {
            Ok(Unique(Value::String(s.into())))
        }
        fn visit_string<E: de::Error>(self, s: String) -> std::result::Result<Unique, E> {
            Ok(Unique(Value::String(s)))
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Unique, E> {
            Ok(Unique(Value::Bool(v)))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Unique, E> {
            Ok(Unique(json!(v)))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Unique, E> {
            Ok(Unique(json!(v)))
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Unique, E> {
            serde_json::Number::from_f64(v)
                .map(|v| Unique(Value::Number(v)))
                .ok_or_else(|| E::custom("invalid receipt number"))
        }
        fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
            Ok(Unique(Value::Null))
        }
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let value = Unique::deserialize(&mut d)?.0;
    d.end()?;
    Ok(value)
}
/// Regex-free equivalent of the legacy collector's execution-command pattern
/// `\b(cargo|nextest|pytest|just|npm|pnpm|make|ctest|uv run|go test|bash\s+\S+\.sh|\./\S+)\b|\bpython3?\s+(-m\s+\w+|[\w/.-]+\.py)\b`
/// (searched anywhere, case-sensitive).
fn execution_command(command: &str) -> bool {
    let chars = command.chars().collect::<Vec<_>>();
    let n = chars.len();
    let word = |i: usize| i < n && (chars[i].is_alphanumeric() || chars[i] == '_');
    let boundary = |i: usize| (i > 0 && word(i - 1)) != word(i);
    let at = |i: usize, s: &str| {
        let s = s.chars().collect::<Vec<_>>();
        i + s.len() <= n && chars[i..i + s.len()] == s[..]
    };
    let spaces = |mut i: usize| {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        i
    };
    let run = |mut i: usize, member: &dyn Fn(char) -> bool| {
        while i < n && member(chars[i]) {
            i += 1;
        }
        i
    };
    (0..n).any(|i| {
        if !boundary(i) {
            return false;
        }
        let literal = [
            "cargo", "nextest", "pytest", "just", "npm", "pnpm", "make", "ctest", "uv run",
            "go test",
        ]
        .iter()
        .any(|s| at(i, s) && boundary(i + s.chars().count()));
        let script = at(i, "bash") && {
            let start = spaces(i + 4);
            let end = run(start, &|c| !c.is_whitespace());
            start > i + 4
                && (start + 1..end.saturating_sub(2)).any(|m| at(m, ".sh") && !word(m + 3))
        };
        let relative = at(i, "./") && {
            let end = run(i + 2, &|c| !c.is_whitespace());
            (i + 3..=end).any(&boundary)
        };
        let python = at(i, "python") && {
            let version = if at(i + 6, "3") { i + 7 } else { i + 6 };
            let start = spaces(version);
            start > version
                && ((at(start, "-m") && {
                    let module = spaces(start + 2);
                    module > start + 2 && word(module)
                }) || {
                    let end = run(start, &|c| {
                        c.is_alphanumeric() || c == '_' || matches!(c, '/' | '.' | '-')
                    });
                    (start + 1..end.saturating_sub(2)).any(|m| at(m, ".py") && !word(m + 3))
                })
        };
        literal || script || relative || python
    })
}
/// A stated verification time must be a timezone-qualified instant, not in the
/// future, and not earlier than the receipt commit: an older receipt relabeled
/// to a newer commit was not re-run. An absent time follows the major-release
/// receipt contract, which does not require one.
async fn verification_time(
    repo: &Path,
    receipt: &Value,
    sha: &str,
) -> std::result::Result<(), String> {
    let Some(stated) = ["verified_at_utc", "verified_at"]
        .iter()
        .map(|key| &receipt[*key])
        .find(|value| !value.is_null())
    else {
        return Ok(());
    };
    let at = stated
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .ok_or("receipt verification time is not a timezone-qualified instant")?
        .timestamp();
    if at > chrono::Utc::now().timestamp() + 30 {
        return Err("receipt verification time is in the future".into());
    }
    let committed = git(
        repo,
        vec!["log".into(), "-1".into(), "--format=%ct".into(), sha.into()],
        256,
    )
    .await
    .ok()
    .and_then(|s| s.parse::<i64>().ok())
    .ok_or("receipt commit time unknown")?;
    if at < committed {
        return Err("receipt predates its commit; relabeled to a newer commit, not re-run".into());
    }
    Ok(())
}
fn verify_cases(
    task: &Value,
    receipt: &Value,
    sha: &str,
    receipt_path: &Path,
    remaining: &mut usize,
    logs: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> std::result::Result<(), String> {
    if receipt.get("task_id").is_some_and(|v| v != &task["id"]) {
        return Err("receipt task identity differs".into());
    }
    if let Some(criterion) = task["completion_criterion"].as_str()
        && receipt
            .get("completion_criterion")
            .is_some_and(|v| v.as_str() != Some(criterion))
    {
        return Err("completion criterion differs".into());
    }
    let cases = receipt["case_to_test_map"]
        .as_array()
        .or_else(|| receipt["cases"].as_array())
        .ok_or("receipt has no case records")?;
    if cases.is_empty() || cases.len() > MAX_CASES {
        return Err("receipt cases outside bound".into());
    }
    if let Some(expected) = task["acceptance_cases"].as_array() {
        let expected = expected
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let actual = cases
            .iter()
            .filter_map(|v| v["acceptance_case"].as_str())
            .collect::<BTreeSet<_>>();
        if expected.is_empty() || actual != expected || actual.len() != cases.len() {
            return Err("acceptance cases do not match".into());
        }
    }
    for case in cases {
        if case["exit_code"].as_i64() != Some(0) || case["passed"] == false {
            return Err("case lacks successful execution".into());
        }
        // A stated command must name an executed check, never e.g. `echo PASS`.
        if let Some(command) = case.get("command").filter(|v| !v.is_null())
            && !command.as_str().is_some_and(execution_command)
        {
            return Err("case command is not an executed check".into());
        }
        if case
            .get("checkout_sha")
            .or_else(|| case.get("head_sha"))
            .is_some_and(|v| v.as_str() != Some(sha))
        {
            return Err("case checkout identity differs".into());
        }
        let digest = string(&case["log_sha256"]);
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("log digest missing/malformed".into());
        }
        let raw = string(&case["log_path"]);
        if raw.is_empty() {
            return Err("log path missing".into());
        }
        let p = PathBuf::from(raw);
        let p = if p.is_absolute() {
            p
        } else {
            receipt_path
                .parent()
                .ok_or("receipt parent missing")?
                .join(p)
        };
        if !logs.contains_key(&p) {
            let bytes = proof_log(&p, MAX_LOG_BYTES.min(*remaining))
                .map_err(|_| "log unavailable, changed or outside bound")?;
            *remaining = remaining
                .checked_sub(bytes.len())
                .ok_or("project log budget exhausted")?;
            logs.insert(p.clone(), bytes);
        }
        let bytes = logs.get(&p).ok_or("log cache unavailable")?;
        if format!("{:x}", Sha256::digest(bytes)) != digest {
            return Err("log SHA-256 differs".into());
        }
        let text = String::from_utf8_lossy(bytes);
        let header = text.split("--- STDOUT ---").next().unwrap_or("");
        for line in header.lines() {
            if let Some((field, value)) = line.trim().split_once(':') {
                let field = field.trim_end();
                if ["head_sha", "checkout_sha"].contains(&field) && value.trim() != sha {
                    return Err("execution header checkout differs".into());
                }
                if field == "exit_code" && value.trim() != "0" {
                    return Err("execution header failed".into());
                }
            }
        }
        let mut fence = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("```") || line.starts_with("~~~") {
                fence = !fence;
                continue;
            }
            if fence || line.starts_with('>') {
                continue;
            }
            if line.starts_with("test result: FAILED")
                || line == "FAILED"
                || line.starts_with("FAILED (")
                || line.contains("Ran 0 tests")
                || line
                    .strip_prefix("test result: ok.")
                    .and_then(|s| s.trim().split_once(" passed;"))
                    .and_then(|(n, _)| n.parse::<u64>().ok())
                    == Some(0)
            {
                return Err("execution log reports failed or empty tests".into());
            }
        }
    }
    Ok(())
}
async fn pull_requests(config: &ObserveConfig, project: &ProjectConfig) -> Value {
    if let Some(value) = optional_json(project, "pr_snapshot_path") {
        return value;
    }
    let Some(program) = &config.gh_program else {
        return Value::Null;
    };
    let repo = string(&project.tracker["github_repo"]);
    if repo.is_empty() || repo.len() > 256 {
        return Value::Null;
    }
    let args=vec!["pr".into(),"list".into(),"--repo".into(),repo.into(),"--state".into(),"all".into(),"--limit".into(),"60".into(),"--json".into(),"number,title,state,headRefName,baseRefName,headRefOid,url,mergedAt,statusCheckRollup,reviewDecision,isDraft,updatedAt".into()];
    match run(
        program,
        &args,
        Some(&project.repository),
        10,
        2 * 1024 * 1024,
    )
    .await
    {
        Ok(output) if output.status == 0 => {
            serde_json::from_str(&output.stdout).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}
fn normalized_pr(raw: &Value) -> Value {
    if raw.is_null() {
        return json!({"state":"unknown","error":"PR source unavailable or not configured"});
    }
    if raw.get("head").is_some() || raw.get("ci").is_some() {
        return raw.clone();
    }
    let mut counts = BTreeMap::<String, usize>::new();
    let mut failing = Vec::new();
    for check in raw["statusCheckRollup"].as_array().into_iter().flatten() {
        let conclusion = check["conclusion"]
            .as_str()
            .or_else(|| check["state"].as_str())
            .unwrap_or("unknown")
            .to_ascii_lowercase();
        *counts.entry(conclusion.clone()).or_default() += 1;
        if ["failure", "failed", "error", "timed_out", "cancelled"].contains(&conclusion.as_str()) {
            failing.push(check["name"].clone())
        }
    }
    let state = if !failing.is_empty() {
        "failure"
    } else if counts.is_empty() {
        "unknown"
    } else if counts
        .keys()
        .all(|k| ["success", "neutral", "skipped"].contains(&k.as_str()))
    {
        "success"
    } else {
        "pending"
    };
    json!({"number":raw["number"],"state":raw["state"],"url":raw["url"],"title":raw["title"],"head":raw["headRefOid"],"branch":raw["headRefName"],"draft":raw["isDraft"],"review_decision":raw["reviewDecision"],"exact_head_approved":false,"merged_at":raw["mergedAt"],"fetched_utc":crate::now_utc(),"ci":{"state":state,"counts":counts,"failing":failing}})
}
fn select_pr(project: &ProjectConfig, prs: &Value) -> Value {
    if !prs.is_array() {
        return normalized_pr(prs);
    }
    let list = prs.as_array().map(Vec::as_slice).unwrap_or_default();
    let number = project.tracker["pr_number"].as_u64();
    let branches = project.tracker["pr_branches"].as_array();
    let matches = |p: &&Value| {
        number.is_some_and(|n| p["number"].as_u64() == Some(n))
            || branches.is_some_and(|bs| bs.iter().any(|b| b == &p["headRefName"]))
    };
    let chosen = if number.is_some() || branches.is_some() {
        list.iter()
            .filter(matches)
            .find(|p| p["state"] == "OPEN")
            .or_else(|| list.iter().find(matches))
    } else {
        list.iter()
            .find(|p| p["state"] == "OPEN")
            .or_else(|| list.first())
    };
    normalized_pr(chosen.unwrap_or(&Value::Null))
}
async fn major_release(
    project: &ProjectConfig,
    screens: &[Value],
    prs: &Value,
) -> crate::Result<Value> {
    let reference = project.tracker["ref"].as_str().unwrap_or("origin/main");
    let path = project.tracker["path"].as_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "major release Git path missing",
        )
    })?;
    if reference.starts_with('-')
        || reference.len() > 256
        || path.starts_with('/')
        || path.split('/').any(|c| c == ".." || c.is_empty())
        || path.len() > 4096
    {
        return err("invalid major release Git scope");
    }
    let main = git(
        &project.repository,
        vec![
            "rev-parse".into(),
            "--verify".into(),
            format!("{reference}^{{commit}}"),
        ],
        256,
    )
    .await?;
    if !oid(&main) {
        return err("main commit identity unknown");
    }
    let ledger: Value = serde_json::from_str(
        &git(
            &project.repository,
            vec!["show".into(), format!("{reference}:{path}")],
            SOURCE_LIMIT as usize,
        )
        .await?,
    )?;
    let task_list = tasks(&ledger)?;
    let ledger_updated = git(
        &project.repository,
        vec![
            "log".into(),
            "-1".into(),
            "--format=%cI".into(),
            reference.into(),
            "--".into(),
            path.into(),
        ],
        256,
    )
    .await
    .ok();
    let verify_dir = source_path(project, "verify_dir").or_else(|| {
        ledger["verify_dir"].as_str().map(|s| {
            let path = PathBuf::from(s);
            if path.is_absolute() {
                path
            } else {
                project.repository.join(path)
            }
        })
    });
    let branch_result = git(
        &project.repository,
        vec![
            "branch".into(),
            "-r".into(),
            "--format=%(refname:short)".into(),
        ],
        256 * 1024,
    )
    .await;
    let branches = branch_result.as_deref().unwrap_or_default();
    let mut stages = BTreeMap::<String, usize>::new();
    for stage in STAGES {
        stages.insert(stage.into(), 0);
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut ancestry = BTreeMap::<String, bool>::new();
    let mut logs = BTreeMap::new();
    let mut remaining = TOTAL_LOG_BYTES;
    let deadline = Instant::now() + Duration::from_secs(30);
    for task in task_list {
        let tid = string(&task["id"]);
        if !valid_id(tid) || !seen.insert(tid) {
            return err("invalid or duplicate major release task ID");
        }
        let claim = task["status"].as_str().unwrap_or("todo");
        let owner = task["owner_screen"].as_str().unwrap_or("");
        let alive = screens
            .iter()
            .any(|s| (s["name"] == owner || s["id"] == owner) && s["agent_running"] == true);
        let task_prs = prs
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| {
                mentions(
                    &format!(
                        "{} {}",
                        string(&p["headRefName"]),
                        string(&p["title"]).split(':').next().unwrap_or("")
                    ),
                    tid,
                )
            })
            .collect::<Vec<_>>();
        let open = task_prs.iter().copied().find(|p| p["state"] == "OPEN");
        let merged = task_prs.iter().copied().find(|p| {
            p["state"] == "MERGED" && p["baseRefName"].as_str() == reference.rsplit('/').next()
        });
        let branch = branches.lines().find(|b| mentions(b, tid));
        let receipt_path = verify_dir.as_ref().map(|d| d.join(format!("{tid}.json")));
        let mut proof = false;
        let mut pass_claim = false;
        let mut why = "no receipt".to_owned();
        if let Some(rpath) = &receipt_path
            && let Ok(receipt) =
                proof_log(rpath, 2 * 1024 * 1024).and_then(|bytes| unique_json(&bytes))
        {
            if string(&receipt["status"]).eq_ignore_ascii_case("PASS") {
                pass_claim = true;
                let sha = string(&receipt["sha"]);
                let checkout = receipt["checkout_sha"].as_str().unwrap_or(sha);
                if !oid(sha) || checkout != sha {
                    why = "bad or mismatched receipt commit".into()
                } else if Instant::now() >= deadline {
                    why = "receipt observation budget exhausted; proof unknown".into()
                } else {
                    let on_main = if sha == main {
                        true
                    } else if let Some(value) = ancestry.get(sha) {
                        *value
                    } else {
                        let ok = git(
                            &project.repository,
                            vec![
                                "merge-base".into(),
                                "--is-ancestor".into(),
                                sha.into(),
                                main.clone(),
                            ],
                            256,
                        )
                        .await
                        .is_ok();
                        ancestry.insert(sha.into(), ok);
                        ok
                    };
                    if !on_main {
                        why = "receipt commit not demonstrated on configured main ref".into()
                    } else if let Err(error) =
                        verification_time(&project.repository, &receipt, sha).await
                    {
                        why = error
                    } else {
                        match verify_cases(task, &receipt, sha, rpath, &mut remaining, &mut logs) {
                            Ok(()) => {
                                proof = true;
                                why = format!(
                                    "PASS receipt and hashed execution logs on {}",
                                    &sha[..12]
                                );
                            }
                            Err(error) => why = error,
                        }
                    }
                }
            } else {
                why = format!("receipt status {}", short(string(&receipt["status"]), 80));
            }
        }
        let (stage, reason) = if proof {
            ("done", why.clone())
        } else if let Some(pr) = merged {
            (
                "on_main",
                format!("merged PR {}; receipt: {why}", pr["number"]),
            )
        } else if pass_claim {
            (
                "in_progress",
                format!("Unverified PASS claim; no completion or main integration inferred: {why}"),
            )
        } else if let Some(pr) = open {
            ("in_pr", format!("open PR {}", pr["number"]))
        } else if let Some(branch) = branch {
            ("in_progress", format!("observed remote branch {branch}"))
        } else if claim.to_ascii_lowercase().starts_with("blocked") {
            ("blocked", format!("tracker claim {claim}"))
        } else if ["in_progress", "partial", "pass"].contains(&claim.to_ascii_lowercase().as_str())
            && alive
        {
            (
                "in_progress",
                format!("tracker claim {claim}; matching agent observed"),
            )
        } else {
            (
                "not_started",
                format!("no corroborating branch/PR/receipt; tracker claim {claim}"),
            )
        };
        *stages.entry(stage.into()).or_default() += 1;
        let status = match stage {
            "done" => "done",
            "on_main" => "done-pending-verify",
            "in_pr" | "in_progress" => "in_progress",
            "blocked" => "blocked",
            _ => "todo",
        };
        rows.push(json!({"id":tid,"title":short(task["completion_criterion"].as_str().or_else(||task["title"].as_str()).unwrap_or(""),300),"release":task["release"],"gate":task["gate"],"owner":owner,"status":status,"stage":stage,"why":reason,"tracker_claim":claim,"pr":open.or(merged),"receipt":receipt_path,"receipt_check":why,"deps":task.get("deps").or_else(||task.get("dependencies"))}));
    }
    let mut result = json!({"total":rows.len(),"done":stages["done"],"counts":{"done":stages["done"],"in_progress":stages["in_pr"]+stages["in_progress"]+stages["on_main"],"blocked":stages["blocked"],"open":stages["not_started"]},"stages":stages,"stage_labels":{"done":"done (on-main PASS receipt and hashed execution logs)","on_main":"merged into configured main, awaiting valid verification","in_pr":"open PR","in_progress":"branch or matching live agent","not_started":"no corroborating work evidence"},"ledger_source":format!("{reference}:{path} @ {main}; tracker status is a claim"),"main_sha":main,"verify_dir":verify_dir,"stage_meta":{"note":"Only matching successful receipt cases with actual hashed logs on a commit contained in configured main qualify done. No network Git fetch is performed."}});
    result["ledger_updated_utc"] = json!(ledger_updated);
    result["evidence_coverage"] = json!({"tracker":"read from configured local main ref","remote_ref_refresh":"external; observer does not fetch","pr_source":if prs.is_array(){"observed"}else{"unknown"},"remote_branches":if branch_result.is_ok(){"observed"}else{"unknown"}});
    let mut flags = Vec::new();
    if !prs.is_array() {
        flags.push("PR evidence unavailable; some work stages may be unknown.");
    }
    if branch_result.is_err() {
        flags.push("Remote branch evidence unavailable; some work stages may be unknown.");
    }
    result["flags"] = json!(flags);
    result["raw_status_counts"] =
        count_values(rows.iter().map(|r| string(&r["tracker_claim"]).to_owned()));
    result["stage_by_task"] = json!(
        rows.iter()
            .map(|r| (string(&r["id"]), json!({"stage":r["stage"],"why":r["why"]})))
            .collect::<BTreeMap<_, _>>()
    );
    result["recent"]=json!(rows.iter().filter(|r|r["stage"]!="not_started").take(20).map(|r|json!({"task":r["id"],"old":null,"new":r["status"],"at":null,"note":r["why"],"source":"receipt/PR/branch observation"})).collect::<Vec<_>>());
    result["mr_tasks"] = json!(rows);
    Ok(result)
}
fn snapshot(project: &ProjectConfig) -> crate::Result<Value> {
    let path = source_path(project, "path").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "snapshot path missing")
    })?;
    let value = read_json(&path, SOURCE_LIMIT)?;
    let value = if let Some(key) = project.tracker["project_key"].as_str() {
        value
            .get("projects")
            .and_then(|v| v.get(key))
            .or_else(|| value.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        value
    };
    if !value.is_object() {
        return err("snapshot project object missing");
    };
    Ok(value)
}
fn observed_statuses(record: &Value) -> BTreeMap<String, String> {
    if let Some(states) = record["_statuses"].as_object() {
        return states
            .iter()
            .filter_map(|(id, status)| status.as_str().map(|s| (id.clone(), s.to_owned())))
            .collect();
    }
    record["stage_by_task"]
        .as_object()
        .map(|states| {
            states
                .iter()
                .filter_map(|(id, state)| {
                    state["stage"].as_str().map(|s| (id.clone(), s.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}
fn attach_recent(record: &mut Value, previous: &Value) {
    let before = observed_statuses(previous);
    let after = observed_statuses(record);
    let at = crate::now_utc();
    let mut recent = Vec::new();
    for (id, state) in &after {
        if let Some(old) = before.get(id)
            && old != state
        {
            recent.push(json!({"task":id,"old":old,"new":state,"at":at,"source":"Rust observer: observed ledger state change"}));
        }
    }
    recent.extend(record["recent"].as_array().into_iter().flatten().cloned());
    recent.extend(
        previous["recent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["source"] == "Rust observer: observed ledger state change")
            .cloned(),
    );
    let mut seen = BTreeSet::new();
    recent.retain(|v| seen.insert(serde_json::to_string(v).unwrap_or_default()));
    record["recent"] = json!(recent.into_iter().take(30).collect::<Vec<_>>());
}
/// Every configured project gets a record, including a truthful unknown record
/// on missing/malformed inputs. No failure becomes an empty successful ledger.
pub async fn collect(config: &ObserveConfig, screens: &Value) -> crate::Result<Value> {
    if config.projects.len() > 128 {
        return Err("configured project count exceeds bound".into());
    }
    let mut identities = BTreeSet::new();
    for project in &config.projects {
        if !valid_id(&project.id) || !identities.insert(project.id.as_str()) {
            return Err("invalid or duplicate configured project identity".into());
        }
    }
    let screen_list = screens
        .as_array()
        .or_else(|| screens.get("screens").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut projects = Map::new();
    let previous =
        read_json(&config.output_dir.join("status.json"), 32 * 1024 * 1024).unwrap_or(Value::Null);
    for project in config.projects.iter().take(128) {
        let prs = pull_requests(config, project).await;
        let result = match project.tracker["kind"].as_str().unwrap_or("snapshot") {
            "task_array" => task_array(project),
            "packet_progress" => packet_progress(project),
            "markdown_table" => markdown_table(project),
            "major_release" => major_release(project, screen_list, &prs).await,
            "snapshot" => snapshot(project),
            _ => err("unsupported tracker adapter"),
        };
        let mut record = match result {
            Ok(v) => v,
            Err(error) => {
                json!({"total":null,"done":null,"counts":{},"stages":{},"raw_status_counts":{},"error":format!("ledger unavailable: {}",short(&error.to_string(),300)),"flags":["Task counts and acceptance are unknown until this source is readable."]})
            }
        };
        if let Some(extra) = optional_json(project, "supplemental_path") {
            for key in [
                "lease",
                "leases",
                "validator",
                "live_status",
                "waves",
                "active_claims",
                "in_progress_breakdown",
                "assigned_from_captain_state",
                "queued_assignments_no_live_screen",
                "previous_milestone",
            ] {
                if let Some(v) = extra.get(key) {
                    record[key] = v.clone();
                }
            }
        }
        record["name"] = json!(project.name);
        record["repository"] = json!(project.repository);
        record["observer"] = json!("admiralty-observe Rust read-only project adapter");
        record["observed_utc"] = json!(crate::now_utc());
        if !prs.is_null() || record.get("pr").is_none() {
            record["pr"] = select_pr(project, &prs);
        }
        let manager = project.manager_screen.as_deref().and_then(|name| {
            let matches = screen_list
                .iter()
                .filter(|s| s["id"] == name || s["name"] == name)
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                matches.first().copied()
            } else {
                None
            }
        });
        record["captain"]=manager.cloned().unwrap_or_else(||json!({"name":project.manager_screen,"state":"unknown","model":"unknown","identity_state":"absent"}));
        record["workers"] = json!(
            screen_list
                .iter()
                .filter(|s| manager.is_none_or(|m| !std::ptr::eq(*s, m))
                    && project
                        .worker_prefixes
                        .iter()
                        .any(|p| !p.is_empty() && string(&s["name"]).starts_with(p)))
                .cloned()
                .collect::<Vec<_>>()
        );
        if let Some(path) = &project.coordination_log {
            match read_text_tail(path, 64 * 1024) {
                Ok(text) => {
                    record["coord_log"] = json!({"path":path,"tail":text.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev().map(|l|short(l,1500)).collect::<Vec<_>>(),"mtime_utc":mtime_utc(path)});
                }
                Err(_) => {
                    record["coord_log"] = json!({"path":path,"error":"coordination source unavailable","mtime_utc":mtime_utc(path)})
                }
            }
        }
        attach_recent(&mut record, &previous["projects"][&project.id]);
        projects.insert(project.id.clone(), record);
    }
    Ok(Value::Object(projects))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project(root: &Path, tracker: Value) -> ProjectConfig {
        ProjectConfig {
            id: "any-project".into(),
            name: "Configured project".into(),
            repository: root.into(),
            tracker,
            manager_screen: None,
            worker_prefixes: vec![],
            coordination_log: None,
            git_health: None,
        }
    }
    #[test]
    fn task_array_preserves_raw_counts_and_progress_metadata() {
        let root = tempfile::tempdir().expect("fixture");
        let p = root.path().join("tasks.json");
        fs::write(&p,json!({"revision":3,"tasks":[{"id":"A","status":"done","kind":"planning"},{"id":"B","status":"review"},{"id":"C","status":"unknown"}]}).to_string()).expect("tasks");
        let progress = root.path().join("progress.json");
        fs::write(&progress,json!({"ledger_revision":4,"blockers":["independent review needed"],"active_leases":[{"owner":"w","tasks":["B"],"lease_id":"L"}]}).to_string()).expect("progress");
        let result = task_array(&project(
            root.path(),
            json!({"path":p,"progress_path":progress}),
        ))
        .expect("actual adapter");
        assert_eq!(result["total"], 3);
        assert_eq!(result["stages"]["done"], 1);
        assert_eq!(result["stages"]["unknown"], 1);
        assert_eq!(result["raw_status_counts"]["review"], 1);
        assert_eq!(result["ledger_revision"], 4);
        assert_eq!(result["leases"][0]["owner"], "w");
    }
    #[test]
    fn packets_missing_progress_is_unknown_and_audit_holds_are_preserved() {
        let root = tempfile::tempdir().expect("fixture");
        let tasks = root.path().join("TASKS.json");
        fs::write(&tasks,json!({"packets":[{"id":"P1","tasks":[{"id":"T-01"},{"id":"T-02"}]},{"id":"P2","tasks":[{"id":"T-03"}]}]}).to_string()).expect("tasks");
        fs::write(
            root.path().join("P1.json"),
            json!({"tasks":{"T-01":{"status":"done"},"T-02":{"status":"todo"}}}).to_string(),
        )
        .expect("progress");
        let audit_path = root.path().join("audit.md");
        fs::write(
            &audit_path,
            "Verdict: FAIL\nT-02 AUDIT-HELD\n```\nVerdict: FAIL\n```\n> Verdict: FAIL\n",
        )
        .expect("audit");
        let value = packet_progress(&project(
            root.path(),
            json!({"path":tasks,"progress_dir":root.path(),"validator_path":audit_path}),
        ))
        .expect("packets");
        assert_eq!(value["total"], 3);
        assert_eq!(value["stages"]["done"], 1);
        assert_eq!(value["stages"]["blocked"], 1);
        assert_eq!(value["stages"]["unknown"], 1);
        assert_eq!(value["validator"]["fail_mentions"], 1);
        assert_eq!(value["raw_status_counts"]["todo"], 1);
    }
    #[test]
    fn markdown_status_column_handles_unescaped_math_pipes_and_fenced_examples() {
        let text = "| Task ID | Title | Details | Status | Owner |\n| --- | --- | --- | --- | --- |\n| WP-01 | One | norm |x| | `DONE` | worker |\n| CERT-01 | Two | proof | BLOCKED awaiting AUT-01 | w |\n```\n| Task ID | Title | Status |\n| FAKE-01 | fake | DONE |\n```";
        let parsed = markdown_tasks(text).expect("markdown");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed["WP-01"]["status"], "DONE");
        assert_eq!(parsed["CERT-01"]["status"], "BLOCKED");
        assert!(
            parsed["CERT-01"]["status_text"]
                .as_str()
                .expect("status")
                .contains("AUT-01")
        );
    }
    #[test]
    fn major_task_matching_does_not_confuse_adjacent_ids() {
        assert!(mentions("feat/mr-01-implementation", "MR01"));
        assert!(mentions("MRFV final verifier", "MRFV"));
        assert!(!mentions("feat/mr-010-implementation", "MR01"));
        assert!(!mentions("prefixmr01suffix", "MR01"));
    }
    #[test]
    fn receipt_hashes_real_bytes_and_rejects_mutation_or_mismatched_task() {
        let root = tempfile::tempdir().expect("fixture");
        let root = fs::canonicalize(root.path()).expect("canonical");
        let log = root.join("run.log");
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let bytes = format!(
            "checkout_sha: {sha}\nexit_code: 0\n--- STDOUT ---\ntest result: ok. 2 passed; 0 failed;\n"
        );
        fs::write(&log, &bytes).expect("actual log");
        let task = json!({"id":"MR01","completion_criterion":"scope","acceptance_cases":["case1"]});
        let receipt = json!({"task_id":"MR01","completion_criterion":"scope","cases":[{"acceptance_case":"case1","exit_code":0,"passed":true,"log_path":log,"log_sha256":format!("{:x}",Sha256::digest(bytes.as_bytes())),"checkout_sha":sha}]});
        let mut remaining = TOTAL_LOG_BYTES;
        assert!(
            verify_cases(
                &task,
                &receipt,
                sha,
                &root.join("MR01.json"),
                &mut remaining,
                &mut BTreeMap::new()
            )
            .is_ok()
        );
        fs::write(&log, "tampered").expect("mutate");
        let mut remaining = TOTAL_LOG_BYTES;
        assert!(
            verify_cases(
                &task,
                &receipt,
                sha,
                &root.join("MR01.json"),
                &mut remaining,
                &mut BTreeMap::new()
            )
            .is_err()
        );
        let mut wrong = receipt.clone();
        wrong["task_id"] = json!("MR02");
        assert!(
            verify_cases(
                &task,
                &wrong,
                sha,
                &root.join("MR01.json"),
                &mut remaining,
                &mut BTreeMap::new()
            )
            .is_err()
        );
    }
    #[test]
    fn receipt_duplicate_fields_and_nested_collisions_are_rejected() {
        assert!(unique_json(br#"{"status":"FAIL","status":"PASS"}"#).is_err());
        assert!(unique_json(br#"{"cases":[{"exit_code":1,"exit_code":0}]}"#).is_err());
        assert_eq!(
            unique_json(br#"{"a":[true,null,2,-3,4.5,"text"]}"#).expect("ordinary receipt JSON")["a"]
                [2],
            2
        );
    }
    #[tokio::test]
    async fn missing_tracker_emits_unknown_not_empty_success() {
        let root = tempfile::tempdir().expect("fixture");
        let config:ObserveConfig=serde_json::from_value(json!({"schema_version":1,"node_id":"configured-node","output_dir":root.path(),"projects":[{"id":"configured","name":"Configured","repository":root.path(),"tracker":{"kind":"task_array","path":root.path().join("missing.json")}}]})).expect("config");
        let records = collect(&config, &json!([]))
            .await
            .expect("error record, not failed observer");
        assert!(records["configured"]["total"].is_null());
        assert!(records["configured"]["done"].is_null());
        assert_eq!(records["configured"]["stages"], json!({}));
        assert!(records["configured"]["error"].is_string());
    }
    #[tokio::test]
    async fn major_release_reads_real_main_tracker_and_requires_receipt_logs_for_done() {
        let owned = tempfile::tempdir().expect("fixture");
        let root = fs::canonicalize(owned.path()).expect("canonical fixture");
        let verify = root.join("verify");
        fs::create_dir(&verify).expect("verify directory");
        let git_setup = |args: &[&str]| {
            let result = std::process::Command::new("/usr/bin/git")
                .arg("-c")
                .arg("core.hooksPath=/dev/null")
                .arg("-c")
                .arg("user.name=Observer Test")
                .arg("-c")
                .arg("user.email=observer@example.invalid")
                .arg("-C")
                .arg(&root)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .expect("fixture Git");
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout)
                .expect("Git UTF8")
                .trim()
                .to_owned()
        };
        git_setup(&["init", "--quiet"]);
        let tasks=(1..=47).map(|i|json!({"id":format!("MR{i:02}"),"status":"done","owner_screen":"requested-manager","completion_criterion":"fixture scope","acceptance_cases":["actual-case"]})).collect::<Vec<_>>();
        fs::write(
            root.join("tracker.json"),
            json!({"tasks":tasks}).to_string(),
        )
        .expect("tracked ledger");
        git_setup(&["add", "tracker.json"]);
        git_setup(&["commit", "--quiet", "-m", "fixture tracker"]);
        let sha = git_setup(&["rev-parse", "HEAD"]);
        git_setup(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // A newer uncommitted working-tree claim must not replace origin/main.
        fs::write(root.join("tracker.json"), r#"{"tasks":[]}"#).expect("working tree alteration");
        let log = verify.join("case.log");
        let bytes = format!(
            "checkout_sha: {sha}\nexit_code: 0\n--- STDOUT ---\ntest result: ok. 10 passed; 0 failed;\n"
        );
        fs::write(&log, &bytes).expect("fixture log bytes");
        let receipt = json!({"task_id":"MR01","status":"PASS","sha":sha,"checkout_sha":sha,"completion_criterion":"fixture scope","cases":[{"acceptance_case":"actual-case","exit_code":0,"passed":true,"checkout_sha":sha,"log_path":log,"log_sha256":format!("{:x}",Sha256::digest(bytes.as_bytes()))}]});
        fs::write(verify.join("MR01.json"), receipt.to_string()).expect("receipt");
        let p = project(
            &root,
            json!({"kind":"major_release","path":"tracker.json","ref":"origin/main","verify_dir":verify}),
        );
        let observed = major_release(
            &p,
            &[json!({"name":"requested-manager","agent_running":false})],
            &json!([]),
        )
        .await
        .expect("actual Git main adapter");
        assert_eq!(observed["total"], 47);
        assert_eq!(observed["stages"]["done"], 1);
        assert_eq!(observed["stages"]["not_started"], 46);
        assert_eq!(observed["stage_by_task"]["MR01"]["stage"], "done");
        assert_eq!(observed["stage_by_task"]["MR02"]["stage"], "not_started");
        fs::write(&log, "mutated after receipt").expect("mutation");
        let bad = major_release(&p, &[], &json!([]))
            .await
            .expect("bad proof remains observation");
        assert_eq!(bad["stages"]["done"], 0);
        assert_eq!(
            bad["stage_by_task"]["MR01"]["stage"], "in_progress",
            "a PASS claim cannot imply main integration"
        );
        let prs = json!([{"number":1,"title":"MR02: feature branch","state":"MERGED","baseRefName":"feature","headRefName":"work/mr02"},{"number":2,"title":"MR03: main branch","state":"MERGED","baseRefName":"main","headRefName":"work/mr03"}]);
        let mixed = major_release(&p, &[], &prs)
            .await
            .expect("merged PR source distinctions");
        assert_ne!(mixed["stage_by_task"]["MR02"]["stage"], "on_main");
        assert_eq!(mixed["stage_by_task"]["MR03"]["stage"], "on_main");
        assert!(string(&bad["stage_by_task"]["MR01"]["why"]).contains("SHA-256 differs"));
    }
    #[test]
    fn recent_state_changes_preserve_previous_event_without_inventing_initial_completion() {
        let previous = json!({"_statuses":{"T1":"todo"},"recent":[]});
        let mut current = json!({"_statuses":{"T1":"in_progress"}});
        attach_recent(&mut current, &previous);
        assert_eq!(current["recent"][0]["old"], "todo");
        assert_eq!(current["recent"][0]["new"], "in_progress");
        let mut unchanged = json!({"_statuses":{"T1":"in_progress"}});
        attach_recent(&mut unchanged, &current);
        assert_eq!(unchanged["recent"], current["recent"]);
        let mut initial = json!({"_statuses":{"T1":"done"}});
        attach_recent(&mut initial, &Value::Null);
        assert_eq!(initial["recent"], json!([]));
    }
}
