//! JopeDime: spec-integration verify receipts (`audit_jopedime`) and the
//! 2026-10-18 major-release tracker (`audit_jopedime_major_release`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::{
    Buckets, any_py, concat_lists, get_str_default, get_str_or_empty, is_hex, iter_or_empty,
};
use crate::ctx::{
    Audit, glob_names, rj, rj_or_empty, sha256_file, star_match, tail_text, two_digit_json,
};
use crate::py::{self, Dt, J, Obj, PyErr, PyResult};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

/// `EXEC`: an executed command (not a document/keyword match).
pub static EXEC: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b(cargo|nextest|pytest|just|npm|pnpm|make|ctest|uv run|go test|gh run|gh api|bash\s+\S+\.sh|\./\S+)\b|\bpython3?\s+(-m\s+\w+|[\w/.-]+\.py)\b",
    )
});
/// `MEASURE`: criteria that need a real measurement/run.
pub static MEASURE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(benchmark\w*|experiment run\w*|real (run|measurement|workload)|measured (reduction|saving|improvement|speedup|information/byte|cost|bytes?)|cost (reduction|saving|improvement)|cheaper|wall.?clock|on (the )?GPU)\b",
    )
});
/// `EXEC_CRIT`: criteria that need executed evidence.
pub static EXEC_CRIT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(review|CI|exact.head|provenance|receiving|merge|run|runs|tested|tests?|measured|executes?|hash|sha256|receipt)\b",
    )
});
static SHA40_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\b[0-9a-f]{40}\b"));
static SHA256_FIELD: LazyLock<Regex> = LazyLock::new(|| re(r"\A(sha256:)?[0-9a-f]{64}\z"));
static EVIDENCE_SHA: LazyLock<Regex> =
    LazyLock::new(|| re(r"work-packets/evidence/I\d+/([0-9a-f]{40})"));
static EVERY_EXACT: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(each|every|all)\b[^.]*--exact"));
static STATUS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"STATUS \| implemented=(\d+)/\d+ integrated_open_pr=(\d+) on_main_ci_green=(\d+) acceptance_verified=(\d+) done=(\d+)",
    )
});

fn sha40_set(text: &str) -> BTreeSet<String> {
    SHA40_WORD
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// `' '.join(str(c.get(k, '')) for k in ('command', 'test_or_code'))`.
fn case_text(c: &J) -> PyResult<String> {
    Ok(format!(
        "{} {}",
        get_str_default(c, "command")?,
        get_str_default(c, "test_or_code")?
    ))
}

/// `set(x)` of a JSON list used for string-set comparisons (unhashable items raise).
fn str_set_of(items: &[J]) -> PyResult<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for x in items {
        match x {
            J::Str(s) => {
                out.insert(s.clone());
            }
            J::Arr(_) | J::Obj(_) => {
                return Err(PyErr::type_error(format!(
                    "unhashable type: '{}'",
                    x.type_name()
                )));
            }
            // a non-string never equals a string sha
            _ => {}
        }
    }
    Ok(out)
}

/// `[str(x).strip() for x in (spec.get('acceptance_cases') or []) if str(x).strip()]`.
fn expected_cases(spec: &J) -> PyResult<Vec<String>> {
    Ok(iter_or_empty(spec.pget("acceptance_cases")?)?
        .iter()
        .map(|x| py::py_strip(&py::py_str(x)).to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// `{t['id']: t for t in tasks if isinstance(t, dict) and t.get('id')}` (keys via `str()`).
fn task_map(list: &J) -> PyResult<Vec<(String, J)>> {
    let mut out: Vec<(String, J)> = Vec::new();
    for t in iter_or_empty(list)? {
        if !matches!(t, J::Obj(_)) || !t.pget("id")?.truthy() {
            continue;
        }
        let key = py::py_str(t.pidx("id")?);
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = t.clone(),
            None => out.push((key, t.clone())),
        }
    }
    Ok(out)
}

fn lookup<'a>(map: &'a [(String, J)], key: &str) -> Option<&'a J> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn file_tid(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.strip_suffix(".json").unwrap_or(base).to_string()
}

/// `hashlib.sha256(open(lp, 'rb').read()).hexdigest() != hs` treating unreadable files as mismatch.
fn log_hash_differs(lp: &J, hs: &str) -> PyResult<bool> {
    match lp {
        J::Str(p) => Ok(sha256_file(p).is_none_or(|h| h != hs)),
        J::Int(_) | J::Bool(_) => Ok(true),
        other => Err(PyErr::type_error(format!(
            "expected str, bytes or os.PathLike object, not {}",
            other.type_name()
        ))),
    }
}

fn receipt_time(r: &J) -> PyResult<Option<Dt>> {
    let v = r.pget("verified_at_utc")?;
    let v = if v.truthy() {
        v
    } else {
        r.pget("verified_at")?
    };
    Ok(py::parse_utc(v))
}

impl Audit<'_> {
    /// `audit_jopedime_major_release(repo, main)`.
    pub fn audit_jopedime_major_release(&mut self, repo: &str, main: Option<&str>) -> PyResult<J> {
        let je_mr = self.cfg.je_mr.clone();
        let candidates = [
            format!("{repo}/docs/plans/major-release-20261018/MAJOR_RELEASE_TRACKER.json"),
            format!("{je_mr}/tasks.json"),
            format!(
                "{}/jopedime_major_release_20261018/MAJOR_RELEASE_TRACKER.json",
                self.cfg.kick
            ),
        ];
        let tpath = candidates
            .iter()
            .find(|p| crate::ctx::is_file(p))
            .unwrap_or(&candidates[0])
            .clone();
        let tdata = rj_or_empty(&tpath);
        let vd = tdata.pget("verify_dir")?;
        let vdir = if vd.truthy() {
            py::py_str(vd)
        } else {
            format!("{je_mr}/verify")
        };
        let tmap = task_map(tdata.pget("tasks")?)?;
        let req_task_keys = [
            "id",
            "owner_screen",
            "deps",
            "gate",
            "exact_acceptance_test",
            "receipt_format",
        ];
        let mut bad = Buckets::default();
        for (tid, t) in &tmap {
            let mut incomplete = false;
            for k in req_task_keys {
                let v = t.pget(k)?;
                if !t.has_key(k)? || v.is_null() || py::eq_str(v, "") {
                    incomplete = true;
                    break;
                }
            }
            if incomplete {
                bad.push("schema_incomplete_task", tid.clone());
            }
        }
        let files = glob_names(&vdir, |n| two_digit_json(n, "MR"));
        let mut raw_pass: Vec<(String, String)> = Vec::new();
        let main_truthy = main.filter(|m| !m.is_empty());
        for f in &files {
            let tid = file_tid(f);
            let Some(r) = rj(f) else {
                bad.push("unreadable", tid);
                continue;
            };
            let st =
                py::py_str(&r.pget_or("status", &J::Str(String::new()))?.clone()).to_uppercase();
            let sha = get_str_or_empty(&r, "sha")?;
            let csha = get_str_or_empty(&r, "checkout_sha")?;
            if !is_hex(&sha, 40) || !is_hex(&csha, 40) {
                bad.push("truncated_or_bad_sha", tid);
                continue;
            }
            if sha != csha {
                bad.push("sha_mismatch", tid);
                continue;
            }
            if st != "PASS" {
                continue;
            }
            let va = receipt_time(&r)?;
            let ct = self.commit_time(repo, &sha);
            if let (Some(va), Some(ct)) = (va, ct)
                && va < ct
            {
                bad.push("receipt_predates_sha", tid);
                continue;
            }
            if let Some(m) = main_truthy
                && sha != m
            {
                bad.push("sha_not_current_main", tid);
                continue;
            }
            let t_spec = lookup(&tmap, &tid)
                .filter(|t| t.truthy())
                .cloned()
                .unwrap_or(J::Obj(Obj::new()));
            let exp_cmd =
                py::py_strip(&get_str_or_empty(&t_spec, "exact_acceptance_test")?).to_string();
            let exp_ac = expected_cases(&t_spec)?;
            let cases_j = {
                let a = r.pget("case_to_test_map")?;
                if a.truthy() {
                    a.clone()
                } else {
                    r.pget("cases")?.clone()
                }
            };
            let cases = iter_or_empty(&cases_j)?;
            let nonzero = cases.is_empty()
                || any_py(&cases, |c| {
                    Ok(!py::eq_int(c.pget("exit_code")?, 0)
                        || matches!(c.pget("passed")?, J::Bool(false)))
                })?;
            if nonzero {
                bad.push("pass_with_nonzero_exit", tid);
                continue;
            }
            let mut act_ac = Vec::new();
            let mut act_cmds = Vec::new();
            for c in &cases {
                act_ac.push(py::py_strip(&get_str_or_empty(c, "acceptance_case")?).to_string());
            }
            for c in &cases {
                act_cmds.push(py::py_strip(&get_str_or_empty(c, "command")?).to_string());
            }
            let act_set: BTreeSet<&String> = act_ac.iter().collect();
            let unmapped_cases = !exp_ac.is_empty() && !exp_ac.iter().all(|e| act_set.contains(e));
            let unmapped_cmd =
                !exp_cmd.is_empty() && !act_cmds.iter().any(|c| c.contains(exp_cmd.as_str()));
            if unmapped_cases || unmapped_cmd {
                bad.push("unmapped_exact_acceptance_test", tid);
                continue;
            }
            if !act_cmds
                .iter()
                .filter(|c| !c.is_empty())
                .all(|c| EXEC.is_match(c))
            {
                bad.push("non_executable_command", tid);
                continue;
            }
            let mut lbad = false;
            for c in &cases {
                let lp = c.pget("log_path")?;
                let hs = c.pget("log_sha256")?;
                if !lp.truthy() || !hs.truthy() || !is_hex(&py::py_str(hs), 64) {
                    lbad = true;
                    break;
                }
                if log_hash_differs(lp, &py::py_str(hs))? {
                    lbad = true;
                    break;
                }
            }
            if lbad {
                bad.push("log_sha256_mismatch", tid);
                continue;
            }
            match raw_pass.iter_mut().find(|(k, _)| *k == tid) {
                Some(slot) => slot.1 = sha,
                None => raw_pass.push((tid, sha)),
            }
        }
        raw_pass.sort();
        let mut ok_pass = Vec::new();
        let mut on_main_pass = Vec::new();
        for (tid, sha) in &raw_pass {
            let spec = lookup(&tmap, tid)
                .filter(|t| t.truthy())
                .cloned()
                .unwrap_or(J::Obj(Obj::new()));
            let deps = iter_or_empty(spec.pget("deps")?)?;
            let mut missing_dep = false;
            for d in &deps {
                if matches!(d, J::Arr(_) | J::Obj(_)) {
                    return Err(PyErr::type_error(format!(
                        "unhashable type: '{}'",
                        d.type_name()
                    )));
                }
                if !matches!(d, J::Str(s) if raw_pass.iter().any(|(k, _)| k == s)) {
                    missing_dep = true;
                    break;
                }
            }
            if missing_dep {
                bad.push("deps_not_passed", tid.clone());
                continue;
            }
            ok_pass.push(tid.clone());
            if main_truthy.is_some_and(|m| m == sha) {
                on_main_pass.push(tid.clone());
            }
        }
        for (k, ids) in bad.iter() {
            let text = format!(
                "major-release tracker {k}: {}",
                py::sorted_set(ids.iter().cloned()).join(" ")
            );
            self.f(
                "red",
                "jopedime",
                &format!("MR_{}", k.to_uppercase()),
                &text,
                Some(&vdir),
            );
        }
        Ok(crate::jobj! {
            "source" => tpath,
            "verify_dir" => vdir,
            "total" => tmap.len(),
            "receipts" => files.len(),
            "genuine_pass" => ok_pass.len(),
            "genuine_pass_on_main" => on_main_pass.len(),
            "done_recomputed" => on_main_pass.len(),
            "suspect" => bad.to_json_sorted(),
        })
    }

    /// `audit_jopedime(sc_claims)`.
    pub fn audit_jopedime(&mut self) -> PyResult<J> {
        let je = self.cfg.je.clone();
        let mut res = Obj::new();
        res.set("source", format!("{je}/verify/*.json"));
        let vdir = format!("{je}/verify");
        let repo = self.cfg.repo("jopedime");
        let first = self.rev(&repo, "origin/main");
        let main = if first.as_deref().is_some_and(|s| !s.is_empty()) {
            first
        } else {
            self.rev(&repo, "main")
        };
        res.set("main_sha", main.clone());
        let main_truthy = main.as_deref().filter(|m| !m.is_empty());
        let files = glob_names(&vdir, |n| two_digit_json(n, "I"));
        let tasks = rj_or_empty(&format!("{je}/packet-v4-921643bc3ae0/tasks.json"));
        let tmap = task_map(tasks.pget("tasks")?)?;
        let total = if tmap.is_empty() { 66 } else { tmap.len() };
        res.set("total", total);
        res.set("receipts", files.len());
        let present: BTreeSet<String> = files.iter().map(|f| file_tid(f)).collect();
        let missing: Vec<String> = py::sorted_set(
            tmap.iter()
                .map(|(k, _)| k.clone())
                .filter(|k| !present.contains(k)),
        );
        if !missing.is_empty() {
            let shown: Vec<&str> = missing.iter().take(20).map(String::as_str).collect();
            let text = format!(
                "{} verify receipts missing: {}",
                missing.len(),
                shown.join(" ")
            );
            self.f("amber", "jopedime", "RECEIPT_MISSING", &text, Some(&vdir));
        }
        let mut ok_pass: Vec<String> = Vec::new();
        let mut on_main_pass: Vec<String> = Vec::new();
        let mut bad = Buckets::default();
        for f in &files {
            let tid = file_tid(f);
            let Some(r) = rj(f) else {
                bad.push("unreadable", tid);
                continue;
            };
            let st = py::py_str(r.pget_or("status", &J::Str(String::new()))?).to_uppercase();
            let sha = get_str_or_empty(&r, "sha")?;
            let csha = get_str_or_empty(&r, "checkout_sha")?;
            if !is_hex(&sha, 40) || (!csha.is_empty() && !is_hex(&csha, 40)) {
                bad.push("truncated_or_bad_sha", tid.clone());
            }
            let mism = !csha.is_empty() && !sha.is_empty() && csha != sha;
            if mism {
                bad.push("sha_mismatch", tid.clone());
            }
            if let J::Obj(o) = &r {
                for (k, v) in o.iter() {
                    if k.to_lowercase().contains("sha256")
                        && let J::Str(s) = v
                        && !SHA256_FIELD.is_match(s)
                    {
                        bad.push("truncated_sha256", tid.clone());
                    }
                }
            }
            if st != "PASS" || mism {
                continue;
            }
            let va = receipt_time(&r)?;
            let ct = if is_hex(&sha, 40) {
                self.commit_time(&repo, &sha)
            } else {
                None
            };
            if let (Some(va), Some(ct)) = (va, ct)
                && va < ct
            {
                bad.push("receipt_predates_sha", tid);
                continue;
            }
            let proof = get_str_default(&r, "completion_criterion_proof")?;
            let proof_shas = sha40_set(&proof);
            if proof_shas.iter().any(|s| *s != sha) && !proof.contains(sha.as_str()) {
                bad.push("proof_names_other_sha", tid);
                continue;
            }
            let cases_j = {
                let a = r.pget("case_to_test_map")?;
                if a.truthy() {
                    a.clone()
                } else {
                    r.pget("cases")?.clone()
                }
            };
            let cases_j = if cases_j.truthy() {
                cases_j
            } else {
                J::Arr(Vec::new())
            };
            let cmds_j = {
                let c = r.pget("commands")?;
                if c.truthy() {
                    c.clone()
                } else {
                    J::Arr(Vec::new())
                }
            };
            let cmds_cases = concat_lists(&cmds_j, &cases_j)?;
            let cases = cases_j.py_iter()?;
            let cmds = cmds_j.py_iter()?;
            let nonzero =
                any_py(&cmds_cases, |c| {
                    let ec = c.pget("exit_code")?;
                    Ok(!(py::eq_int(ec, 0) || ec.is_null()))
                })? || any_py(&cases, |c| Ok(matches!(c.pget("passed")?, J::Bool(false))))?;
            if nonzero {
                bad.push("pass_with_nonzero_exit", tid);
                continue;
            }
            let mut texts = Vec::new();
            for c in &cases {
                texts.push(case_text(c)?);
            }
            if texts.is_empty() {
                for c in &cmds {
                    texts.push(get_str_default(c, "command")?);
                }
            }
            let doc = texts
                .iter()
                .filter(|t| t.to_lowercase().contains("documentary") || !EXEC.is_match(t))
                .count();
            let crit = get_str_default(&r, "completion_criterion")?;
            if get_str_default(&r, "completion_criterion_proof")?
                .to_lowercase()
                .contains("documentary")
                || (!texts.is_empty() && doc == texts.len())
            {
                let exec_crit = EXEC_CRIT.is_match(&crit);
                bad.push(
                    if exec_crit {
                        "pass_on_doc_keyword_match"
                    } else {
                        "pass_doc_only_doc_criterion"
                    },
                    tid.clone(),
                );
                if exec_crit {
                    continue;
                }
            }
            if let Some(m) = main_truthy
                && sha != m
            {
                bad.push("sha_not_current_main", tid);
                continue;
            }
            let t_spec = lookup(&tmap, &tid)
                .filter(|t| t.truthy())
                .cloned()
                .unwrap_or(J::Obj(Obj::new()));
            let exp_crit =
                py::py_strip(&get_str_or_empty(&t_spec, "completion_criterion")?).to_string();
            let exp_ac = expected_cases(&t_spec)?;
            let mut act_ac = Vec::new();
            for c in &cases {
                act_ac.push(py::py_strip(&get_str_or_empty(c, "acceptance_case")?).to_string());
            }
            let act_set: BTreeSet<&String> = act_ac.iter().collect();
            let crit_s = py::py_strip(&crit);
            if crit_s.is_empty()
                || (!exp_crit.is_empty() && crit_s != exp_crit)
                || act_ac.iter().any(String::is_empty)
                || (!exp_ac.is_empty() && !exp_ac.iter().all(|e| act_set.contains(e)))
            {
                bad.push("empty_or_unmapped_criterion_cases", tid);
                continue;
            }
            if cases_j.py_len()? == 0 {
                bad.push("pass_without_cases", tid);
                continue;
            }
            let mut exec_cases = 0;
            for c in &cases {
                if EXEC.is_match(&case_text(c)?) {
                    exec_cases += 1;
                }
            }
            if exec_cases > 0 {
                let has_log = any_py(&cmds_cases_order(&cases, &cmds), |c| {
                    Ok(c.pget("log_path")?.truthy() && c.pget("log_sha256")?.truthy())
                })?;
                if !has_log {
                    bad.push("relabeled_without_log", tid);
                    continue;
                }
            }
            let raw_r = py::dumps(&r, None);
            let ev_shas: BTreeSet<String> = EVIDENCE_SHA
                .captures_iter(&raw_r)
                .filter_map(|m| m.get(1).map(|g| g.as_str().to_string()))
                .filter(|s| *s != sha)
                .collect();
            let prior_ev = r.pget("prior_evidence_sha")?;
            let prior_set: BTreeSet<String> = match prior_ev {
                J::Arr(items) => str_set_of(items)?,
                other if other.truthy() => [py::py_str(other)].into_iter().collect(),
                _ => BTreeSet::new(),
            };
            let only_main: BTreeSet<String> = [sha.clone()].into_iter().collect();
            let mut rewritten =
                !ev_shas.is_empty() && proof_shas == only_main && !ev_shas.is_subset(&prior_set);
            let relabeled_pattern = format!("{tid}.relabeled*.json");
            for rf in glob_names(&vdir, |n| star_match(n, &relabeled_pattern)) {
                let rr = rj_or_empty(&rf);
                let rproof = get_str_or_empty(&rr, "completion_criterion_proof")?;
                let rshas: BTreeSet<String> = sha40_set(&rproof)
                    .into_iter()
                    .filter(|s| *s != sha)
                    .collect();
                let allowed: BTreeSet<String> = proof_shas.union(&prior_set).cloned().collect();
                if !rshas.is_empty() && !rshas.is_subset(&allowed) {
                    rewritten = true;
                }
            }
            if rewritten {
                bad.push("rewritten_proof_sha", tid.clone());
            }
            let has_measure_key = r
                .as_obj()
                .is_some_and(|o| o.keys().any(|k| k.to_lowercase().contains("measure")));
            if MEASURE.is_match(&crit)
                && !has_measure_key
                && !raw_r.to_lowercase().contains("measurement")
            {
                bad.push("measurement_criterion_unit_tests_only", tid);
                continue;
            }
            let mut allcmds = Vec::new();
            for c in &cmds_cases_order(&cases, &cmds) {
                if c.pget("command")?.truthy() {
                    allcmds.push(get_str_default(c, "command")?);
                }
            }
            if EVERY_EXACT.is_match(&proof)
                && allcmds
                    .iter()
                    .any(|c| c.contains("cargo test") && !c.contains("--exact"))
            {
                bad.push("proof_claim_false_exact", tid);
                continue;
            }
            let reused = any_py(&cases, |c| {
                if !c.pget("reused_existing_execution")?.truthy() {
                    return Ok(false);
                }
                let sc = get_str_or_empty(c, "source_commit")?;
                let sc7 = py::prefix(&sc, 7).to_string();
                if sc7.is_empty() {
                    return Ok(false);
                }
                let full = py::py_str(c.pget("source_commit")?);
                Ok(!sha.starts_with(py::prefix(&full, 7)))
            })?;
            if reused {
                bad.push("case_reused_other_commit", tid.clone());
            }
            let fail_pattern = format!("{tid}.*fail*.json");
            for ff in glob_names(&vdir, |n| star_match(n, &fail_pattern)) {
                let fr = rj_or_empty(&ff);
                if get_str_or_empty(&fr, "sha")? != sha
                    || py::py_str(fr.pget_or("status", &J::Str(String::new()))?).to_uppercase()
                        == "PASS"
                {
                    continue;
                }
                let fva = receipt_time(&fr)?;
                if let (Some(va), Some(fva)) = (va, fva)
                    && fva > va
                {
                    continue;
                }
                let fcases = {
                    let a = fr.pget("case_to_test_map")?;
                    if a.truthy() {
                        a.clone()
                    } else {
                        fr.pget("cases")?.clone()
                    }
                };
                let mut fcmds = Vec::new();
                for c in iter_or_empty(&fcases)? {
                    fcmds.push(get_str_default(&c, "command")?);
                }
                let mut pcmds = Vec::new();
                for c in &cases {
                    pcmds.push(get_str_default(c, "command")?);
                }
                if fcmds != pcmds {
                    bad.push("pass_after_fail_cmd_changed", tid.clone());
                } else {
                    let label = format!(
                        "{tid}({}->{})",
                        py::py_str(fr.pget_or("host", &J::from("?"))?),
                        py::py_str(r.pget_or("host", &J::from("?"))?)
                    );
                    bad.push("pass_after_fail_same_cmd", label);
                }
                break;
            }
            let mut lbad = false;
            for c in &cases {
                let lp = c.pget("log_path")?;
                let hs = c.pget("log_sha256")?;
                if lp.truthy()
                    && hs.truthy()
                    && is_hex(&py::py_str(hs), 64)
                    && log_hash_differs(lp, &py::py_str(hs))?
                {
                    lbad = true;
                }
            }
            if lbad {
                bad.push("log_sha256_mismatch", tid);
                continue;
            }
            ok_pass.push(tid.clone());
            if main_truthy.is_some_and(|m| m == sha) {
                on_main_pass.push(tid);
            }
        }
        for (k, ids) in bad.iter() {
            let (sev, txt) = label(k);
            let text = format!("{txt}: {}", py::sorted_set(ids.iter().cloned()).join(" "));
            self.f(
                sev,
                "jopedime",
                &format!("RECEIPT_{}", k.to_uppercase()),
                &text,
                Some(&vdir),
            );
        }
        res.set("genuine_pass", ok_pass.len());
        res.set("genuine_pass_on_main", on_main_pass.len());
        res.set("suspect", bad.to_json_sorted());
        let log = self.cfg.log("jopedime");
        let log_text = tail_text(&log, 400_000);
        if let Some(m) = STATUS_LINE.captures_iter(&log_text).last() {
            let g = |i: usize| m.get(i).and_then(|x| py::py_int(x.as_str())).unwrap_or(0);
            res.set(
                "captain_claim",
                crate::jobj! {"implemented" => g(1), "acceptance_verified" => g(4), "done" => g(5)},
            );
            if g(5) > on_main_pass.len() as i64 {
                let short = py::prefix(&py::py_str(&J::from(main.clone())), 9).to_string();
                let text = format!(
                    "captain STATUS says done={} but only {} receipts are a genuine PASS on a sha contained in main {short}",
                    g(5),
                    on_main_pass.len()
                );
                self.f("red", "jopedime", "COUNT_OVERCLAIM", &text, Some(&log));
            }
            if g(4) > ok_pass.len() as i64 {
                let text = format!(
                    "captain STATUS says acceptance_verified={} but audit finds {} genuine PASS receipts",
                    g(4),
                    ok_pass.len()
                );
                self.f(
                    "amber",
                    "jopedime",
                    "VERIFIED_OVERCLAIM",
                    &text,
                    Some(&vdir),
                );
            }
        }
        res.set("done_recomputed", on_main_pass.len());
        let mr = self.audit_jopedime_major_release(&repo, main.as_deref())?;
        res.set("major_release", mr);
        Ok(J::Obj(res))
    }
}

/// `cases + cmds` (both already validated as lists).
fn cmds_cases_order(cases: &[J], cmds: &[J]) -> Vec<J> {
    cases.iter().chain(cmds).cloned().collect()
}

fn label(k: &str) -> (&'static str, &'static str) {
    match k {
        "truncated_or_bad_sha" => (
            "red",
            "PASS/receipt carries a truncated or malformed git sha",
        ),
        "sha_mismatch" => (
            "red",
            "sha != checkout_sha (verified a different commit than the receipt names; not counted)",
        ),
        "truncated_sha256" => ("red", "sha256 field is truncated"),
        "pass_with_nonzero_exit" => (
            "red",
            "status PASS but a command/case has nonzero exit or passed=false",
        ),
        "pass_on_doc_keyword_match" => (
            "red",
            "status PASS but evidence is a keyword/doc match while the criterion needs executed evidence (review/CI/run/provenance)",
        ),
        "pass_doc_only_doc_criterion" => (
            "info",
            "PASS on doc text only (criterion is itself a document deliverable; spot-check)",
        ),
        "measurement_criterion_unit_tests_only" => (
            "red",
            "criterion needs a real measurement/run or explicit hypothesis qualification; receipt shows unit tests only (not counted)",
        ),
        "sha_not_current_main" => (
            "red",
            "receipt sha is an older ancestor commit, not current main (not counted)",
        ),
        "empty_or_unmapped_criterion_cases" => (
            "red",
            "empty completion_criterion or blank/unmapped/reworded acceptance_case vs packet-v4 tasks.json (not counted)",
        ),
        "relabeled_without_log" => (
            "red",
            "receipt edited by script (finalize_*) without a new log_path and matching log_sha256 (relabeled; not counted)",
        ),
        "rewritten_proof_sha" => (
            "amber",
            "completion_criterion_proof rewrote prior commit SHAs to main_sha without preserving prior_evidence_sha",
        ),
        "pass_without_cases" => ("red", "status PASS with no acceptance cases mapped"),
        "receipt_predates_sha" => (
            "red",
            "verified_at is older than the commit the receipt names (relabeled to a newer sha, not re-run; not counted)",
        ),
        "proof_names_other_sha" => (
            "red",
            "proof says the tests ran at a different full sha than the receipt names (not counted)",
        ),
        "log_sha256_mismatch" => (
            "red",
            "a case log is missing or no longer matches the log_sha256 in the receipt (overwritten; not counted)",
        ),
        "proof_claim_false_exact" => (
            "red",
            "proof says every command uses --exact but a cargo test command does not (substring filter may run other tests; not counted)",
        ),
        "case_reused_other_commit" => (
            "amber",
            "a case reuses an execution from a different commit instead of re-running on the named sha (spot-check)",
        ),
        "pass_after_fail_cmd_changed" => (
            "amber",
            "PASS follows a FAIL on the same sha with changed commands (retry-to-green; confirm the change is a legit fix, not a weakened check)",
        ),
        "pass_after_fail_same_cmd" => (
            "amber",
            "PASS follows a FAIL on the same sha with identical commands (flaky or host-dependent, failing->passing host shown; root-cause before it counts toward the xbabe1-4 order)",
        ),
        _ => ("amber", "receipt JSON unreadable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_regex_examples() {
        assert!(EXEC.is_match("cargo test -p x"));
        assert!(EXEC.is_match("python3 -m unittest t"));
        assert!(EXEC.is_match("python tools/x.py"));
        assert!(!EXEC.is_match("see docs/SPEC.md section 3"));
        assert!(EXEC_CRIT.is_match("independent review on the exact head"));
        assert!(MEASURE.is_match("measured speedup on the GPU"));
    }
}
