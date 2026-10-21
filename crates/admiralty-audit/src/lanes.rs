//! LANE-line parsing, CPU-set helpers and read-only git queries.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::ctx::{Audit, tail_text};
use crate::py::{self, Dt, J, Obj, PyErr, PyResult};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

static KV: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\A(screen|wt|worktree|state|status|cpus|task|host|model|tier|effort|class)\s*[= ]\s*(.+)\z",
    )
});
static LANE_START: LazyLock<Regex> = LazyLock::new(|| re(r"\ALANE\b"));
static LANE_TASK: LazyLock<Regex> = LazyLock::new(|| re(r"\ALANE\s+([A-Z0-9][A-Z0-9-]+)\z"));
static POSITIONAL_SCREEN: LazyLock<Regex> = LazyLock::new(|| re(r"\A\d+\.[\w.-]+\z"));
static PID_PREFIX: LazyLock<Regex> = LazyLock::new(|| re(r"\A\d+\."));
/// `SCREEN_ID_RE`.
pub static SCREEN_ID_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{3,8})\.([A-Za-z][\w-]+)\b"));
static CLOSED: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)(\||=|\s)(closed|done|retired|exited|killed|finished|merged)(\s|\||$)")
});

/// `re.split(r'\s+(?=(?:model|tier|effort)=)', s)`.
pub fn split_before_kv(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut seg_start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if !py::py_isspace(c) {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some(&(j, d)) = iter.peek() {
            if py::py_isspace(d) {
                end = j + d.len_utf8();
                iter.next();
            } else {
                break;
            }
        }
        let rest = &s[end..];
        if rest.starts_with("model=") || rest.starts_with("tier=") || rest.starts_with("effort=") {
            out.push(&s[seg_start..i]);
            seg_start = end;
        }
    }
    out.push(&s[seg_start..]);
    out
}

/// `raw.replace('`', '').strip().lstrip('-* ').strip()`.
fn clean_lane_line(raw: &str) -> String {
    let no_ticks = raw.replace('`', "");
    py::py_strip(py::py_lstrip_chars(py::py_strip(&no_ticks), "-* ")).to_string()
}

/// `lanes_from_log(project)` over already-read text.
pub fn lanes_from_text(txt: &str) -> PyResult<Obj> {
    let mut lanes = Obj::new();
    for raw in txt.split('\n') {
        if !raw.contains("LANE") {
            continue;
        }
        let l = clean_lane_line(raw);
        if !LANE_START.is_match(&l) {
            continue;
        }
        let segs: Vec<&str> = l.split('|').map(py::py_strip).collect();
        let mut d = Obj::new();
        if let Some(m) = segs.first().and_then(|s0| LANE_TASK.captures(s0))
            && let Some(g) = m.get(1)
        {
            d.set("task", g.as_str());
        }
        for s in segs.iter().skip(1) {
            if POSITIONAL_SCREEN.is_match(s) {
                d.set("screen", *s);
                continue;
            }
            for sub in split_before_kv(s) {
                let Some(m) = KV.captures(py::py_strip(sub)) else {
                    continue;
                };
                let (Some(k), Some(v)) = (m.get(1), m.get(2)) else {
                    continue;
                };
                let mut k = k.as_str().to_lowercase();
                if k == "worktree" {
                    k = "wt".to_string();
                } else if k == "status" {
                    k = "state".to_string();
                }
                let value = py::py_strip(v.as_str());
                let value = if matches!(k.as_str(), "screen" | "wt" | "state" | "cpus" | "host") {
                    py::py_split_ws(value)
                        .first()
                        .copied()
                        .ok_or_else(PyErr::index)?
                        .to_string()
                } else {
                    value.to_string()
                };
                if !d.contains(&k) {
                    d.set(k, value);
                }
            }
        }
        let screen = match d.get("screen") {
            Some(J::Str(s)) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        let name = PID_PREFIX.replace(&screen, "").to_string();
        d.set("name", name.as_str());
        d.set("line", py::prefix(&l, 300));
        lanes.set(name, J::Obj(d));
    }
    Ok(lanes)
}

/// `lane_screens_any_format(project)` over already-read text: screen -> latest LANE line.
pub fn lane_screens_from_text(txt: &str) -> Obj {
    let mut last = Obj::new();
    for raw in txt.split('\n') {
        if !raw.contains("LANE") {
            continue;
        }
        let l = clean_lane_line(raw);
        if !l.starts_with("LANE") {
            continue;
        }
        for m in SCREEN_ID_RE.captures_iter(&l) {
            if let Some(name) = m.get(2) {
                last.set(name.as_str(), l.as_str());
            }
        }
    }
    last.iter()
        .filter(|(_, l)| !CLOSED.is_match(l.as_str().unwrap_or("")))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// `cpuset(s)`.
pub fn cpuset(s: &str) -> BTreeSet<i64> {
    let mut out = BTreeSet::new();
    for part in s.split(|c: char| c == ',' || py::py_isspace(c)) {
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if py::py_isdigit(a)
                && py::py_isdigit(b)
                && let (Some(a), Some(b)) = (py::py_int(a), py::py_int(b))
            {
                out.extend(a..=b);
            }
        } else if py::py_isdigit(part)
            && let Some(v) = py::py_int(part)
        {
            out.insert(v);
        }
    }
    out
}

/// `fmt_cpus(s)`.
pub fn fmt_cpus(s: &BTreeSet<i64>) -> String {
    let v: Vec<i64> = s.iter().copied().collect();
    let mut r = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        r.push(if i == j {
            v[i].to_string()
        } else {
            format!("{}-{}", v[i], v[j])
        });
        i = j + 1;
    }
    r.join(",")
}

/// One `git worktree list --porcelain` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub locked: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
}

impl Audit<'_> {
    /// `lanes_from_log(project)`.
    pub fn lanes_from_log(&self, project: &str) -> PyResult<Obj> {
        lanes_from_text(&tail_text(&self.cfg.log(project), 600_000))
    }

    /// `lane_screens_any_format(project)`.
    pub fn lane_screens_any_format(&self, project: &str) -> Obj {
        lane_screens_from_text(&tail_text(&self.cfg.log(project), 600_000))
    }

    /// `worktrees(repo)` in porcelain order.
    pub fn worktrees(&self, repo: &str) -> Vec<Worktree> {
        let out = self.run(&["git", "-C", repo, "worktree", "list", "--porcelain"], 15);
        let mut wts: Vec<Worktree> = Vec::new();
        let mut cur: Option<usize> = None;
        for l in out.stdout.split('\n') {
            if let Some(rest) = l.strip_prefix("worktree ") {
                let path = py::py_strip(rest).to_string();
                let idx = match wts.iter().position(|w| w.path == path) {
                    Some(i) => {
                        wts[i] = Worktree {
                            path,
                            ..Worktree::default()
                        };
                        i
                    }
                    None => {
                        wts.push(Worktree {
                            path,
                            ..Worktree::default()
                        });
                        wts.len() - 1
                    }
                };
                cur = Some(idx);
            } else if let Some(i) = cur {
                if l.starts_with("locked") {
                    wts[i].locked = true;
                } else if let Some(rest) = l.strip_prefix("branch ") {
                    wts[i].branch = Some(py::py_strip(rest).to_string());
                } else if let Some(rest) = l.strip_prefix("HEAD ") {
                    wts[i].head = Some(py::py_strip(rest).to_string());
                }
            }
        }
        wts
    }

    /// `is_anc(repo, a, b)`.
    pub fn is_anc(&self, repo: &str, a: &str, b: &str) -> bool {
        self.run(
            &["git", "-C", repo, "merge-base", "--is-ancestor", a, b],
            15,
        )
        .rc == 0
    }

    /// `commit_time(repo, sha)`.
    pub fn commit_time(&self, repo: &str, sha: &str) -> Option<Dt> {
        let out = self.run(&["git", "-C", repo, "log", "-1", "--format=%ct", sha], 10);
        if out.rc != 0 {
            return None;
        }
        let secs = py::py_int(&out.stdout)?;
        py::utc_from_timestamp(secs as f64)
    }

    /// `rev(repo, ref)`.
    pub fn rev(&self, repo: &str, reference: &str) -> Option<String> {
        let spec = format!("{reference}^{{commit}}");
        let out = self.run(
            &["git", "-C", repo, "rev-parse", "--verify", "-q", &spec],
            10,
        );
        if out.rc == 0 {
            Some(py::py_strip(&out.stdout).to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_before_kv_matches_python() {
        assert_eq!(
            split_before_kv("task X model=gpt tier=hi"),
            vec!["task X", "model=gpt", "tier=hi"]
        );
        assert_eq!(split_before_kv("a  b"), vec!["a  b"]);
        assert_eq!(split_before_kv("x \t effort=5"), vec!["x", "effort=5"]);
        assert_eq!(
            split_before_kv("modelx=1 Model=2"),
            vec!["modelx=1 Model=2"]
        );
    }

    #[test]
    fn lane_lines_parse_key_values_and_positional_screen() {
        let txt = "- `LANE JD-01 | 123.jd_worker | state=running | wt=/w/a extra | host xbabe1 | task=t model=m tier=x`\nLANE | 2830191.jopedime_agy | X | active\n";
        let lanes = lanes_from_text(txt).expect("parse");
        let d = lanes.get("jd_worker").and_then(J::as_obj).expect("lane");
        assert_eq!(d.get("task"), Some(&J::from("JD-01")));
        assert_eq!(d.get("state"), Some(&J::from("running")));
        assert_eq!(d.get("wt"), Some(&J::from("/w/a")));
        assert_eq!(d.get("host"), Some(&J::from("xbabe1")));
        assert_eq!(d.get("model"), Some(&J::from("m")));
        assert_eq!(d.get("tier"), Some(&J::from("x")));
        assert!(lanes.get("jopedime_agy").is_some());
        let any = lane_screens_from_text(
            "LANE | 2830191.jopedime_agy | X | active\nLANE | 555.old_worker | closed\n",
        );
        assert!(any.contains("jopedime_agy"));
        assert!(!any.contains("old_worker"));
    }

    #[test]
    fn cpu_sets_round_trip() {
        let s = cpuset("0-3, 7,9-10");
        assert_eq!(fmt_cpus(&s), "0-3,7,9-10");
        assert!(cpuset("").is_empty());
        assert_eq!(fmt_cpus(&cpuset("5-3")), "");
    }
}
