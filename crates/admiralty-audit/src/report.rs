//! The audit run (`main()` of sc_audit.py): collects screens, lanes, ledgers,
//! GitHub state, progress and watchdog findings, then writes
//! `<OUT>/state.json`, `<OUT>/latest.json`, `<OUT>/latest.md` (tmp + rename
//! where the Python did) and appends `<OUT>/audit.jsonl`.
//!
//! Never writes to project repos, never types into screens, never kills anything.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::AuditConfig;
use crate::ctx::{Audit, next_since, prev_entry, rj, rj_or_empty, tail_text};
use crate::host::Host;
use crate::lanes::{cpuset, fmt_cpus};
use crate::py::{self, Dt, J, Obj, PyErr, PyResult};
use crate::queue_health::{DEFAULT_MAX_THREADS, DEFAULT_STALE_MS, read_queue_health};
use crate::screen::screen_state;

static CPU_CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A(\d+),(\d+)\s+(\w+)\s+(\S+)\s+\S+Z\s*\z").expect("valid built-in regex")
});
static AGENT_PROC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"codex|claude|agy|cargo|python|pytest|rustc|node").expect("valid built-in regex")
});
static CPUS_ALLOWED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Cpus_allowed_list:\s*(\S+)").expect("valid built-in regex"));

/// Command-line options of one audit run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditOptions {
    pub out: String,
    pub quiet: bool,
    pub no_gh: bool,
}

/// Why a run failed (Python would have exited 1 with a traceback).
#[derive(Debug)]
pub enum AuditFailure {
    Py(PyErr),
    Io(String),
}

impl std::fmt::Display for AuditFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditFailure::Py(e) => write!(f, "{}: {}", e.kind, e.msg),
            AuditFailure::Io(e) => write!(f, "OSError: {e}"),
        }
    }
}

impl std::error::Error for AuditFailure {}

impl From<PyErr> for AuditFailure {
    fn from(e: PyErr) -> Self {
        AuditFailure::Py(e)
    }
}

/// Result of a successful run.
#[derive(Debug, Clone)]
pub struct AuditOutput {
    pub report: J,
    pub markdown: String,
    pub state: J,
}

fn sev_rank(s: &str) -> u8 {
    match s {
        "red" => 0,
        "amber" => 1,
        "info" => 2,
        _ => 3,
    }
}

fn projects_mut(report: &mut Obj) -> &mut Obj {
    report.obj_entry("projects")
}

fn sub_obj_mut<'o>(report: &'o mut Obj, key: &str) -> &'o mut Obj {
    report.obj_entry(key)
}

fn atomic_write(path: &str, text: &str) -> Result<(), AuditFailure> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, text).map_err(|e| AuditFailure::Io(format!("{tmp}: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| AuditFailure::Io(format!("{path}: {e}")))
}

/// Render `latest.md` from the report (Python f-string parity).
pub fn render_markdown(report: &Obj) -> String {
    let get = |o: &J, k: &str| {
        o.as_obj()
            .and_then(|x| x.get(k))
            .cloned()
            .unwrap_or_default()
    };
    let mut md = vec![
        format!(
            "# super_captain audit {}",
            py::py_str(report.get("utc").unwrap_or(&J::Null))
        ),
        String::new(),
        "| Project | done (recomputed) / total | super_captain reported | captain claim | source |"
            .to_string(),
        "|---|---|---|---|---|".to_string(),
    ];
    if let Some(J::Obj(projects)) = report.get("projects") {
        for (p, r) in projects.iter() {
            let claim = [
                get(r, "captain_claim"),
                get(r, "captain_claim_max_recent"),
                get(r, "declared_counter"),
            ]
            .into_iter()
            .find(J::truthy)
            .map_or_else(String::new, |c| py::py_str(&c));
            let source = r
                .as_obj()
                .and_then(|o| o.get("source"))
                .map_or_else(String::new, py::py_str);
            md.push(format!(
                "| {p} | {} / {} | {} | {claim} | {source} |",
                py::py_str(&get(r, "done_recomputed")),
                py::py_str(&get(r, "total")),
                py::py_str(&get(r, "super_captain_reported_done"))
            ));
        }
    }
    md.extend([
        String::new(),
        "## Progress since last check-in (accountability)".to_string(),
        String::new(),
        "| Project | done now (baseline) | blocked now (baseline) | PRs merged since baseline | done last increased | baseline |"
            .to_string(),
        "|---|---|---|---|---|---|".to_string(),
    ]);
    if let Some(J::Obj(progress)) = report.get("progress") {
        for (p, i) in progress.iter() {
            let mut line = format!(
                "| {p} | {} ({}) | {} ({}) | {} | {} | {} |",
                py::py_str(&get(i, "done")),
                py::py_str(&get(i, "baseline_done")),
                py::py_str(&get(i, "blocked")),
                py::py_str(&get(i, "baseline_blocked")),
                py::py_str(&get(i, "merged_prs_since_baseline")),
                py::py_str(&get(i, "last_done_increase_utc")),
                py::py_str(&get(i, "baseline_utc"))
            );
            if let Some(dm) = i.as_obj().and_then(|o| o.get("done_on_main")) {
                line.push_str(&format!(" done on origin/main: {}", py::py_str(dm)));
            }
            md.push(line);
        }
    }
    let finding_line = |f: &J| {
        format!(
            "- **{}** `{}` {}: {}",
            py::py_str(&get(f, "severity")),
            py::py_str(&get(f, "scope")),
            py::py_str(&get(f, "code")),
            py::py_str(&get(f, "text"))
        )
    };
    let must_act = report
        .get("must_act")
        .and_then(J::as_arr)
        .cloned()
        .unwrap_or_default();
    md.extend([
        String::new(),
        format!("## MUST ACT THIS TICK ({})", must_act.len()),
        String::new(),
    ]);
    if must_act.is_empty() {
        md.push("- none".to_string());
    } else {
        md.extend(must_act.iter().map(finding_line));
    }
    let summary = report.get("summary").cloned().unwrap_or_default();
    md.extend([
        String::new(),
        format!("## Findings ({})", py::py_repr(&summary)),
        String::new(),
    ]);
    let findings = report
        .get("findings")
        .and_then(J::as_arr)
        .cloned()
        .unwrap_or_default();
    if findings.is_empty() {
        md.push("- none".to_string());
    } else {
        for f in &findings {
            let mut line = finding_line(f);
            let ev = get(f, "evidence");
            if ev.truthy() {
                line.push_str(&format!(" (evidence: `{}`)", py::py_str(&ev)));
            }
            md.push(line);
        }
    }
    md.join("\n") + "\n"
}

impl Audit<'_> {
    fn captain_screens(
        &mut self,
        report: &mut Obj,
        scr: &crate::screen::Screens,
        prev: &J,
        newh: &mut Obj,
        now: &Dt,
    ) -> PyResult<()> {
        let mut pairs: Vec<(String, String)> = self
            .cfg
            .projects
            .iter()
            .map(|p| (p.name.clone(), p.captain.clone()))
            .collect();
        pairs.push(("super_captain".to_string(), "super_captain".to_string()));
        let now_iso = py::iso_seconds(now);
        for (proj, name) in pairs {
            if !scr.contains_key(&name) {
                self.f(
                    "red",
                    &proj,
                    "SCREEN_MISSING",
                    &format!("{name} screen is not running"),
                    None,
                );
                continue;
            }
            let hc = self.cfg.audit_hc(&name);
            let txt = self.hardcopy(&name, &hc);
            let s = screen_state(&txt);
            let p = prev_entry(prev, &name)?;
            let since = next_since(&p, &J::from(s.hash.as_str()), &now_iso)?;
            newh.set(
                name.clone(),
                crate::jobj! {"hash" => s.hash.as_str(), "since" => since.clone()},
            );
            let mut entry = match s.to_json_without_hash() {
                J::Obj(o) => o,
                _ => Obj::new(),
            };
            if s.survey {
                self.f(
                    "red",
                    &proj,
                    "SURVEY_PROMPT",
                    &format!("{name} shows a survey/feedback prompt; inspect verified owner/endpoint and supported queue/priority status without terminal input. Preserve the prompt/draft; notLoaded is BLOCKED_TRANSPORT, never resume or duplicate"),
                    Some(&hc),
                );
            }
            if s.approval_prompt {
                self.f(
                    "red",
                    &proj,
                    "APPROVAL_PROMPT",
                    &format!("{name} is waiting on an approval/confirm prompt"),
                    Some(&hc),
                );
            }
            if s.quota {
                self.f(
                    "amber",
                    &proj,
                    "QUOTA_TEXT",
                    &format!("{name} tail mentions quota/rate limit"),
                    Some(&hc),
                );
            }
            if s.queued_msgs {
                self.f(
                    "amber",
                    &proj,
                    "QUEUED_MESSAGES",
                    &format!("{name} has user-queued messages not yet delivered (busy turn); consumption remains unverified; preserve drafts and confirm the actual recipient"),
                    Some(&hc),
                );
            }
            let age = if since.truthy() {
                py::since_age_min(now, &since)?
            } else {
                0.0
            };
            entry.set("unchanged_min", py::py_round(age));
            if age >= 25.0 && s.busy {
                self.f(
                    "amber",
                    &proj,
                    "SCREEN_FROZEN",
                    &format!(
                        "{name} busy but screen unchanged for {} min (hung tool call?)",
                        py::py_round(age)
                    ),
                    None,
                );
            }
            if age >= 25.0 && s.idle_prompt && proj != "super_captain" {
                self.f(
                    "amber",
                    &proj,
                    "CAPTAIN_IDLE",
                    &format!("{name} idle at prompt for {} min", py::py_round(age)),
                    None,
                );
            }
            if s.typed_input {
                entry.set(
                    "note",
                    "text typed at prompt (maybe Jepson) - do not overwrite",
                );
            }
            sub_obj_mut(report, "screens").set(name.clone(), J::Obj(entry));
        }
        Ok(())
    }

    /// CPU pinning probe of a lane's process tree: (allowed cpus, cwd).
    fn agent_pinning(&self, pid: i64) -> (BTreeSet<i64>, Option<String>) {
        let mut allowed = BTreeSet::new();
        let mut cwd = None;
        for p in self.descendants(pid) {
            let Some(raw) = self.host.read_proc(&format!("/proc/{p}/cmdline")) else {
                continue;
            };
            let cl: Vec<u8> = raw
                .iter()
                .map(|b| if *b == 0 { b' ' } else { *b })
                .collect();
            let cl = String::from_utf8_lossy(&cl);
            if !AGENT_PROC.is_match(&cl) {
                continue;
            }
            let Some(status) = self.host.read_proc(&format!("/proc/{p}/status")) else {
                continue;
            };
            let Ok(status) = String::from_utf8(status) else {
                continue;
            };
            let Some(list) = CPUS_ALLOWED.captures(&status).and_then(|m| m.get(1)) else {
                continue;
            };
            allowed = cpuset(list.as_str());
            let Some(link) = self.host.read_link(&format!("/proc/{p}/cwd")) else {
                continue;
            };
            cwd = Some(link);
            break;
        }
        (allowed, cwd)
    }

    fn lanes_section(
        &mut self,
        report: &mut Obj,
        scr: &crate::screen::Screens,
        prev: &J,
        newh: &mut Obj,
        now: &Dt,
    ) -> PyResult<()> {
        let mut overflow_claims: Vec<(String, BTreeSet<i64>)> = Vec::new();
        for l in tail_text(&format!("{}/CPU_SLICES.md", self.cfg.kick), 200_000).split('\n') {
            if let Some(m) = CPU_CLAIM.captures(py::py_strip(l)) {
                let (Some(a), Some(b), Some(n)) = (m.get(1), m.get(2), m.get(4)) else {
                    continue;
                };
                let (Some(a), Some(b)) = (py::py_int(a.as_str()), py::py_int(b.as_str())) else {
                    continue;
                };
                let name = n.as_str().to_string();
                match overflow_claims.iter_mut().find(|(k, _)| *k == name) {
                    Some(slot) => {
                        slot.1.insert(a);
                        slot.1.insert(b);
                    }
                    None => overflow_claims.push((name, [a, b].into_iter().collect())),
                }
            }
        }
        let allowed_range = cpuset(&self.cfg.agent_range);
        let reserved = cpuset(&self.cfg.reserved_non_joil);
        let overflow = cpuset(&self.cfg.overflow);
        let now_iso = py::iso_seconds(now);
        let projects: Vec<crate::config::ProjectConfig> = self.cfg.projects.clone();
        for pc in &projects {
            let proj = pc.name.as_str();
            let lanes = self.lanes_from_log(proj)?;
            let wts = self.worktrees(&pc.repo);
            let locked: Vec<String> = wts
                .iter()
                .filter(|w| w.locked)
                .map(|w| w.path.clone())
                .collect();
            let mut wt_roots: Vec<String> = wts.iter().map(|w| w.path.clone()).collect();
            wt_roots.sort_by_key(|w| std::cmp::Reverse(py::char_len(w)));
            let mut by_wt: Vec<(String, Vec<String>)> = Vec::new();
            let mut out = Obj::new();
            let mut dead: Vec<String> = Vec::new();
            for (n, d) in lanes.iter() {
                let state = d
                    .as_obj()
                    .and_then(|o| o.get("state"))
                    .map_or_else(String::new, py::py_str);
                if !matches!(
                    state.to_lowercase().as_str(),
                    "running" | "in_progress" | "active"
                ) {
                    continue;
                }
                let mut d = d.as_obj().cloned().unwrap_or_default();
                let host = d
                    .get("host")
                    .map_or_else(|| self.cfg.local_host.clone(), py::py_str);
                let task = d.get("task").cloned().unwrap_or_default();
                let task_s = py::py_str(&task);
                let mut info = Obj::new();
                info.set("task", task.clone());
                info.set("wt", d.get("wt").cloned().unwrap_or_default());
                info.set("host", host.as_str());
                info.set("alive", J::Null);
                if host == self.cfg.local_host {
                    let alive = scr.contains_key(n);
                    info.set("alive", alive);
                    if !alive {
                        dead.push(format!("{n}({task_s})"));
                    } else {
                        let hc = self.cfg.audit_hc(n);
                        let txt = self.hardcopy(n, &hc);
                        let s = screen_state(&txt);
                        let p = prev_entry(prev, n)?;
                        let since = next_since(&p, &J::from(s.hash.as_str()), &now_iso)?;
                        newh.set(
                            n.clone(),
                            crate::jobj! {"hash" => s.hash.as_str(), "since" => since.clone()},
                        );
                        let age = py::since_age_min(now, &since)?;
                        info.set("unchanged_min", py::py_round(age));
                        if age >= 30.0 {
                            let text = format!(
                                "{n} ({task_s}) screen unchanged {} min while LANE says running",
                                py::py_round(age)
                            );
                            self.f("amber", proj, "LANE_STALLED", &text, Some(&hc));
                        }
                        if s.survey || s.approval_prompt {
                            self.f(
                                "red",
                                proj,
                                "LANE_PROMPT_BLOCKED",
                                &format!("{n} is stuck on a survey/approval prompt"),
                                Some(&hc),
                            );
                        }
                        if s.quota {
                            self.f(
                                "amber",
                                proj,
                                "LANE_QUOTA",
                                &format!("{n} mentions quota/rate limit"),
                                Some(&hc),
                            );
                        }
                        let pid = scr.get(n).copied().unwrap_or(0);
                        let (allowed, cwd) = self.agent_pinning(pid);
                        if let Some(cwd) = cwd.filter(|c| !c.is_empty())
                            && let Some(root) = wt_roots
                                .iter()
                                .find(|w| cwd == **w || cwd.starts_with(&format!("{w}/")))
                        {
                            info.set("wt_actual", root.as_str());
                            d.set("wt", root.as_str());
                        }
                        if !allowed.is_empty() {
                            info.set("cpus_allowed", fmt_cpus(&allowed));
                            let mut own = cpuset(&pc.slice);
                            if let Some((_, claims)) = overflow_claims.iter().find(|(k, _)| k == n)
                            {
                                own.extend(claims.iter().copied());
                            }
                            let outside: BTreeSet<i64> =
                                allowed.difference(&allowed_range).copied().collect();
                            let overlap: BTreeSet<i64> =
                                allowed.intersection(&reserved).copied().collect();
                            let beyond: BTreeSet<i64> = allowed
                                .iter()
                                .filter(|c| !own.contains(c) && !overflow.contains(c))
                                .copied()
                                .collect();
                            if allowed.len() >= 120 {
                                let text =
                                    format!("{n} is not CPU-pinned (all {} CPUs)", allowed.len());
                                self.f("amber", proj, "LANE_UNPINNED", &text, None);
                            } else if !outside.is_empty() {
                                let text = format!(
                                    "{n} may run on {} (outside 0-50,64-114)",
                                    fmt_cpus(&outside)
                                );
                                self.f("red", proj, "LANE_CPU_OUTSIDE_AGENT_RANGE", &text, None);
                            } else if proj != "joil" && !overlap.is_empty() && allowed.len() < 60 {
                                let text = format!(
                                    "{n} pinned to {}, overlapping reserved CPUs {} (Joil lease/68-77 are off-limits per CPU_SLICES.md/COMMON.md)",
                                    fmt_cpus(&allowed),
                                    fmt_cpus(&overlap)
                                );
                                self.f("red", proj, "LANE_CPU_RESERVED", &text, None);
                            } else if proj != "joil" && !beyond.is_empty() {
                                let text = format!(
                                    "{n} pinned to {}; not confined to {proj} slice {} (+claimed overflow)",
                                    fmt_cpus(&allowed),
                                    pc.slice
                                );
                                self.f("amber", proj, "LANE_CPU_OUTSIDE_SLICE", &text, None);
                            }
                        }
                    }
                    let wt = d.get("wt").cloned().unwrap_or_default();
                    if wt.truthy() && alive {
                        let wt = py::py_str(&wt).trim_end_matches('/').to_string();
                        if locked.contains(&wt) {
                            let text = format!("{n} ({task_s}) runs in LOCKED worktree {wt}");
                            self.f("red", proj, "LOCKED_WORKTREE_IN_USE", &text, Some(&pc.log));
                        }
                        if wt != pc.repo {
                            match by_wt.iter_mut().find(|(k, _)| *k == wt) {
                                Some(slot) => slot.1.push(n.clone()),
                                None => by_wt.push((wt, vec![n.clone()])),
                            }
                        }
                    }
                } else if self.cfg.remote_lane_hosts.contains(&host) {
                    match self.remote_screens(&host) {
                        None => info.set("alive", "unreachable"),
                        Some(rs) => {
                            let alive = rs.contains_key(n);
                            info.set("alive", alive);
                            if !alive {
                                dead.push(format!("{n}@{host}({task_s})"));
                            } else {
                                let h = J::from(self.remote_hash(&host, n));
                                let key = format!("{host}:{n}");
                                let p = prev_entry(prev, &key)?;
                                let since = next_since(&p, &h, &now_iso)?;
                                newh.set(
                                    key,
                                    crate::jobj! {"hash" => h.clone(), "since" => since.clone()},
                                );
                                let age = py::since_age_min(now, &since)?;
                                info.set("unchanged_min", py::py_round(age));
                                if h.truthy() && age >= 30.0 {
                                    let text = format!(
                                        "{n}@{host} ({task_s}) screen unchanged {} min while LANE says running",
                                        py::py_round(age)
                                    );
                                    self.f("amber", proj, "LANE_STALLED", &text, None);
                                }
                            }
                        }
                    }
                }
                out.set(n.clone(), J::Obj(info));
            }
            if !dead.is_empty() {
                let shown: Vec<&str> = dead.iter().take(25).map(String::as_str).collect();
                let text = format!(
                    "{} LANE lines still say running but their screens are gone: {}; captain must post closing LANE lines",
                    dead.len(),
                    shown.join(" ")
                );
                self.f("amber", proj, "LANE_LINES_STALE", &text, Some(&pc.log));
            }
            for (wt, ns) in &by_wt {
                if ns.len() > 1 {
                    let text = format!(
                        "{} running lanes share worktree {wt}: {} (one writer per worktree; read-only verify lanes must use their own checkout)",
                        ns.len(),
                        ns.join(", ")
                    );
                    self.f(
                        "amber",
                        proj,
                        "TWO_WRITERS_ONE_WORKTREE",
                        &text,
                        Some(&pc.log),
                    );
                }
            }
            for w in &locked {
                let mut users: Vec<J> = Vec::new();
                for pid in self.host.proc_pids() {
                    if let Some(c) = self.host.read_link(&format!("/proc/{pid}/cwd"))
                        && (c == *w || c.starts_with(&format!("{w}/")))
                    {
                        users.push(J::Int(pid));
                    }
                }
                if !users.is_empty() {
                    let text = format!(
                        "{} process(es) have cwd inside locked worktree {w} (pids {})",
                        users.len(),
                        py::py_repr(&J::Arr(users.iter().take(5).cloned().collect()))
                    );
                    self.f("red", proj, "LOCKED_WORKTREE_PROCESS", &text, Some(w));
                }
            }
            sub_obj_mut(report, "lanes").set(proj, J::Obj(out));
        }
        Ok(())
    }

    fn native_queue(&mut self, report: &mut Obj, now: &Dt) -> PyResult<()> {
        let now_ms = (py::timestamp_f64(now) * 1000.0) as i64;
        let nq = read_queue_health(
            Path::new(&self.cfg.queue_db),
            now_ms,
            DEFAULT_MAX_THREADS,
            DEFAULT_STALE_MS,
        )?;
        report.set("native_queue", nq.clone());
        let source = py::py_str(nq.pget("source")?);
        if !py::eq_str(nq.pget("status")?, "observed") {
            self.f(
                "amber",
                "super_captain",
                "NATIVE_QUEUE_UNKNOWN",
                "Native queue observation is unavailable or incomplete; delivery and empty queue are unverified",
                Some(&source),
            );
        }
        for queued in nq.pget("threads")?.py_iter()? {
            let state = queued.pidx("state")?;
            let stalled = py::eq_str(state, "stalled") || py::eq_str(state, "clock_unknown");
            let text = format!(
                "Thread {} has {} persisted pending messages; oldest age_ms={}. Queued is not consumed or started. \
Identify its current owner/endpoint and obtain correlated consumption/start; preserve all drafts/backlog, never blindly resend, resume or type.",
                py::py_str(queued.pidx("thread_id")?),
                py::py_str(queued.pidx("pending")?),
                py::py_str(queued.pidx("oldest_age_ms")?)
            );
            self.f(
                if stalled { "red" } else { "amber" },
                "super_captain",
                if stalled {
                    "NATIVE_QUEUE_STALLED"
                } else {
                    "NATIVE_QUEUE_PENDING"
                },
                &text,
                Some(&source),
            );
        }
        Ok(())
    }

    fn sc_reported(&mut self, report: &mut Obj, now: &Dt) -> PyResult<()> {
        let status_path = self.cfg.status_json();
        let scs = rj_or_empty(&status_path);
        report.set("super_captain_status_utc", scs.pget("updated_utc")?.clone());
        let caps = scs.pget("captains")?;
        let caps = if caps.truthy() {
            caps.clone()
        } else {
            J::Obj(Obj::new())
        };
        let names: Vec<String> = projects_mut(report).keys().cloned().collect();
        for proj in names {
            let blk = caps.pget(&proj)?;
            let blk = if blk.truthy() {
                blk.clone()
            } else {
                J::Obj(Obj::new())
            };
            let claimed = blk.pget("done")?.clone();
            let Some(J::Obj(r)) = projects_mut(report).get_mut(&proj) else {
                continue;
            };
            r.set("super_captain_reported_done", claimed.clone());
            let r = r.clone();
            let mut cands: Vec<J> = Vec::new();
            for k in ["done_recomputed", "packets_accepted"] {
                let v = r.get(k).cloned().unwrap_or_default();
                if !v.is_null() && !cands.iter().any(|c| py::py_eq(c, &v)) {
                    cands.push(v);
                }
            }
            if let Some(c) = claimed.as_inst_int()
                && !cands.is_empty()
            {
                let mut max = cands[0].clone();
                for x in &cands[1..] {
                    if py::py_cmp(x, &max)? == std::cmp::Ordering::Greater {
                        max = x.clone();
                    }
                }
                if py::py_cmp(&J::Int(c), &max)? == std::cmp::Ordering::Greater {
                    let g = |k: &str| py::py_str(r.get(k).unwrap_or(&J::Null));
                    let packets = if r.get("packets_total").is_some_and(J::truthy) {
                        format!(
                            " (packets {}/{})",
                            g("packets_accepted"),
                            g("packets_total")
                        )
                    } else {
                        String::new()
                    };
                    let text = format!(
                        "super_captain_status.json reports done={} but the ledger gives {}/{}{packets} - super_captain must take done counts from this audit, not from captain claims",
                        py::py_str(&claimed),
                        g("done_recomputed"),
                        g("total")
                    );
                    self.ma(
                        "red",
                        &proj,
                        "SC_REPORTED_OVERCLAIM",
                        &text,
                        Some(&status_path),
                    );
                }
            }
        }
        let updated = py::py_str(scs.pget("updated_utc")?).replace('Z', "+00:00");
        match py::fromisoformat(&updated).and_then(|p| p.aware()) {
            Some(upd) => {
                let secs = py::total_seconds(now, &upd);
                if secs > 40.0 * 60.0 {
                    let text = format!(
                        "super_captain_status.json not updated for {} min",
                        (secs / 60.0) as i64
                    );
                    self.f("red", "super_captain", "SC_STALE", &text, None);
                }
            }
            None => self.f(
                "amber",
                "super_captain",
                "SC_STATUS_UNREADABLE",
                "super_captain_status.json missing updated_utc",
                None,
            ),
        }
        Ok(())
    }
}

/// Run the audit and write its outputs. Prints nothing (the caller prints the Markdown).
pub fn run_audit(
    cfg: &AuditConfig,
    host: &dyn Host,
    opts: &AuditOptions,
) -> Result<AuditOutput, AuditFailure> {
    let out = opts.out.clone();
    std::fs::create_dir_all(&out).map_err(|e| AuditFailure::Io(format!("{out}: {e}")))?;
    let now = host.now();
    let state_j = match rj(&format!("{out}/state.json")) {
        Some(s) if s.truthy() => s,
        _ => crate::jobj! {"hashes" => J::Obj(Obj::new())},
    };
    let prev = state_j.pget_or("hashes", &J::Obj(Obj::new()))?.clone();
    let mut state = match state_j {
        J::Obj(o) => o,
        other => return Err(PyErr::attr_get(&other).into()),
    };
    let mut audit = Audit::new(cfg, host);
    let scr = audit.screens();
    let mut newh = Obj::new();
    let mut report = Obj::new();
    report.set("utc", py::iso_seconds(&now));
    report.set("projects", J::Obj(Obj::new()));
    report.set("screens", J::Obj(Obj::new()));
    report.set("lanes", J::Obj(Obj::new()));

    // The terminal hint is not the durable Codex queue. Observe the latter
    // independently, including Admiral. Never turn unreadable/missing into zero.
    audit.native_queue(&mut report, &now)?;
    // 1. captain + super_captain screens
    audit.captain_screens(&mut report, &scr, &prev, &mut newh, &now)?;
    // 2. lanes: alive, stalled, shared/locked worktrees, cpu pinning
    audit.lanes_section(&mut report, &scr, &prev, &mut newh, &now)?;
    // 3. ledgers / receipts recomputed
    let jope = audit.audit_jopedime()?;
    projects_mut(&mut report).set("jopedime", jope);
    let dope = audit.audit_dope(None, None, None)?;
    projects_mut(&mut report).set("dope", dope);
    let jar = audit.audit_jarambit();
    projects_mut(&mut report).set("jarambit", jar);
    let joil = audit.audit_joil();
    projects_mut(&mut report).set("joil", joil);
    let adm = audit.audit_admiralty(&scr, &prev, &mut newh, &now)?;
    projects_mut(&mut report).set("admiralty", adm);
    if !opts.no_gh {
        let names: Vec<String> = cfg.projects.iter().map(|p| p.name.clone()).collect();
        for proj in names {
            let prs = audit.open_prs(&proj)?;
            if let Some(J::Obj(r)) = projects_mut(&mut report).get_mut(&proj) {
                r.set("open_prs", prs);
            } else {
                return Err(PyErr::key(&proj).into());
            }
            let ci = audit.main_ci(&proj)?;
            if let Some(J::Obj(r)) = projects_mut(&mut report).get_mut(&proj) {
                r.set("main_ci", ci);
            }
        }
    }
    audit.progress_accountability(&mut report, &mut state, &out, &now, opts.no_gh)?;
    // must-act detectors (idle workers, overdue ETAs, undelivered re-drives, no push, false claims)
    audit.watchdog(&mut report, &scr, &prev, &mut newh, &now, opts.no_gh)?;
    // 4. super_captain's own last report vs recomputed numbers
    audit.sc_reported(&mut report, &now)?;

    let mut findings = std::mem::take(&mut audit.findings);
    findings.sort_by(|a, b| {
        (sev_rank(&a.severity), &a.scope, &a.code).cmp(&(sev_rank(&b.severity), &b.scope, &b.code))
    });
    let findings_j: Vec<J> = findings.iter().map(|f| f.to_json()).collect();
    report.set("findings", J::Arr(findings_j.clone()));
    let mut summary = Obj::new();
    for f in &findings {
        let n = summary
            .get(&f.severity)
            .and_then(J::as_inst_int)
            .unwrap_or(0);
        summary.set(f.severity.clone(), n + 1);
    }
    let must_act: Vec<J> = findings
        .iter()
        .filter(|f| f.must_act)
        .map(|f| f.to_json())
        .collect();
    report.set("summary", J::Obj(summary.clone()));
    report.set("must_act", J::Arr(must_act.clone()));
    summary.set("must_act", must_act.len());
    report.set("summary", J::Obj(summary.clone()));
    state.set("hashes", J::Obj(newh));
    state.set("last_utc", report.get("utc").cloned().unwrap_or_default());
    let state_j = J::Obj(state);
    atomic_write(&format!("{out}/state.json"), &py::dumps(&state_j, Some(1)))?;
    let report_j = J::Obj(report.clone());
    atomic_write(
        &format!("{out}/latest.json"),
        &py::dumps(&report_j, Some(1)),
    )?;
    let done: Obj = match report.get("projects") {
        Some(J::Obj(p)) => p
            .iter()
            .map(|(k, r)| {
                (
                    k.clone(),
                    r.as_obj()
                        .and_then(|o| o.get("done_recomputed"))
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect(),
        _ => Obj::new(),
    };
    let line = crate::jobj! {
        "utc" => report.get("utc").cloned().unwrap_or_default(),
        "summary" => J::Obj(summary),
        "findings" => J::Arr(findings_j),
        "done" => J::Obj(done),
    };
    let jsonl = format!("{out}/audit.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl)
        .map_err(|e| AuditFailure::Io(format!("{jsonl}: {e}")))?;
    f.write_all(format!("{}\n", py::dumps(&line, None)).as_bytes())
        .map_err(|e| AuditFailure::Io(format!("{jsonl}: {e}")))?;
    let markdown = render_markdown(&report);
    std::fs::write(format!("{out}/latest.md"), &markdown)
        .map_err(|e| AuditFailure::Io(format!("{out}/latest.md: {e}")))?;
    Ok(AuditOutput {
        report: report_j,
        markdown,
        state: state_j,
    })
}
