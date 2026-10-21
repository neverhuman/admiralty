//! Joil v5: ledger recomputation (`audit_joil`) and the status-flip audit
//! (GB red-team 2026-10-18, Joil PR #9 85->155).
//!
//! A Joil task that sat blocked on external causes (Cause B measured data /
//! Cause E owner authority) must not become "done" merely because a Fleet
//! Admiral decision unblocked it. Flags, read-only on origin/main:
//! - `JOIL_DONE_STATUS_FLIP` (red): turned done in a first-parent commit that
//!   added < [`JOIL_FLIP_MIN_PROD_LINES`] production lines per newly-done task,
//!   after an external-cause block; waived only by [`joil_flip_waived`].
//! - `JOIL_DONE_DECISION_SAYS_OPEN` (red): the disposition says verification remains open.
//! - `JOIL_SUBSTITUTE_UNLABELLED` (red): substitute:/rescope: disposition without a label.
//! - `JOIL_DONE_SHARED_RECEIPT` (amber): only packet-wide aggregate receipts.
//! - `JOIL_LOG_SUMMARY_ONLY` (amber): every test log is a one-line summary.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::iter_or_empty;
use crate::ctx::{Audit, rj};
use crate::py::{self, J, Obj, PyErr, PyResult};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

pub const JOIL_FLIP_MIN_PROD_LINES: f64 = 10.0;
static JOIL_EXTERNAL_CAUSE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)Cause [BE]\b|measured|metrology|unsigned|signed|owner|authority|physical|accession|SC-DEC-\d+|Fleet Admiral|unblocked by",
    )
});
static JOIL_OPEN_RX: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)remains? open|not yet executed|not executed|verification remains"));
static SUBSTITUTE: LazyLock<Regex> = LazyLock::new(|| re(r"substitute:|rescope:"));
static HUNK: LazyLock<Regex> = LazyLock::new(|| re(r"\+(\d+)(?:,(\d+))?"));
static V5_ID: LazyLock<Regex> = LazyLock::new(|| re(r"V5-\d\d-\d\d"));
static TEST_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^test \S+ \.\.\. "));
static UNITTEST_LINE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^(test_\w+|\w+ \(\S+\)) \.\.\. "));
static AUDIT_HELD: LazyLock<Regex> = LazyLock::new(|| re(r"(V5-\d\d-\d\d)[^\n]{0,60}AUDIT-HELD"));
pub const JOIL_PROGRESS_DIR: &str = "docs/v5-plan/progress";

/// `joil_is_prod_path(p)`.
pub fn joil_is_prod_path(p: &str) -> bool {
    if ![".rs", ".py", ".ts", ".tsx", ".js"]
        .iter()
        .any(|e| p.ends_with(e))
    {
        return false;
    }
    let base = p.rsplit('/').next().unwrap_or(p);
    if ["docs/", "tests/", "experiments/"]
        .iter()
        .any(|s| p.starts_with(s))
        || p.contains("/tests/")
    {
        return false;
    }
    if base.starts_with("test_")
        || ["_test.py", ".test.ts", ".test.tsx", ".spec.ts"]
            .iter()
            .any(|e| base.ends_with(e))
    {
        return false;
    }
    true
}

/// `_joil_log_is_summary(path)`: `None` when unreadable.
pub fn joil_log_is_summary(path: &J) -> Option<bool> {
    let J::Str(p) = path else { return None };
    let bytes = std::fs::read(p).ok()?;
    let txt = py::universal_newlines(&String::from_utf8_lossy(&bytes));
    Some(!TEST_LINE.is_match(&txt) && !UNITTEST_LINE.is_match(&txt))
}

/// `cmd.get('command') or ' '.join(cmd.get('argv') or [])`.
fn command_text(cmd: &J) -> PyResult<J> {
    let c = cmd.pget("command")?;
    if c.truthy() {
        return Ok(c.clone());
    }
    let argv = cmd.pget("argv")?;
    let argv = if argv.truthy() {
        argv.clone()
    } else {
        J::Arr(Vec::new())
    };
    Ok(J::Str(py::py_join(" ", &argv)?))
}

fn is_test_command(s: &J) -> PyResult<bool> {
    Ok(py::py_contains(s, &J::from("cargo test"))?
        || py::py_contains(s, &J::from("unittest"))?
        || py::py_contains(s, &J::from("pytest"))?)
}

fn task_ids_len(d: &J) -> PyResult<usize> {
    d.pget_or("task_ids", &J::Arr(Vec::new()))?.py_len()
}

/// `joil_flip_waived(mine)`.
pub fn joil_flip_waived(mine: &[(String, J)]) -> PyResult<bool> {
    for (_, d) in mine {
        let res = d.pget("decision_resolution")?;
        let res_s = if res.truthy() {
            py::py_str(res)
        } else {
            String::new()
        };
        if task_ids_len(d)? > 2 || py::py_strip(&res_s).is_empty() {
            continue;
        }
        for cmd in iter_or_empty(d.pget("commands")?)? {
            let s = command_text(&cmd)?;
            if is_test_command(&s)? && py::eq_int(cmd.pget("exit_code")?, 0) {
                let lp = cmd.pget("log_path")?;
                let lp = if lp.truthy() { lp.clone() } else { J::from("") };
                if joil_log_is_summary(&lp) == Some(false) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// One done transition.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub commit: String,
    pub parent: Option<String>,
    pub prev_status: J,
    pub prev_note: J,
    pub blocked_note: J,
}

/// `st_tasks.get(tid)` with Python dict semantics (JSON keys are strings).
fn lookup_task(st_tasks: &J, tid: &J) -> PyResult<J> {
    if !matches!(st_tasks, J::Obj(_)) {
        return Err(PyErr::attr_get(st_tasks));
    }
    if matches!(tid, J::Arr(_) | J::Obj(_)) {
        return Err(PyErr::type_error(format!(
            "unhashable type: '{}'",
            tid.type_name()
        )));
    }
    match tid {
        J::Str(k) => Ok(st_tasks.pget(k)?.clone()),
        _ => Ok(J::Null),
    }
}

fn or_empty_obj(v: &J) -> J {
    if v.truthy() {
        v.clone()
    } else {
        J::Obj(Obj::new())
    }
}

fn note_or_empty(v: &J) -> PyResult<J> {
    let n = or_empty_obj(v).pget("note")?.clone();
    Ok(if n.truthy() { n } else { J::from("") })
}

impl Audit<'_> {
    /// `joil_prod_added_lines(repo, base, head)` -> (total, per-file).
    pub fn joil_prod_added_lines(
        &self,
        repo: &str,
        base: &str,
        head: &str,
    ) -> (i64, BTreeMap<String, i64>) {
        let out = self.run(
            &["git", "-C", repo, "diff", "-U0", "--no-color", base, head],
            90,
        );
        let mut total = 0i64;
        let mut per: BTreeMap<String, i64> = BTreeMap::new();
        let mut cur: Option<String> = None;
        let mut cut: Option<i64> = None;
        for line in py::py_splitlines(&out.stdout, false) {
            if line.starts_with("+++ ") {
                let path = line.strip_prefix("+++ b/");
                cur = path
                    .filter(|p| !p.is_empty() && joil_is_prod_path(p))
                    .map(str::to_string);
                cut = None;
                if let Some(c) = cur.as_deref().filter(|c| c.ends_with(".rs")) {
                    let spec = format!("{head}:{c}");
                    let src = self.run(&["git", "-C", repo, "show", &spec], 15);
                    for (i, l) in py::py_splitlines(&src.stdout, false).iter().enumerate() {
                        if py::py_strip(l) == "#[cfg(test)]" {
                            cut = Some(i as i64 + 1);
                            break;
                        }
                    }
                }
                continue;
            }
            if let Some(c) = &cur
                && line.starts_with("@@")
            {
                let Some(m) = HUNK.captures(line) else {
                    continue;
                };
                let start = m.get(1).and_then(|g| py::py_int(g.as_str())).unwrap_or(0);
                let mut n = m
                    .get(2)
                    .map_or(Some(1), |g| py::py_int(g.as_str()))
                    .unwrap_or(1);
                if let Some(cut) = cut.filter(|c| *c != 0) {
                    n = 0.max(n.min(cut - start));
                }
                if n != 0 {
                    *per.entry(c.clone()).or_insert(0) += n;
                    total += n;
                }
            }
        }
        (total, per)
    }

    /// `_joil_progress_at(repo, rev, files)`.
    pub fn joil_progress_at(&self, repo: &str, rev: &str, files: Option<&[String]>) -> Obj {
        let owned;
        let files: &[String] = match files {
            Some(f) => f,
            None => {
                let dir = format!("{JOIL_PROGRESS_DIR}/");
                let out = self.run(
                    &["git", "-C", repo, "ls-tree", "--name-only", rev, &dir],
                    15,
                );
                owned = if out.rc == 0 {
                    py::py_split_ws(&out.stdout)
                        .into_iter()
                        .map(str::to_string)
                        .collect()
                } else {
                    Vec::new()
                };
                &owned
            }
        };
        let mut tasks = Obj::new();
        for f in files {
            let spec = format!("{rev}:{f}");
            let js = self.run(&["git", "-C", repo, "show", &spec], 15);
            let Some(doc) = py::loads(&js.stdout) else {
                continue;
            };
            let Ok(t) = doc.pget("tasks") else { continue };
            match t {
                J::Obj(o) => {
                    for (k, v) in o.iter() {
                        tasks.set(k.clone(), v.clone());
                    }
                }
                other if !other.truthy() => {}
                // dict.update() of a non-mapping raises and is swallowed by `except Exception: pass`
                _ => {}
            }
        }
        tasks
    }

    /// `joil_done_transitions(repo, ref, max_commits)`.
    pub fn joil_done_transitions(
        &self,
        repo: &str,
        reference: &str,
        max_commits: usize,
    ) -> PyResult<Vec<(String, Transition)>> {
        let n = max_commits.to_string();
        let out = self.run(
            &[
                "git",
                "-C",
                repo,
                "log",
                "--first-parent",
                "--format=%H",
                "-n",
                &n,
                reference,
                "--",
                JOIL_PROGRESS_DIR,
            ],
            30,
        );
        let mut commits: Vec<String> = if out.rc == 0 {
            py::py_split_ws(&out.stdout)
                .into_iter()
                .map(str::to_string)
                .collect()
        } else {
            Vec::new()
        };
        commits.reverse();
        let mut trans: Vec<(String, Transition)> = Vec::new();
        let mut blocked_note: BTreeMap<String, J> = BTreeMap::new();
        for c in &commits {
            let parent_spec = format!("{c}^1");
            let pr = self.run(&["git", "-C", repo, "rev-parse", &parent_spec], 10);
            let par = if pr.rc == 0 {
                Some(py::py_strip(&pr.stdout).to_string())
            } else {
                None
            };
            let (before, after) = match par.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => {
                    let ch = self.run(
                        &[
                            "git",
                            "-C",
                            repo,
                            "diff",
                            "--name-only",
                            p,
                            c,
                            "--",
                            JOIL_PROGRESS_DIR,
                        ],
                        15,
                    );
                    let files: Vec<String> = py::py_split_ws(&ch.stdout)
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                    (
                        self.joil_progress_at(repo, p, Some(&files)),
                        self.joil_progress_at(repo, c, Some(&files)),
                    )
                }
                None => (Obj::new(), self.joil_progress_at(repo, c, None)),
            };
            for (k, v) in before.iter().chain(after.iter()) {
                let v = or_empty_obj(v);
                if py::eq_str(v.pget("status")?, "blocked") {
                    blocked_note.insert(k.clone(), note_or_empty(&v)?);
                }
            }
            for (k, v) in after.iter() {
                let prev_entry = or_empty_obj(before.get(k).unwrap_or(&J::Null));
                let prev = prev_entry.pget("status")?.clone();
                if py::eq_str(or_empty_obj(v).pget("status")?, "done") && !py::eq_str(&prev, "done")
                {
                    let t = Transition {
                        commit: c.clone(),
                        parent: par.clone(),
                        prev_status: prev,
                        prev_note: note_or_empty(&prev_entry)?,
                        blocked_note: J::from(""),
                    };
                    match trans.iter_mut().find(|(key, _)| key == k) {
                        Some(slot) => slot.1 = t,
                        None => trans.push((k.clone(), t)),
                    }
                }
            }
        }
        for (k, t) in &mut trans {
            t.blocked_note = blocked_note.get(k).cloned().unwrap_or_else(|| J::from(""));
        }
        Ok(trans)
    }

    fn joil_receipts_at(&self, repo: &str, reference: &str) -> Vec<(String, J)> {
        let out = self.run(
            &[
                "git",
                "-C",
                repo,
                "ls-tree",
                "-r",
                "--name-only",
                reference,
                "docs/receipts",
            ],
            15,
        );
        let mut recs = Vec::new();
        if out.rc != 0 {
            return recs;
        }
        for f in py::py_split_ws(&out.stdout) {
            if !f.ends_with(".json") {
                continue;
            }
            let spec = format!("{reference}:{f}");
            let js = self.run(&["git", "-C", repo, "show", &spec], 15);
            if let Some(d @ J::Obj(_)) = py::loads(&js.stdout)
                && d.pget("task_ids").is_ok_and(J::truthy)
            {
                recs.push((f.to_string(), d));
            }
        }
        recs
    }

    fn joil_dispositions_at(&self, repo: &str, reference: &str) -> PyResult<Vec<(String, Vec<J>)>> {
        let out = self.run(
            &[
                "git",
                "-C",
                repo,
                "ls-tree",
                "-r",
                "--name-only",
                reference,
                "docs/release",
            ],
            15,
        );
        let mut by_task: Vec<(String, Vec<J>)> = Vec::new();
        if out.rc != 0 {
            return Ok(by_task);
        }
        for f in py::py_split_ws(&out.stdout) {
            if !f.to_uppercase().contains("DISPOSITIONS") || !f.ends_with(".json") {
                continue;
            }
            let spec = format!("{reference}:{f}");
            let js = self.run(&["git", "-C", repo, "show", &spec], 15);
            let rows = match py::loads(&js.stdout).map(|d| d.pget("decisions").cloned()) {
                Some(Ok(r)) if r.truthy() => r,
                _ => J::Arr(Vec::new()),
            };
            for r in rows.py_iter()? {
                let tid = py::py_str(r.pget_or("task_id", &J::from(""))?);
                let ids = r.pget("task_ids")?;
                let ids = if ids.truthy() {
                    ids.clone()
                } else {
                    J::Arr(Vec::new())
                };
                let hay = format!("{tid} {}", py::py_join(" ", &ids)?);
                for m in V5_ID.find_iter(&hay) {
                    let t = m.as_str().to_string();
                    match by_task.iter_mut().find(|(k, _)| *k == t) {
                        Some(slot) => slot.1.push(r.clone()),
                        None => by_task.push((t, vec![r.clone()])),
                    }
                }
            }
        }
        Ok(by_task)
    }

    /// `audit_joil_flips(repo, ref, emit)`.
    pub fn audit_joil_flips(&mut self, repo: &str, reference: &str, emit: bool) -> PyResult<J> {
        let progress = self.joil_progress_at(repo, reference, None);
        let mut done: Vec<String> = Vec::new();
        for (k, v) in progress.iter() {
            if py::eq_str(or_empty_obj(v).pget("status")?, "done") {
                done.push(k.clone());
            }
        }
        done.sort();
        let trans = self.joil_done_transitions(repo, reference, 200)?;
        let recs = self.joil_receipts_at(repo, reference);
        let disp = self.joil_dispositions_at(repo, reference)?;
        let get_t = |k: &str| trans.iter().find(|(key, _)| key == k).map(|(_, t)| t);
        let mut flags: Vec<(String, Vec<String>)> =
            done.iter().map(|k| (k.clone(), Vec::new())).collect();
        let mut by_commit: Vec<(String, Vec<String>)> = Vec::new();
        for k in &done {
            if let Some(t) = get_t(k) {
                match by_commit.iter_mut().find(|(c, _)| *c == t.commit) {
                    Some(slot) => slot.1.push(k.clone()),
                    None => by_commit.push((t.commit.clone(), vec![k.clone()])),
                }
            }
        }
        let mut prod_cache: BTreeMap<String, i64> = BTreeMap::new();
        for (c, ks) in &by_commit {
            let par = ks
                .first()
                .and_then(|k| get_t(k))
                .and_then(|t| t.parent.clone())
                .filter(|p| !p.is_empty());
            let v = match par {
                Some(p) => self.joil_prod_added_lines(repo, &p, c).0,
                None => 1_000_000_000,
            };
            prod_cache.insert(c.clone(), v);
        }
        for (k, fl) in &mut flags {
            let mut mine: Vec<(String, J)> = Vec::new();
            for (f, d) in &recs {
                if py::py_contains(
                    d.pget_or("task_ids", &J::Arr(Vec::new()))?,
                    &J::Str(k.clone()),
                )? {
                    mine.push((f.clone(), d.clone()));
                }
            }
            if let Some(t) = get_t(k)
                && t.parent.as_deref().is_some_and(|p| !p.is_empty())
            {
                let bn = if t.blocked_note.truthy() {
                    t.blocked_note.clone()
                } else {
                    J::from("")
                };
                let pn = if t.prev_note.truthy() {
                    t.prev_note.clone()
                } else {
                    J::from("")
                };
                let left = py::py_add_str(&bn, " ")?;
                let J::Str(pn) = &pn else {
                    return Err(PyErr::type_error(format!(
                        "can only concatenate str (not \"{}\") to str",
                        pn.type_name()
                    )));
                };
                let joined = format!("{left}{pn}");
                if JOIL_EXTERNAL_CAUSE.is_match(&joined) {
                    let n = by_commit
                        .iter()
                        .find(|(c, _)| *c == t.commit)
                        .map_or(1, |(_, ks)| ks.len().max(1));
                    let per_task = *prod_cache.get(&t.commit).unwrap_or(&0) as f64 / n as f64;
                    if per_task < JOIL_FLIP_MIN_PROD_LINES && !joil_flip_waived(&mine)? {
                        fl.push("JOIL_DONE_STATUS_FLIP".to_string());
                    }
                }
            }
            let entry = py::dumps(&or_empty_obj(progress.get(k).unwrap_or(&J::Null)), None);
            if let Some((_, rows)) = disp.iter().find(|(t, _)| t == k) {
                for r in rows {
                    let mode_j = {
                        let a = r.pget("disposition_mode")?;
                        if a.truthy() {
                            a.clone()
                        } else {
                            r.pget("disposition")?.clone()
                        }
                    };
                    let mode = if mode_j.truthy() {
                        py::py_str(&mode_j)
                    } else {
                        String::new()
                    };
                    let summary = format!(
                        "{} {}",
                        py::py_str(r.pget_or("summary", &J::from(""))?),
                        py::py_str(r.pget_or("rationale", &J::from(""))?)
                    );
                    if JOIL_OPEN_RX.is_match(&summary) {
                        fl.push("JOIL_DONE_DECISION_SAYS_OPEN".to_string());
                    }
                    if SUBSTITUTE.is_match(&mode) && !SUBSTITUTE.is_match(&entry) {
                        fl.push("JOIL_SUBSTITUTE_UNLABELLED".to_string());
                    }
                }
            }
            if !mine.is_empty() {
                let mut any_small = false;
                for (_, d) in &mine {
                    if task_ids_len(d)? <= 2 {
                        any_small = true;
                        break;
                    }
                }
                if !any_small {
                    fl.push("JOIL_DONE_SHARED_RECEIPT".to_string());
                }
            }
            let mut logs: Vec<Option<bool>> = Vec::new();
            for (_, d) in &mine {
                for cmd in iter_or_empty(d.pget("commands")?)? {
                    let s = command_text(&cmd)?;
                    if is_test_command(&s)? && cmd.pget("log_path")?.truthy() {
                        logs.push(joil_log_is_summary(cmd.pidx("log_path")?));
                    }
                }
            }
            let known: Vec<bool> = logs.into_iter().flatten().collect();
            if !known.is_empty() && known.iter().all(|x| *x) {
                fl.push("JOIL_LOG_SUMMARY_ONLY".to_string());
            }
            *fl = py::sorted_set(fl.drain(..));
        }
        let red_codes = [
            "JOIL_DONE_STATUS_FLIP",
            "JOIL_DONE_DECISION_SAYS_OPEN",
            "JOIL_SUBSTITUTE_UNLABELLED",
        ];
        let mut red: Vec<String> = flags
            .iter()
            .filter(|(_, v)| v.iter().any(|c| red_codes.contains(&c.as_str())))
            .map(|(k, _)| k.clone())
            .collect();
        red.sort();
        if emit {
            let mut agg: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (k, v) in &flags {
                for code in v {
                    agg.entry(code.clone()).or_default().push(k.clone());
                }
            }
            let evidence = format!("{repo} {reference}");
            for (code, ks) in &agg {
                let sev = if red_codes.contains(&code.as_str()) {
                    "red"
                } else {
                    "amber"
                };
                let mut sorted = ks.clone();
                sorted.sort();
                let shown: Vec<&str> = sorted.iter().take(40).map(String::as_str).collect();
                let text = format!(
                    "{} done-on-main task(s): {}{}",
                    ks.len(),
                    shown.join(" "),
                    if ks.len() > 40 { " ..." } else { "" }
                );
                self.f(sev, "joil", code, &text, Some(&evidence));
            }
        }
        let flagged: Obj = flags
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.clone(), J::from(v.clone())))
            .collect();
        Ok(crate::jobj! {
            "done_on_main" => done.len(),
            "done_red_flagged" => red.len(),
            "done_unflagged_on_main" => done.len() - red.len(),
            "red_flagged_tasks" => red,
            "flags" => flagged,
        })
    }

    /// `audit_joil()`.
    pub fn audit_joil(&mut self) -> J {
        let mut res = Obj::new();
        if let Err(e) = self.audit_joil_inner(&mut res) {
            let text = py::prefix(&e.msg, 200).to_string();
            self.f("amber", "joil", "LEDGER_UNREADABLE", &text, None);
        }
        J::Obj(res)
    }

    fn audit_joil_inner(&mut self, res: &mut Obj) -> PyResult<()> {
        let wt = self.cfg.joil_wt.clone();
        let tasks = rj(&format!("{wt}/docs/v5-plan/TASKS.json")).unwrap_or(J::Null);
        let packets = tasks.pidx("packets")?.py_iter()?;
        let progress_of = |pk: &J| -> PyResult<J> {
            let id = pk.pidx("id")?;
            let J::Str(id) = id else {
                return Err(PyErr::type_error(format!(
                    "can only concatenate str (not \"{}\") to str",
                    id.type_name()
                )));
            };
            Ok(match rj(&format!("{wt}/docs/v5-plan/progress/{id}.json")) {
                Some(j) if j.truthy() => j,
                _ => crate::jobj! {"tasks" => J::Obj(Obj::new())},
            })
        };
        // sts: task id -> (status, evidence), keyed with Python dict semantics on the JSON id
        let mut sts: Vec<(J, (J, J))> = Vec::new();
        for pk in &packets {
            let st = progress_of(pk)?;
            for t in pk.pidx("tasks")?.py_iter()? {
                let tid = t.pidx("id")?.clone();
                if matches!(tid, J::Arr(_) | J::Obj(_)) {
                    return Err(PyErr::type_error(format!(
                        "unhashable type: '{}'",
                        tid.type_name()
                    )));
                }
                let entry = or_empty_obj(&lookup_task(st.pidx("tasks")?, &tid)?);
                let val = (
                    entry.pget("status")?.clone(),
                    entry.pget("evidence")?.clone(),
                );
                match sts.iter_mut().find(|(k, _)| py::py_eq(k, &tid)) {
                    Some(slot) => slot.1 = val,
                    None => sts.push((tid, val)),
                }
            }
        }
        let done: Vec<J> = sts
            .iter()
            .filter(|(_, (s, _))| py::eq_str(s, "done"))
            .map(|(k, _)| k.clone())
            .collect();
        let mut pk_done = 0usize;
        for pk in &packets {
            let tl = pk.pidx("tasks")?;
            if !tl.truthy() {
                continue;
            }
            let mut all_done = true;
            for t in tl.py_iter()? {
                let id = t.pidx("id")?;
                let found = sts
                    .iter()
                    .find(|(k, _)| py::py_eq(k, id))
                    .ok_or_else(|| PyErr::new("KeyError", py::py_repr(id)))?;
                if !py::eq_str(&found.1.0, "done") {
                    all_done = false;
                    break;
                }
            }
            if all_done {
                pk_done += 1;
            }
        }
        res.set("total", sts.len());
        res.set("done_recomputed", done.len());
        res.set("packets_accepted", pk_done);
        res.set("packets_total", packets.len());
        res.set("source", format!("{wt}/docs/v5-plan/progress/*.json"));
        let progress_dir = format!("{wt}/docs/v5-plan/progress");
        for k in &done {
            let ev = sts
                .iter()
                .find(|(key, _)| py::py_eq(key, k))
                .map(|(_, (_, e))| e.clone())
                .unwrap_or_default();
            if !ev.truthy() {
                let text = format!("{} done with empty evidence", py::py_str(k));
                self.f(
                    "amber",
                    "joil",
                    "DONE_NO_EVIDENCE",
                    &text,
                    Some(&progress_dir),
                );
            }
        }
        let validator = self.cfg.joil_validator.clone();
        let val_bytes = std::fs::read(&validator).map_err(|e| PyErr::os(&validator, &e))?;
        let val_text = py::universal_newlines(&String::from_utf8_lossy(&val_bytes));
        let val = val_text.split("## Previous").next().unwrap_or("");
        let held: Vec<String> = py::sorted_set(
            AUDIT_HELD
                .captures_iter(val)
                .filter_map(|m| m.get(1).map(|g| g.as_str().to_string())),
        );
        let bad: Vec<&String> = held
            .iter()
            .filter(|h| done.iter().any(|d| py::eq_str(d, h)))
            .collect();
        if !bad.is_empty() {
            let text = format!(
                "validator AUDIT-HELD but ledger done: {}",
                bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")
            );
            self.f(
                "red",
                "joil",
                "DONE_BUT_AUDIT_HELD",
                &text,
                Some("joil validator LATEST.md"),
            );
        }
        res.set("validator_audit_held", J::from(held.clone()));
        let mut counts: Vec<(J, usize)> = Vec::new();
        for (_, (s, _)) in &sts {
            if matches!(s, J::Arr(_) | J::Obj(_)) {
                return Err(PyErr::type_error(format!(
                    "unhashable type: '{}'",
                    s.type_name()
                )));
            }
            match counts.iter_mut().find(|(k, _)| py::py_eq(k, s)) {
                Some(slot) => slot.1 += 1,
                None => counts.push((s.clone(), 1)),
            }
        }
        let status_counts: Obj = counts
            .iter()
            .map(|(k, n)| (py::json_key(k), J::from(*n)))
            .collect();
        let blocked = counts
            .iter()
            .find(|(k, _)| py::eq_str(k, "blocked"))
            .map_or(0, |(_, n)| *n);
        res.set("status_counts", J::Obj(status_counts));
        res.set("blocked", blocked);
        let mut notes: Vec<(String, J)> = Vec::new();
        for pk in &packets {
            let st = progress_of(pk)?;
            for t in pk.pidx("tasks")?.py_iter()? {
                let tid = t.pidx("id")?;
                let v = or_empty_obj(&lookup_task(st.pidx("tasks")?, tid)?);
                if py::eq_str(v.pget("status")?, "blocked") {
                    let n = v.pget("note")?;
                    let n = if n.truthy() { n.clone() } else { J::from("") };
                    let k = py::json_key(tid);
                    match notes.iter_mut().find(|(x, _)| *x == k) {
                        Some(slot) => slot.1 = n,
                        None => notes.push((k, n)),
                    }
                }
            }
        }
        res.set("blocked_reasons", J::Obj(notes.into_iter().collect()));
        let repo = self.cfg.repo("joil");
        let dir = format!("{JOIL_PROGRESS_DIR}/");
        let out = self.run(
            &[
                "git",
                "-C",
                &repo,
                "ls-tree",
                "--name-only",
                "origin/main",
                &dir,
            ],
            15,
        );
        if out.rc == 0 && !py::py_strip(&out.stdout).is_empty() {
            let mut dm = 0i64;
            for f in py::py_split_ws(&out.stdout) {
                let spec = format!("origin/main:{f}");
                let js = self.run(&["git", "-C", &repo, "show", &spec], 15);
                let count = (|| -> Option<i64> {
                    let doc = py::loads(&js.stdout)?;
                    let tasks = doc.pidx("tasks").ok()?;
                    let J::Obj(o) = tasks else { return None };
                    let mut n = 0;
                    for v in o.values() {
                        if py::eq_str(v.pget("status").ok()?, "done") {
                            n += 1;
                        }
                    }
                    Some(n)
                })();
                dm += count.unwrap_or(0);
            }
            res.set("done_on_main", dm);
            match self.audit_joil_flips(&repo, "origin/main", true) {
                Ok(fa) => {
                    let unflagged = fa
                        .pget("done_unflagged_on_main")
                        .cloned()
                        .unwrap_or_default();
                    res.set("flip_audit", fa);
                    res.set("done_on_main_unflagged", unflagged);
                }
                Err(e) => {
                    let text = py::prefix(&e.msg, 200).to_string();
                    self.f("amber", "joil", "FLIP_AUDIT_ERROR", &text, None);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prod_path_rules() {
        assert!(joil_is_prod_path("crates/a/src/lib.rs"));
        assert!(!joil_is_prod_path("crates/a/tests/t.rs"));
        assert!(!joil_is_prod_path("docs/x.py"));
        assert!(!joil_is_prod_path("src/test_x.py"));
        assert!(!joil_is_prod_path("web/a.spec.ts"));
        assert!(!joil_is_prod_path("README.md"));
    }
}
