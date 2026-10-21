//! Bounded read-only fleet observations. Process presence is not task progress.
use crate::config::{NodeConfig, ObserveConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_NODES: usize = 32;
const MAX_SCREENS: usize = 256;
const MAX_CAPTURES: usize = 64;
const MAX_PROCESS_NODES: usize = 512;
const PROBE_BYTES: usize = 128 * 1024;
const TAIL_BYTES: usize = 32 * 1024;
const MODEL_CACHE_BYTES: u64 = 512 * 1024;
const MODEL_CACHE_TTL_SECONDS: i64 = 300;

// Static scripts contain no configuration interpolation, credentials, or writes.
const LINUX_PROBE: &str = r#"
printf '\n@@NPROC\n'; getconf _NPROCESSORS_ONLN
printf '\n@@LOAD\n'; cat /proc/loadavg
printf '\n@@MEM\n'; cat /proc/meminfo
printf '\n@@DF\n'; df -Pk
printf '\n@@GPU\n'
if test -x /usr/bin/nvidia-smi; then /usr/bin/nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits; fi
printf '\n@@SCREENS\n'; /usr/bin/screen -ls
printf '\n@@SCREEN_RC %s\n' "$?"
printf '\n@@END\n'
"#;
const MAC_PROBE: &str = r#"
printf '\n@@NPROC\n'; /usr/sbin/sysctl -n hw.ncpu
printf '\n@@LOAD\n'; /usr/sbin/sysctl -n vm.loadavg
printf '\n@@MEMTOTAL\n'; /usr/sbin/sysctl -n hw.memsize
printf '\n@@VM\n'; /usr/bin/vm_stat
printf '\n@@DF\n'; /bin/df -Pk
printf '\n@@SCREENS\n'; /usr/bin/screen -ls
printf '\n@@SCREEN_RC %s\n' "$?"
printf '\n@@END\n'
"#;

pub async fn collect(config: &ObserveConfig) -> crate::Result<Value> {
    let mut nodes = config.nodes.clone();
    if !nodes
        .iter()
        .any(|n| n.id == config.node_id && n.ssh.is_none())
    {
        nodes.insert(
            0,
            NodeConfig {
                id: config.node_id.clone(),
                ssh: None,
                os: if cfg!(target_os = "macos") {
                    "macos"
                } else {
                    "linux"
                }
                .into(),
            },
        );
    }
    let mut warnings = Vec::new();
    if nodes.len() > MAX_NODES {
        warnings.push("Configured node observation bound exceeded; excess nodes unobserved.");
    }
    let mut tasks = tokio::task::JoinSet::new();
    let mut hosts = Vec::new();
    for node in nodes.into_iter().take(MAX_NODES) {
        tasks.spawn(async move { observe_node(&node).await });
        if tasks.len() >= 4 {
            match tasks.join_next().await {
                Some(Ok(host)) => hosts.push(host),
                Some(Err(_)) => {
                    warnings.push("A resource observation did not complete; its state is unknown.")
                }
                None => {}
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(host) => hosts.push(host),
            Err(_) => {
                warnings.push("A resource observation did not complete; its state is unknown.")
            }
        }
    }
    hosts.sort_by(|a, b| a["host"].as_str().cmp(&b["host"].as_str()));
    let local = hosts
        .iter()
        .find(|h| h["host"] == config.node_id && h["local"] == true);
    let mut screens = local
        .and_then(|h| h["screen_observations"].as_array())
        .cloned()
        .unwrap_or_default();
    let now = chrono::Utc::now();
    let mut cache = ModelCache::load(&config.output_dir);
    let processes = screens
        .iter()
        .map(|screen| {
            screen["pid"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .and_then(agent_process)
        })
        .collect::<Vec<_>>();
    let mut retained = BTreeMap::new();
    for (screen, process) in screens.iter_mut().zip(&processes) {
        screen["agent_running"] = process.as_ref().map_or(Value::Null, |_| json!(true));
        screen["agent"] = process.as_ref().map_or(Value::Null, |p| json!(p.family));
        screen["agent_pid"] = process.as_ref().map_or(Value::Null, |p| json!(p.pid));
        screen["model"] = Value::Null;
        screen["effort"] = Value::Null;
        screen["model_observed_utc"] = Value::Null;
        screen["model_source"] = json!("unknown; requested process model is not observed identity");
        screen["tail"] = json!([]);
        screen["task"] = Value::Null;
        screen["project"] = project_for_screen(config, screen["name"].as_str().unwrap_or_default())
            .map_or_else(|| json!("unassigned"), |v| json!(v));
        if let (Some(id), Some(process)) = (screen["id"].as_str(), process)
            && let Some(entry) = cache
                .entries
                .get(id)
                .filter(|entry| entry.matches(process, now))
        {
            retained.insert(id.to_owned(), entry.clone());
            apply_model(screen, entry, true);
        }
        screen["state"] = json!("unknown");
    }
    cache.entries = retained;
    if let Ok(directory) = CaptureDirectory::new(&config.output_dir) {
        let captures = Arc::new(directory);
        let order = capture_order(
            config,
            &screens,
            &processes,
            cache.cursor,
            cache.priority_cursor,
        );
        let capture_deadline = Instant::now() + Duration::from_secs(3);
        let mut next = 0;
        let mut worker_attempts = 0usize;
        let mut pending = tokio::task::JoinSet::new();
        loop {
            while pending.len() < 8
                && next < order.len().min(MAX_CAPTURES)
                && Instant::now() < capture_deadline
            {
                let (index, worker) = order[next];
                let id = screens[index]["id"].as_str().unwrap_or_default().to_owned();
                let directory = Arc::clone(&captures);
                let ordinal = next + 1;
                if worker {
                    worker_attempts += 1;
                }
                pending.spawn(async move { (index, directory.capture(&id, ordinal).await) });
                next += 1;
            }
            let Some(result) = pending.join_next().await else {
                break;
            };
            if let Ok((index, Some(text))) = result {
                let Some(before) = processes[index].as_ref() else {
                    continue;
                };
                let still_same = screens[index]["pid"]
                    .as_u64()
                    .and_then(|p| u32::try_from(p).ok())
                    .and_then(agent_process)
                    .is_some_and(|after| before.same_identity(&after));
                if !still_same {
                    let id = screens[index]["id"].as_str().unwrap_or_default();
                    cache.entries.remove(id);
                    screens[index]["model"] = Value::Null;
                    screens[index]["effort"] = Value::Null;
                    screens[index]["model_observed_utc"] = Value::Null;
                    screens[index]["model_source"] =
                        json!("unknown; process identity changed during capture");
                    continue;
                }
                let clean = safe_tail(&text);
                if let Some((model, effort)) = observed_footer_for_family(&clean, before.family) {
                    let entry = ModelObservation {
                        pid: before.pid,
                        start: before.start.clone(),
                        family: before.family.into(),
                        model,
                        effort,
                        observed_utc: chrono::Utc::now()
                            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    };
                    let screen = &mut screens[index];
                    cache.entries.insert(
                        screen["id"].as_str().unwrap_or_default().to_owned(),
                        entry.clone(),
                    );
                    apply_model(screen, &entry, false);
                }
                screens[index]["tail"] = json!(
                    clean
                        .lines()
                        .filter(|s| !s.trim().is_empty())
                        .rev()
                        .take(8)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                );
            }
        }
        cache.cursor = cache.cursor.wrapping_add(worker_attempts);
        cache.priority_cursor = cache.priority_cursor.wrapping_add(1);
    }
    if cache.save(&config.output_dir).is_err() {
        warnings.push("Model observation cache was not persisted; future uncaptured identities remain unknown.");
    }
    let mut remote_screens = Vec::new();
    for host in hosts.iter().filter(|h| h["local"] != true) {
        if let Some(observed) = host["screen_observations"].as_array() {
            for screen in observed {
                let mut s = screen.clone();
                s["agent_running"] = Value::Null;
                s["model"] = Value::Null;
                s["effort"] = Value::Null;
                s["state"] = json!("remote");
                s["tail"] = json!([]);
                s["task"] = Value::Null;
                s["project"] = project_for_screen(config, s["name"].as_str().unwrap_or_default())
                    .map_or_else(|| json!("unassigned"), |v| json!(v));
                remote_screens.push(s);
            }
        }
    }
    let agents = screens.iter().filter(|s| s["agent_running"] == true).map(|s| {
        json!({"id":s["id"],"screen":s["name"],"host":s["host"],"project":s["project"],
            "family":s["agent"],"model":s["model"],"effort":s["effort"],"model_source":s["model_source"],
            "model_observed_utc":s["model_observed_utc"],
            "state":"unknown","task":null,"flags":[],"tier":null})
    }).collect::<Vec<_>>();
    let super_captain = admiral_observation(config, &screens).await;
    // Growth needs this observer's own earlier samples; see disk_rates.
    let now_s = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
    if !crate::disk_rates::apply(&config.output_dir, &mut hosts, now_s) {
        warnings
            .push("Disk growth samples were not persisted; the next projection may be delayed.");
    }
    let resource_warnings = hosts
        .iter()
        .flat_map(|h| h["warnings"].as_array().into_iter().flatten().cloned())
        .collect::<Vec<_>>();
    Ok(
        json!({"screens":screens,"agents":agents,"remote_screens":remote_screens,
        "global_resources":{"generated_utc":crate::now_utc(),"hosts":hosts,"warnings":resource_warnings,"observation_warnings":warnings,
            "source":"admiralty.observe/rust","assurance":"Read-only probes; no enforcement, progress or acceptance authority."},
        "super_captain":super_captain}),
    )
}

fn project_for_screen<'a>(config: &'a ObserveConfig, name: &str) -> Option<&'a str> {
    let mut matches = config.projects.iter().filter(|p| {
        p.manager_screen
            .as_deref()
            .is_some_and(|s| screen_name(s) == name)
            || p.worker_prefixes
                .iter()
                .any(|prefix| !prefix.is_empty() && name.starts_with(prefix))
    });
    let first = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(&first.id)
    }
}

fn screen_name(value: &str) -> &str {
    value
        .split_once('.')
        .filter(|(pid, _)| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
        .map_or(value, |(_, name)| name)
}

fn valid_target(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 200
        && !target.starts_with('-')
        && target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
}

async fn observe_node(node: &NodeConfig) -> Value {
    let script = match node.os.as_str() {
        "linux" => LINUX_PROBE,
        "macos" | "darwin" => MAC_PROBE,
        _ => return unknown_host(node, "unsupported configured operating system"),
    };
    let (program, args) = if let Some(target) = &node.ssh {
        if !valid_target(target) {
            return unknown_host(node, "invalid SSH target; no command executed");
        }
        (
            Path::new("/usr/bin/ssh"),
            vec![
                "-o".into(),
                "BatchMode=yes".into(),
                "-o".into(),
                "ConnectTimeout=3".into(),
                "-o".into(),
                "ServerAliveInterval=2".into(),
                "-o".into(),
                "ServerAliveCountMax=1".into(),
                "--".into(),
                target.clone(),
                format!("LC_ALL=C /bin/sh -c {}", shell_quote(script)),
            ],
        )
    } else {
        (Path::new("/bin/sh"), vec!["-c".into(), script.into()])
    };
    match crate::io::run(program, &args, None, 10, PROBE_BYTES).await {
        Ok(output) if output.status == 0 && output.stdout.lines().any(|s| s == "@@END") => {
            parse_resources(node, &output.stdout)
        }
        _ => unknown_host(node, "resource probe unavailable or incomplete"),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn unknown_host(node: &NodeConfig, reason: &str) -> Value {
    json!({"host":node.id,"local":node.ssh.is_none(),"reachable":false,"error":reason,"nproc":null,
        "load":null,"mem":null,"disks":[],"gpus":[],"screens":[],"screen_observations":[],"screen_inventory_available":false,
        "top_rss":[],"d_state_procs":null,"warnings":[{"level":"amber","text":reason}]})
}

fn parse_resources(node: &NodeConfig, text: &str) -> Value {
    let mut sections = BTreeMap::<&str, Vec<&str>>::new();
    let mut section = "";
    let mut inventory = false;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("@@") {
            if let Some(rc) = name.strip_prefix("SCREEN_RC ") {
                inventory = matches!(rc, "0" | "1");
            }
            section = name;
        } else {
            sections.entry(section).or_default().push(line);
        }
    }
    let lines = |key| sections.get(key).map(Vec::as_slice).unwrap_or_default();
    let nproc = lines("NPROC")
        .first()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0 && *n <= 1_000_000);
    let load = lines("LOAD")
        .iter()
        .flat_map(|s| s.split_whitespace())
        .filter_map(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .take(3)
        .collect::<Vec<_>>();
    let mut memory = BTreeMap::new();
    for line in lines("MEM") {
        if let Some((key, rest)) = line.split_once(':')
            && let Some(size) = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .and_then(|v| v.checked_mul(1024))
        {
            memory.insert(key, size);
        }
    }
    let mut mem = Value::Null;
    if let (Some(total), Some(avail)) = (memory.get("MemTotal"), memory.get("MemAvailable")) {
        mem = memory_value(
            *total,
            *avail,
            memory.get("SwapTotal").copied(),
            memory.get("SwapFree").copied(),
            "kernel MemAvailable",
        );
    } else if let Some(total) = lines("MEMTOTAL")
        .first()
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        let page = lines("VM")
            .first()
            .and_then(|s| s.split("page size of ").nth(1))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok());
        let mut pages = BTreeMap::new();
        for line in lines("VM") {
            if let Some((key, value)) = line.split_once(':')
                && let Ok(n) = value.trim().trim_end_matches('.').parse::<u64>()
            {
                pages.insert(key, n);
            }
        }
        if let (Some(page), Some(free), Some(inactive)) =
            (page, pages.get("Pages free"), pages.get("Pages inactive"))
            && let Some(avail) = free
                .checked_add(*inactive)
                .and_then(|v| v.checked_mul(page))
        {
            mem = memory_value(total, avail, None, None, "free+inactive pages estimate");
        }
    }
    let disks = lines("DF")
        .iter()
        .filter_map(|line| parse_disk(line))
        .take(64)
        .collect::<Vec<_>>();
    let gpus = lines("GPU")
        .iter()
        .filter_map(|line| parse_gpu(line))
        .take(32)
        .collect::<Vec<_>>();
    let screen_text = lines("SCREENS").join("\n");
    let screens = parse_screens(&screen_text, &node.id);
    if inventory && screens.is_empty() {
        inventory = screen_text.to_lowercase().contains("no sockets found")
            || screen_text.to_lowercase().contains("no screen session");
    }
    let level = nproc.and_then(|n| {
        load.first().map(|l| {
            if *l > n as f64 {
                "red"
            } else if *l > 0.85 * n as f64 {
                "amber"
            } else {
                "ok"
            }
        })
    });
    let mut warnings = Vec::new();
    for disk in &disks {
        if disk["level"] != "ok" {
            warnings.push(json!({"level":disk["level"],"kind":"disk","text":format!("{} disk {} {:.1}% used",node.id,disk["mount"].as_str().unwrap_or("unknown"),disk["pct"].as_f64().unwrap_or_default())}));
        }
    }
    if mem["level"].as_str().is_some_and(|s| s != "ok") {
        warnings.push(
            json!({"level":mem["level"],"text":format!("{} memory availability is low",node.id)}),
        );
    }
    if level.is_some_and(|s| s != "ok") {
        warnings.push(json!({"level":level,"text":format!("{} CPU load is high",node.id)}));
    }
    json!({"host":node.id,"local":node.ssh.is_none(),"reachable":true,"error":null,"nproc":nproc,
        "load":if load.len()==3 {json!(load)} else {Value::Null},"load_level":level,"mem":mem,"disks":disks,"gpus":gpus,
        "screens":screens.iter().filter_map(|v| v["id"].as_str()).collect::<Vec<_>>(),"screen_observations":screens,
        "screen_inventory_available":inventory,"gpu_observation":if gpus.is_empty(){"unavailable_or_none_observed"}else{"observed"},"top_rss":[],"d_state_procs":null,"warnings":warnings})
}

fn memory_value(
    total: u64,
    avail: u64,
    swap_total: Option<u64>,
    swap_free: Option<u64>,
    basis: &str,
) -> Value {
    if total == 0 || avail > total {
        return Value::Null;
    }
    let pct = 100.0 * avail as f64 / total as f64;
    json!({"total_gb":total as f64/1073741824.0,"avail_gb":avail as f64/1073741824.0,"avail_pct":pct,
        "swap_total_gb":swap_total.map(|n|n as f64/1073741824.0),"swap_used_gb":swap_total.zip(swap_free).and_then(|(a,b)|a.checked_sub(b)).map(|n|n as f64/1073741824.0),
        "level":if pct<8.0 {"red"} else if pct<15.0 {"amber"} else {"ok"},"availability_basis":basis})
}

fn parse_disk(line: &str) -> Option<Value> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 6 || matches!(fields[0], "tmpfs" | "devtmpfs" | "overlay" | "devfs") {
        return None;
    }
    let size = fields[1].parse::<u64>().ok()?.checked_mul(1024)?;
    let used = fields[2].parse::<u64>().ok()?.checked_mul(1024)?;
    let avail = fields[3].parse::<u64>().ok()?.checked_mul(1024)?;
    let total = used.checked_add(avail)?;
    if total == 0 || size == 0 {
        return None;
    }
    let pct = 100.0 * used as f64 / total as f64;
    Some(
        json!({"mount":crate::io::clean(&fields[5..].join(" ")),"size":size,"used":used,"avail":avail,"pct":pct,
        "growth_gb_per_h":null,"hours_to_full":null,"rate_window_min":null,"level":if pct>=92.0 {"red"} else if pct>=85.0 {"amber"} else {"ok"}}),
    )
}

fn parse_gpu(line: &str) -> Option<Value> {
    let f = line.split(',').map(str::trim).collect::<Vec<_>>();
    if f.len() != 6 {
        return None;
    }
    let index = f[0].parse::<u64>().ok()?;
    let numbers = f[2..]
        .iter()
        .map(|s| s.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0))
        .collect::<Option<Vec<_>>>()?;
    Some(
        json!({"idx":index,"name":crate::io::clean(f[1]),"util":numbers[0],"mem_used_mib":numbers[1],"mem_total_mib":numbers[2],"temp_c":numbers[3]}),
    )
}

fn parse_screens(text: &str, host: &str) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    text.lines()
        .filter_map(|line| {
            let id = line.split_whitespace().next()?;
            let (pid, name) = id.split_once('.')?;
            let pid = pid.parse::<u32>().ok().filter(|v| *v > 1)?;
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                || !seen.insert(id.to_owned())
            {
                return None;
            }
            Some(json!({"id":id,"name":name,"pid":pid,"host":host,"alive":true}))
        })
        .take(MAX_SCREENS)
        .collect()
}

#[derive(Clone)]
struct AgentProcess {
    pid: u32,
    family: &'static str,
    start: String,
}

impl AgentProcess {
    fn same_identity(&self, other: &Self) -> bool {
        self.pid == other.pid && self.family == other.family && self.start == other.start
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelCache {
    cursor: usize,
    #[serde(default)]
    priority_cursor: usize,
    entries: BTreeMap<String, ModelObservation>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelObservation {
    pid: u32,
    start: String,
    family: String,
    model: String,
    effort: Option<String>,
    observed_utc: String,
}

impl ModelObservation {
    fn matches(&self, process: &AgentProcess, now: chrono::DateTime<chrono::Utc>) -> bool {
        let age = chrono::DateTime::parse_from_rfc3339(&self.observed_utc)
            .ok()
            .map(|at| now.signed_duration_since(at).num_seconds());
        self.pid == process.pid
            && self.start == process.start
            && self.family == process.family
            && model_matches_family(&self.model, process.family)
            && age.is_some_and(|seconds| (0..=MODEL_CACHE_TTL_SECONDS).contains(&seconds))
            && !self.model.is_empty()
            && self.model.len() <= 128
            && crate::io::clean(&self.model) == self.model
            && self
                .effort
                .as_deref()
                .is_none_or(|s| matches!(s, "low" | "medium" | "high" | "xhigh" | "max" | "ultra"))
    }
}

impl ModelCache {
    fn load(parent: &Path) -> Self {
        crate::io::read_json(
            &parent.join(".rust-model-observations.json"),
            MODEL_CACHE_BYTES,
        )
        .ok()
        .and_then(|v| serde_json::from_value::<Self>(v).ok())
        .filter(|cache| cache.entries.len() <= MAX_SCREENS)
        .unwrap_or_default()
    }
    fn save(&self, parent: &Path) -> crate::Result<()> {
        crate::io::atomic_json(
            &parent.join(".rust-model-observations.json"),
            &serde_json::to_value(self)?,
        )
    }
}

fn apply_model(screen: &mut Value, entry: &ModelObservation, cached: bool) {
    screen["model"] = json!(entry.model);
    screen["effort"] = json!(entry.effort);
    screen["model_observed_utc"] = json!(entry.observed_utc);
    screen["model_source"] = json!(if cached {
        "cached screen footer; matching process identity, at most 300s old; not authenticated provider identity"
    } else {
        "observed screen footer; not authenticated provider identity"
    });
}

fn capture_order(
    config: &ObserveConfig,
    screens: &[Value],
    processes: &[Option<AgentProcess>],
    cursor: usize,
    priority_cursor: usize,
) -> Vec<(usize, bool)> {
    let mut priority = Vec::new();
    let mut workers = Vec::new();
    for (index, screen) in screens.iter().enumerate() {
        if processes.get(index).is_none_or(Option::is_none) {
            continue;
        }
        let id = screen["id"].as_str().unwrap_or_default();
        let name = screen["name"].as_str().unwrap_or_default();
        let configured = |s: &str| s == id || (screen_name(s) == s && s == name);
        let admiral = config.admiral_screen.as_deref().is_some_and(configured);
        let manager = config
            .projects
            .iter()
            .any(|p| p.manager_screen.as_deref().is_some_and(configured));
        if admiral || manager {
            priority.push((!admiral, index));
        } else {
            workers.push(index);
        }
    }
    priority.sort_by_key(|&(not_admiral, index)| (not_admiral, index));
    // Admiral stays first; project managers rotate when more than one batch is
    // needed. Every batch also reserves slots for ordinary task workers.
    let rotate_from = usize::from(
        priority
            .first()
            .is_some_and(|(not_admiral, _)| !not_admiral),
    );
    let rotate_count = priority.len().saturating_sub(rotate_from);
    if rotate_count > 0 {
        priority[rotate_from..].rotate_left(priority_cursor % rotate_count);
    }
    workers.sort_by(|&a, &b| screens[a]["id"].as_str().cmp(&screens[b]["id"].as_str()));
    let count = workers.len();
    if count > 0 {
        workers.rotate_left(cursor % count);
    }
    let priority = priority
        .into_iter()
        .map(|(_, index)| index)
        .collect::<Vec<_>>();
    let mut order = Vec::new();
    for chunk in 0..priority.len().max(workers.len()).div_ceil(4) {
        order.extend(
            priority
                .iter()
                .skip(chunk * 4)
                .take(4)
                .map(|&index| (index, false)),
        );
        order.extend(
            workers
                .iter()
                .skip(chunk * 4)
                .take(4)
                .map(|&index| (index, true)),
        );
    }
    order
}

fn process_start(pid: u32) -> Option<String> {
    let stat = read_proc_bounded(&format!("/proc/{pid}/stat"), 4096)?;
    let stat = std::str::from_utf8(&stat).ok()?;
    // comm may contain spaces and ')'; the final ')' precedes field 3.
    let ticks = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()?;
    let boot = read_proc_bounded("/proc/sys/kernel/random/boot_id", 64)?;
    let boot = std::str::from_utf8(&boot).ok()?.trim();
    uuid::Uuid::parse_str(boot).ok()?;
    Some(format!("{boot}:{ticks}"))
}

fn agent_process(screen_pid: u32) -> Option<AgentProcess> {
    // /proc ancestry is used only on Linux. Missing ancestry is unknown, never
    // proof of an absent agent; no command line or prompt is published.
    let mut frontier = vec![screen_pid];
    let mut seen = BTreeSet::new();
    for _ in 0..7 {
        let mut next = Vec::new();
        for pid in frontier {
            if !seen.insert(pid) || seen.len() > MAX_PROCESS_NODES {
                return None;
            }
            let Some(children) =
                read_proc_bounded(&format!("/proc/{pid}/task/{pid}/children"), 16 * 1024)
            else {
                continue;
            };
            for child in String::from_utf8_lossy(&children)
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .filter(|p| *p > 1)
                .take(128)
            {
                let before = process_start(child);
                if let Some(bytes) =
                    read_proc_bounded(&format!("/proc/{child}/cmdline"), 128 * 1024)
                {
                    let args = bytes
                        .split(|b| *b == 0)
                        .take(3)
                        .filter_map(|b| std::str::from_utf8(b).ok())
                        .collect::<Vec<_>>();
                    if let Some(family) = agent_family(&args)
                        && let Some(start) =
                            before.filter(|start| process_start(child).as_ref() == Some(start))
                    {
                        return Some(AgentProcess {
                            pid: child,
                            family,
                            start,
                        });
                    }
                }
                next.push(child);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    None
}

fn read_proc_bounded(path: &str, max: usize) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= max).then_some(bytes)
}

fn agent_family(args: &[&str]) -> Option<&'static str> {
    for (i, arg) in args.iter().enumerate().take(2) {
        let name = Path::new(arg).file_name()?.to_str()?;
        let family = match name {
            "agy" => "agy",
            "codex" => "codex",
            "claude" => "claude",
            "gemini" => "gemini",
            "opencode" => "opencode",
            _ => continue,
        };
        if i == 0
            || args.first().is_some_and(|s| {
                matches!(
                    Path::new(s).file_name().and_then(|n| n.to_str()),
                    Some("node" | "nodejs" | "bun")
                )
            })
        {
            return Some(family);
        }
    }
    None
}

fn safe_tail(text: &str) -> String {
    crate::io::clean(text)
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.contains(".private")
                || line.contains("CONTROL")
                || line.trim_start().starts_with(['>', '›'])
            {
                "[input/private line withheld]".into()
            } else {
                line.chars().take(240).collect::<String>()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn observed_footer(text: &str) -> Option<(String, Option<String>)> {
    if text.len() > TAIL_BYTES {
        return None;
    }
    for line in text.lines().rev().take(16) {
        if line.len() > 1024 {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let trimmed = line.trim_start();
        let cancel_footer = trimmed
            .get(..13)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("esc to cancel"))
            && trimmed
                .get(13..)
                .is_some_and(|rest| rest.chars().next().is_some_and(char::is_whitespace));
        if !(lower.contains("context left")
            || (trimmed.starts_with('│') && trimmed.trim_end().ends_with('│'))
            || codex_path_footer(trimmed)
            || cancel_footer
            || lower.trim_start().starts_with("model:"))
            || lower.contains("withheld")
        {
            continue;
        }
        // GNU Screen can decode the UI separator into U+FFFD. The exact known
        // footer prefix still identifies the status row, without interpreting
        // requested models from command lines or prose as observed identity.
        let body = if cancel_footer { &trimmed[13..] } else { line };
        let tokens = body
            .split_whitespace()
            .map(|t| t.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || "-._/".contains(c))))
            .collect::<Vec<_>>();
        for (i, t) in tokens.iter().enumerate() {
            let model_label = i == 1 && lower.trim_start().starts_with("model:");
            if (cancel_footer && i != 0)
                || (!model_label && !tokens[..i].iter().all(|token| token.is_empty()))
            {
                continue;
            }
            let normalized = t.to_ascii_lowercase();
            let (model, consumed) = if (normalized.starts_with("gpt-") && normalized.len() > 4)
                || (normalized.starts_with("claude-") && normalized.len() > 7)
            {
                (normalized, 1)
            } else if matches!(
                t.to_ascii_lowercase().as_str(),
                "haiku" | "sonnet" | "opus" | "fable"
            ) {
                let version = tokens
                    .get(i + 1)
                    .filter(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()));
                version.map_or_else(
                    || ((*t).to_owned(), 1),
                    |version| (format!("{t} {version}"), 2),
                )
            } else if t.eq_ignore_ascii_case("Gemini")
                && tokens.get(i + 1).is_some_and(|s| {
                    s.chars().next().is_some_and(|c| c.is_ascii_digit())
                        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
                })
                && tokens.get(i + 2).is_some_and(|s| {
                    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
            {
                (
                    format!("Gemini {} {}", tokens.get(i + 1)?, tokens.get(i + 2)?),
                    3,
                )
            } else if i == 1 && lower.trim_start().starts_with("model:") {
                ((*t).to_owned(), 1)
            } else {
                continue;
            };
            if model.len() > 128 || !specific_model_label(&model) || i > 2 {
                continue;
            }
            let effort = tokens
                .iter()
                .skip(i + consumed)
                .find(|token| !token.is_empty() && **token != "-")
                .filter(|s| matches!(**s, "low" | "medium" | "high" | "xhigh" | "max" | "ultra"))
                .map(|s| (*s).to_owned());
            return Some((model, effort));
        }
    }
    None
}

fn specific_model_label(model: &str) -> bool {
    !model.is_empty()
        && !matches!(
            model.to_ascii_lowercase().as_str(),
            "codex"
                | "claude"
                | "gemini"
                | "opencode"
                | "agy"
                | "unknown"
                | "auto"
                | "default"
                | "none"
                | "null"
        )
}

// The observed Codex status row has model[/effort], a rendered separator,
// then its workspace path. A replacement character alone in prose is not a
// footer marker. Existing context/box/label formats are handled above.
fn codex_path_footer(line: &str) -> bool {
    let Some((left, right)) = line.split_once(['\u{fffd}', '·', '│']) else {
        return false;
    };
    let mut tokens = left.split_whitespace();
    let Some(model) = tokens.next() else {
        return false;
    };
    if !model.to_ascii_lowercase().starts_with("gpt-") || model.len() <= 4 {
        return false;
    }
    if let Some(effort) = tokens.next()
        && !matches!(
            effort,
            "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
    {
        return false;
    }
    tokens.next().is_none()
        && (right.trim_start().starts_with("~/") || right.trim_start().starts_with('/'))
}

fn model_matches_family(model: &str, family: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    match family {
        "codex" => lower.starts_with("gpt-") && lower.len() > 4,
        "claude" => {
            (lower.starts_with("claude-") && lower.len() > 7)
                || matches!(
                    lower.split_whitespace().next(),
                    Some("haiku" | "sonnet" | "opus" | "fable")
                )
        }
        "agy" | "gemini" => lower.starts_with("gemini ") || lower.starts_with("gemini-"),
        // OpenCode can display configured own models as well as hosted models.
        "opencode" => specific_model_label(model),
        _ => false,
    }
}

fn observed_footer_for_family(text: &str, family: &str) -> Option<(String, Option<String>)> {
    observed_footer(text).filter(|(model, _)| model_matches_family(model, family))
}

struct CaptureDirectory {
    root: PathBuf,
}
impl CaptureDirectory {
    fn new(parent: &Path) -> std::io::Result<Self> {
        let root = parent.join(format!(".rust-screen-capture-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        Ok(Self { root })
    }
    async fn capture(&self, id: &str, ordinal: usize) -> Option<String> {
        let path = self.root.join(format!("{ordinal}.txt"));
        let args = vec![
            "-S".into(),
            id.into(),
            "-X".into(),
            "hardcopy".into(),
            path.to_str()?.into(),
        ];
        let output = crate::io::run(Path::new("/usr/bin/screen"), &args, None, 2, 1024)
            .await
            .ok()?;
        if output.status != 0 {
            return None;
        }
        crate::io::read_text_tail(&path, TAIL_BYTES).ok()
    }
}
impl Drop for CaptureDirectory {
    fn drop(&mut self) {
        for ordinal in 1..=MAX_CAPTURES {
            let _ = fs::remove_file(self.root.join(format!("{ordinal}.txt")));
        }
        let _ = fs::remove_dir(&self.root);
    }
}

async fn admiral_observation(config: &ObserveConfig, screens: &[Value]) -> Value {
    let observed = config
        .admiral_screen
        .as_deref()
        .and_then(|id| {
            screens
                .iter()
                .find(|s| s["id"] == id || (s["name"] == id && screen_name(id) == id))
        })
        .cloned();
    let mut result = json!({"screen":observed.unwrap_or_else(||json!({"name":config.admiral_screen,"alive":null})),"status":null,"log_tail":[],"decisions":[],"checks_logged":null,"decisions_total":null,"ticker":{"alive":null}});
    if let Some(dir) = &config.coordination_dir {
        let status_path = dir.join("super_captain_status.json");
        match crate::io::read_json(&status_path, 128 * 1024) {
            Ok(status) => result["status"] = sanitize_value(status, 0),
            Err(error) => {
                result["status_error"] = json!(crate::io::clean(&format!(
                    "Admiral status unavailable (128 KiB source limit): {error}"
                )));
            }
        }
        result["status_mtime_utc"] =
            crate::io::mtime_utc(&status_path).map_or(Value::Null, |v| json!(v));
        if let Ok(text) = crate::io::read_text_tail(&dir.join("super_captain_LOG.md"), 32 * 1024) {
            let text = safe_tail(&text);
            result["log_tail"] = json!(
                text.lines()
                    .rev()
                    .take(30)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
            );
            result["checks_window_count"] =
                json!(text.lines().filter(|s| s.starts_with("## CHECK ")).count());
        }
        if let Ok(text) =
            crate::io::read_text_tail(&dir.join("super_captain_DECISIONS.md"), 32 * 1024)
        {
            result["decisions"] = json!(decision_headers(&text));
        }
    }
    result
}

fn decision_headers(text: &str) -> Vec<Value> {
    safe_tail(text)
        .lines()
        .filter(|s| s.starts_with("## SC-DEC-"))
        .rev()
        .take(12)
        .map(|title| {
            json!({"title":title,"id":null,"when":null,"project":null,
            "decision":null,"reversibility":"unknown"})
        })
        .collect()
}

fn sanitize_value(value: Value, depth: usize) -> Value {
    if depth > 12 {
        return Value::Null;
    }
    match value {
        Value::String(s) => json!(crate::io::clean(&s)),
        Value::Array(a) => Value::Array(
            a.into_iter()
                .take(256)
                .map(|v| sanitize_value(v, depth + 1))
                .collect(),
        ),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .take(256)
                .map(|(k, v)| (crate::io::clean(&k), sanitize_value(v, depth + 1)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decision_headers_preserve_observation_and_retained_view_unknown_fields() {
        let rows = decision_headers(
            "## SC-DEC-090 Observed heading\nBody is not parsed as decision authority.\n",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["title"], "## SC-DEC-090 Observed heading");
        assert_eq!(
            rows[0]["reversibility"]
                .as_str()
                .expect("retained view calls split"),
            "unknown"
        );
        for key in ["id", "when", "project", "decision"] {
            assert!(rows[0][key].is_null());
        }
        assert!(decision_headers("## SC-DEC-091 API_KEY = private-fixture").is_empty());
    }
    fn node() -> NodeConfig {
        NodeConfig {
            id: "custom-node".into(),
            ssh: Some("operator@configured-host".into()),
            os: "linux".into(),
        }
    }
    #[test]
    fn screen_inventory_accepts_only_actual_bounded_numeric_identities() {
        let screens = parse_screens(
            "There are screens on:\n 12.real_name (Detached)\n 12.real_name (Detached)\n 1.bad (Detached)\n 13../escape (Detached)",
            "custom-node",
        );
        assert_eq!(screens.len(), 1);
        assert_eq!(screens[0]["id"], "12.real_name");
        assert_eq!(screens[0]["host"], "custom-node");
    }
    #[test]
    fn resources_keep_missing_values_unknown_and_parse_real_units() {
        let v = parse_resources(
            &node(),
            "@@NPROC\n8\n@@LOAD\n1.5 2.0 3.0 1/9\n@@MEM\nMemTotal: 2097152 kB\nMemAvailable: 1048576 kB\n@@DF\n/dev/root 1000 900 100 90% /\n@@SCREENS\nNo Sockets found\n@@SCREEN_RC 1\n@@END\n",
        );
        assert_eq!(v["mem"]["total_gb"], 2.0);
        assert_eq!(v["mem"]["avail_gb"], 1.0);
        assert_eq!(v["disks"][0]["size"], 1024000);
        assert_eq!(v["screen_inventory_available"], true);
        assert!(v["mem"]["swap_total_gb"].is_null());
        assert!(parse_resources(&node(), "@@END\n")["nproc"].is_null());
    }
    #[test]
    fn process_requested_models_and_prompt_mentions_never_establish_model_identity() {
        assert_eq!(
            agent_family(&["node", "/some/codex", "--model"]),
            Some("codex")
        );
        assert_eq!(agent_family(&["bash", "codex", "--model"]), None);
        assert_eq!(
            observed_footer("Please use gpt-6-luna medium for this task"),
            None
        );
        assert_eq!(
            observed_footer(&safe_tail("> model: gpt-6-luna medium")),
            None
        );
        assert_eq!(
            observed_footer("gpt-6-luna medium · 82% context left"),
            Some(("gpt-6-luna".into(), Some("medium".into())))
        );
        assert_eq!(
            observed_footer("│ Gemini 4 Argon · ready │"),
            Some(("Gemini 4 Argon".into(), None))
        );
    }
    #[test]
    fn actual_gemini_cancel_footer_survives_replacement_separator_bytes() {
        for footer in [
            "esc to cancel                         Gemini 4 Argon � high � 1 task(s) � /tasks",
            "esc to cancel                                              Gemini 4 Argon � high",
            "  esc to cancel  Gemini 4 Argon · high",
        ] {
            assert_eq!(
                observed_footer(&safe_tail(footer)),
                Some(("Gemini 4 Argon".into(), Some("high".into())))
            );
        }
        for prose in [
            "Please use Gemini 4 Argon high",
            "Requested Gemini 4 Argon � high",
            "Please use Gemini 4 Argon · high",
            "--model gpt-6-luna medium · 80% context left",
            "esc to cancel requested Gemini 4 Argon high",
            "The footer says esc to cancel Gemini 4 Argon high",
            "> esc to cancel Gemini 4 Argon high",
        ] {
            assert_eq!(observed_footer(&safe_tail(prose)), None);
        }
    }

    #[test]
    fn actual_uppercase_codex_footer_survives_screen_replacement_separator() {
        // Sanitized shape from an actual GNU Screen hardcopy; the path is synthetic.
        let capture =
            "unrelated output\n\0\u{1b}[32m  GPT-6.1-Sol xhigh � ~/owned-worktree\u{1b}[0m\0\n";
        assert_eq!(
            observed_footer(&safe_tail(capture)),
            Some(("gpt-6.1-sol".into(), Some("xhigh".into())))
        );
    }

    #[test]
    fn footer_identity_is_bound_to_the_actual_process_family() {
        let codex = safe_tail("GPT-6.1-Sol xhigh � ~/owned-worktree");
        let gemini = safe_tail("esc to cancel  Gemini 4 Argon � high");
        assert_eq!(
            observed_footer_for_family(&codex, "codex"),
            Some(("gpt-6.1-sol".into(), Some("xhigh".into())))
        );
        for family in ["gemini", "agy", "claude", "unknown"] {
            assert!(observed_footer_for_family(&codex, family).is_none());
        }
        for family in ["gemini", "agy"] {
            assert_eq!(
                observed_footer_for_family(&gemini, family),
                Some(("Gemini 4 Argon".into(), Some("high".into())))
            );
        }
        assert!(observed_footer_for_family(&gemini, "codex").is_none());
        // Preserve the existing explicit-model/boxed formats for other clients.
        assert_eq!(
            observed_footer_for_family("│ Sonnet 4.5 · ready │", "claude"),
            Some(("Sonnet 4.5".into(), None))
        );
        assert_eq!(
            observed_footer_for_family("Model: CLAUDE-SONNET-4-5", "claude"),
            Some(("claude-sonnet-4-5".into(), None))
        );
        assert_eq!(
            observed_footer_for_family("Model: Ornith-1.5-35B-A3B", "opencode"),
            Some(("Ornith-1.5-35B-A3B".into(), None))
        );
        for family in ["codex", "claude", "gemini"] {
            assert!(observed_footer_for_family("Model: Ornith-1.5-35B-A3B", family).is_none());
        }
    }

    #[test]
    fn footer_rejects_prose_inputs_private_lines_and_unspecific_family_labels() {
        for text in [
            "Please use GPT-6.1-Sol xhigh � ~/owned-worktree",
            "Requested GPT-6.1-Sol xhigh · 80% context left",
            "--model gpt-6-luna medium · 80% context left",
            "GPT-6.1-Sol xhigh � requested ~/owned-worktree",
            "GPT-6.1-Sol please xhigh � ~/owned-worktree",
            "gpt-6-luna medium · requested for this task",
            "> GPT-6.1-Sol xhigh � ~/owned-worktree",
            "› \u{1b}[32mGPT-6.1-Sol xhigh � ~/owned-worktree",
            "GPT-6.1-Sol xhigh � ~/owned/.private/worktree",
            "GPT-6.1-Sol xhigh � ~/owned-worktree API_KEY=private-fixture",
            "Model: Codex",
            "Model: Claude",
            "Model: Gemini",
            "Model: OpenCode",
            "Model: unknown",
        ] {
            assert_eq!(observed_footer(&safe_tail(text)), None, "{text}");
        }
        assert!(observed_footer(&"a".repeat(TAIL_BYTES + 1)).is_none());
        assert!(observed_footer(&format!("Model: {}", "x".repeat(1024))).is_none());
        let old = format!(
            "GPT-6.1-Sol xhigh � ~/owned-worktree\n{}",
            "ordinary output\n".repeat(16)
        );
        assert!(observed_footer(&old).is_none());
    }

    #[test]
    fn codex_path_footer_keeps_separator_and_effort_semantics() {
        for separator in ["�", "·", "│"] {
            assert_eq!(
                observed_footer(&format!("GPT-6.1-Sol xhigh {separator} ~/owned-worktree")),
                Some(("gpt-6.1-sol".into(), Some("xhigh".into())))
            );
        }
        assert_eq!(
            observed_footer("GPT-6.1-Sol � /owned-worktree"),
            Some(("gpt-6.1-sol".into(), None))
        );
        assert_eq!(
            observed_footer("gpt-6-luna medium · 82% context left"),
            Some(("gpt-6-luna".into(), Some("medium".into())))
        );
    }
    #[tokio::test]
    async fn invalid_target_and_unsupported_os_return_unknown_without_commands() {
        let mut n = node();
        n.ssh = Some("-oProxyCommand=bad".into());
        let v = observe_node(&n).await;
        assert_eq!(v["reachable"], false);
        assert!(v["nproc"].is_null());
        n.os = "unqualified".into();
        assert_eq!(observe_node(&n).await["reachable"], false);
        assert!(!valid_target("host\ncommand"));
        assert!(valid_target("user@custom-node"));
    }
    #[test]
    fn disk_overflow_gpu_unknowns_and_unsafe_screen_ids_are_rejected() {
        assert!(parse_disk("/dev/root 18446744073709551615 1 1 1% /").is_none());
        assert!(parse_gpu("0, GPU, N/A, 1, 2, 3").is_none());
        assert!(parse_screens(" 42.name;evil (Detached)", "n").is_empty());
    }

    #[test]
    fn unreachable_node_does_not_fabricate_zero_resources_or_agent_identity() {
        let value = unknown_host(&node(), "unreachable fixture");
        assert_eq!(value["reachable"], false);
        assert!(value["nproc"].is_null());
        assert!(value["mem"].is_null());
        assert!(value["load"].is_null());
        assert_eq!(value["screen_inventory_available"], false);
        assert_eq!(value["screen_observations"], json!([]));
    }

    #[test]
    fn model_cache_requires_process_identity_and_original_fresh_timestamp() {
        let now = chrono::Utc::now();
        let process = AgentProcess {
            pid: 42,
            family: "codex",
            start: "boot:900".into(),
        };
        let entry = ModelObservation {
            pid: 42,
            family: "codex".into(),
            start: "boot:900".into(),
            model: "gpt-6-luna".into(),
            effort: Some("medium".into()),
            observed_utc: (now - chrono::Duration::seconds(200)).to_rfc3339(),
        };
        assert!(entry.matches(&process, now));
        assert!(
            !ModelObservation {
                model: "Gemini 4 Argon".into(),
                ..entry.clone()
            }
            .matches(&process, now)
        );
        assert!(
            !ModelObservation {
                observed_utc: (now + chrono::Duration::seconds(1)).to_rfc3339(),
                ..entry.clone()
            }
            .matches(&process, now)
        );
        assert!(!entry.matches(
            &AgentProcess {
                start: "boot:901".into(),
                ..process.clone()
            },
            now
        ));
        assert!(!entry.matches(
            &AgentProcess {
                pid: 43,
                ..process.clone()
            },
            now
        ));
        assert!(!entry.matches(
            &AgentProcess {
                family: "claude",
                ..process
            },
            now
        ));
        assert!(!entry.matches(
            &AgentProcess {
                pid: 42,
                family: "codex",
                start: "boot:900".into()
            },
            now + chrono::Duration::seconds(101)
        ));
        let mut screen = json!({});
        apply_model(&mut screen, &entry, true);
        assert_eq!(screen["model_observed_utc"], entry.observed_utc);
        assert!(
            screen["model_source"]
                .as_str()
                .expect("source")
                .starts_with("cached")
        );
    }

    #[tokio::test]
    async fn admiral_status_overflow_and_parse_errors_are_visible_and_compact_audits_recover() {
        let temp = tempfile::tempdir().expect("private audit fixture directory");
        let config: ObserveConfig =
            serde_json::from_value(json!({"schema_version":1,"node_id":"n",
            "output_dir":temp.path(),"coordination_dir":temp.path(),"projects":[]}))
            .expect("audit fixture configuration");
        let path = temp.path().join("super_captain_status.json");
        let mut compact = json!({"captains":{"p":{"hygiene":{"score":null,"grade":null,
            "possible_score_range":[30,80],"known_component_subtotal":80,
            "observed_utc":"2026-10-20T06:02:00Z"}}}});
        compact["embedded_evidence"] = json!("x".repeat(250_000));
        std::fs::write(
            &path,
            serde_json::to_vec(&compact).expect("oversized audit JSON"),
        )
        .expect("write oversized audit fixture");
        let oversized = admiral_observation(&config, &[]).await;
        assert!(oversized["status"].is_null());
        assert!(
            oversized["status_error"]
                .as_str()
                .expect("oversized audit diagnostic")
                .contains("byte limit")
        );
        assert!(
            oversized["status_error"]
                .as_str()
                .expect("oversized audit diagnostic")
                .contains("128 KiB")
        );
        std::fs::write(&path, b"{").expect("write malformed audit fixture");
        let malformed = admiral_observation(&config, &[]).await;
        assert!(malformed["status"].is_null());
        assert!(
            malformed["status_error"]
                .as_str()
                .expect("malformed audit diagnostic")
                .contains("unavailable")
        );
        compact
            .as_object_mut()
            .expect("audit object")
            .remove("embedded_evidence");
        std::fs::write(
            &path,
            serde_json::to_vec(&compact).expect("compact audit JSON"),
        )
        .expect("write compact audit fixture");
        let recovered = admiral_observation(&config, &[]).await;
        assert!(recovered["status_error"].is_null());
        let hygiene = &recovered["status"]["captains"]["p"]["hygiene"];
        assert!(hygiene["score"].is_null());
        assert_eq!(hygiene["possible_score_range"], json!([30, 80]));
        assert_eq!(hygiene["observed_utc"], "2026-10-20T06:02:00Z");
    }

    #[test]
    fn capture_plan_prioritizes_managers_and_rotates_workers_without_starvation() {
        let config:ObserveConfig=serde_json::from_value(json!({"schema_version":1,"node_id":"n","output_dir":"/owned",
            "admiral_screen":"20.admiral","projects":[{"id":"p","name":"P","repository":"/repo","tracker":{},"manager_screen":"21.manager"}]})).expect("config");
        let text = (20..40)
            .map(|pid| {
                format!(
                    "{pid}.{}",
                    match pid {
                        20 => "admiral".into(),
                        21 => "manager".into(),
                        _ => format!("worker_{pid}"),
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let screens = parse_screens(&text, "n");
        let processes = (20..40)
            .map(|pid| {
                Some(AgentProcess {
                    pid,
                    family: "codex",
                    start: format!("boot:{pid}"),
                })
            })
            .collect::<Vec<_>>();
        let first = capture_order(&config, &screens, &processes, 0, 0);
        let second = capture_order(&config, &screens, &processes, 4, 1);
        assert_eq!(first[0], (0, false));
        assert_eq!(first[1], (1, false));
        let first_workers = first
            .iter()
            .filter(|(_, worker)| *worker)
            .take(4)
            .collect::<Vec<_>>();
        let second_workers = second
            .iter()
            .filter(|(_, worker)| *worker)
            .take(4)
            .collect::<Vec<_>>();
        assert!(
            first_workers
                .iter()
                .all(|item| !second_workers.contains(item))
        );
        assert!(first.iter().take(8).any(|(_, worker)| *worker));
        assert_eq!(first.len(), screens.len());
    }

    #[test]
    fn cache_is_private_bounded_and_corruption_falls_back_to_unknown() {
        let dir = tempfile::tempdir().expect("fixture");
        let cache = ModelCache {
            cursor: 17,
            ..ModelCache::default()
        };
        cache.save(dir.path()).expect("cache write");
        assert_eq!(ModelCache::load(dir.path()).cursor, 17);
        use std::os::unix::fs::MetadataExt;
        let path = dir.path().join(".rust-model-observations.json");
        assert_eq!(fs::metadata(&path).expect("metadata").mode() & 0o077, 0);
        fs::write(&path, b"corrupted").expect("corrupt fixture");
        assert!(ModelCache::load(dir.path()).entries.is_empty());
        assert_eq!(ModelCache::load(dir.path()).cursor, 0);
    }
}
