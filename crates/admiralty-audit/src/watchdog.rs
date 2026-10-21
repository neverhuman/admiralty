//! Watchdog: misses that let workers idle (GB 2026-10-18 09:35 MDT). Every
//! finding here carries `must_act`. Terminal hints never authorize delivery:
//! the texts direct the super_captain to verified owner/endpoint and native
//! queue status, never to type, clear, resume or resend.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::ctx::{Audit, next_since, prev_entry, rj_or_empty, tail_text};
use crate::lanes::SCREEN_ID_RE;
use crate::py::{self, Dt, GpuRows, GpuSampler, J, Obj, PyErr, PyResult, TimeLookup};
use crate::screen::{Screens, screen_state, ssh_argv, tail_hash};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

/// Managed screen idle (not busy) and unchanged this long -> red.
pub const IDLE_RED_MIN: f64 = 20.0;
/// Project with open work and no push to any branch this long -> red.
pub const NO_PUSH_RED_MIN: f64 = 30.0;
/// An ETA in super_captain_status.json this far in the past -> red.
pub const ETA_GRACE_MIN: f64 = 5.0;
/// A 're-drove X' claim needs terminal input on X no earlier than this before the claim.
pub const REDRIVE_GRACE_MIN: i64 = 15;
pub const ETA_LIVE_KEYS: [&str; 7] = [
    "captains",
    "overall",
    "summary",
    "needs_jepson",
    "escalations",
    "issues",
    "actions",
];
/// Every reachable P1 GPU idle this long -> red must-act.
pub const GPU_IDLE_RED_MIN: f64 = 15.0;
pub const GPU_BUSY_UTIL_PCT: i64 = 5;
pub const GPU_BUSY_MEM_MIB: i64 = 1024;

static NOT_WORKER: LazyLock<Regex> =
    LazyLock::new(|| re(r"\A(sc_|disk_|captains_dash|super_captain)"));
static ETA_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(?:ETA|due|by)\s*`?\s*(\d{1,2}):(\d{2})\s*(Z|UTC|AM MDT|PM MDT|MDT)\b`?")
});
static ETA_SKIP_PATH: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)resolved|history|decision|done_log|archive"));
static ETA_CLOSED: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b(MERGED|COMPLETED|CLOSED|CLEARED|DONE)\b"));
static REDRIVE_HDR: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^##\s.*?(\d{4}-\d\d-\d\dT\d\d:\d\d(?::\d\d)?Z)"));
static REDRIVE_WORD: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\bre-?(dr(o|i)ve\w*|nudged|kicked)"));
/// `STATE_COUNT_RE` without its look-ahead (checked by [`state_count_lookahead`]).
static STATE_COUNT_CORE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(\d{1,4})\s*/\s*(\d{1,4})\b"));
static STATE_COUNT_AHEAD: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)[^.;|]{0,30}\b(done|accepted|integrated|complete)"));

/// `re.split(r'(?<=[.;])\s', line)`: split at a whitespace char preceded by `.` or `;`.
pub fn split_sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<char> = None;
    for (i, c) in line.char_indices() {
        if py::py_isspace(c) && matches!(prev, Some('.' | ';')) {
            out.push(&line[start..i]);
            start = i + c.len_utf8();
        }
        prev = Some(c);
    }
    out.push(&line[start..]);
    out
}

/// The look-ahead `(?=[^.;|]{0,30}\b(done|accepted|integrated|complete))` at byte `pos`.
fn state_count_lookahead(text: &str, pos: usize) -> bool {
    STATE_COUNT_AHEAD
        .find_at(text, pos)
        .is_some_and(|m| m.start() == pos)
}

/// `STATE_COUNT_RE.finditer(text)` -> `[(group0, group1, group2)]` with Python scanning semantics.
pub fn state_count_matches(text: &str) -> Vec<(String, i64, i64)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at <= text.len() {
        let Some(m) = STATE_COUNT_CORE.captures_at(text, at) else {
            break;
        };
        let Some(whole) = m.get(0) else { break };
        if state_count_lookahead(text, whole.end()) {
            let g = |i: usize| m.get(i).and_then(|x| py::py_int(x.as_str())).unwrap_or(0);
            out.push((whole.as_str().to_string(), g(1), g(2)));
            at = whole.end().max(whole.start() + 1);
        } else {
            // retry from the next character after the failed start, like a backtracking engine
            let next = text[whole.start()..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
            at = whole.start() + next;
        }
    }
    out
}

/// `managed_screens(scs, lanes_by_proj)`: screen name -> project.
pub fn managed_screens(
    captains: &[(String, String)],
    scs: &J,
    lanes_by_proj: &[(String, Obj)],
) -> PyResult<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let setdefault = |out: &mut Vec<(String, String)>, n: &str, p: &str| {
        if !out.iter().any(|(k, _)| k == n) {
            out.push((n.to_string(), p.to_string()));
        }
    };
    let caps = if scs.truthy() {
        scs.pget("captains")?.clone()
    } else {
        J::Null
    };
    let caps = if caps.truthy() {
        caps
    } else {
        J::Obj(Obj::new())
    };
    let J::Obj(caps) = caps else {
        return Err(PyErr::new(
            "AttributeError",
            format!("'{}' object has no attribute 'items'", caps.type_name()),
        ));
    };
    for (proj, blk) in caps.iter() {
        let dumped = py::dumps(blk, None);
        for m in SCREEN_ID_RE.captures_iter(&dumped) {
            if let Some(n) = m.get(2) {
                setdefault(&mut out, n.as_str(), proj);
            }
        }
    }
    for (proj, lanes) in lanes_by_proj {
        for n in lanes.keys() {
            setdefault(&mut out, n, proj);
        }
    }
    for (proj, name) in captains {
        match out.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = proj.clone(),
            None => out.push((name.clone(), proj.clone())),
        }
    }
    Ok(out
        .into_iter()
        .filter(|(n, _)| !NOT_WORKER.is_match(n))
        .collect())
}

/// One overdue ETA: (scope, minutes late, context).
pub type EtaHit = (String, i64, String);

/// `eta_overdue(scs, now, grace)`.
pub fn eta_overdue(scs: &J, now: &Dt, grace: f64) -> PyResult<Vec<EtaHit>> {
    let mut hits = Vec::new();
    fn walk(
        o: &J,
        path: &mut Vec<String>,
        now: &Dt,
        grace: f64,
        hits: &mut Vec<EtaHit>,
    ) -> PyResult<()> {
        if let Some(first) = path.first()
            && (!ETA_LIVE_KEYS.contains(&first.as_str())
                || path.iter().any(|x| ETA_SKIP_PATH.is_match(x)))
        {
            return Ok(());
        }
        match o {
            J::Obj(map) => {
                for (k, v) in map.iter() {
                    path.push(k.clone());
                    walk(v, path, now, grace, hits)?;
                    path.pop();
                }
            }
            J::Arr(items) => {
                for (i, v) in items.iter().enumerate() {
                    path.push(i.to_string());
                    walk(v, path, now, grace, hits)?;
                    path.pop();
                }
            }
            J::Str(s) => {
                for m in ETA_RE.captures_iter(s) {
                    let (Some(whole), Some(h), Some(mi), Some(z)) =
                        (m.get(0), m.get(1), m.get(2), m.get(3))
                    else {
                        continue;
                    };
                    let mut hh = py::py_int(h.as_str()).unwrap_or(0);
                    let mm = py::py_int(mi.as_str()).unwrap_or(0);
                    let z = z.as_str().to_uppercase();
                    if z.contains("PM") && hh < 12 {
                        hh += 12;
                    }
                    if z.contains("AM") && hh == 12 {
                        hh = 0;
                    }
                    if z.contains("MDT") {
                        hh = (hh + 6).rem_euclid(24);
                    }
                    if hh > 23 || mm > 59 {
                        continue;
                    }
                    let Some(mut t) = py::replace_hm(now, hh as u32, mm as u32) else {
                        continue;
                    };
                    if t.signed_duration_since(*now) > chrono::Duration::hours(12) {
                        t -= chrono::Duration::days(1);
                    }
                    let late = py::total_seconds(now, &t) / 60.0;
                    let start = py::char_index(s, whole.start());
                    let end = py::char_index(s, whole.end());
                    if ETA_CLOSED.is_match(py::char_slice(s, start.saturating_sub(60), start)) {
                        continue;
                    }
                    if late >= grace && late < 18.0 * 60.0 {
                        let first = path.first().ok_or_else(PyErr::index)?;
                        let scope = if path.len() > 1 && first == "captains" {
                            path[1].clone()
                        } else {
                            first.clone()
                        };
                        let ctx =
                            py::char_slice(s, start.saturating_sub(140), end).replace('\n', " ");
                        hits.push((scope, py::py_round(late), ctx));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(scs, &mut Vec::new(), now, grace, &mut hits)?;
    Ok(hits)
}

/// `redrive_claims(log_text, now, window_min)`: `[(claim_utc, screen, snippet)]`.
pub fn redrive_claims(log_text: &str, now: &Dt, window_min: f64) -> Vec<(Dt, String, String)> {
    let mut out = Vec::new();
    let marks: Vec<regex::Captures<'_>> = REDRIVE_HDR.captures_iter(log_text).collect();
    for (i, m) in marks.iter().enumerate() {
        let (Some(whole), Some(ts)) = (m.get(0), m.get(1)) else {
            continue;
        };
        let Some(t) = py::parse_utc(&J::from(ts.as_str())) else {
            continue;
        };
        if py::total_seconds(now, &t) > window_min * 60.0 {
            continue;
        }
        let end = marks
            .get(i + 1)
            .and_then(|n| n.get(0))
            .map_or(log_text.len(), |n| n.start());
        let body = &log_text[whole.end()..end.max(whole.end())];
        for line in body.split('\n') {
            for sent in split_sentences(line) {
                if !REDRIVE_WORD.is_match(sent) {
                    continue;
                }
                for s in SCREEN_ID_RE.captures_iter(sent) {
                    if let Some(n) = s.get(2) {
                        out.push((
                            t,
                            n.as_str().to_string(),
                            py::prefix(py::py_strip(sent), 200).to_string(),
                        ));
                    }
                }
            }
        }
    }
    out
}

impl Audit<'_> {
    /// `idle_watch(scr, managed, prev, newh, now)`.
    pub fn idle_watch(
        &mut self,
        scr: &Screens,
        managed: &[(String, String)],
        prev: &J,
        newh: &mut Obj,
        now: &Dt,
    ) -> PyResult<J> {
        let mut res = Obj::new();
        let mut idle: Vec<(String, Vec<String>)> = Vec::new();
        let mut sorted: Vec<(String, String)> = managed.to_vec();
        sorted.sort();
        for (n, proj) in &sorted {
            if !scr.contains_key(n) {
                continue;
            }
            let hc = self.cfg.watch_hc(n);
            let txt = self.host.hardcopy(n, &hc);
            if txt.is_empty() {
                continue;
            }
            let h = tail_hash(&txt);
            let key = format!("watch:{n}");
            let p = prev_entry(prev, &key)?;
            let since = next_since(&p, &J::from(h.as_str()), &py::iso_seconds(now))?;
            newh.set(
                key,
                crate::jobj! {"hash" => h.as_str(), "since" => since.clone()},
            );
            let unchanged = py::since_age_min(now, &since)?;
            let screen = screen_state(&txt);
            let busy = screen.busy;
            let idle_eligible = !busy && screen.idle_prompt;
            let li = scr.get(n).and_then(|pid| self.pty_last_input(*pid));
            res.set(
                n.clone(),
                crate::jobj! {
                    "project" => proj.as_str(),
                    "busy" => busy,
                    "unchanged_min" => py::py_round(unchanged),
                    "last_input_utc" => li.as_ref().map(py::iso_seconds),
                    "typed_input" => screen.typed_input,
                    "queued_msgs" => screen.queued_msgs,
                    "composer_state" => screen.composer_state,
                    "idle_eligible" => idle_eligible,
                },
            );
            if !busy && unchanged >= IDLE_RED_MIN {
                if idle_eligible {
                    let mut entry = format!("{n} {}m", py::py_round(unchanged));
                    if let Some(li) = &li {
                        entry.push_str(&format!(" (last input {})", py::hhmm_z(li)));
                    }
                    match idle.iter_mut().find(|(k, _)| k == proj) {
                        Some(slot) => slot.1.push(entry),
                        None => idle.push((proj.clone(), vec![entry])),
                    }
                } else if screen.typed_input {
                    let text = format!(
                        "{n} has unsent composer text; preserve it and identify the current owner without typing, clearing, resuming or resending"
                    );
                    self.ma(
                        "amber",
                        proj,
                        "MANAGED_SCREEN_DRAFT_PROTECTED",
                        &text,
                        Some(&hc),
                    );
                } else {
                    let text = format!(
                        "{n} has a queued-message hint or no verified empty composer; inspect owner/endpoint and correlated consumption without typing, clearing, resuming or resending"
                    );
                    self.ma(
                        "amber",
                        proj,
                        "MANAGED_SCREEN_INPUT_UNVERIFIED",
                        &text,
                        Some(&hc),
                    );
                }
            } else if busy && unchanged >= IDLE_RED_MIN {
                let text = format!(
                    "{n} shows busy but screen unchanged {} min (hung tool call?)",
                    py::py_round(unchanged)
                );
                self.ma("amber", proj, "MANAGED_SCREEN_FROZEN", &text, Some(&hc));
            }
        }
        for (proj, lst) in &idle {
            let shown: Vec<&str> = lst.iter().take(12).map(String::as_str).collect();
            let text = format!(
                "{} managed screen(s) idle (no \"esc to cancel/interrupt\") and unchanged >= {} min: {}. \
Must act this tick: inspect the verified exact-thread owner, endpoint and supported queue/priority status; correlate request consumption and action start. \
A notLoaded endpoint is BLOCKED_TRANSPORT: preserve drafts/backlog, never type, clear, resume or duplicate a request. Terminal hints alone authorize no delivery",
                lst.len(),
                IDLE_RED_MIN as i64,
                shown.join("; ")
            );
            self.ma(
                "red",
                proj,
                "MANAGED_SCREEN_IDLE",
                &text,
                Some("/tmp/sc_watch_hc_<screen>.txt"),
            );
        }
        Ok(J::Obj(res))
    }

    /// `check_eta_overdue(scs, now)`.
    pub fn check_eta_overdue(&mut self, scs: &J, now: &Dt) -> PyResult<()> {
        let mut by: Vec<(String, Vec<(String, i64)>)> = Vec::new();
        for (scope, late, ctx) in eta_overdue(scs, now, ETA_GRACE_MIN)? {
            let sc = if self.cfg.is_captain_project(&scope) {
                scope
            } else {
                "super_captain".to_string()
            };
            let idx = match by.iter().position(|(k, _)| *k == sc) {
                Some(i) => i,
                None => {
                    by.push((sc, Vec::new()));
                    by.len() - 1
                }
            };
            let key = py::suffix(&ctx, 110).to_string();
            if !by[idx].1.iter().any(|(c, _)| *c == key) {
                by[idx].1.push((key, late));
            }
        }
        let status = self.cfg.status_json();
        for (sc, items) in by {
            let mut top = items.clone();
            top.sort_by_key(|t| std::cmp::Reverse(t.1));
            let listed: Vec<String> = top
                .iter()
                .take(3)
                .map(|(c, l)| format!("\"...{c}\" ({l}m late)"))
                .collect();
            let text = format!(
                "{} ETA(s) in super_captain_status.json are already past (up to {} min): {}. Must act this tick: verify each deliverable and inspect verified owner/endpoint and supported exact-thread queue/priority status; record actual consumption/start or BLOCKED_TRANSPORT and a new checkpoint. Preserve drafts/backlog; no typing, clearing, resume or duplicate request",
                top.len(),
                top.first().map_or(0, |t| t.1),
                listed.join(" | ")
            );
            self.ma("red", &sc, "OWED_ETA_OVERDUE", &text, Some(&status));
        }
        Ok(())
    }

    /// `check_redrive_delivered(log_text, scr, now, last_input)`.
    pub fn check_redrive_delivered(
        &mut self,
        log_text: &str,
        scr: &Screens,
        now: &Dt,
        last_input: Option<TimeLookup<'_>>,
    ) {
        let mut done: Vec<String> = Vec::new();
        for (t, n, _snip) in redrive_claims(log_text, now, 60.0) {
            if done.contains(&n) || !scr.contains_key(&n) {
                continue;
            }
            let li = match last_input {
                Some(f) => f(&n),
                None => scr.get(&n).and_then(|pid| self.pty_last_input(*pid)),
            };
            let Some(li) = li else { continue };
            if li < t - chrono::Duration::minutes(REDRIVE_GRACE_MIN) {
                done.push(n.clone());
                let text = format!(
                    "super_captain_LOG {} claims delivery to {n}, but the last recorded terminal read is {}; \
this does not prove native queue submission, recipient consumption or action start. Inspect verified exact-thread owner/endpoint and supported queue/priority status, preserving payload and draft. \
notLoaded means BLOCKED_TRANSPORT; do not type, clear, resume, resend or create a duplicate",
                    py::hhmm_z(&t),
                    py::hhmm_z(&li)
                );
                self.ma("red", "super_captain", "REDRIVE_NOT_DELIVERED", &text, None);
            }
        }
    }

    /// `check_no_push(report, now, push_fn)`.
    pub fn check_no_push(
        &mut self,
        report: &mut Obj,
        now: &Dt,
        push_fn: Option<TimeLookup<'_>>,
    ) -> PyResult<()> {
        let names: Vec<String> = match report.get("projects") {
            Some(J::Obj(p)) => p.keys().cloned().collect(),
            _ => Vec::new(),
        };
        for proj in names {
            let r = match report.get("projects") {
                Some(J::Obj(p)) => p.get(&proj).cloned().unwrap_or_default(),
                _ => J::Null,
            };
            let done = r.pget("done_recomputed")?.clone();
            let total = r.pget("total")?.clone();
            let complete =
                matches!((done.as_inst_int(), total.as_inst_int()), (Some(d), Some(t)) if d >= t);
            let open_work = !complete || r.pget("open_prs")?.truthy();
            let Some(gh_repo) = self.cfg.gh_repo(&proj) else {
                continue;
            };
            if !open_work {
                continue;
            }
            let t = match push_fn {
                Some(f) => f(&proj),
                None => self.last_push(&proj)?,
            };
            let Some(t) = t else { continue };
            let age = py::total_seconds(now, &t) / 60.0;
            if let Some(J::Obj(projects)) = report.get_mut("projects")
                && let Some(J::Obj(ro)) = projects.get_mut(&proj)
            {
                ro.set("last_push_utc", py::iso_seconds(&t));
            }
            if age >= NO_PUSH_RED_MIN {
                let text = format!(
                    "{gh_repo} has open work ({}/{}) but nothing pushed to any branch for {} min (last {}). \
Must act this tick: find which worker owes a push and why it has not pushed; every worker pushes its branch every 30 min",
                    py::py_str(&done),
                    py::py_str(&total),
                    py::py_round(age),
                    py::hhmm_z(&t)
                );
                self.ma("red", &proj, "NO_PUSH_30M", &text, None);
            }
        }
        Ok(())
    }

    /// `check_sc_claims(report, scs, watch)`.
    pub fn check_sc_claims(&mut self, report: &Obj, scs: &J, watch: &J) -> PyResult<()> {
        let caps = if scs.truthy() {
            scs.pget("captains")?.clone()
        } else {
            J::Null
        };
        let caps = if caps.truthy() {
            caps
        } else {
            J::Obj(Obj::new())
        };
        let status = self.cfg.status_json();
        let Some(J::Obj(projects)) = report.get("projects") else {
            return Ok(());
        };
        for (proj, r) in projects.iter() {
            let blk = caps.pget(proj)?;
            let blk = if blk.truthy() {
                blk.clone()
            } else {
                J::Obj(Obj::new())
            };
            let done = r.pget("done_recomputed")?;
            let total = r.pget("total")?;
            let state_j = blk.pget("state")?;
            let state = if state_j.truthy() {
                py::py_str(state_j)
            } else {
                String::new()
            };
            if let (Some(d), Some(t)) = (done.as_inst_int(), total.as_inst_int()) {
                let packets = r.pget("packets_accepted")?;
                let mut bad = Vec::new();
                for (whole, a, b) in state_count_matches(&state) {
                    if b == t && a > d {
                        let within_packets = match packets {
                            J::Null => false,
                            other => py::py_cmp(&J::Int(a), other)? != std::cmp::Ordering::Greater,
                        };
                        if !within_packets {
                            bad.push(whole);
                        }
                    }
                }
                if let Some(first) = bad.first() {
                    let text = format!(
                        "super_captain_status.json captains.{proj}.state still says \"{first} ...\" while the audit gives {}/{}. The headline text must use the audit number",
                        py::py_str(done),
                        py::py_str(total)
                    );
                    self.ma("red", proj, "SC_STATE_TEXT_OVERCLAIM", &text, Some(&status));
                }
            }
            let mut idle = Vec::new();
            if let J::Obj(w) = watch {
                for (n, info) in w.iter() {
                    let same = py::eq_str(info.pidx("project")?, proj);
                    if same && !info.pidx("busy")?.truthy() {
                        let um = info.pidx("unchanged_min")?;
                        if um.as_f64().is_some_and(|v| v >= IDLE_RED_MIN) {
                            idle.push(n.clone());
                        }
                    }
                }
            }
            idle.sort();
            if !idle.is_empty() && py::eq_int(blk.pget("workers_idle")?, 0) {
                let text = format!(
                    "super_captain_status.json says workers_idle=0 for {proj} but {} idle >= {} min. Count idle workers from this audit, not from memory",
                    idle.join(", "),
                    IDLE_RED_MIN as i64
                );
                self.ma("red", proj, "SC_IDLE_CLAIM_FALSE", &text, Some(&status));
            }
        }
        Ok(())
    }

    /// `gpu_sample(host)`: `[(util_pct, mem_mib)]` or `None`. Never xbabe0.
    pub fn gpu_sample(&self, host: &str) -> Option<GpuRows> {
        if host == self.cfg.forbidden_host {
            return None;
        }
        let q = "nvidia-smi --query-gpu=utilization.gpu,memory.used --format=csv,noheader,nounits";
        let login = format!("{}@{host}", self.cfg.ssh_user);
        let local: Vec<&str> = q.split(' ').collect();
        let remote = ssh_argv(q, &login);
        let argv: &[&str] = if host == self.cfg.local_host {
            &local
        } else {
            &remote
        };
        let out = self.host.run(argv, Some(Duration::from_secs(25)), None);
        if out.rc != 0 {
            return None;
        }
        let mut rows = Vec::new();
        for l in py::py_splitlines(py::py_strip(&out.stdout), false) {
            let parts: Vec<&str> = l.split(',').take(2).collect();
            if parts.len() != 2 {
                continue;
            }
            let (Some(u), Some(m)) = (py::py_int_of_float(parts[0]), py::py_int_of_float(parts[1]))
            else {
                continue;
            };
            rows.push((u, m));
        }
        if rows.is_empty() { None } else { Some(rows) }
    }

    /// `check_gpu_idle(report, prev, newh, now, sample_fn)`.
    pub fn check_gpu_idle(
        &mut self,
        report: &mut Obj,
        prev: &J,
        newh: &mut Obj,
        now: &Dt,
        sample_fn: Option<GpuSampler<'_>>,
    ) -> PyResult<()> {
        let hosts = self.cfg.gpu_hosts.clone();
        let samples: Vec<(String, Option<GpuRows>)> = hosts
            .iter()
            .map(|h| {
                (
                    h.clone(),
                    match sample_fn {
                        Some(f) => f(h),
                        None => self.gpu_sample(h),
                    },
                )
            })
            .collect();
        let gpus: Obj = samples
            .iter()
            .map(|(h, r)| {
                let v = match r {
                    Some(rows) => J::Arr(
                        rows.iter()
                            .map(|(u, m)| crate::jobj! {"util_pct" => *u, "mem_mib" => *m})
                            .collect(),
                    ),
                    None => J::Null,
                };
                (h.clone(), v)
            })
            .collect();
        report.set("gpus", J::Obj(gpus));
        let reached: Vec<(&String, &Vec<(i64, i64)>)> = samples
            .iter()
            .filter_map(|(h, r)| r.as_ref().filter(|v| !v.is_empty()).map(|v| (h, v)))
            .collect();
        if reached.is_empty() {
            let text = format!(
                "could not read nvidia-smi on any of {}; P1 GPU usage unverified",
                hosts.join(", ")
            );
            self.f("amber", "jopedime", "GPU_UNREACHABLE", &text, None);
            return Ok(());
        }
        if reached.iter().any(|(_, r)| {
            r.iter()
                .any(|(u, m)| *u >= GPU_BUSY_UTIL_PCT || *m >= GPU_BUSY_MEM_MIB)
        }) {
            return Ok(());
        }
        let key = "gpu:p1_all_idle";
        let p = prev_entry(prev, key)?;
        let since = p.pget("since")?;
        let since = if since.truthy() {
            since.clone()
        } else {
            J::from(py::iso_seconds(now))
        };
        newh.set(
            key,
            crate::jobj! {"hash" => "idle", "since" => since.clone()},
        );
        let age = py::since_age_min(now, &since)?;
        report.set("gpus_idle_min", py::py_round(age));
        if age >= GPU_IDLE_RED_MIN {
            let detail: Vec<String> = reached
                .iter()
                .map(|(h, r)| {
                    format!(
                        "{h} {}",
                        r.iter()
                            .map(|(u, m)| format!("{u}%/{m}MiB"))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect();
            let names: Vec<&str> = reached.iter().map(|(h, _)| h.as_str()).collect();
            let text = format!(
                "every reachable P1 GPU ({}) has been idle (<{GPU_BUSY_UTIL_PCT}% util and <{GPU_BUSY_MEM_MIB} MiB) for {} min: {}. \
Jepson P1 is JopeDime GPU testing on xbabe1/2/3. Must act this tick: start an S3-backed JopeDime GPU test or benchmark on at least one host now \
(do not wait for a later queue item or ETA), confirm load in a fresh nvidia-smi, and record it in super_captain_status.json gpu_status. \
An ETA alone does not clear this",
                names.join(", "),
                py::py_round(age),
                detail.join("; ")
            );
            let evidence = format!("nvidia-smi on {}", names.join(", "));
            self.ma("red", "jopedime", "P1_GPU_IDLE", &text, Some(&evidence));
        }
        Ok(())
    }

    /// `watchdog(report, scr, prev, newh, now, no_gh)`.
    pub fn watchdog(
        &mut self,
        report: &mut Obj,
        scr: &Screens,
        prev: &J,
        newh: &mut Obj,
        now: &Dt,
        no_gh: bool,
    ) -> PyResult<()> {
        let scs = rj_or_empty(&self.cfg.status_json());
        let names: Vec<String> = self.cfg.projects.iter().map(|p| p.name.clone()).collect();
        let lanes: Vec<(String, Obj)> = names
            .iter()
            .map(|p| (p.clone(), self.lane_screens_any_format(p)))
            .collect();
        let captains: Vec<(String, String)> = self
            .cfg
            .projects
            .iter()
            .map(|p| (p.name.clone(), p.captain.clone()))
            .collect();
        let managed = managed_screens(&captains, &scs, &lanes)?;
        let watch = self.idle_watch(scr, &managed, prev, newh, now)?;
        report.set("watch", watch.clone());
        self.check_eta_overdue(&scs, now)?;
        self.jepson_bottleneck(report, &scs, no_gh)?;
        let log = tail_text(&format!("{}/super_captain_LOG.md", self.cfg.kick), 300_000);
        self.check_redrive_delivered(&log, scr, now, None);
        if !no_gh {
            self.check_no_push(report, now, None)?;
        }
        self.check_sc_claims(report, &scs, &watch)?;
        if !no_gh {
            self.check_gpu_idle(report, prev, newh, now, None)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_split_matches_python_lookbehind() {
        assert_eq!(split_sentences("a. b; c d"), vec!["a.", "b;", "c d"]);
        assert_eq!(split_sentences("x.y z"), vec!["x.y z"]);
        assert_eq!(split_sentences("end.  two"), vec!["end.", " two"]);
        assert_eq!(split_sentences(""), vec![""]);
    }

    #[test]
    fn state_count_lookahead_semantics() {
        let m = state_count_matches("P1-A TOP PRIORITY — 66/66 done on main");
        assert_eq!(m, vec![("66/66".to_string(), 66, 66)]);
        assert!(state_count_matches("66/66. done").is_empty());
        assert!(state_count_matches("66/66 | done").is_empty());
        // a failed candidate does not hide one that starts inside it
        let text = format!("1/2/3{}done", " ".repeat(29));
        let got = state_count_matches(&text);
        assert_eq!(got, vec![("2/3".to_string(), 2, 3)]);
        assert_eq!(state_count_matches("12345/6 done"), vec![]);
        assert_eq!(
            state_count_matches("5 / 37 accepted"),
            vec![("5 / 37".to_string(), 5, 37)]
        );
    }
}
