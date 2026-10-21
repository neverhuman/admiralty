//! Admiralty v2 rollout: packets from rollout.json, workers from
//! live-lanes.json + screens. Read-only; `deployments/` and `controller/`
//! belong to the local Codex integrator.

use std::sync::LazyLock;

use regex::Regex;

use crate::ctx::{Audit, rj};
use crate::py::{self, Dt, J, Obj, PyResult};
use crate::screen::{Screens, screen_state};

/// Recorded provenance only; receipt contents/main acceptance are not verified here.
pub const ADM_DONE_STATES: [&str; 1] = ["integrated"];
/// Exact lane states, never substrings.
pub const ADM_FINISHED: [&str; 6] = [
    "finished",
    "integrated",
    "verified",
    "closed",
    "retired",
    "released",
];

static REV: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(?:[0-9a-f]{40}|[0-9a-f]{64})\z").expect("valid regex"));
static PID_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A\d+\.").expect("valid regex"));

/// `_age_min(now, v)`.
pub fn age_min(now: &Dt, v: &J) -> Option<f64> {
    py::parse_utc(v).map(|t| py::total_seconds(now, &t) / 60.0)
}

/// `admiralty_verification_record(packet, now)`: recorded rollout provenance,
/// never receipt contents or acceptance.
pub fn admiralty_verification_record(packet: &J, now: &Dt) -> bool {
    let Some(packet) = packet.as_obj() else {
        return false;
    };
    let Some(proof) = packet.get("verification").and_then(J::as_obj) else {
        return false;
    };
    let placeholders = [
        "unknown",
        "unverified",
        "false",
        "none",
        "null",
        "pending",
        "failed",
    ];
    for key in ["receipt", "revision", "verifier", "observed_utc"] {
        let Some(J::Str(value)) = proof.get(key) else {
            return false;
        };
        let n = py::char_len(value);
        let stripped = py::py_strip(value);
        if !(1..=1000).contains(&n)
            || stripped.is_empty()
            || placeholders.contains(&stripped.to_lowercase().as_str())
            || value.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
        {
            return false;
        }
    }
    let revision = proof.get("revision").cloned().unwrap_or_default();
    if !REV.is_match(revision.as_str().unwrap_or("")) {
        return false;
    }
    let observed = proof.get("observed_utc").and_then(J::as_str).unwrap_or("");
    match py::fromisoformat(&observed.replace('Z', "+00:00")) {
        Some(p) => match p.aware() {
            Some(t) if t <= *now => {}
            _ => return false,
        },
        None => return false,
    }
    for key in ["status", "result", "outcome"] {
        if let Some(v) = proof.get(key) {
            match v {
                J::Str(s)
                    if matches!(
                        py::py_strip(s).to_lowercase().as_str(),
                        "pass" | "passed" | "success" | "verified"
                    ) => {}
                _ => return false,
            }
        }
    }
    for key in ["passed", "verified", "success"] {
        if let Some(v) = proof.get(key)
            && *v != J::Bool(true)
        {
            return false;
        }
    }
    if let Some(candidate) = packet.get("candidate_revision")
        && !candidate.is_null()
        && !py::eq_str(candidate, "")
        && !py::py_eq(candidate, &revision)
    {
        return false;
    }
    if let Some(integration) = packet.get("integration").and_then(J::as_obj)
        && let Some(r) = integration.get("revision")
        && !py::py_eq(r, &revision)
    {
        return false;
    }
    if let Some(claim) = packet.get("claim").and_then(J::as_obj) {
        let verifier = proof.get("verifier").cloned().unwrap_or_default();
        if py::py_eq(claim.get("owner").unwrap_or(&J::Null), &verifier) {
            return false;
        }
    }
    true
}

impl Audit<'_> {
    /// `audit_admiralty(scr, prev, newh, now)`.
    pub fn audit_admiralty(
        &mut self,
        scr: &Screens,
        prev: &J,
        newh: &mut Obj,
        now: &Dt,
    ) -> PyResult<J> {
        let adm = self.cfg.adm.clone();
        let rollout_path = format!("{adm}/rollout.json");
        let lanes_path = format!("{adm}/live-lanes.json");
        let mut res = Obj::new();
        res.set("source", format!("{adm}/rollout.json (+ live-lanes.json)"));
        let r = rj(&rollout_path).unwrap_or_default();
        if !r.truthy() || !matches!(r.pget("packets")?, J::Arr(_)) {
            self.f(
                "amber",
                "admiralty",
                "LEDGER_UNREADABLE",
                "rollout.json unreadable",
                Some(&rollout_path),
            );
            return Ok(J::Obj(res));
        }
        let pk = r.pidx("packets")?.py_iter()?;
        let mut st: Vec<(String, usize)> = Vec::new();
        for x in &pk {
            let s = py::py_str(x.pget("status")?);
            match st.iter_mut().find(|(k, _)| *k == s) {
                Some(slot) => slot.1 += 1,
                None => st.push((s, 1)),
            }
        }
        let mut done = 0usize;
        let mut claimed: Vec<J> = Vec::new();
        for x in &pk {
            let status = x.pget("status")?;
            if ADM_DONE_STATES.iter().any(|s| py::eq_str(status, s))
                && admiralty_verification_record(x, now)
            {
                done += 1;
            }
            if py::eq_str(status, "verified") || py::eq_str(status, "integrated") {
                claimed.push(x.clone());
            }
        }
        res.set("total", pk.len());
        res.set("done_recomputed", done);
        res.set(
            "status_counts",
            J::Obj(st.iter().map(|(k, n)| (k.clone(), J::from(*n))).collect()),
        );
        res.set(
            "blocked",
            st.iter()
                .find(|(k, _)| k == "blocked")
                .map_or(0, |(_, n)| *n),
        );
        res.set("verified_or_integrated_claimed", claimed.len());
        res.set("pr", r.pget("pr")?.clone());
        res.set(
            "note",
            "done = integrated with a well-formed recorded verification reference; receipt contents and acceptance on main are not independently verified by this audit",
        );
        for x in &claimed {
            if !admiralty_verification_record(x, now) {
                let text = format!(
                    "{} is {} with missing, invalid or contradictory verification provenance",
                    py::py_str(x.pget("id")?),
                    py::py_str(x.pget("status")?)
                );
                self.f(
                    "red",
                    "admiralty",
                    "DONE_NO_EVIDENCE",
                    &text,
                    Some(&rollout_path),
                );
            }
        }
        let age = age_min(now, r.pget("updated_utc")?);
        res.set("rollout_age_min", age.map(py::py_round));
        if let Some(a) = age.filter(|a| *a >= 30.0) {
            let text = format!(
                "rollout.json not updated for {} min (packet statuses on the dashboard are stale)",
                py::py_round(a)
            );
            self.f(
                "amber",
                "admiralty",
                "ROLLOUT_STALE",
                &text,
                Some(&rollout_path),
            );
        }
        let ll = rj(&lanes_path)
            .filter(J::truthy)
            .unwrap_or_else(|| J::Obj(Obj::new()));
        let lanes = match &ll {
            J::Obj(o) => o.get("lanes").cloned(),
            _ => None,
        };
        let Some(J::Arr(lanes)) = lanes else {
            self.f(
                "amber",
                "admiralty",
                "LANES_UNREADABLE",
                "live-lanes.json unreadable",
                Some(&lanes_path),
            );
            return Ok(J::Obj(res));
        };
        let mut workers = Obj::new();
        let mut dead = Vec::new();
        let mut stale = Vec::new();
        let mut unverified = Vec::new();
        for l in &lanes {
            let name_src = {
                let a = l.pget("screen")?;
                let a = if a.truthy() {
                    a.clone()
                } else {
                    l.pget("id")?.clone()
                };
                if a.truthy() {
                    py::py_str(&a)
                } else {
                    "?".to_string()
                }
            };
            let n = PID_PREFIX.replace(&name_src, "").to_string();
            let status_j = l.pget("status")?;
            let status = if status_j.truthy() {
                py::py_str(status_j)
            } else {
                String::new()
            };
            let a = age_min(now, l.pget("updated_utc")?);
            let packet = {
                let mut v = J::Null;
                for k in ["packet_id", "packet", "id", "branch"] {
                    v = l.pget(k)?.clone();
                    if v.truthy() {
                        break;
                    }
                }
                v
            };
            let packet_s = py::py_str(&packet);
            let alive = scr.contains_key(&n);
            let mut info = Obj::new();
            info.set("packet", packet.clone());
            info.set("branch", l.pget("branch")?.clone());
            info.set("status", status.as_str());
            info.set("reported_age_min", a.map(py::py_round));
            info.set("alive", alive);
            let finished = ADM_FINISHED.contains(&py::py_strip(&status).to_lowercase().as_str());
            if !alive && !finished {
                dead.push(format!("{n}({packet_s})"));
            }
            if alive {
                let hc = self.cfg.audit_hc(&n);
                let s = screen_state(&self.hardcopy(&n, &hc));
                let p = crate::ctx::prev_entry(prev, &n)?;
                let since = if py::eq_str(p.pget("hash")?, &s.hash) {
                    p.pget("since")?.clone()
                } else {
                    J::from(py::iso_seconds(now))
                };
                newh.set(
                    n.clone(),
                    crate::jobj! {"hash" => s.hash.as_str(), "since" => since.clone()},
                );
                let unchanged = py::py_round(py::since_age_min(now, &since)?);
                info.set("unchanged_min", unchanged);
                info.set("idle_prompt", s.idle_prompt);
                if s.survey || s.approval_prompt {
                    self.f(
                        "red",
                        "admiralty",
                        "LANE_PROMPT_BLOCKED",
                        &format!("{n} is stuck on a survey/approval prompt"),
                        Some(&hc),
                    );
                }
                if s.quota {
                    self.f(
                        "amber",
                        "admiralty",
                        "LANE_QUOTA",
                        &format!("{n} mentions quota/rate limit"),
                        Some(&hc),
                    );
                }
                if !finished && (a.is_some_and(|a| a >= 30.0) || unchanged >= 30) {
                    let reported = py::py_str(&J::from(a.map(py::py_round)));
                    stale.push(format!(
                        "{n}({packet_s}, reported {reported}m ago, screen unchanged {unchanged}m{})",
                        if s.idle_prompt { ", idle" } else { "" }
                    ));
                }
            }
            if status.to_lowercase().contains("unverified") && a.is_some_and(|a| a >= 60.0) {
                unverified.push(format!("{n}({packet_s})"));
            }
            workers.set(n, J::Obj(info));
        }
        res.set("workers", J::Obj(workers));
        if !dead.is_empty() {
            let shown: Vec<&str> = dead.iter().take(20).map(String::as_str).collect();
            let text = format!(
                "{} live-lanes entries are not finished but their screens are gone: {}",
                dead.len(),
                shown.join(" ")
            );
            self.f("amber", "admiralty", "LANE_DEAD", &text, Some(&lanes_path));
        }
        if !stale.is_empty() {
            let shown: Vec<&str> = stale.iter().take(20).map(String::as_str).collect();
            let text = format!(
                "{} workers stale (>=30 min without a reported transition or screen change): {}. agy_captain_admiralty must inspect verified owner/endpoint and supported queue/priority status; notLoaded is BLOCKED_TRANSPORT, never type, clear, resume or duplicate",
                stale.len(),
                shown.join("; ")
            );
            self.f("amber", "admiralty", "LANE_STALE", &text, Some(&lanes_path));
        }
        if !unverified.is_empty() {
            let shown: Vec<&str> = unverified.iter().take(20).map(String::as_str).collect();
            let text = format!(
                "{} candidates committed but unverified for >=60 min: {}; drive verification on the exact commit, then hand to the integrator",
                unverified.len(),
                shown.join(" ")
            );
            self.f(
                "amber",
                "admiralty",
                "UNVERIFIED_CANDIDATES_AGING",
                &text,
                Some(&lanes_path),
            );
        }
        Ok(J::Obj(res))
    }
}
