//! Port of `dashboard/globalres.py`: read-only resource sampler for
//! xbabe1/2/3 (never xbabe0). Writes `global_resources.json` and
//! `.res_history.json` under `CDASH_RUNTIME` and prints a table.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::Ctx;
use crate::compat::{
    JsonLayout, last_chars, py_float_repr, py_json, py_list_repr, py_round, py_round_int, py_strip,
    splitlines, take_chars,
};
use crate::system::{CmdSpec, utc_iso};

/// The probe script, byte-identical to the Python's `PROBE`.
pub const PROBE: &str = r#"
echo "@@HOST $(hostname)"; echo "@@NPROC $(nproc)"; echo "@@LOAD $(cut -d' ' -f1-3 /proc/loadavg)"
echo "@@MEM"; grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo
echo "@@DF"; df -B1 -P -x tmpfs -x devtmpfs -x overlay -x squashfs -x efivarfs -x nfs -x nfs4 -x fuse.sshfs 2>/dev/null | awk 'NR>1 && $2>5000000000 {print $6, $2, $3, $4}'
echo "@@GPU"; nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>/dev/null
echo "@@DSTATE $(ps -eo stat= | grep -c '^D')"
echo "@@TOP"; ps -eo pid=,user=,rss=,pcpu=,etimes=,comm= --sort=-rss | head -6
echo "@@SCREENS"; screen -ls 2>/dev/null | awk '/\t[0-9]+\./{print $1}'
echo "@@END"
"#;

pub const THRESHOLDS: &str = "disk red >=92% or <12h to full; amber >=85% or <24h; RAM red <8% avail, amber <15%; growth from samples 20min-3h old";
pub const NOTE: &str = "xbabe0 is deliberately never probed";

/// Hosts that must never be probed.
pub fn refuse_forbidden_hosts(hosts: &[String]) -> Result<(), String> {
    match hosts
        .iter()
        .find(|h| py_strip(h).eq_ignore_ascii_case("xbabe0"))
    {
        Some(h) => Err(format!(
            "refusing to probe {h}: xbabe0 is deliberately never probed"
        )),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Disk {
    pub mount: String,
    pub size: i64,
    pub used: i64,
    pub avail: i64,
    pub pct: f64,
    pub growth_gb_per_h: Option<f64>,
    pub hours_to_full: Option<f64>,
    pub rate_window_min: Option<i64>,
    pub level: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gpu {
    pub idx: i64,
    pub name: String,
    pub util: f64,
    pub mem_used_mib: f64,
    pub mem_total_mib: f64,
    pub temp_c: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TopProc {
    pub pid: i64,
    pub user: String,
    pub rss_gb: f64,
    pub pcpu: f64,
    pub etime_s: i64,
    pub comm: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mem {
    pub total_gb: f64,
    pub avail_gb: f64,
    pub swap_total_gb: f64,
    pub swap_used_gb: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avail_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Warning {
    pub level: String,
    pub text: String,
}

/// One host record; optional keys are omitted exactly when the Python never
/// set them (key order follows the fixed PROBE output order).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostRecord {
    pub host: String,
    pub reachable: bool,
    pub error: Option<String>,
    pub disks: Vec<Disk>,
    pub gpus: Vec<Gpu>,
    pub top_rss: Vec<TopProc>,
    pub screens: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nproc: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d_state_procs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem: Option<Mem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_level: Option<String>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceDoc {
    pub generated_utc: String,
    pub hosts: Vec<HostRecord>,
    pub thresholds: &'static str,
    pub warnings: Vec<Warning>,
    pub note: &'static str,
}

/// Python `int(s)` (whitespace and underscores between digits allowed).
fn py_int(s: &str) -> Result<i64, String> {
    let t = py_strip(s);
    let cleaned: String = t.chars().filter(|c| *c != '_').collect();
    cleaned.parse::<i64>().map_err(|_| {
        format!(
            "invalid literal for int() with base 10: {}",
            crate::compat::py_str_repr(s)
        )
    })
}

/// Python `float(s)`.
fn py_float(s: &str) -> Result<f64, String> {
    let t = py_strip(s);
    let cleaned: String = t.chars().filter(|c| *c != '_').collect();
    cleaned.parse::<f64>().map_err(|_| {
        format!(
            "could not convert string to float: {}",
            crate::compat::py_str_repr(s)
        )
    })
}

/// `parse(h, out, err)`. `Err` mirrors an uncaught Python exception.
pub fn parse_probe(host: &str, out: &str, err: &str) -> Result<HostRecord, String> {
    let reachable = out.contains("@@END");
    let mut r = HostRecord {
        host: host.to_string(),
        reachable,
        error: if reachable {
            None
        } else {
            let e = py_strip(err);
            Some(if e.is_empty() {
                "no output".to_string()
            } else {
                e.to_string()
            })
        },
        disks: Vec::new(),
        gpus: Vec::new(),
        top_rss: Vec::new(),
        screens: Vec::new(),
        hostname: None,
        nproc: None,
        load: None,
        d_state_procs: None,
        mem: None,
        load_level: None,
        warnings: Vec::new(),
    };
    let mut sec = String::new();
    let mut mem: Vec<(String, i64)> = Vec::new();
    for line in splitlines(out) {
        if let Some(rest) = line.strip_prefix("@@") {
            let (name, val) = rest.split_once(' ').unwrap_or((rest, ""));
            sec = name.to_string();
            let or_zero = |v: &str| if v.is_empty() { Ok(0) } else { py_int(v) };
            match name {
                "NPROC" => r.nproc = Some(or_zero(val)?),
                "LOAD" => {
                    r.load = Some(
                        val.split_whitespace()
                            .map(py_float)
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                "DSTATE" => r.d_state_procs = Some(or_zero(val)?),
                "HOST" => r.hostname = Some(val.to_string()),
                _ => {}
            }
            continue;
        }
        match sec.as_str() {
            "MEM" => {
                let parts: Vec<&str> = line.split(':').collect();
                let [k, v] = parts.as_slice() else {
                    return Err(format!("cannot parse meminfo line {line:?}"));
                };
                let first = v
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| format!("cannot parse meminfo line {line:?}"))?;
                let bytes = py_int(first)? * 1024;
                match mem.iter_mut().find(|(key, _)| key == k) {
                    Some(slot) => slot.1 = bytes,
                    None => mem.push(((*k).to_string(), bytes)),
                }
            }
            "DF" => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let [m, size, used, avail] = parts.as_slice() else {
                    return Err(format!("cannot parse df line {line:?}"));
                };
                let (size, used, avail) = (py_int(size)?, py_int(used)?, py_int(avail)?);
                let denom = (used + avail).max(1) as f64;
                r.disks.push(Disk {
                    mount: (*m).to_string(),
                    size,
                    used,
                    avail,
                    pct: py_round(100.0 * used as f64 / denom, 1),
                    growth_gb_per_h: None,
                    hours_to_full: None,
                    rate_window_min: None,
                    level: String::new(),
                });
            }
            "GPU" if !py_strip(line).is_empty() => {
                let f: Vec<&str> = line.split(',').map(py_strip).collect();
                let parsed = (|| -> Result<Gpu, String> {
                    Ok(Gpu {
                        idx: py_int(f.first().copied().ok_or("idx")?)?,
                        name: f.get(1).copied().ok_or("name")?.to_string(),
                        util: py_float(f.get(2).copied().ok_or("util")?)?,
                        mem_used_mib: py_float(f.get(3).copied().ok_or("mem")?)?,
                        mem_total_mib: py_float(f.get(4).copied().ok_or("mem")?)?,
                        temp_c: py_float(f.get(5).copied().ok_or("temp")?)?,
                    })
                })();
                if let Ok(g) = parsed {
                    r.gpus.push(g);
                }
            }
            "TOP" if !py_strip(line).is_empty() => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 6 {
                    return Err(format!("cannot parse ps line {line:?}"));
                }
                // `split(None, 5)`: the 6th field is the rest of the line.
                let comm = nth_field_rest(line, 5);
                r.top_rss.push(TopProc {
                    pid: py_int(f[0])?,
                    user: f[1].to_string(),
                    rss_gb: py_round(py_int(f[2])? as f64 / 1_048_576.0, 1),
                    pcpu: py_float(f[3])?,
                    etime_s: py_int(f[4])?,
                    comm: take_chars(comm, 40).to_string(),
                });
            }
            "SCREENS" if !py_strip(line).is_empty() => {
                r.screens.push(py_strip(line).to_string());
            }
            _ => {}
        }
    }
    if !mem.is_empty() {
        let get = |k: &str| mem.iter().find(|(key, _)| key == k).map_or(0, |(_, v)| *v);
        let gib = 1_073_741_824.0;
        r.mem = Some(Mem {
            total_gb: py_round(get("MemTotal") as f64 / gib, 1),
            avail_gb: py_round(get("MemAvailable") as f64 / gib, 1),
            swap_total_gb: py_round(get("SwapTotal") as f64 / gib, 1),
            swap_used_gb: py_round((get("SwapTotal") - get("SwapFree")) as f64 / gib, 1),
            avail_pct: None,
            level: None,
        });
    }
    Ok(r)
}

/// Rest of the line starting at whitespace-separated field `n` (0-based),
/// like the last element of Python's `split(None, n)` (trailing whitespace
/// kept).
fn nth_field_rest(line: &str, n: usize) -> &str {
    let mut rest = line.trim_start_matches(crate::compat::is_py_space);
    for _ in 0..n {
        let end = rest.find(crate::compat::is_py_space).unwrap_or(rest.len());
        rest = rest[end..].trim_start_matches(crate::compat::is_py_space);
    }
    rest
}

/// `.res_history.json`: insertion-ordered `{ "host:mount": [[ts, used], ...] }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History(pub Vec<(String, Vec<Value>)>);

impl History {
    pub fn get(&self, key: &str) -> Option<&Vec<Value>> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn set(&mut self, key: &str, value: Vec<Value>) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// `json.load(open(HIST))` with any failure → `{}`.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<History>(&t).ok())
            .unwrap_or_default()
    }
}

impl Serialize for History {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for History {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = History;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<History, A::Error> {
                let mut h = History::default();
                while let Some((k, v)) = map.next_entry::<String, Value>()? {
                    // Non-list values cannot be iterated by the Python; drop them.
                    let list = match v {
                        Value::Array(items) => items,
                        _ => Vec::new(),
                    };
                    h.set(&k, list);
                }
                Ok(h)
            }
        }
        d.deserialize_map(V)
    }
}

fn sample_ts(x: &Value) -> Option<f64> {
    x.as_array()?.first()?.as_f64()
}

/// Python `ts = datetime.now(timezone.utc).timestamp()`.
pub fn py_timestamp(t: DateTime<Utc>) -> f64 {
    t.timestamp_micros() as f64 / 1e6
}

/// Growth / hours-to-full / level for one disk and the history update.
pub fn assess_disk(host: &str, d: &mut Disk, hist: &mut History, ts: f64) -> Option<Warning> {
    let key = format!("{host}:{}", d.mount);
    let mut hl: Vec<Value> = hist
        .get(&key)
        .map(|v| {
            v.iter()
                .filter(|x| sample_ts(x).is_some_and(|t| ts - t < 6.0 * 3600.0))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    hl.push(serde_json::json!([ts, d.used]));
    hist.set(&key, hl.clone());
    let base = hl
        .iter()
        .find(|x| sample_ts(x).is_some_and(|t| ts - t <= 3.0 * 3600.0 && ts - t >= 20.0 * 60.0));
    let base = base.and_then(|x| Some((sample_ts(x)?, x.as_array()?.get(1)?.clone())));
    let mut rate: Option<f64> = None;
    if let Some((bts, bused)) = &base
        && ts - bts > 0.0
    {
        let delta = match bused.as_i64() {
            Some(bi) => (d.used - bi) as f64,
            None => d.used as f64 - bused.as_f64().unwrap_or(0.0),
        };
        rate = Some(delta / (ts - bts) * 3600.0);
    }
    d.growth_gb_per_h = rate.map(|r| py_round(r / 1e9, 2));
    d.hours_to_full = match rate {
        Some(r) if r != 0.0 && r > 0.0 => Some(py_round(d.avail as f64 / r, 1)),
        _ => None,
    };
    d.rate_window_min = base
        .as_ref()
        .map(|(bts, _)| py_round_int((ts - bts) / 60.0));
    let htf = d.hours_to_full;
    let level = if d.pct >= 92.0 || htf.is_some_and(|h| h < 12.0) {
        "red"
    } else if d.pct >= 85.0 || htf.is_some_and(|h| h < 24.0) {
        "amber"
    } else {
        "ok"
    };
    d.level = level.to_string();
    if level == "ok" {
        return None;
    }
    let suffix = match (htf, d.growth_gb_per_h) {
        (Some(h), Some(g)) if h != 0.0 => format!(", ~{h:.1} h to full at {g:.1} GB/h"),
        _ => String::new(),
    };
    Some(Warning {
        level: level.to_string(),
        text: format!(
            "{host} {} {:.1}% used, {:.0} GB free{suffix}",
            d.mount,
            d.pct,
            d.avail as f64 / 1e9
        ),
    })
}

/// Memory / load / GPU / reachability warnings (in the Python's order, after
/// the disk warnings).
pub fn assess_host(r: &mut HostRecord, mut warnings: Vec<Warning>) {
    let h = r.host.clone();
    if let Some(m) = r.mem.as_mut() {
        let ap = 100.0 * m.avail_gb / m.total_gb.max(1.0);
        m.avail_pct = Some(py_round(ap, 1));
        let level = if ap < 8.0 {
            "red"
        } else if ap < 15.0 {
            "amber"
        } else {
            "ok"
        };
        m.level = Some(level.to_string());
        if level != "ok" {
            warnings.push(Warning {
                level: level.to_string(),
                text: format!(
                    "{h} RAM low: {:.1} GB available of {:.0} GB",
                    m.avail_gb, m.total_gb
                ),
            });
        }
        if m.swap_total_gb != 0.0 && m.swap_used_gb >= 0.95 * m.swap_total_gb {
            warnings.push(Warning {
                level: if level != "ok" { "red" } else { "amber" }.to_string(),
                text: format!(
                    "{h} swap full ({:.0}/{:.0} GB)",
                    m.swap_used_gb, m.swap_total_gb
                ),
            });
        }
    }
    if let (Some(load), Some(nproc)) = (r.load.as_ref(), r.nproc)
        && let (Some(&l1), true) = (load.first(), nproc != 0)
    {
        let n = nproc as f64;
        let level = if l1 > n {
            "red"
        } else if l1 > 0.85 * n {
            "amber"
        } else {
            "ok"
        };
        r.load_level = Some(level.to_string());
        if level != "ok" {
            warnings.push(Warning {
                level: level.to_string(),
                text: format!("{h} load {l1:.1} on {nproc} CPUs"),
            });
        }
    }
    for g in &r.gpus {
        if g.temp_c >= 85.0 {
            warnings.push(Warning {
                level: "amber".to_string(),
                text: format!("{h} GPU{} {}°C", g.idx, g.temp_c.trunc() as i64),
            });
        }
    }
    if !r.reachable {
        warnings.push(Warning {
            level: "amber".to_string(),
            text: format!("{h} unreachable: {}", r.error.as_deref().unwrap_or("None")),
        });
    }
    r.warnings = warnings;
}

/// Command used to probe `host`.
pub fn probe_command(ctx: &Ctx<'_>, host: &str) -> CmdSpec {
    let cfg = ctx.cfg;
    let timeout = Duration::from_secs(cfg.probe_timeout_secs);
    if host == cfg.resource_local_host {
        return CmdSpec::new(cfg.bash_bin.clone(), timeout)
            .arg("-c")
            .arg(PROBE);
    }
    CmdSpec::new(cfg.ssh_bin.clone(), timeout)
        .arg("-n")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={}", cfg.ssh_connect_timeout_secs))
        .arg("-o")
        .arg(format!(
            "ServerAliveInterval={}",
            cfg.ssh_server_alive_interval_secs
        ))
        .arg(OsString::from(format!("{}@{host}", cfg.ssh_user)))
        .arg(PROBE)
}

/// `probe(h)`: `(stdout, last 200 chars of stderr)` or `('', error[:200])`.
pub fn probe(ctx: &Ctx<'_>, host: &str) -> (String, String) {
    let spec = probe_command(ctx, host);
    match ctx.runner.run(&spec) {
        Ok(out) if out.timed_out => {
            let msg = format!(
                "Command '{}' timed out after {} seconds",
                py_list_repr(&spec.argv_lossy()),
                spec.timeout.as_secs()
            );
            (String::new(), take_chars(&msg, 200).to_string())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            (
                String::from_utf8_lossy(&out.stdout).into_owned(),
                last_chars(&stderr, 200).to_string(),
            )
        }
        Err(e) => {
            let msg = if e.kind() == io::ErrorKind::NotFound {
                format!(
                    "[Errno 2] No such file or directory: {}",
                    crate::compat::py_str_repr(&spec.program.to_string_lossy())
                )
            } else {
                e.to_string()
            };
            (String::new(), take_chars(&msg, 200).to_string())
        }
    }
}

fn opt_repr<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "None".to_string(), |x| x.to_string())
}

fn opt_float(v: Option<f64>) -> String {
    v.map_or_else(|| "None".to_string(), py_float_repr)
}

/// The printed table (non-`--quiet`).
pub fn render_table(doc: &ResourceDoc) -> String {
    let mut s = String::new();
    for r in &doc.hosts {
        let m = r.mem.as_ref();
        let load0 = r
            .load
            .as_ref()
            .and_then(|l| l.first().copied())
            .map_or_else(|| "?".to_string(), py_float_repr);
        s.push_str(&format!(
            "== {} reachable={} load={}/{} RAM avail {}/{} GB swap {}/{} GB\n",
            r.host,
            if r.reachable { "True" } else { "False" },
            load0,
            opt_repr(r.nproc),
            opt_float(m.map(|m| m.avail_gb)),
            opt_float(m.map(|m| m.total_gb)),
            opt_float(m.map(|m| m.swap_used_gb)),
            opt_float(m.map(|m| m.swap_total_gb)),
        ));
        for d in &r.disks {
            s.push_str(&format!(
                "   {:<40} {:5.1}% used, {:6.0} GB free  growth {} GB/h  to-full {} h  [{}]\n",
                d.mount,
                d.pct,
                d.avail as f64 / 1e9,
                opt_float(d.growth_gb_per_h),
                opt_float(d.hours_to_full),
                d.level
            ));
        }
        for g in &r.gpus {
            s.push_str(&format!(
                "   GPU{} {} util {}% mem {}/{} MiB {} C\n",
                g.idx,
                g.name,
                py_float_repr(g.util),
                py_float_repr(g.mem_used_mib),
                py_float_repr(g.mem_total_mib),
                py_float_repr(g.temp_c)
            ));
        }
    }
    s.push_str("WARNINGS:\n");
    if doc.warnings.is_empty() {
        s.push_str("  none\n");
    }
    for w in &doc.warnings {
        s.push_str(&format!("  [{}] {}\n", w.level, w.text));
    }
    s
}

/// Sample every host, update history, build the document.
pub fn sample(
    ctx: &Ctx<'_>,
    hist: &mut History,
    now: DateTime<Utc>,
) -> Result<ResourceDoc, String> {
    refuse_forbidden_hosts(&ctx.cfg.resource_hosts)?;
    let ts = py_timestamp(now);
    let mut hosts = Vec::new();
    for h in &ctx.cfg.resource_hosts {
        let (out, err) = probe(ctx, h);
        let mut r = parse_probe(h, &out, &err)?;
        let mut w = Vec::new();
        for d in &mut r.disks {
            if let Some(x) = assess_disk(h, d, hist, ts) {
                w.push(x);
            }
        }
        assess_host(&mut r, w);
        hosts.push(r);
    }
    let warnings = hosts.iter().flat_map(|r| r.warnings.clone()).collect();
    Ok(ResourceDoc {
        generated_utc: utc_iso(now),
        hosts,
        thresholds: THRESHOLDS,
        warnings,
        note: NOTE,
    })
}

fn write_replace(path: &Path, text: &str) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// `globalres.py [--quiet]`.
pub fn resources(ctx: &mut Ctx<'_>, quiet: bool) -> io::Result<i32> {
    if let Err(e) = refuse_forbidden_hosts(&ctx.cfg.resource_hosts) {
        writeln!(ctx.err, "error: {e}")?;
        return Ok(1);
    }
    let base = ctx.cfg.dashboard_runtime.clone();
    let out_path = base.join("global_resources.json");
    let hist_path = base.join(".res_history.json");
    let now = ctx.clock.now();
    let mut hist = History::load(&hist_path);
    let doc = match sample(ctx, &mut hist, now) {
        Ok(d) => d,
        Err(e) => {
            writeln!(ctx.err, "error: {e}")?;
            return Ok(1);
        }
    };
    let hist_json = py_json(&hist, JsonLayout::Compact, true)?;
    let doc_json = py_json(&doc, JsonLayout::Indent(1), true)?;
    for (path, text) in [(&hist_path, &hist_json), (&out_path, &doc_json)] {
        if let Err(e) = write_replace(path, text) {
            writeln!(ctx.err, "error: cannot write {}: {e}", path.display())?;
            return Ok(1);
        }
    }
    if !quiet {
        ctx.out.write_all(render_table(&doc).as_bytes())?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests;
