//! Progress accountability (Jepson 2026-10-18 04:59 MDT): "The job of the
//! super_captain is to hold captains accountable if they haven't made serious
//! progress since the latest check-in."
//!
//! The stall detectors key ONLY on the done count: nothing but a count
//! increase (or the project reaching total) clears them. Merges, ETAs, busy
//! screens and plans do not. Rolled-back overclaims do not reset the clock.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::AuditConfig;
use crate::ctx::Audit;
use crate::host::read_text_replace;
use crate::py::{self, Dt, J, Obj, PyErr, PyResult};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

static BLOCK_CAUSES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "final_gate_or_ci",
            re(
                r"(?i)final (exact-head )?gate|exact-head|head CI|eight-suite|final source/head|full gate",
            ),
        ),
        (
            "defect_control",
            re(r"(?i)defect closure|defect control|\bD-\d\d"),
        ),
        (
            "owner_decision",
            re(
                r"(?i)unsigned|unapproved|owner review|owner acceptance|disclosure|approval|spending|authority",
            ),
        ),
        (
            "external_measured_data",
            re(
                r"(?i)measur|metrology|specimen|fatigue data|wet loads|calibration data|accession|labels unavailable|carrier-field|manufacturing/load|as-built|scientific observations|interface dataset|process/material inputs",
            ),
        ),
        (
            "native_compute_or_lease_scope",
            re(
                r"(?i)native|GPU|CFD|solver|Blender|render|campaign|parity|benchmark|mutation|not run|have not run|unexecuted|not executed|frozen corpus|material partition|drills|browser|lease",
            ),
        ),
        (
            "upstream_admission_dependency",
            re(
                r"(?i)admission|admitted|observer|registry|DecisionProfile|accepted|qualif|preregistration|statistical|dependency",
            ),
        ),
    ]
});
static BOILERPLATE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(Integrated software checkpoint \w+; full gate/CI and acceptance pending\.|Software integrated with positive/negative controls; acceptance open:)\s*",
    )
});
static STALL_PLAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"STALL_PLAN_CHANGE\s+`?(\w+)`?\s+`?(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2})?Z)")
});

/// Red when more than this share of a project's tasks are blocked.
pub const BLOCKED_SHARE_RED: f64 = 0.25;
/// Compare against the newest snapshot at least this old (one check-in).
pub const WINDOW_MIN: i64 = 25;
/// No done increase this long -> must-act.
pub const STALL_ESCALATE_MIN: i64 = 60;
/// Still flat one hour later -> repeated stall.
pub const STALL_REPEAT_MIN: i64 = 120;

/// `classify_block(note)`.
pub fn classify_block(note: &J) -> PyResult<&'static str> {
    let text = match note {
        J::Str(s) => s.clone(),
        other if !other.truthy() => String::new(),
        other => {
            return Err(PyErr::type_error(format!(
                "expected string or bytes-like object, got '{}'",
                other.type_name()
            )));
        }
    };
    let note = BOILERPLATE.replace(&text, "");
    for (name, rx) in BLOCK_CAUSES.iter() {
        if rx.is_match(&note) {
            return Ok(name);
        }
    }
    Ok("other_or_unstated")
}

/// `classify_block` for a plain string.
pub fn classify_block_str(note: &str) -> &'static str {
    classify_block(&J::from(note)).unwrap_or("other_or_unstated")
}

/// `stall_plan_changes(paths)`: `{project: [utc datetimes]}`.
pub fn stall_plan_changes(paths: &[String]) -> BTreeMap<String, Vec<Dt>> {
    let mut out: BTreeMap<String, Vec<Dt>> = BTreeMap::new();
    for path in paths {
        let Some(text) = read_text_replace(path) else {
            continue;
        };
        for m in STALL_PLAN_RE.captures_iter(&text) {
            let (Some(p), Some(t)) = (m.get(1), m.get(2)) else {
                continue;
            };
            let Some(t) = py::fromiso_aware(t.as_str()) else {
                continue;
            };
            out.entry(p.as_str().to_lowercase()).or_default().push(t);
        }
    }
    out
}

/// One escalation: `(severity, code, text)`.
pub type Escalation = (&'static str, &'static str, String);

/// `stall_escalation(proj, done, total, stuck_since_iso, now, plan_changes)`.
pub fn stall_escalation(
    cfg: &AuditConfig,
    proj: &str,
    done: &J,
    total: &J,
    stuck_since_iso: &J,
    now: &Dt,
    plan_changes: &[Dt],
) -> PyResult<Vec<Escalation>> {
    if done.as_inst_int().is_none() || !stuck_since_iso.truthy() {
        return Ok(Vec::new());
    }
    if total.truthy() && py::py_cmp(done, total)? != std::cmp::Ordering::Less {
        return Ok(Vec::new());
    }
    let Some(parsed) = py::fromisoformat(&py::py_str(stuck_since_iso).replace('Z', "+00:00"))
    else {
        return Ok(Vec::new());
    };
    let since = parsed.aware().ok_or_else(|| {
        PyErr::type_error("can't subtract offset-naive and offset-aware datetimes")
    })?;
    let mins = (py::total_seconds(now, &since) / 60.0) as i64;
    if mins < STALL_ESCALATE_MIN {
        return Ok(Vec::new());
    }
    let cap = cfg.captain_or(proj);
    let done_s = py::py_str(done);
    let total_s = py::py_str(total);
    let since_s = py::py_str(stuck_since_iso);
    let mut out = vec![(
        "red",
        "STALL_60M",
        format!(
            "done has been {done_s}/{total_s} for {mins} min (no increase since {since_s}). Merges, ETAs and \"verified busy\" do NOT clear this; only a done increase does. \
super_captain MUST this tick: (1) personally hardcopy {cap} (all windows) and every worker screen, (2) name the real blocker of each open task, \
(3) change the plan: add workers/hosts (xbabe1-3, never xbabe0), re-scope, or decide reversible blockers as owner, (4) write \
\"STALL_PLAN_CHANGE {proj} <ISO-Z>\" in super_captain_DECISIONS.md with the change, and (5) get a count increase within 60 min"
        ),
    )];
    let recent: Vec<&Dt> = plan_changes.iter().filter(|t| **t >= since).collect();
    if recent.is_empty() {
        out.push((
            "red",
            "STALL_NO_PLAN_CHANGE",
            format!(
                "stalled {mins} min and no \"STALL_PLAN_CHANGE {proj}\" decision recorded since the stall began ({since_s}); a plan change is mandatory, not an ETA"
            ),
        ));
    }
    if mins >= STALL_REPEAT_MIN {
        let last = recent.iter().max().copied();
        let tail = match last {
            Some(l) => format!(
                "; last plan change {} did not move the count",
                py::iso_minutes(l)
            ),
            None => String::new(),
        };
        out.push((
            "red",
            "STALL_REPEATED",
            format!(
                "done flat {mins} min (>= {STALL_REPEAT_MIN}){tail}. Repeated no-progress: the previous plan failed. Change it again (more parallel workers, re-scope tasks so they can close, \
or decide the blocker); escalate to Jepson only if the blocker is reserved (xbabe0/Jeryu, outside messages, new paid spend/legal, force-push main, deleting prod data, writing /home/ubuntu/admiralty)"
            ),
        ));
    }
    Ok(out)
}

/// `last_sustained_increase(h, current)`: utc of the last snapshot where done
/// rose and never later fell below that new value (including `current`).
pub fn last_sustained_increase(h: &[J], current: &J) -> PyResult<J> {
    let mut vals = Vec::with_capacity(h.len());
    for x in h {
        vals.push(x.pget("done")?.as_inst_int());
    }
    let tail: Vec<Option<i64>> = match current.as_inst_int() {
        Some(c) => vec![Some(c)],
        None => Vec::new(),
    };
    let mut last = J::Null;
    for i in 1..h.len() {
        let (Some(a), Some(b)) = (vals[i - 1], vals[i]) else {
            continue;
        };
        if b <= a {
            continue;
        }
        if vals[i + 1..]
            .iter()
            .chain(tail.iter())
            .flatten()
            .all(|v| *v >= b)
        {
            last = h[i].pidx("utc")?.clone();
        }
    }
    Ok(last)
}

fn le_str(a: &J, b: &str) -> PyResult<bool> {
    Ok(py::py_cmp(a, &J::from(b))? != std::cmp::Ordering::Greater)
}

/// Seed `hist` from `audit.jsonl` (stops at the first malformed line, like the Python try/except).
fn seed_history(hist: &mut Obj, jsonl: &str) {
    let Ok(bytes) = std::fs::read(jsonl) else {
        return;
    };
    for raw in bytes.split_inclusive(|b| *b == b'\n') {
        let Ok(line) = std::str::from_utf8(raw) else {
            return;
        };
        let Some(d) = py::loads(line) else { return };
        let Ok(done) = d.pget("done") else { return };
        let done = if done.truthy() {
            done.clone()
        } else {
            J::Obj(Obj::new())
        };
        let J::Obj(done) = done else { return };
        for (p, v) in done.iter() {
            if v.as_inst_int().is_none() {
                continue;
            }
            let Ok(utc) = d.pidx("utc") else { return };
            let entry =
                crate::jobj! {"utc" => utc.clone(), "done" => v.clone(), "blocked" => J::Null};
            match hist.setdefault(p, J::Arr(Vec::new())) {
                J::Arr(a) => a.push(entry),
                _ => return,
            }
        }
    }
}

impl Audit<'_> {
    /// `progress_accountability(report, state, now, no_gh)`.
    pub fn progress_accountability(
        &mut self,
        report: &mut Obj,
        state: &mut Obj,
        out_dir: &str,
        now: &Dt,
        no_gh: bool,
    ) -> PyResult<()> {
        let mut hist = match state.get("progress") {
            Some(J::Obj(o)) => o.clone(),
            None => Obj::new(),
            Some(other) => return Err(PyErr::attr_get(other)),
        };
        if hist.is_empty() {
            seed_history(&mut hist, &format!("{out_dir}/audit.jsonl"));
        }
        let cutoff = py::iso_seconds(&(*now - chrono::Duration::hours(8)));
        let window = py::iso_seconds(&(*now - chrono::Duration::minutes(WINDOW_MIN)));
        let plan_paths = [
            format!("{}/super_captain_DECISIONS.md", self.cfg.kick),
            format!("{}/super_captain_LOG.md", self.cfg.kick),
        ];
        let plan_changes = stall_plan_changes(&plan_paths);
        let report_utc = report.get("utc").cloned().unwrap_or_default();
        let mut out = Obj::new();
        let project_names: Vec<String> = match report.get("projects") {
            Some(J::Obj(p)) => p.keys().cloned().collect(),
            _ => Vec::new(),
        };
        let result = (|| -> PyResult<()> {
            for proj in &project_names {
                let r = match report.get("projects") {
                    Some(J::Obj(p)) => p.get(proj).cloned().unwrap_or_default(),
                    _ => J::Null,
                };
                let done = r.pget("done_recomputed")?.clone();
                let total = r.pget("total")?.clone();
                let mut blocked = r.pget("blocked")?.clone();
                if blocked.is_null()
                    && let J::Obj(sc) = r.pget("status_counts")?
                {
                    blocked = sc.get("blocked").cloned().unwrap_or_default();
                }
                let mut h: Vec<J> = Vec::new();
                for x in hist.get(proj).map_or_else(|| Ok(Vec::new()), J::py_iter)? {
                    if py::py_cmp(x.pidx("utc")?, &J::from(cutoff.as_str()))?
                        != std::cmp::Ordering::Less
                    {
                        h.push(x);
                    }
                }
                let mut base: Option<J> = None;
                for x in &h {
                    if le_str(x.pidx("utc")?, &window)? {
                        base = Some(x.clone());
                    }
                }
                let last_up = last_sustained_increase(&h, &done)?;
                let mut info = Obj::new();
                info.set("done", done.clone());
                info.set("blocked", blocked.clone());
                info.set("total", total.clone());
                match &base {
                    Some(b) => {
                        info.set("baseline_utc", b.pidx("utc")?.clone());
                        info.set("baseline_done", b.pget("done")?.clone());
                        info.set("baseline_blocked", b.pget("blocked")?.clone());
                    }
                    None => {
                        info.set("baseline_utc", J::Null);
                        info.set("baseline_done", J::Null);
                        info.set("baseline_blocked", J::Null);
                    }
                }
                info.set("last_done_increase_utc", last_up.clone());
                if let (Some(b), Some(_)) = (&base, done.as_inst_int()) {
                    let base_utc = b.pidx("utc")?.clone();
                    let base_utc_s = match &base_utc {
                        J::Str(s) => s.clone(),
                        other => {
                            return Err(PyErr::new(
                                "AttributeError",
                                format!(
                                    "'{}' object has no attribute 'replace'",
                                    other.type_name()
                                ),
                            ));
                        }
                    };
                    let merges = if no_gh {
                        J::Null
                    } else {
                        self.merged_since(proj, &base_utc_s)?
                    };
                    info.set("merged_prs_since_baseline", merges.clone());
                    let base_done = b.pget("done")?;
                    let base_done = if base_done.truthy() {
                        base_done.clone()
                    } else {
                        J::Int(0)
                    };
                    if py::py_cmp(&done, &base_done)? != std::cmp::Ordering::Greater
                        && !merges.truthy()
                    {
                        let mins = py::since_age_min(now, &base_utc)? as i64;
                        let stuck_since = if last_up.truthy() {
                            last_up.clone()
                        } else if let Some(first) = h.first() {
                            first.pidx("utc")?.clone()
                        } else {
                            base_utc.clone()
                        };
                        let text = format!(
                            "done {} -> {} over the last {mins} min and no PR merged to main; done has not increased since {}. \
super_captain: demand a concrete unblock plan + ETA from {} this check, decide reversible blockers yourself as owner, escalate only irreversible ones to Jepson",
                            py::py_str(b.pget("done")?),
                            py::py_str(&done),
                            py::py_str(&stuck_since),
                            self.cfg.captain_or(proj)
                        );
                        self.f("red", proj, "NO_PROGRESS", &text, None);
                    }
                    let base_blocked = b.pget("blocked")?;
                    if let (Some(now_b), Some(prev_b)) =
                        (blocked.as_inst_int(), base_blocked.as_inst_int())
                        && now_b > prev_b
                    {
                        let text = format!(
                            "blocked {} -> {} since {}; every new blocker needs an owner, a cause and an unblock action this check",
                            py::py_str(b.pidx("blocked")?),
                            py::py_str(&blocked),
                            py::py_str(&base_utc)
                        );
                        self.f("red", proj, "BLOCKED_ROSE", &text, None);
                    }
                }
                let stuck_since = if last_up.truthy() {
                    last_up.clone()
                } else if let Some(first) = h.first() {
                    first.pidx("utc")?.clone()
                } else {
                    J::Null
                };
                info.set("stuck_since_utc", stuck_since.clone());
                let empty = Vec::new();
                let changes = plan_changes.get(proj.as_str()).unwrap_or(&empty);
                for (sev, code, text) in
                    stall_escalation(self.cfg, proj, &done, &total, &stuck_since, now, changes)?
                {
                    self.ma(sev, proj, code, &text, None);
                }
                if let Some(b) = blocked.as_inst_int()
                    && total.truthy()
                {
                    let total_f = total.as_f64().ok_or_else(|| {
                        PyErr::type_error(format!(
                            "unsupported operand type(s) for /: 'int' and '{}'",
                            total.type_name()
                        ))
                    })?;
                    if b as f64 / total_f > BLOCKED_SHARE_RED {
                        let reasons = r.pget("blocked_reasons")?;
                        let reasons = if reasons.truthy() {
                            reasons.clone()
                        } else {
                            J::Obj(Obj::new())
                        };
                        let J::Obj(reasons) = reasons else {
                            return Err(PyErr::new(
                                "AttributeError",
                                format!(
                                    "'{}' object has no attribute 'values'",
                                    reasons.type_name()
                                ),
                            ));
                        };
                        let mut groups: Vec<(&'static str, i64)> = Vec::new();
                        for n in reasons.values() {
                            let c = classify_block(n)?;
                            match groups.iter_mut().find(|(k, _)| *k == c) {
                                Some(slot) => slot.1 += 1,
                                None => groups.push((c, 1)),
                            }
                        }
                        let g = if groups.is_empty() {
                            "reasons not machine-readable; captain must list them".to_string()
                        } else {
                            let mut most = groups.clone();
                            most.sort_by_key(|g| std::cmp::Reverse(g.1));
                            most.iter()
                                .map(|(k, v)| format!("{k} {v}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        info.set(
                            "blocked_by_cause",
                            J::Obj(
                                groups
                                    .iter()
                                    .map(|(k, v)| ((*k).to_string(), J::from(*v)))
                                    .collect(),
                            ),
                        );
                        let total_i = total.as_inst_int().unwrap_or(total_f as i64);
                        let pct = if total_i != 0 {
                            (100 * b).div_euclid(total_i)
                        } else {
                            0
                        };
                        let text = format!(
                            "{}/{} tasks ({pct}%) are blocked (> {}%). By cause (keyword grouping): {g}. \
Captain owes a per-cause unblock plan; super_captain decides reversible causes (lease/compute scope, caps, lane launches, dependency order) as owner now",
                            py::py_str(&blocked),
                            py::py_str(&total),
                            (BLOCKED_SHARE_RED * 100.0) as i64
                        );
                        self.f("red", proj, "BLOCKED_SHARE", &text, None);
                    }
                }
                if proj == "joil" {
                    if let Some(J::Int(dm)) = r.as_obj().and_then(|o| o.get("done_on_main")) {
                        info.set("done_on_main", *dm);
                    } else if let Some(J::Bool(dm)) = r.as_obj().and_then(|o| o.get("done_on_main"))
                    {
                        info.set("done_on_main", *dm);
                    }
                }
                h.push(crate::jobj! {"utc" => report_utc.clone(), "done" => done.clone(), "blocked" => blocked.clone()});
                let keep = h.len().saturating_sub(200);
                hist.set(proj.clone(), J::Arr(h[keep..].to_vec()));
                out.set(proj.clone(), J::Obj(info));
            }
            Ok(())
        })();
        state.set("progress", J::Obj(hist));
        result?;
        report.set("progress", J::Obj(out));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_block_groups() {
        assert_eq!(
            classify_block_str("Cause B: measured data absent"),
            "external_measured_data"
        );
        assert_eq!(
            classify_block_str("waiting on owner review"),
            "owner_decision"
        );
        assert_eq!(
            classify_block_str(
                "Integrated software checkpoint abc; full gate/CI and acceptance pending. GPU campaign not run"
            ),
            "native_compute_or_lease_scope"
        );
        assert_eq!(classify_block_str(""), "other_or_unstated");
        assert!(classify_block(&J::Int(3)).is_err());
    }
}
