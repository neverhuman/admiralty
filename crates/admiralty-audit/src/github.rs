//! GitHub observations through the `gh` CLI (read-only): open PRs, stale PRs,
//! stuck queued checks, main CI, merges, last push and Jepson bottlenecks.

use std::sync::LazyLock;

use regex::Regex;

use crate::config::AuditConfig;
use crate::ctx::Audit;
use crate::ledgers::iter_or_empty;
use crate::py::{self, Dt, J, Obj, PyErr, PyResult};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

/// Jepson 6:24 AM MDT Oct 18: JopeDime GPU testing + Admiralty come first.
pub const P1_PROJECTS: [&str; 2] = ["jopedime", "admiralty"];
/// A non-draft PR open this long is must-act.
pub const STALE_PR_MIN: f64 = 45.0;
pub const MAIN_RED_ESCALATE_MIN: i64 = 30;
pub const MAIN_RED_ESCALATE_RUNS: usize = 3;
pub const JEPSON_LOGINS: [&str; 1] = ["jepsontaylor"];
static BACKUP_BRANCH_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(backup|bak|salvage|tmp|scratch)[/_-]"));
static JEPSON_TEXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(waiting|wait|blocked|pending|awaiting|needs?|requires?)\W+(?:\w+\W+){0,3}?(on|for)?\W*jepson|needs[\s_-]*jepson|jepson[\s_-]*(approval|sign[\s-]?off|review|decision|to (approve|merge|decide|review))",
    )
});
static RESERVED_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)xbabe0|jeryu|outside|external (party|people|recipient)|email|paid|spend|purchase|licen[cs]e|legal|contract|force[\s-]?push|delet\w* (prod|production)|/home/ubuntu/admiralty\b",
    )
});
static CODEOWNER_JEPSON: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)@jepsontaylor\b"));

/// `JEPSON_TAKEOVER`.
pub const JEPSON_TAKEOVER: &str = "super_captain takes over now under JEPSON_DELEGATION_20261018T1158Z_fleet_admiral.md: approve via neverhumanbot (gh-role reviewer) and merge \
--rebase --match-head-commit, or decide it as owner. Only reserved items (xbabe0/Jeryu, outside messages, new paid spend/legal, \
force-push main, deleting prod data, writing /home/ubuntu/admiralty) may wait for Jepson";

/// `x.upper()` (requires a str).
fn py_upper(x: &J) -> PyResult<String> {
    match x {
        J::Str(s) => Ok(s.to_uppercase()),
        other => Err(PyErr::new(
            "AttributeError",
            format!("'{}' object has no attribute 'upper'", other.type_name()),
        )),
    }
}

/// `x.lower()` (requires a str).
fn py_lower(x: &J) -> PyResult<String> {
    match x {
        J::Str(s) => Ok(s.to_lowercase()),
        other => Err(PyErr::new(
            "AttributeError",
            format!("'{}' object has no attribute 'lower'", other.type_name()),
        )),
    }
}

/// `x[:9]` for a str (as used in f-strings).
fn head9(x: &J) -> PyResult<String> {
    match x {
        J::Str(s) => Ok(py::prefix(s, 9).to_string()),
        J::Arr(a) => Ok(py::py_repr(&J::Arr(a.iter().take(9).cloned().collect()))),
        other => Err(PyErr::type_error(format!(
            "'{}' object is not subscriptable",
            other.type_name()
        ))),
    }
}

/// First truthy of `c.get(k1) or c.get(k2) ...`, else `default`.
fn first_truthy(c: &J, keys: &[&str], default: J) -> PyResult<J> {
    for k in keys {
        let v = c.pget(k)?;
        if v.truthy() {
            return Ok(v.clone());
        }
    }
    Ok(default)
}

/// `(now - fromisoformat(str(t).replace('Z', '+00:00'))).total_seconds() / 60`, `None` on any exception.
fn age_minutes(now: &Dt, t: &J) -> Option<f64> {
    let parsed = py::fromisoformat(&py::py_str(t).replace('Z', "+00:00"))?;
    let t = parsed.aware()?;
    Some(py::total_seconds(now, &t) / 60.0)
}

/// `stuck_queued_checks(roll, now, mins)`: checks still queued (no runner) for >= mins.
pub fn stuck_queued_checks(roll: &J, now: &Dt, mins: f64) -> PyResult<Vec<(String, i64)>> {
    let mut out = Vec::new();
    for c in iter_or_empty(roll)? {
        if c.pget("conclusion")?.truthy() {
            continue;
        }
        let st = first_truthy(&c, &["status", "state"], J::from(""))?;
        let st = py::py_str(&st).to_uppercase();
        if !matches!(
            st.as_str(),
            "QUEUED" | "PENDING" | "WAITING" | "EXPECTED" | "REQUESTED"
        ) {
            continue;
        }
        let t = first_truthy(&c, &["startedAt", "createdAt"], J::Null)?;
        let Some(age) = age_minutes(now, &t) else {
            continue;
        };
        if age >= mins {
            let name = first_truthy(&c, &["name", "context"], J::from("?"))?;
            out.push((py::py_str(&name), py::py_round(age)));
        }
    }
    Ok(out)
}

/// `stale_pr_problems(p, now, max_age_min)`. Pure; never calls gh.
pub fn stale_pr_problems(p: &J, now: &Dt, max_age_min: f64) -> PyResult<Vec<String>> {
    if p.pget("isDraft")?.truthy() {
        return Ok(Vec::new());
    }
    let mut probs = Vec::new();
    if let Some(age) = age_minutes(now, p.pget("createdAt")?)
        && age >= max_age_min
    {
        probs.push(format!("open {} min", py::py_round(age)));
    }
    let mergeable = p.pget("mergeable")?;
    let mergeable = if mergeable.truthy() {
        py::py_str(mergeable)
    } else {
        String::new()
    };
    if mergeable.to_uppercase() == "CONFLICTING" {
        probs.push("conflicts with main".to_string());
    }
    let mut latest: Vec<(String, (String, String))> = Vec::new();
    for c in iter_or_empty(p.pget("statusCheckRollup")?)? {
        let n = py::py_str(&first_truthy(&c, &["name", "context"], J::from("?"))?);
        let t = first_truthy(&c, &["completedAt", "startedAt"], J::from(""))?;
        let t = py::py_str(&t);
        let k = py_upper(&first_truthy(
            &c,
            &["conclusion", "state", "status"],
            J::from(""),
        )?)?;
        match latest.iter_mut().find(|(name, _)| *name == n) {
            Some(slot) => {
                if t >= slot.1.0 {
                    slot.1 = (t, k);
                }
            }
            None => latest.push((n, (t, k))),
        }
    }
    let mut bad: Vec<String> = latest
        .iter()
        .filter(|(_, (_, k))| {
            matches!(
                k.as_str(),
                "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED"
            )
        })
        .map(|(n, _)| n.clone())
        .collect();
    bad.sort();
    if !bad.is_empty() {
        probs.push(format!("failing {}", bad.join(",")));
    }
    let head_ref = p.pget("headRefName")?;
    let head_ref_s = if head_ref.truthy() {
        py::py_str(head_ref)
    } else {
        String::new()
    };
    if BACKUP_BRANCH_RE.is_match(&head_ref_s) {
        probs.push(format!(
            "opened from backup branch {} (almost certainly not work: close it)",
            py::py_str(head_ref)
        ));
    }
    Ok(probs)
}

/// `main_ci_severity(project, head, bad, runs, now)`.
pub fn main_ci_severity(
    cfg: &AuditConfig,
    project: &str,
    head: &J,
    bad: &J,
    runs: &[J],
    now: &Dt,
) -> PyResult<(&'static str, &'static str, String)> {
    let mut shas: Vec<J> = Vec::new();
    let mut red_since = J::Null;
    for r in runs {
        let c = r.pget("conclusion")?;
        if py::eq_str(c, "success") {
            break;
        }
        if ["failure", "cancelled", "timed_out"]
            .iter()
            .any(|f| py::eq_str(c, f))
        {
            let sha = r.pidx("headSha")?;
            if !shas.iter().any(|s| py::py_eq(s, sha)) {
                shas.push(sha.clone());
            }
            let created = r.pget("createdAt")?;
            if created.truthy() {
                red_since = created.clone();
            }
        }
    }
    let mins: Option<i64> = if red_since.truthy() {
        match &red_since {
            J::Str(s) => py::fromiso_aware(s).map(|t| (py::total_seconds(now, &t) / 60.0) as i64),
            _ => None,
        }
    } else {
        None
    };
    let base = format!(
        "main @ {} failing: {}",
        head9(head)?,
        py::py_join(", ", bad)?
    );
    if P1_PROJECTS.contains(&project)
        && (shas.len() >= MAIN_RED_ESCALATE_RUNS || mins.unwrap_or(0) >= MAIN_RED_ESCALATE_MIN)
    {
        let mins_s = mins.map_or_else(|| "None".to_string(), |m| m.to_string());
        let text = format!(
            "{base}; main red on {} consecutive commit(s) for >= {mins_s} min. P1 main must be green: \
{} names the failing test/job, re-runs infra failures (runner lost/cancelled), lands a fix with green exact-head CI, \
and merges nothing else onto red main until it is green. super_captain verifies this tick",
            shas.len(),
            cfg.captain_or(project)
        );
        return Ok(("red", "MAIN_CI_RED_PERSISTENT", text));
    }
    Ok(("amber", "MAIN_CI_RED", base))
}

/// `jepson_pr_reasons(p)`: reasons an open PR is waiting on Jepson personally.
pub fn jepson_pr_reasons(p: &J) -> PyResult<Vec<String>> {
    let mut out = Vec::new();
    if p.pget("isDraft")?.truthy() {
        return Ok(out);
    }
    for r in iter_or_empty(p.pget("reviewRequests")?)? {
        let login = first_truthy(&r, &["login", "name"], J::from(""))?;
        if JEPSON_LOGINS.contains(&py::py_str(&login).to_lowercase().as_str()) {
            out.push("review requested from jepsontaylor".to_string());
        }
    }
    for r in iter_or_empty(p.pget("latestReviews")?)? {
        let author = r.pget("author")?;
        let author = if author.truthy() {
            author.clone()
        } else {
            J::Obj(Obj::new())
        };
        let login = author.pget("login")?;
        let login = if login.truthy() {
            py::py_str(login)
        } else {
            String::new()
        };
        if JEPSON_LOGINS.contains(&login.to_lowercase().as_str())
            && py::py_str(r.pget("state")?).to_uppercase() == "CHANGES_REQUESTED"
        {
            out.push("jepsontaylor requested changes".to_string());
        }
    }
    Ok(out)
}

/// `jepson_text_hits(texts)`: `[(source, snippet)]` for non-reserved "waiting on Jepson" text.
pub fn jepson_text_hits(texts: &[(String, String)]) -> Vec<(String, String)> {
    let mut hits = Vec::new();
    for (src, text) in texts {
        let len = py::char_len(text);
        for m in JEPSON_TEXT_RE.find_iter(text) {
            let start = py::char_index(text, m.start());
            let end = py::char_index(text, m.end());
            let a = start.saturating_sub(160);
            let b = (end + 160).min(len);
            let snip = py::char_slice(text, a, b).replace('\n', " ");
            if RESERVED_RE.is_match(&snip) {
                continue;
            }
            hits.push((src.clone(), py::py_strip(&snip).to_string()));
            break;
        }
    }
    hits
}

/// `[u.get('login', '').lower() for u in users]`.
fn logins(users: &J) -> PyResult<Vec<String>> {
    let mut out = Vec::new();
    for u in iter_or_empty(users)? {
        out.push(py_lower(u.pget_or("login", &J::from(""))?)?);
    }
    Ok(out)
}

/// `jepson_protection_reasons(protection, codeowners_text)`.
pub fn jepson_protection_reasons(protection: &J, codeowners_text: &str) -> PyResult<Vec<String>> {
    let mut out = Vec::new();
    let prot = if protection.truthy() {
        protection.clone()
    } else {
        J::Obj(Obj::new())
    };
    let prr = prot.pget("required_pull_request_reviews")?;
    let prr = if prr.truthy() {
        prr.clone()
    } else {
        J::Obj(Obj::new())
    };
    let owners_jepson = py::py_splitlines(codeowners_text, false)
        .iter()
        .filter(|l| !py::py_strip(l).starts_with('#'))
        .any(|l| CODEOWNER_JEPSON.is_match(l));
    if prr.pget("require_code_owner_reviews")?.truthy() && owners_jepson {
        out.push("main requires CODEOWNER review and CODEOWNERS names @jepsontaylor".to_string());
    }
    let bypass = prr.pget("bypass_pull_request_allowances")?;
    let bypass = if bypass.truthy() {
        bypass.clone()
    } else {
        J::Obj(Obj::new())
    };
    let _users = logins(bypass.pget("users")?)?;
    let rest = prot.pget("restrictions")?;
    let rest = if rest.truthy() {
        rest.clone()
    } else {
        J::Obj(Obj::new())
    };
    let pushers = logins(rest.pget("users")?)?;
    if pushers == ["jepsontaylor"] {
        out.push("only jepsontaylor may push/merge to main (restrictions.users)".to_string());
    }
    Ok(out)
}

impl Audit<'_> {
    fn gh_repo(&self, project: &str) -> PyResult<String> {
        self.cfg.gh_repo(project).ok_or_else(|| PyErr::key(project))
    }

    /// `merged_since(project, since_iso)`: merged PR numbers since the baseline (None if gh fails).
    pub fn merged_since(&self, project: &str, since_iso: &str) -> PyResult<J> {
        let repo = self.gh_repo(project)?;
        let out = self.run(
            &[
                &self.cfg.gh,
                "pr",
                "list",
                "-R",
                &repo,
                "--state",
                "merged",
                "--json",
                "number,mergedAt",
                "-L",
                "20",
            ],
            30,
        );
        let Some(prs) = py::loads(&out.stdout) else {
            return Ok(J::Null);
        };
        let since = py::prefix(&since_iso.replace("+00:00", "Z"), 19).to_string();
        let mut numbers = Vec::new();
        for p in prs.py_iter()? {
            let merged = p.pget("mergedAt")?;
            let merged = if merged.truthy() {
                merged.clone()
            } else {
                J::from("")
            };
            if py::py_cmp(&merged, &J::from(since.as_str()))? != std::cmp::Ordering::Less {
                numbers.push(p.pidx("number")?.clone());
            }
        }
        Ok(J::Arr(numbers))
    }

    /// `open_prs(project)`.
    pub fn open_prs(&mut self, project: &str) -> PyResult<J> {
        let repo = self.gh_repo(project)?;
        let out = self.run(
            &[
                &self.cfg.gh,
                "pr",
                "list",
                "-R",
                &repo,
                "--state",
                "open",
                "--json",
                "number,title,headRefName,headRefOid,isDraft,reviewDecision,statusCheckRollup,updatedAt,createdAt,mergeable,reviewRequests,latestReviews",
                "-L",
                "20",
            ],
            30,
        );
        let Some(prs) = py::loads(&out.stdout) else {
            return Ok(J::Arr(Vec::new()));
        };
        let mut res = Vec::new();
        for p in prs.py_iter()? {
            let roll = p.pget("statusCheckRollup")?;
            let roll = if roll.truthy() {
                roll.clone()
            } else {
                J::Arr(Vec::new())
            };
            let mut concl: Vec<(String, i64)> = Vec::new();
            for c in roll.py_iter()? {
                let k = py_upper(&first_truthy(
                    &c,
                    &["conclusion", "state", "status"],
                    J::from("UNKNOWN"),
                )?)?;
                match concl.iter_mut().find(|(x, _)| *x == k) {
                    Some(slot) => slot.1 += 1,
                    None => concl.push((k, 1)),
                }
            }
            let failing: i64 = concl
                .iter()
                .filter(|(k, _)| {
                    matches!(
                        k.as_str(),
                        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
                    )
                })
                .map(|(_, v)| v)
                .sum();
            let pending: i64 = concl
                .iter()
                .filter(|(k, _)| {
                    matches!(
                        k.as_str(),
                        "PENDING" | "QUEUED" | "IN_PROGRESS" | "EXPECTED" | "WAITING"
                    )
                })
                .map(|(_, v)| v)
                .sum();
            let ci = if failing != 0 {
                "red"
            } else if pending != 0 || !roll.truthy() {
                "pending"
            } else {
                "green"
            };
            let checks = J::Obj(
                concl
                    .iter()
                    .map(|(k, v)| (k.clone(), J::from(*v)))
                    .collect(),
            );
            let number = py::py_str(p.pidx("number")?);
            let head = head9(p.pidx("headRefOid")?)?;
            res.push(crate::jobj! {
                "number" => p.pidx("number")?.clone(),
                "branch" => p.pidx("headRefName")?.clone(),
                "head" => head.as_str(),
                "draft" => p.pidx("isDraft")?.clone(),
                "review" => p.pget("reviewDecision")?.clone(),
                "ci" => ci,
                "checks" => checks.clone(),
            });
            let view = format!("gh pr view {number} -R {repo}");
            let checks_ev = format!("gh pr checks {number} -R {repo}");
            for why in jepson_pr_reasons(&p)? {
                let text =
                    format!("PR #{number} @ {head} is waiting on Jepson: {why}. {JEPSON_TAKEOVER}");
                self.ma("red", project, "JEPSON_BOTTLENECK", &text, Some(&view));
            }
            if ci == "red" {
                let text = format!(
                    "PR #{number} @ {head} has failing checks {}",
                    py::py_repr(&checks)
                );
                self.f("amber", project, "PR_CI_RED", &text, Some(&checks_ev));
            }
            let stuck = stuck_queued_checks(&roll, &self.host.now(), 30.0)?;
            let sp = stale_pr_problems(&p, &self.host.now(), STALE_PR_MIN)?;
            if !sp.is_empty() {
                let title = p.pget("title")?;
                let title = if title.truthy() {
                    py::py_str(title)
                } else {
                    String::new()
                };
                let text = format!(
                    "PR #{number} @ {head} ({}): {}. Must act this tick: {} lands it (exact-head approval + --rebase --match-head-commit), closes it with a comment if stale/superseded, \
or fixes the conflict/failure and records a NEW ETA. An open PR nobody is landing is a miss",
                    py::prefix(&title, 60),
                    sp.join("; "),
                    self.cfg.captain_or(project)
                );
                self.ma("red", project, "STALE_OPEN_PR", &text, Some(&view));
            }
            if !stuck.is_empty() && !p.pidx("isDraft")?.truthy() {
                let max_age = stuck.iter().map(|(_, a)| *a).max().unwrap_or(0);
                let listed: Vec<String> = stuck
                    .iter()
                    .take(6)
                    .map(|(n, a)| format!("{n} {a}m"))
                    .collect();
                let text = format!(
                    "PR #{number} @ {head}: {} check(s) queued with no runner for {max_age} min ({}). \
A queued check is not progress: find out why no runner picks it up (runs-on label vs available runners, GitHub-hosted minutes/billing for private repos) and fix it or decide the merge path this check; do not keep re-reporting an ETA",
                    stuck.len(),
                    listed.join(", ")
                );
                let sev = if P1_PROJECTS.contains(&project) {
                    "red"
                } else {
                    "amber"
                };
                self.f(sev, project, "PR_CI_QUEUED_STUCK", &text, Some(&checks_ev));
            }
        }
        Ok(J::Arr(res))
    }

    /// `main_ci(project)`.
    pub fn main_ci(&mut self, project: &str) -> PyResult<J> {
        let repo = self.gh_repo(project)?;
        let out = self.run(
            &[
                &self.cfg.gh,
                "run",
                "list",
                "-R",
                &repo,
                "--branch",
                "main",
                "-L",
                "6",
                "--json",
                "headSha,conclusion,status,workflowName,createdAt",
            ],
            30,
        );
        let Some(runs) = py::loads(&out.stdout) else {
            return Ok(J::Null);
        };
        if !runs.truthy() {
            return Ok(J::Null);
        }
        let first = match &runs {
            J::Arr(a) => a.first().cloned().unwrap_or_default(),
            J::Obj(_) => return Err(PyErr::new("KeyError", "0")),
            other => {
                return Err(PyErr::type_error(format!(
                    "'{}' object is not subscriptable",
                    other.type_name()
                )));
            }
        };
        let runs = runs.py_iter()?;
        let head = first.pidx("headSha")?.clone();
        let mut same = Vec::new();
        for r in &runs {
            if py::py_eq(r.pidx("headSha")?, &head) {
                same.push(r.clone());
            }
        }
        let mut bad = Vec::new();
        for r in &same {
            let c = r.pget("conclusion")?;
            if ["failure", "cancelled", "timed_out"]
                .iter()
                .any(|f| py::eq_str(c, f))
            {
                bad.push(r.pidx("workflowName")?.clone());
            }
        }
        if !bad.is_empty() {
            let now = self.host.now();
            let (sev, code, text) =
                main_ci_severity(self.cfg, project, &head, &J::Arr(bad), &runs, &now)?;
            self.f(sev, project, code, &text, None);
        }
        let mut listed = Vec::new();
        for r in &same {
            let c = first_truthy(r, &["conclusion"], J::Null)?;
            let c = if c.truthy() {
                c
            } else {
                r.pget("status")?.clone()
            };
            listed.push(J::Arr(vec![r.pidx("workflowName")?.clone(), c]));
        }
        Ok(crate::jobj! {"head" => head9(&head)?, "runs" => J::Arr(listed)})
    }

    /// `last_push(project)`: newest push/merge/branch activity.
    pub fn last_push(&self, project: &str) -> PyResult<Option<Dt>> {
        let repo = self.gh_repo(project)?;
        let path = format!("repos/{repo}/activity?per_page=10");
        let out = self.run(&[&self.cfg.gh, "api", &path], 30);
        let Some(acts) = py::loads(&out.stdout) else {
            return Ok(None);
        };
        let mut best: Option<Dt> = None;
        for a in acts.py_iter()? {
            let kind = a.pget("activity_type")?;
            if [
                "push",
                "force_push",
                "pr_merge",
                "merge_queue_merge",
                "branch_creation",
            ]
            .iter()
            .any(|k| py::eq_str(kind, k))
                && let Some(t) = py::parse_utc(a.pget("timestamp")?)
                && best.is_none_or(|b| t > b)
            {
                best = Some(t);
            }
        }
        Ok(best)
    }

    /// `jepson_bottleneck(report, scs, no_gh)`.
    pub fn jepson_bottleneck(&mut self, report: &Obj, scs: &J, no_gh: bool) -> PyResult<()> {
        let mut texts = vec![(
            "super_captain_status.json".to_string(),
            py::dumps(scs, None),
        )];
        if let Some(J::Obj(projects)) = report.get("projects") {
            for (proj, r) in projects.iter() {
                let br = r.pget("blocked_reasons")?;
                if br.truthy() {
                    texts.push((format!("{proj} blocked_reasons"), py::dumps(br, None)));
                }
            }
        }
        for (src, snip) in jepson_text_hits(&texts) {
            let text = format!(
                "\"waiting on Jepson\" text in {src}: ...{}... {JEPSON_TAKEOVER}",
                py::prefix(&snip, 260)
            );
            self.ma(
                "red",
                "super_captain",
                "JEPSON_BOTTLENECK",
                &text,
                Some(&src),
            );
        }
        if no_gh {
            return Ok(());
        }
        let projects: Vec<(String, String)> = self
            .cfg
            .projects
            .iter()
            .map(|p| (p.name.clone(), p.gh_repo.clone()))
            .collect();
        for (proj, repo) in projects {
            let path = format!("repos/{repo}/branches/main/protection");
            let out = self.run(&[&self.cfg.gh, "api", &path], 20);
            let prot = if out.rc == 0 {
                py::loads(&out.stdout).unwrap_or_else(|| J::Obj(Obj::new()))
            } else {
                J::Obj(Obj::new())
            };
            let mut co = String::new();
            for cpath in [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"] {
                let api = format!("repos/{repo}/contents/{cpath}");
                let o2 = self.run(
                    &[
                        &self.cfg.gh,
                        "api",
                        &api,
                        "-H",
                        "Accept: application/vnd.github.raw",
                    ],
                    20,
                );
                if o2.rc == 0
                    && !o2.stdout.is_empty()
                    && !py::py_lstrip(&o2.stdout).starts_with('{')
                {
                    co.push_str(&o2.stdout);
                    co.push('\n');
                }
            }
            for why in jepson_protection_reasons(&prot, &co)? {
                let text = format!(
                    "{repo}: {why}. super_captain may change it so neverhumanbot approval suffices (back up the protection JSON first; never allow force-push to main)"
                );
                self.ma(
                    "red",
                    &proj,
                    "JEPSON_BOTTLENECK",
                    &text,
                    Some(&path_ev(&repo)),
                );
            }
        }
        Ok(())
    }
}

fn path_ev(repo: &str) -> String {
    format!("gh api repos/{repo}/branches/main/protection")
}
