//! DOPE per-task receipts (super_captain request 2026-10-18 ~06:00 MDT).
//!
//! A DOPE task counts as done only when its own receipt proves it: receipt file
//! exists (and matches the `#sha256` pinned in tasks.json), status passed,
//! integer exit code(s) == 0 (never prose), every command has a log_path that
//! exists with a matching log_sha256, checkout sha == receipt sha (40-hex) and
//! that sha is on origin/main, and the receipt is this task's own.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use super::{is_hex, iter_or_empty};
use crate::ctx::{Audit, is_file, rj, rj_or_empty, sha256_file, tail_text};
use crate::host::read_text_replace;
use crate::py::{self, Dt, J, Obj, PyErr, PyResult, ShaPredicate, TimeLookup};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

/// `DOPE_VERIFY_REF`.
static DOPE_VERIFY_REF: LazyLock<Regex> =
    LazyLock::new(|| re(r"(/\S+?/verify/\S+?\.json)(?:#sha256=([0-9a-f]{64}))?\s*\z"));
/// Same command proving >= this many tasks is reported (amber) as generic evidence.
pub const DOPE_GENERIC_CMD_MIN: usize = 5;
static DOPE_CONTRADICTORY_SUBSTATUS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)blocked|partial|fail"));
static AWAITING: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)awaiting|pending"));
static CARGO_RESULT: LazyLock<Regex> =
    LazyLock::new(|| re(r"test result: \w+\. (\d+) passed; (\d+) failed"));
static UNITTEST_RAN: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^Ran (\d+) tests? in"));
static PYTEST_NONE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^(no tests ran|collected 0 items)\b"));
static LOG_HEAD_SHA: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^head_sha:\s*([0-9a-f]{40})\s*$"));
static LOG_EXIT: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^exit_code:\s*(-?\d+)\s*$"));
static ENV_ASSIGN: LazyLock<Regex> = LazyLock::new(|| re(r"\A[A-Za-z_][A-Za-z0-9_]*=\S*\z"));
static WRAPPER_ARG: LazyLock<Regex> = LazyLock::new(|| re(r"\A[0-9][0-9,.\-]*[smhd]?\z"));
static LOG_SHA_FIELD: LazyLock<Regex> = LazyLock::new(|| re(r"\A(sha256:)?[0-9a-f]{64}\z"));

/// `DOPE_PROBLEMS`.
pub fn dope_problem_text(code: &str) -> Option<&'static str> {
    Some(match code {
        "no_receipt" => {
            "no verify receipt referenced in tasks.json evidence (prose/typed evidence only)"
        }
        "receipt_missing" => "referenced receipt file does not exist",
        "receipt_unreadable" => "receipt JSON unreadable",
        "receipt_sha256_mismatch" => {
            "receipt file no longer matches the #sha256 pinned in tasks.json (edited after pinning)"
        }
        "receipt_not_for_task" => "receipt names a different task_id",
        "shared_receipt_no_task_entry" => {
            "receipt is shared by several tasks and has no per-task entry (with its own log) for this task"
        }
        "status_not_passed" => "receipt status is not passed",
        "contradictory_sub_status" => {
            "ledger sub_status explicitly says blocked, partial or failed despite status=done"
        }
        "bad_or_missing_sha" => "receipt sha missing, truncated or malformed (need 40-hex)",
        "no_commands" => "receipt lists no executed commands",
        "exit_code_not_integer" => "exit code missing or typed as text/prose instead of an integer",
        "nonzero_exit" => "a command exited nonzero",
        "no_log_path" => "a command has no log_path",
        "log_missing" => "log_path does not exist",
        "no_log_sha256" => "log_sha256 missing or malformed",
        "log_sha256_mismatch" => "log file does not match log_sha256 (overwritten or edited)",
        "log_names_other_sha" => "log header names a different head_sha than the receipt",
        "log_exit_code_mismatch" => {
            "log header exit_code differs from the recorded integer exit code"
        }
        "log_shared_across_tasks" => {
            "the same log (same log_sha256) is cited as proof for several tasks"
        }
        "no_checkout_sha" => "no checkout_sha/head_sha recorded for the commands",
        "checkout_sha_mismatch" => "checkout sha differs from the receipt sha",
        "sha_not_on_main" => "receipt sha is not on origin/main",
        "receipt_predates_sha" => {
            "verified_at is older than the commit it names (relabeled, not re-run)"
        }
        "vacuous_zero_tests" => {
            "log shows the test command ran ZERO tests (cargo \"0 passed ... N filtered out\", unittest \"Ran 0 tests\", pytest \"no tests ran\") - a filter that matches nothing passes vacuously"
        }
        "not_a_test_command" => {
            "command is not a test/validation run (sha256sum, git log, gh run list, ls, cat, echo, ...) - proves a file exists, not that acceptance passes"
        }
        "empty_input_hash" => {
            "log contains the SHA-256 of EMPTY input (e3b0c442...) - a hash command was run with no file arguments"
        }
        _ => return None,
    })
}

/// `DOPE_NON_TEST_CMDS`.
pub const DOPE_NON_TEST_CMDS: [&str; 18] = [
    "sha256sum",
    "sha1sum",
    "md5sum",
    "git",
    "gh",
    "ls",
    "cat",
    "echo",
    "stat",
    "wc",
    "true",
    "test",
    "head",
    "tail",
    "jq",
    "printf",
    "find",
    "du",
];
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// `dope_cmd_program(cmd)`: first real program, skipping env assignments and wrappers.
pub fn dope_cmd_program(cmd: &str) -> String {
    let wrappers = [
        "env", "nice", "ionice", "time", "command", "exec", "taskset", "timeout", "stdbuf",
    ];
    let mut wrapped = false;
    for t in py::py_split_ws(py::py_strip(cmd)) {
        if ENV_ASSIGN.is_match(t) {
            continue;
        }
        if wrappers.contains(&t) {
            wrapped = true;
            continue;
        }
        if wrapped && (t.starts_with('-') || WRAPPER_ARG.is_match(t)) {
            continue;
        }
        return t.rsplit('/').next().unwrap_or(t).to_string();
    }
    String::new()
}

/// `dope_log_vacuous(text)`: the log proves the runner executed zero tests.
pub fn dope_log_vacuous(text: &str) -> bool {
    let cargo: Vec<(i64, i64)> = CARGO_RESULT
        .captures_iter(text)
        .map(|m| {
            let g = |i| m.get(i).and_then(|x| py::py_int(x.as_str())).unwrap_or(0);
            (g(1), g(2))
        })
        .collect();
    if !cargo.is_empty() && cargo.iter().map(|(a, b)| a + b).sum::<i64>() == 0 {
        return true;
    }
    let ran: Vec<i64> = UNITTEST_RAN
        .captures_iter(text)
        .filter_map(|m| m.get(1).and_then(|x| py::py_int(x.as_str())))
        .collect();
    if !ran.is_empty() && ran.iter().sum::<i64>() == 0 {
        return true;
    }
    PYTEST_NONE.is_match(text)
}

/// `dope_receipt_refs(task)`: `[(path, pinned_sha256_or_None)]`.
pub fn dope_receipt_refs(task: &J) -> PyResult<Vec<(String, Option<String>)>> {
    let mut refs = Vec::new();
    for e in iter_or_empty(task.pget("evidence")?)? {
        let s = py::py_str(&e);
        if let Some(m) = DOPE_VERIFY_REF.captures(py::py_strip(&s))
            && let Some(p) = m.get(1)
        {
            refs.push((
                p.as_str().to_string(),
                m.get(2).map(|h| h.as_str().to_string()),
            ));
        }
    }
    Ok(refs)
}

/// Info returned alongside the problems of one receipt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptInfo {
    pub receipt: String,
    pub shared_receipt: bool,
    pub sha: Option<String>,
    pub cmds: Vec<String>,
    pub log_hashes: Vec<String>,
}

/// `open(lp, errors='replace').read(2000000)` (2,000,000 characters, text mode).
fn read_log_chars(path: &str) -> PyResult<String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = py::universal_newlines(&String::from_utf8_lossy(&bytes));
            Ok(py::prefix(&text, 2_000_000).to_string())
        }
        Err(e) => Err(PyErr::os(path, &e)),
    }
}

fn int_list(ecs: &[J]) -> Option<Vec<i64>> {
    ecs.iter()
        .map(|x| if let J::Int(i) = x { Some(*i) } else { None })
        .collect()
}

/// `check_dope_receipt(task_id, ref_path, ref_hash, on_main, commit_time_fn)`.
pub fn check_dope_receipt(
    task_id: &J,
    ref_path: &str,
    ref_hash: Option<&str>,
    on_main: ShaPredicate<'_>,
    commit_time_fn: Option<TimeLookup<'_>>,
) -> PyResult<(Vec<String>, ReceiptInfo)> {
    let mut probs: BTreeSet<&'static str> = BTreeSet::new();
    let mut info = ReceiptInfo {
        receipt: ref_path.to_string(),
        ..ReceiptInfo::default()
    };
    let done = |p: BTreeSet<&'static str>| p.into_iter().map(str::to_string).collect::<Vec<_>>();
    if !is_file(ref_path) {
        return Ok((vec!["receipt_missing".to_string()], info));
    }
    if let Some(h) = ref_hash.filter(|h| !h.is_empty())
        && sha256_file(ref_path).as_deref() != Some(h)
    {
        probs.insert("receipt_sha256_mismatch");
    }
    let r = match rj(ref_path) {
        Some(r @ J::Obj(_)) => r,
        _ => {
            probs.insert("receipt_unreadable");
            return Ok((done(probs), info));
        }
    };
    let mut entry = r.clone();
    let mut entry_is_r = true;
    if !py::py_eq(r.pget("task_id")?, task_id) {
        let per = r.pget("tasks")?;
        let found = match per {
            J::Obj(o) => match task_id {
                J::Str(k) => o.get(k).filter(|v| matches!(v, J::Obj(_))).cloned(),
                _ => None,
            },
            J::Arr(items) => {
                let mut hit = None;
                for x in items {
                    if matches!(x, J::Obj(_)) && py::py_eq(x.pget("task_id")?, task_id) {
                        hit = Some(x.clone());
                        break;
                    }
                }
                hit
            }
            _ => None,
        };
        match found {
            Some(e) => {
                entry = e;
                entry_is_r = false;
            }
            None => {
                probs.insert(if r.pget("task_id")?.truthy() {
                    "receipt_not_for_task"
                } else {
                    "shared_receipt_no_task_entry"
                });
                return Ok((done(probs), info));
            }
        }
        info.shared_receipt = true;
    }
    let g = |k: &str| -> PyResult<J> {
        let v = entry.pget(k)?;
        if !v.is_null() {
            Ok(v.clone())
        } else if !entry_is_r {
            Ok(r.pget(k)?.clone())
        } else {
            Ok(J::Null)
        }
    };
    let status = g("status")?;
    let status_s = if status.truthy() {
        py::py_str(&status)
    } else {
        String::new()
    };
    if !matches!(
        py::py_strip(&status_s).to_lowercase().as_str(),
        "passed" | "pass"
    ) {
        probs.insert("status_not_passed");
    }
    let sha_j = {
        let a = g("sha")?;
        if a.truthy() { a } else { g("head_sha")? }
    };
    let sha = if sha_j.truthy() {
        py::py_str(&sha_j)
    } else {
        String::new()
    };
    info.sha = Some(sha.clone());
    let good_sha = is_hex(&sha, 40);
    if !good_sha {
        probs.insert("bad_or_missing_sha");
    }
    let cmds_j = entry.pget("commands")?;
    let cmds: Vec<J> = match cmds_j {
        J::Arr(a) if !a.is_empty() => a.clone(),
        _ => {
            if entry.pget("log_path")?.truthy()
                || entry.pget("cmd")?.truthy()
                || entry.pget("command")?.truthy()
            {
                vec![entry.clone()]
            } else {
                Vec::new()
            }
        }
    };
    if cmds.is_empty() {
        probs.insert("no_commands");
    }
    let mut checkout: BTreeSet<String> = BTreeSet::new();
    if entry.pget("checkout_sha")?.truthy() {
        checkout.insert(py::py_str(entry.pidx("checkout_sha")?));
    }
    for c in &cmds {
        if !matches!(c, J::Obj(_)) {
            probs.insert("no_commands");
            continue;
        }
        let cmd_v = {
            let a = c.pget("cmd")?;
            if a.truthy() {
                a.clone()
            } else {
                c.pget("command")?.clone()
            }
        };
        info.cmds.push(if cmd_v.truthy() {
            py::py_str(&cmd_v)
        } else {
            String::new()
        });
        let prog = dope_cmd_program(&if cmd_v.truthy() {
            py::py_str(&cmd_v)
        } else {
            String::new()
        });
        if DOPE_NON_TEST_CMDS.contains(&prog.as_str()) {
            probs.insert("not_a_test_command");
        }
        let ec = if c.has_key("exit_codes")? {
            c.pidx("exit_codes")?.clone()
        } else {
            c.pget("exit_code")?.clone()
        };
        let ecs: Vec<J> = match &ec {
            J::Arr(a) => a.clone(),
            other => vec![other.clone()],
        };
        let ints = int_list(&ecs);
        match &ints {
            Some(v) if !v.is_empty() => {
                if v.iter().any(|x| *x != 0) {
                    probs.insert("nonzero_exit");
                }
            }
            _ => {
                probs.insert("exit_code_not_integer");
            }
        }
        let lp = c.pget("log_path")?;
        let lh = c.pget("log_sha256")?;
        let lp_s = py::py_str(lp);
        if !lp.truthy() {
            probs.insert("no_log_path");
        } else if !is_file(&lp_s) {
            probs.insert("log_missing");
        } else if !matches!(lh, J::Str(s) if LOG_SHA_FIELD.is_match(s)) {
            probs.insert("no_log_sha256");
        } else {
            let lh_s = lh.as_str().unwrap_or("");
            let want = lh_s.rsplit(':').next().unwrap_or(lh_s);
            if sha256_file(&lp_s).as_deref() != Some(want) {
                probs.insert("log_sha256_mismatch");
            } else {
                info.log_hashes.push(want.to_string());
                let full = read_log_chars(&lp_s)?;
                let head = py::prefix(&full, 20_000);
                if dope_log_vacuous(&full) {
                    probs.insert("vacuous_zero_tests");
                }
                if full.contains(EMPTY_SHA256) {
                    probs.insert("empty_input_hash");
                }
                let ls: BTreeSet<String> = LOG_HEAD_SHA
                    .captures_iter(head)
                    .filter_map(|m| m.get(1).map(|g| g.as_str().to_string()))
                    .collect();
                if !ls.is_empty() && good_sha && (ls.len() != 1 || !ls.contains(&sha)) {
                    probs.insert("log_names_other_sha");
                }
                let le: BTreeSet<i64> = LOG_EXIT
                    .captures_iter(head)
                    .filter_map(|m| m.get(1).and_then(|g| py::py_int(g.as_str())))
                    .collect();
                if let Some(v) = &ints
                    && !le.is_empty()
                    && !v.iter().all(|x| le.contains(x))
                {
                    probs.insert("log_exit_code_mismatch");
                }
            }
        }
        let cs = {
            let a = c.pget("checkout_sha")?;
            if a.truthy() {
                a.clone()
            } else {
                c.pget("head_sha")?.clone()
            }
        };
        if cs.truthy() {
            checkout.insert(py::py_str(&cs));
        }
    }
    if checkout.is_empty() {
        probs.insert("no_checkout_sha");
    } else if checkout.len() != 1 || !checkout.contains(&sha) {
        probs.insert("checkout_sha_mismatch");
    }
    if good_sha && !on_main(&sha) {
        probs.insert("sha_not_on_main");
    }
    let va = {
        let a = g("verified_at_utc")?;
        py::parse_utc(&if a.truthy() { a } else { g("verified_at")? })
    };
    let ct = match commit_time_fn {
        Some(f) if good_sha => f(&sha),
        _ => None,
    };
    if let (Some(va), Some(ct)) = (va, ct)
        && va < ct
    {
        probs.insert("receipt_predates_sha");
    }
    Ok((done(probs), info))
}

/// `verify_dope_tasks(tasks, on_main, commit_time_fn)` -> (verified, {id: problems}, generic cmds).
pub type DopeVerification = (
    Vec<String>,
    BTreeMap<String, Vec<String>>,
    Vec<(String, Vec<String>)>,
);

/// `verify_dope_tasks`.
pub fn verify_dope_tasks(
    tasks: &[J],
    on_main: ShaPredicate<'_>,
    commit_time_fn: Option<TimeLookup<'_>>,
) -> PyResult<DopeVerification> {
    let mut bad: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut infos: Vec<(String, ReceiptInfo)> = Vec::new();
    for x in tasks {
        if !py::eq_str(x.pget("status")?, "done") {
            continue;
        }
        let tid_j = x.pget("id")?.clone();
        let tid = py::py_str(&tid_j);
        let sub = x.pget("sub_status")?;
        let contradiction = DOPE_CONTRADICTORY_SUBSTATUS.is_match(&if sub.truthy() {
            py::py_str(sub)
        } else {
            String::new()
        });
        let refs = dope_receipt_refs(x)?;
        let Some((path, pin)) = refs.last() else {
            let mut p = vec!["no_receipt".to_string()];
            if contradiction {
                p.push("contradictory_sub_status".to_string());
            }
            bad.insert(tid, py::sorted_set(p));
            continue;
        };
        let (mut p, i) = check_dope_receipt(&tid_j, path, pin.as_deref(), on_main, commit_time_fn)?;
        if contradiction {
            p.push("contradictory_sub_status".to_string());
            p = py::sorted_set(p);
        }
        match infos.iter_mut().find(|(k, _)| *k == tid) {
            Some(slot) => slot.1 = i,
            None => infos.push((tid.clone(), i)),
        }
        if !p.is_empty() {
            bad.insert(tid, p);
        }
    }
    let mut by_log: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (tid, i) in &infos {
        for h in &i.log_hashes {
            match by_log.iter_mut().find(|(k, _)| k == h) {
                Some(slot) => {
                    slot.1.insert(tid.clone());
                }
                None => by_log.push((h.clone(), [tid.clone()].into_iter().collect())),
            }
        }
    }
    for (_, ids) in &by_log {
        if ids.len() > 1 {
            for tid in ids {
                let mut v = bad.get(tid).cloned().unwrap_or_default();
                v.push("log_shared_across_tasks".to_string());
                bad.insert(tid.clone(), py::sorted_set(v));
            }
        }
    }
    let verified: Vec<String> = infos
        .iter()
        .filter(|(t, _)| !bad.contains_key(t))
        .map(|(t, _)| t.clone())
        .collect();
    let mut by_cmd: Vec<(String, Vec<String>)> = Vec::new();
    for tid in &verified {
        let Some((_, info)) = infos.iter().find(|(t, _)| t == tid) else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for c in &info.cmds {
            if !seen.insert(c.clone()) {
                continue;
            }
            match by_cmd.iter_mut().find(|(k, _)| k == c) {
                Some(slot) => slot.1.push(tid.clone()),
                None => by_cmd.push((c.clone(), vec![tid.clone()])),
            }
        }
    }
    let generic: Vec<(String, Vec<String>)> = by_cmd
        .into_iter()
        .filter(|(c, ids)| !c.is_empty() && ids.len() >= DOPE_GENERIC_CMD_MIN)
        .map(|(c, mut ids)| {
            ids.sort();
            (c, ids)
        })
        .collect();
    let mut verified = verified;
    verified.sort();
    Ok((verified, bad, generic))
}

/// `max_claim(logp, total, scr)` reading the screen hardcopy from `hc_path`.
pub fn max_claim(hc_path: &str, logp: &str, total: usize, scr: &str) -> Option<i64> {
    let scr_txt = if std::path::Path::new(hc_path).exists() {
        let text = read_text_replace(hc_path).unwrap_or_default();
        let lines: Vec<&str> = text.split('\n').collect();
        lines[lines.len().saturating_sub(150)..].join("\n")
    } else {
        String::new()
    };
    let log_tail = tail_text(logp, 12_000);
    let own_all: Vec<&str> = log_tail
        .split('\n')
        .filter(|l| l.contains(scr) && l.to_uppercase().contains("STATUS"))
        .collect();
    let own = &own_all[own_all.len().saturating_sub(2)..];
    let txt = format!("{scr_txt}\n{}", own.join("\n"));
    let noise = Regex::new(r"(?i)gorkbot|grok ?bot|ENG.watcher|super_captain|\bNOT\b|claimed|\bclaim\b|\brev\d+|ledger")
        .expect("valid built-in regex");
    let txt: String = txt
        .split('\n')
        .filter(|l| !noise.is_match(l))
        .collect::<Vec<_>>()
        .join("\n");
    let a = Regex::new(&format!(
        r"(?i)\b(\d{{1,3}})\s*/\s*{total}\b[^\n]{{0,25}}\b(done|complete|accepted|verified)"
    ))
    .expect("valid built-in regex");
    let b = Regex::new(&format!(
        r"(?i)\b(done|complete|accepted)\b[^\n]{{0,6}}?[:= ]\s*(\d{{1,3}})\s*/\s*{total}\b"
    ))
    .expect("valid built-in regex");
    let mut vals: Vec<i64> = a
        .captures_iter(&txt)
        .filter_map(|m| m.get(1).and_then(|g| py::py_int(g.as_str())))
        .collect();
    vals.extend(
        b.captures_iter(&txt)
            .filter_map(|m| m.get(2).and_then(|g| py::py_int(g.as_str()))),
    );
    vals.into_iter().max()
}

impl Audit<'_> {
    /// `audit_dope(packet, on_main, commit_time_fn)`.
    pub fn audit_dope(
        &mut self,
        packet: Option<&str>,
        on_main: Option<ShaPredicate<'_>>,
        commit_time_fn: Option<TimeLookup<'_>>,
    ) -> PyResult<J> {
        let packet = packet
            .filter(|p| !p.is_empty())
            .map_or_else(|| self.cfg.dope_packet.clone(), str::to_string);
        let repo = self.cfg.repo("dope");
        let mut res = Obj::new();
        res.set(
            "source",
            format!("{packet}/tasks.json (+ per-task verify receipts)"),
        );
        let t = rj(&format!("{packet}/tasks.json"));
        let p = rj_or_empty(&format!("{packet}/progress.json"));
        let t = match t {
            Some(t) if t.truthy() => t,
            _ => {
                self.f(
                    "amber",
                    "dope",
                    "LEDGER_UNREADABLE",
                    "tasks.json unreadable",
                    Some(&packet),
                );
                return Ok(J::Obj(res));
            }
        };
        let main_holder;
        let default_on_main;
        let default_ct;
        let (on_main, commit_time_fn): (ShaPredicate<'_>, Option<TimeLookup<'_>>) = match on_main {
            Some(f) => (f, commit_time_fn),
            None => {
                self.run(&["git", "-C", &repo, "fetch", "-q", "origin", "main"], 60);
                main_holder = self.rev(&repo, "origin/main");
                res.set("main_sha", main_holder.clone());
                let this: &Audit<'_> = self;
                let repo_a = repo.clone();
                let main_a = main_holder.clone();
                default_on_main = move |s: &str| -> bool {
                    match main_a.as_deref() {
                        Some(m) if !m.is_empty() => this.is_anc(&repo_a, s, m),
                        _ => false,
                    }
                };
                let repo_b = repo.clone();
                default_ct = move |s: &str| this.commit_time(&repo_b, s);
                let ct: &dyn Fn(&str) -> Option<Dt> = &default_ct;
                (&default_on_main, Some(commit_time_fn.unwrap_or(ct)))
            }
        };
        let tasks_list = t.pidx("tasks")?.py_iter()?;
        let mut counts: Vec<(J, usize)> = Vec::new();
        for x in &tasks_list {
            let s = x.pget("status")?.clone();
            if matches!(s, J::Arr(_) | J::Obj(_)) {
                return Err(PyErr::type_error(format!(
                    "unhashable type: '{}'",
                    s.type_name()
                )));
            }
            match counts.iter_mut().find(|(k, _)| py::py_eq(k, &s)) {
                Some(slot) => slot.1 += 1,
                None => counts.push((s, 1)),
            }
        }
        let mut done: Vec<J> = Vec::new();
        for x in &tasks_list {
            if py::eq_str(x.pget("status")?, "done") {
                done.push(x.clone());
            }
        }
        let (verified, bad, generic) = verify_dope_tasks(&tasks_list, on_main, commit_time_fn)?;
        res.set("total", tasks_list.len());
        res.set("ledger_done", done.len());
        res.set("done_recomputed", verified.len());
        res.set(
            "status_counts",
            J::Obj(
                counts
                    .iter()
                    .map(|(k, n)| (py::json_key(k), J::from(*n)))
                    .collect(),
            ),
        );
        res.set(
            "unverified_done",
            J::from(bad.keys().cloned().collect::<Vec<_>>()),
        );
        res.set(
            "note",
            "done_recomputed counts only ledger-done tasks whose own receipt verifies (log+sha256, int exit codes, checkout sha on main)",
        );
        res.set(
            "receipt_problems",
            J::Obj(
                bad.iter()
                    .map(|(k, v)| (k.clone(), J::from(v.clone())))
                    .collect(),
            ),
        );
        let mut shas: Vec<(String, usize)> = Vec::new();
        for x in &done {
            let refs = dope_receipt_refs(x)?;
            let rr = match refs.last() {
                Some((path, _)) => rj_or_empty(path),
                None => J::Obj(Obj::new()),
            };
            let v = {
                let a = rr.pget("head_sha")?;
                let a = if a.truthy() {
                    a.clone()
                } else {
                    rr.pget("sha")?.clone()
                };
                if a.truthy() {
                    py::py_str(&a)
                } else {
                    "none".to_string()
                }
            };
            let key = py::prefix(&v, 9).to_string();
            match shas.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 += 1,
                None => shas.push((key, 1)),
            }
        }
        res.set(
            "receipt_shas",
            J::Obj(shas.into_iter().map(|(k, n)| (k, J::from(n))).collect()),
        );
        let pc = {
            let c = p.pget("counts")?;
            if c.truthy() {
                c.clone()
            } else {
                J::Obj(Obj::new())
            }
        };
        if pc.truthy() && !py::eq_int(pc.pget("done")?, done.len() as i64) {
            let text = format!(
                "progress.json counts.done={} but tasks.json has {} done",
                py::py_str(pc.pget("done")?),
                done.len()
            );
            self.f("red", "dope", "PROGRESS_VS_LEDGER", &text, Some(&packet));
        }
        let tasks_path = format!("{packet}/tasks.json");
        for x in &done {
            if !x.pget("evidence")?.truthy() {
                let text = format!("{} is done with empty evidence", py::py_str(x.pidx("id")?));
                self.f("red", "dope", "DONE_NO_EVIDENCE", &text, Some(&tasks_path));
            }
        }
        let mut contra = Vec::new();
        for x in &done {
            let sub = x.pget("sub_status")?;
            if DOPE_CONTRADICTORY_SUBSTATUS.is_match(&if sub.truthy() {
                py::py_str(sub)
            } else {
                String::new()
            }) {
                contra.push(x.pidx("id")?.clone());
            }
        }
        let contra = py::py_sorted(contra)?;
        if !contra.is_empty() {
            let text = format!(
                "{} task(s) are status=done while their own sub_status says blocked/partial/failed: {} - mark them blocked with the precise blocker instead of done",
                contra.len(),
                py::py_join(" ", &J::Arr(contra.clone()))?
            );
            self.f(
                "red",
                "dope",
                "DONE_BUT_SUBSTATUS_BLOCKED",
                &text,
                Some(&tasks_path),
            );
        }
        let mut stale = Vec::new();
        for x in &done {
            let sub = x.pget("sub_status")?;
            if AWAITING.is_match(&if sub.truthy() {
                py::py_str(sub)
            } else {
                String::new()
            }) {
                let id = x.pidx("id")?;
                if !contra.iter().any(|c| py::py_eq(c, id)) {
                    stale.push(id.clone());
                }
            }
        }
        let stale = py::py_sorted(stale)?;
        if !stale.is_empty() {
            let shown = J::Arr(stale.iter().take(30).cloned().collect());
            let text = format!(
                "{} done task(s) still carry an awaiting/pending sub_status (stale bookkeeping): {}",
                stale.len(),
                py::py_join(" ", &shown)?
            );
            self.f(
                "amber",
                "dope",
                "DONE_SUBSTATUS_STALE",
                &text,
                Some(&tasks_path),
            );
        }
        let mut by_prob: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (tid, ps) in &bad {
            for q in ps {
                by_prob.entry(q.clone()).or_default().push(tid.clone());
            }
        }
        let verify_dir = format!("{packet}/verify");
        for (q, ids) in &by_prob {
            let mut ids = ids.clone();
            ids.sort();
            let shown: Vec<&str> = ids.iter().take(40).map(String::as_str).collect();
            let text = format!(
                "{} (not counted as done): {} task(s) {}",
                dope_problem_text(q).unwrap_or(q.as_str()),
                ids.len(),
                shown.join(" ")
            );
            self.f(
                "red",
                "dope",
                &format!("RECEIPT_{}", q.to_uppercase()),
                &text,
                Some(&verify_dir),
            );
        }
        if !bad.is_empty() {
            let text = format!(
                "{}/{} ledger-done tasks have no verifiable per-task receipt; audited done = {}/{}",
                bad.len(),
                done.len(),
                verified.len(),
                tasks_list.len()
            );
            self.f("red", "dope", "DONE_UNVERIFIED", &text, Some(&tasks_path));
        }
        let mut generic_sorted = generic.clone();
        generic_sorted.sort_by_key(|(_, ids)| std::cmp::Reverse(ids.len()));
        for (c, ids) in &generic_sorted {
            let shown: Vec<&str> = ids.iter().take(25).map(String::as_str).collect();
            let text = format!(
                "one generic command is the only proof for {} different tasks ({}): `{}`; each has its own log so they count, but captain_dope must show the command exercises each task's acceptance",
                ids.len(),
                shown.join(" "),
                py::prefix(c, 120)
            );
            self.f(
                "amber",
                "dope",
                "RECEIPT_GENERIC_COMMAND",
                &text,
                Some(&verify_dir),
            );
        }
        let head = p.pget("pr_269_head_sha")?.clone();
        res.set("pr_head_in_progress_json", head.clone());
        let progress_path = format!("{packet}/progress.json");
        for l in iter_or_empty(p.pget("active_leases")?)? {
            let hc = l.pget("head_commit")?.clone();
            if head.truthy() && hc.truthy() && !py::py_eq(&hc, &head) && l.pget("pr")?.truthy() {
                let hc9 = slice9(&hc)?;
                let head9 = slice9(&head)?;
                let text = format!(
                    "lease {} still pins head {hc9} while PR #269 head is {head9}; any acceptance tied to {hc9} must be re-checked on the current head",
                    py::py_str(l.pget("lease_id")?)
                );
                self.f(
                    "amber",
                    "dope",
                    "LEASE_STALE_HEAD",
                    &text,
                    Some(&progress_path),
                );
            }
        }
        let captain = self.cfg.captain_or("dope");
        let claim = max_claim(
            &self.cfg.audit_hc(&captain),
            &self.cfg.log("dope"),
            tasks_list.len(),
            &captain,
        );
        res.set("captain_claim_max_recent", claim);
        if let Some(c) = claim
            && c > verified.len() as i64
        {
            let text = format!(
                "captain screen/log recently claimed {c}/{} done; audit verifies {} (ledger says {})",
                tasks_list.len(),
                verified.len(),
                done.len()
            );
            let log = self.cfg.log("dope");
            self.f("red", "dope", "COUNT_OVERCLAIM", &text, Some(&log));
        }
        Ok(J::Obj(res))
    }
}

/// `x[:9]` inside an f-string (str or list slicing).
fn slice9(x: &J) -> PyResult<String> {
    match x {
        J::Str(s) => Ok(py::prefix(s, 9).to_string()),
        J::Arr(a) => Ok(py::py_repr(&J::Arr(a.iter().take(9).cloned().collect()))),
        other => Err(PyErr::type_error(format!(
            "'{}' object is not subscriptable",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_ref_regex_without_dollar_newline() {
        let task = crate::jobj! {"evidence" => vec![J::from("see /x/verify/a/B01.json#sha256=".to_string() + &"a".repeat(64)), J::from("prose")]};
        let refs = dope_receipt_refs(&task).expect("refs");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "/x/verify/a/B01.json");
        assert_eq!(refs[0].1.as_deref(), Some("a".repeat(64).as_str()));
    }
}
