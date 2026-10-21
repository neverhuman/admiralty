//! Jarambit: done counts from `docs/WORKPLAN.md` on origin/main only.
//!
//! Never pick the newest-mtime WORKPLAN across worktrees: stale detached /tmp
//! worktrees and unmerged feature branches flap the count (false red 2026-10-18).

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::ctx::{Audit, is_file};
use crate::py::{self, J, Obj};

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid built-in regex")
}

static TASK_ID: LazyLock<Regex> = LazyLock::new(|| re(r"\A[A-Z0-9]+(-[A-Z0-9]+)+\z"));
static DONE_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\bDONE\b|\bVERIFIED\b|\bACCEPTED\b"));
static DONE_ONLY: LazyLock<Regex> = LazyLock::new(|| re(r"\bDONE\b"));
static COUNTER: LazyLock<Regex> = LazyLock::new(|| re(r"Currently Done[^\n]*?`(\d+) / (\d+)`"));

/// Parsed WORKPLAN table: task id -> status cell (insertion order).
pub fn parse_workplan_rows(lines: &[&str]) -> Vec<(String, String)> {
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<(String, String)> = Vec::new();
    for line in lines {
        if !line.starts_with('|') {
            header = None;
            continue;
        }
        let cells: Vec<String> = py::py_strip_chars(py::py_strip(line), "|")
            .split('|')
            .map(|c| py::py_strip(c).to_string())
            .collect();
        let Some(h) = &header else {
            header = Some(cells.iter().map(|c| c.to_lowercase()).collect());
            continue;
        };
        let separator = cells
            .iter()
            .all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
        let (Some(ti), Some(si0)) = (
            h.iter().position(|c| c == "task id"),
            h.iter().position(|c| c == "status"),
        ) else {
            continue;
        };
        if separator {
            continue;
        }
        let tid = if cells.len() > ti {
            py::py_strip_chars(&cells[ti], "` *").to_string()
        } else {
            String::new()
        };
        if !TASK_ID.is_match(&tid) {
            continue;
        }
        let mut si = si0 as i64;
        if cells.len() != h.len() {
            si = cells.len() as i64 - (h.len() as i64 - si);
        }
        let value = if si >= 0 && (si as usize) < cells.len() {
            cells[si as usize].clone()
        } else {
            String::new()
        };
        match rows.iter_mut().find(|(k, _)| *k == tid) {
            Some(slot) => slot.1 = value,
            None => rows.push((tid, value)),
        }
    }
    rows
}

fn bucket(s: &str) -> &'static str {
    let u = s.to_uppercase();
    if u.contains("BLOCKED") {
        "blocked"
    } else if DONE_WORD.is_match(&u) {
        "done"
    } else {
        "other"
    }
}

/// `file.readlines()` in text mode (universal newlines, keep ends).
fn readlines(text: &str) -> Vec<String> {
    let t = py::universal_newlines(text);
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in t.char_indices() {
        if c == '\n' {
            out.push(t[start..=i].to_string());
            start = i + 1;
        }
    }
    if start < t.len() {
        out.push(t[start..].to_string());
    }
    out
}

impl Audit<'_> {
    /// `audit_jarambit()`.
    pub fn audit_jarambit(&mut self) -> J {
        let mut res = Obj::new();
        let repo = self.cfg.repo("jarambit");
        self.host.run(
            &["git", "-C", &repo, "fetch", "-q", "origin", "main"],
            Some(Duration::from_secs(30)),
            None,
        );
        let r = self.host.run(
            &["git", "-C", &repo, "show", "origin/main:docs/WORKPLAN.md"],
            None,
            None,
        );
        let fallback = format!("{repo}/docs/WORKPLAN.md");
        let (wp, lines): (String, Vec<String>) = if r.rc == 0 && !py::py_strip(&r.stdout).is_empty()
        {
            let sha_out = self.host.run(
                &["git", "-C", &repo, "rev-parse", "--short=9", "origin/main"],
                None,
                None,
            );
            let sha = py::py_strip(&sha_out.stdout).to_string();
            (
                format!("{repo}@origin/main({sha}):docs/WORKPLAN.md"),
                py::py_splitlines(&r.stdout, true)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )
        } else if is_file(&fallback) {
            let text = std::fs::read(&fallback)
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default();
            let lines = readlines(&text);
            self.f(
                "amber",
                "jarambit",
                "LEDGER_NOT_FROM_MAIN",
                "could not read origin/main:docs/WORKPLAN.md; fell back to the main checkout file",
                Some(&fallback),
            );
            (fallback, lines)
        } else {
            self.f(
                "amber",
                "jarambit",
                "LEDGER_MISSING",
                "docs/WORKPLAN.md not found on origin/main or in the main checkout",
                None,
            );
            return J::Obj(res);
        };
        res.set("source", wp.as_str());
        let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let rows = parse_workplan_rows(&line_refs);
        for (k, s) in &rows {
            let u = s.to_uppercase();
            if u.contains("BLOCKED") && DONE_ONLY.is_match(&u) {
                let text = format!(
                    "{k} status cell contains both BLOCKED and DONE (\"{}\"); naive parsers count it done",
                    py::prefix(s, 80)
                );
                self.f("red", "jarambit", "AMBIGUOUS_STATUS", &text, Some(&wp));
            }
        }
        let done = rows.iter().filter(|(_, s)| bucket(s) == "done").count();
        let blocked = rows.iter().filter(|(_, s)| bucket(s) == "blocked").count();
        res.set("total", rows.len());
        res.set("done_recomputed", done);
        res.set("blocked", blocked);
        let txt = lines.concat();
        if let Some(m) = COUNTER.captures(&txt) {
            let a = m.get(1).map_or("", |g| g.as_str());
            let b = m.get(2).map_or("", |g| g.as_str());
            res.set("declared_counter", format!("{a}/{b}"));
            let (ai, bi) = (py::py_int(a).unwrap_or(0), py::py_int(b).unwrap_or(0));
            if ai > done as i64 || bi != rows.len() as i64 {
                let text = format!(
                    "WORKPLAN counter says {a}/{b}, table parses {done}/{} (BLOCKED rows not counted)",
                    rows.len()
                );
                self.f("amber", "jarambit", "COUNTER_MISMATCH", &text, Some(&wp));
            }
        }
        J::Obj(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workplan_table_parsing() {
        let md = "# Plan\nCurrently Done: `2 / 3`\n\n| Task ID | Title | Status |\n|---|---|---|\n| `JR-01` | a | DONE |\n| JR-02 | b | BLOCKED (DONE soon) |\n| JR-03 | c | in progress |\n| x | not an id | DONE |\n";
        let lines = py::py_splitlines(md, true);
        let rows = parse_workplan_rows(&lines);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("JR-01".to_string(), "DONE".to_string()));
        assert_eq!(bucket(&rows[1].1), "blocked");
        assert_eq!(bucket(&rows[2].1), "other");
    }

    #[test]
    fn short_rows_index_status_from_the_right() {
        let md = "| Task ID | Title | Owner | Status |\n| JR-01 | t | DONE |\n";
        let rows = parse_workplan_rows(&py::py_splitlines(md, true));
        assert_eq!(rows[0].1, "DONE");
    }
}
