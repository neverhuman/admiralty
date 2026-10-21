//! Token-budgeted fleet brief for agents.
//!
//! A captain or watcher should not need a multi-megabyte `status.json` to decide
//! whether anything needs judgment. This module turns one observer snapshot
//! into a few hundred bytes of stable, line-oriented facts, most urgent first:
//!
//! ```text
//! # fleet gen=2026-10-21T07:12:05Z age=9s host=xbabe2 cycle_ms=1834 turns=0
//! ! git dope attention diverged_branch,merge_conflicts fp=3f2a91c4
//! ! res amber xbabe2 disk / 91.2% used, ~11.4 h to full at 2.10 GB/h
//! proj dope done=61/65 ip=0 blk=4 pr=#212:open ci=failure hyg=C50
//! git dope br=14 merged=9 ahead=3 div=2 old30d=1 co=6 dirty=2 unk=0 int24h=4 tip=3h
//! host xbabe2 load=55.1% mem_avail=61.0% root=91.2% gpu=43%
//! tok 15m=1.21M 60m=4.87M quota=41.0% burn=2.10%/h cost_idx=0.62
//! … omitted=3 budget=600B
//! ```
//!
//! Values are copied or arithmetically summarized from the snapshot; nothing
//! is inferred. Missing or null facts render as `?`, never as zero. The output
//! is deterministic for a given snapshot, options and clock, so repeated briefs
//! of an unchanged fleet are byte-identical (cheap to diff or cache).
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct BriefOptions {
    /// Maximum UTF-8 bytes of the brief. The header line is always included.
    pub budget_bytes: usize,
    /// Only include project-scoped lines for this project id.
    pub project: Option<String>,
    /// Caller clock for source age; `None` leaves age unknown.
    pub now_utc: Option<String>,
}

impl Default for BriefOptions {
    fn default() -> Self {
        BriefOptions {
            budget_bytes: 2_000,
            project: None,
            now_utc: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Brief {
    pub text: String,
    pub lines: usize,
    pub omitted_lines: usize,
    pub bytes: usize,
    pub source_generated_utc: Option<String>,
    pub source_age_s: Option<i64>,
    pub stale: Option<bool>,
}

pub const MIN_BUDGET: usize = 160;
pub const MAX_BUDGET: usize = 64 * 1024;

fn str_of(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// At most two decimals, trailing zeroes trimmed; unknown is `?`.
pub fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x.is_finite() => trim(x, 2),
        _ => "?".into(),
    }
}

fn trim(x: f64, places: usize) -> String {
    let s = format!("{x:.places$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

fn pct(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x.is_finite() => format!("{}%", trim(x, 1)),
        _ => "?".into(),
    }
}

/// Human-scale count: 1234567 -> 1.23M.
pub fn si(v: &Value) -> String {
    let Some(x) = v.as_f64().filter(|x| x.is_finite()) else {
        return "?".into();
    };
    let (scaled, suffix) = match x.abs() {
        a if a >= 1e9 => (x / 1e9, "G"),
        a if a >= 1e6 => (x / 1e6, "M"),
        a if a >= 1e3 => (x / 1e3, "k"),
        _ => (x, ""),
    };
    format!("{}{suffix}", trim(scaled, 2))
}

fn age(seconds: i64) -> String {
    match seconds {
        s if s < 0 => format!("{s}s"),
        s if s < 120 => format!("{s}s"),
        s if s < 2 * 3600 => format!("{}m", s / 60),
        s if s < 2 * 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

fn seconds(utc: &str) -> Option<i64> {
    utc.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

fn count(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::Object(o) => match o.get("value").and_then(Value::as_u64) {
            Some(n) if o.get("lower_bound") == Some(&Value::Bool(true)) => format!("{n}+"),
            Some(n) => n.to_string(),
            None => "?".into(),
        },
        _ => "?".into(),
    }
}

struct Line {
    priority: u8,
    text: String,
}

fn project_ids(snapshot: &Value) -> Vec<String> {
    let mut ids = snapshot["order"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    for map in ["projects", "git_health", "git_metrics"] {
        for id in snapshot[map]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

pub fn brief(snapshot: &Value, options: &BriefOptions) -> Brief {
    let budget = options.budget_bytes.clamp(MIN_BUDGET, MAX_BUDGET);
    let generated = str_of(&snapshot["generated_utc"]).map(str::to_owned);
    let source_age = generated
        .as_deref()
        .and_then(seconds)
        .zip(options.now_utc.as_deref().and_then(seconds))
        .map(|(source, now)| now - source);
    let interval = snapshot["interval_s"].as_i64().filter(|n| *n > 0);
    // Three missed cycles (or 90 s without a known interval) is stale.
    let stale = source_age.map(|a| a > interval.map_or(90, |i| 3 * i.max(30)));
    let collector = &snapshot["collector"];
    let mut header = format!(
        "# fleet gen={} age={} host={} cycle_ms={} turns={}",
        generated.as_deref().unwrap_or("?"),
        source_age.map_or("?".into(), age),
        str_of(&snapshot["host"]).unwrap_or("?"),
        num(&collector["duration_ms"]),
        num(&collector["model_turns"]),
    );
    if stale == Some(true) {
        header.push_str(" STALE");
    }
    if let Some(project) = &options.project {
        header.push_str(&format!(" project={project}"));
    }

    let wanted = |id: &str| options.project.as_deref().is_none_or(|p| p == id);
    let mut lines = Vec::<Line>::new();
    for key in ["usage_error", "history_error"] {
        if let Some(text) = str_of(&snapshot[key]) {
            lines.push(Line {
                priority: 1,
                text: format!("! src {key}: {text}"),
            });
        }
    }
    let ids = project_ids(snapshot);

    // Priority 0/1: actionable Git health and resource warnings.
    for id in ids.iter().filter(|id| wanted(id)) {
        let report = &snapshot["git_health"][id];
        if report["actionable"] != true {
            continue;
        }
        let mut codes = report["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| str_of(&i["code"]))
            .collect::<Vec<_>>();
        codes.sort_unstable();
        codes.dedup();
        let state = str_of(&report["state"]).unwrap_or("?");
        let fingerprint = str_of(&report["fingerprint"])
            .map(|f| {
                f.trim_start_matches("sha256:")
                    .chars()
                    .take(8)
                    .collect::<String>()
            })
            .unwrap_or_else(|| "?".into());
        lines.push(Line {
            priority: if state == "attention" { 0 } else { 1 },
            text: format!("! git {id} {state} {} fp={fingerprint}", codes.join(",")),
        });
    }
    if options.project.is_none() {
        for warning in snapshot["global_resources"]["warnings"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let level = str_of(&warning["level"]).unwrap_or("?");
            if let Some(text) = str_of(&warning["text"]) {
                lines.push(Line {
                    priority: if level == "red" { 0 } else { 1 },
                    text: format!("! res {level} {text}"),
                });
            }
        }
    }

    // Priority 2 (blocked) / 3: project delivery state.
    for id in ids.iter().filter(|id| wanted(id)) {
        let p = &snapshot["projects"][id];
        if !p.is_object() {
            continue;
        }
        let counts = &p["counts"];
        let blocked = counts["blocked"].as_u64();
        let mut text = format!(
            "proj {id} done={}/{} ip={} blk={}",
            num(&p["done"]),
            num(&p["total"]),
            num(&counts["in_progress"]),
            blocked.map_or("?".into(), |b| b.to_string()),
        );
        let pr = &p["pr"];
        if let Some(number) = pr["number"].as_u64() {
            text.push_str(&format!(
                " pr=#{number}:{} ci={}",
                str_of(&pr["state"]).unwrap_or("?"),
                str_of(&pr["ci"]["state"]).unwrap_or("?")
            ));
        }
        let hygiene = &snapshot["super_captain"]["status"]["captains"][id]["hygiene"];
        if let Some(grade) = str_of(&hygiene["grade"]) {
            text.push_str(&format!(" hyg={grade}{}", num(&hygiene["score"])));
        } else if let Some(range) = hygiene["possible_score_range"]
            .as_array()
            .filter(|r| r.len() == 2)
        {
            text.push_str(&format!(
                " hyg={}-{}(partial)",
                num(&range[0]),
                num(&range[1])
            ));
        }
        let flags = p["flags"].as_array().map_or(0, Vec::len);
        if flags > 0 {
            text.push_str(&format!(" flags={flags}"));
        }
        lines.push(Line {
            priority: if blocked.is_some_and(|b| b > 0) { 2 } else { 3 },
            text,
        });
    }

    // Priority 4: Git metrics.
    for id in ids.iter().filter(|id| wanted(id)) {
        let m = &snapshot["git_metrics"][id];
        if !m.is_object() {
            continue;
        }
        let b = &m["branches"];
        let c = &m["checkouts"];
        let day = m["activity"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["window_hours"] == 24));
        let mut text = format!(
            "git {id} br={} merged={} ahead={} div={} old30d={} co={} dirty={} unk={}",
            num(&b["compared"]),
            num(&b["merged"]),
            num(&b["ahead_only"]),
            num(&b["diverged"]),
            num(&b["unmerged_tips_older_30d"]),
            num(&c["total"]),
            num(&c["dirty"]),
            num(&c["state_unknown"]),
        );
        if let Some(day) = day {
            text.push_str(&format!(
                " int24h={}",
                count(&day["integration_first_parent"])
            ));
        }
        if let Some(tip) = m["integration"]["tip_age_seconds"].as_i64() {
            text.push_str(&format!(" tip={}", age(tip)));
        }
        if m["complete"] == false {
            text.push_str(" partial");
        }
        lines.push(Line { priority: 4, text });
    }

    if options.project.is_none() {
        // Priority 5: hosts.
        for host in snapshot["global_resources"]["hosts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let name = str_of(&host["host"]).unwrap_or("?");
            if host["reachable"] == false {
                lines.push(Line {
                    priority: 1,
                    text: format!("! host {name} unreachable"),
                });
                continue;
            }
            let load = host["load"][0]
                .as_f64()
                .zip(host["nproc"].as_f64().filter(|n| *n > 0.0))
                .map_or(Value::Null, |(l, n)| (100.0 * l / n).into());
            let root = host["disks"]
                .as_array()
                .and_then(|d| d.iter().find(|d| d["mount"] == "/"));
            let gpus = host["gpus"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|g| pct(&g["util"]))
                .collect::<Vec<_>>();
            let mut text = format!(
                "host {name} load={} mem_avail={} root={}",
                pct(&load),
                pct(&host["mem"]["avail_pct"]),
                root.map_or("?".into(), |d| pct(&d["pct"])),
            );
            if let Some(hours) = root.and_then(|d| d["hours_to_full"].as_f64()) {
                text.push_str(&format!(" full_in={}h", trim(hours, 1)));
            }
            if !gpus.is_empty() {
                text.push_str(&format!(" gpu={}", gpus.join(",")));
            }
            lines.push(Line { priority: 5, text });
        }
        // Priority 6: usage and agents.
        let efficiency = &snapshot["token_efficiency"];
        if efficiency.is_object() {
            let window = |minutes: &str| {
                let w = &efficiency["windows"][minutes];
                if w["tokens"].is_number() {
                    si(&w["tokens"])
                } else if w["lower_bound_tokens"].is_number() {
                    format!("{}+", si(&w["lower_bound_tokens"]))
                } else {
                    "?".into()
                }
            };
            let fields = [
                window("15"),
                window("60"),
                pct(&efficiency["quota"]["used_percent"]),
                pct(&efficiency["quota"]["burn_pct_per_hour"]),
                num(&efficiency["cost_index_vs_all_luna"]),
            ];
            // A usage block with no measured value adds tokens, not facts.
            if fields.iter().any(|f| f != "?") {
                lines.push(Line {
                    priority: 6,
                    text: format!(
                        "tok 15m={} 60m={} quota={} burn={}/h cost_idx={}",
                        fields[0], fields[1], fields[2], fields[3], fields[4]
                    ),
                });
            }
        }
        if let Some(agents) = snapshot["agents"].as_array() {
            let mut models = std::collections::BTreeMap::<String, usize>::new();
            for agent in agents {
                let model = str_of(&agent["model"]).unwrap_or("unknown").to_owned();
                *models.entry(model).or_default() += 1;
            }
            let mut ranked = models.into_iter().collect::<Vec<_>>();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let mix = ranked
                .iter()
                .take(6)
                .map(|(m, n)| format!("{m}:{n}"))
                .collect::<Vec<_>>()
                .join(",");
            lines.push(Line {
                priority: 6,
                text: format!(
                    "agents n={} models={}",
                    agents.len(),
                    if mix.is_empty() { "-" } else { &mix }
                ),
            });
        }
    }

    // Stable order: priority, then original (project order) position.
    lines.sort_by_key(|l| l.priority);
    let mut text = header;
    let mut included = 1;
    let mut omitted = 0;
    let total = lines.len();
    for (index, line) in lines.iter().enumerate() {
        // Reserve room for an omission marker when later lines might not fit.
        let remaining = total - index - 1;
        let reserve = if remaining > 0 { 40 } else { 0 };
        if omitted == 0 && text.len() + 1 + line.text.len() + reserve <= budget {
            text.push('\n');
            text.push_str(&line.text);
            included += 1;
        } else {
            omitted += 1;
        }
    }
    if omitted > 0 {
        let marker = format!("\n… omitted={omitted} budget={budget}B");
        if text.len() + marker.len() <= budget {
            text.push_str(&marker);
        }
    }
    Brief {
        bytes: text.len(),
        text,
        lines: included,
        omitted_lines: omitted,
        source_generated_utc: generated,
        source_age_s: source_age,
        stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> Value {
        json!({
            "generated_utc":"2026-10-21T07:12:00Z","interval_s":15,"host":"node-a",
            "order":["joil","dope"],
            "collector":{"duration_ms":1834,"model_turns":0},
            "projects":{
                "joil":{"done":160,"total":160,"counts":{"in_progress":0,"blocked":0}},
                "dope":{"done":61,"total":65,"counts":{"in_progress":0,"blocked":4},
                    "pr":{"number":212,"state":"open","ci":{"state":"failure"}},"flags":["x"]}
            },
            "super_captain":{"status":{"captains":{"dope":{"hygiene":{"grade":"C","score":50}},
                "joil":{"hygiene":{"score":null,"possible_score_range":[30,80]}}}}},
            "git_health":{"dope":{"state":"attention","actionable":true,"fingerprint":"sha256:3f2a91c4ffff",
                "issues":[{"code":"merge_conflicts"},{"code":"diverged_branch"},{"code":"merge_conflicts"}]},
                "joil":{"state":"healthy","actionable":false}},
            "git_metrics":{"dope":{"complete":true,"branches":{"compared":14,"merged":9,"ahead_only":3,"diverged":2,"unmerged_tips_older_30d":1},
                "checkouts":{"total":6,"dirty":2,"state_unknown":0},
                "integration":{"tip_age_seconds":10800},
                "activity":[{"window_hours":24,"integration_first_parent":{"value":4,"lower_bound":true}}]}},
            "global_resources":{"warnings":[{"level":"amber","text":"node-a disk / 91.2% used"}],
                "hosts":[{"host":"node-a","reachable":true,"nproc":100,"load":[55.1,1,1],"mem":{"avail_pct":61.0},
                    "disks":[{"mount":"/","pct":91.2,"hours_to_full":11.43}],"gpus":[{"util":43}]},
                    {"host":"node-b","reachable":false}]},
            "token_efficiency":{"windows":{"15":{"tokens":1_210_000},"60":{"tokens":null,"lower_bound_tokens":4_870_000}},
                "quota":{"used_percent":41.0,"burn_pct_per_hour":2.1},"cost_index_vs_all_luna":0.6234},
            "agents":[{"model":"luna"},{"model":"luna"},{"model":null}]
        })
    }

    fn options(budget: usize) -> BriefOptions {
        BriefOptions {
            budget_bytes: budget,
            project: None,
            now_utc: Some("2026-10-21T07:12:09Z".into()),
        }
    }

    #[test]
    fn urgent_facts_come_first_and_unknowns_are_never_zero() {
        let out = brief(&snapshot(), &options(4_000));
        let lines = out.text.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "# fleet gen=2026-10-21T07:12:00Z age=9s host=node-a cycle_ms=1834 turns=0"
        );
        assert_eq!(
            lines[1],
            "! git dope attention diverged_branch,merge_conflicts fp=3f2a91c4"
        );
        assert!(lines[2..4].contains(&"! res amber node-a disk / 91.2% used"));
        assert!(lines[2..4].contains(&"! host node-b unreachable"));
        assert!(
            out.text.contains(
                "proj dope done=61/65 ip=0 blk=4 pr=#212:open ci=failure hyg=C50 flags=1"
            )
        );
        assert!(
            out.text
                .contains("proj joil done=160/160 ip=0 blk=0 hyg=30-80(partial)")
        );
        assert!(out.text.contains(
            "git dope br=14 merged=9 ahead=3 div=2 old30d=1 co=6 dirty=2 unk=0 int24h=4+ tip=3h"
        ));
        assert!(
            out.text
                .contains("host node-a load=55.1% mem_avail=61% root=91.2% full_in=11.4h gpu=43%")
        );
        assert!(
            out.text
                .contains("tok 15m=1.21M 60m=4.87M+ quota=41% burn=2.1%/h cost_idx=0.62")
        );
        assert!(out.text.contains("agents n=3 models=luna:2,unknown:1"));
        assert_eq!(out.omitted_lines, 0);
        assert_eq!(out.stale, Some(false));
        assert_eq!(out.bytes, out.text.len());
    }

    #[test]
    fn budget_keeps_header_and_urgent_lines_and_counts_omissions() {
        let out = brief(&snapshot(), &options(MIN_BUDGET));
        assert!(out.bytes <= MIN_BUDGET, "{}", out.text);
        assert!(out.text.starts_with("# fleet "));
        assert!(out.omitted_lines > 0);
        assert!(
            out.text.ends_with(&format!("budget={MIN_BUDGET}B")),
            "{}",
            out.text
        );
        let full = brief(&snapshot(), &options(4_000));
        assert_eq!(out.lines + out.omitted_lines, full.lines);
    }

    #[test]
    fn project_filter_and_stale_source_are_explicit() {
        let mut options = options(4_000);
        options.project = Some("joil".into());
        options.now_utc = Some("2026-10-21T08:00:00Z".into());
        let out = brief(&snapshot(), &options);
        assert!(
            out.text
                .lines()
                .next()
                .expect("header")
                .ends_with("STALE project=joil")
        );
        assert!(!out.text.contains("dope"));
        assert!(!out.text.contains("host "));
        assert_eq!(out.stale, Some(true));
    }

    #[test]
    fn empty_or_foreign_snapshot_yields_only_an_honest_header() {
        let out = brief(&json!({"unexpected":true}), &BriefOptions::default());
        assert_eq!(out.text, "# fleet gen=? age=? host=? cycle_ms=? turns=?");
        assert_eq!(out.stale, None);
        assert_eq!(out.source_age_s, None);
    }

    #[test]
    fn unmeasured_usage_is_omitted_rather_than_rendered_as_question_marks() {
        let mut snapshot = snapshot();
        snapshot["token_efficiency"] = json!({"windows":{},"quota":{}});
        let out = brief(&snapshot, &options(4_000));
        assert!(!out.text.contains("tok "));
    }

    #[test]
    fn identical_input_is_byte_identical() {
        assert_eq!(
            brief(&snapshot(), &options(900)),
            brief(&snapshot(), &options(900))
        );
    }
}
