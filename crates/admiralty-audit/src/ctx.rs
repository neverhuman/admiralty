//! Audit context: configuration, host, collected findings and shared helpers
//! (`F`, `MA`, `run`, `rj`, `tail_text`, glob).

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use crate::config::AuditConfig;
use crate::host::{CmdOut, Host};
use crate::py::{self, J};

/// One finding: `{'severity', 'scope', 'code', 'text', 'evidence'[, 'must_act']}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub severity: String,
    pub scope: String,
    pub code: String,
    pub text: String,
    pub evidence: Option<String>,
    pub must_act: bool,
}

impl Finding {
    pub fn to_json(&self) -> J {
        let mut o = py::Obj::new();
        o.set("severity", self.severity.as_str());
        o.set("scope", self.scope.as_str());
        o.set("code", self.code.as_str());
        o.set("text", self.text.as_str());
        o.set("evidence", self.evidence.clone());
        if self.must_act {
            o.set("must_act", true);
        }
        J::Obj(o)
    }
}

/// The running audit (replaces the Python module globals).
pub struct Audit<'a> {
    pub cfg: &'a AuditConfig,
    pub host: &'a dyn Host,
    pub findings: Vec<Finding>,
    /// `_rs`: remote `screen -ls` cache per host.
    pub(crate) remote_cache: BTreeMap<String, Option<BTreeMap<String, i64>>>,
}

impl<'a> Audit<'a> {
    pub fn new(cfg: &'a AuditConfig, host: &'a dyn Host) -> Self {
        Self {
            cfg,
            host,
            findings: Vec::new(),
            remote_cache: BTreeMap::new(),
        }
    }

    /// `F(sev, scope, code, text, evidence)`; text truncated to 600 code points.
    pub fn f(&mut self, sev: &str, scope: &str, code: &str, text: &str, evidence: Option<&str>) {
        self.findings.push(Finding {
            severity: sev.to_string(),
            scope: scope.to_string(),
            code: code.to_string(),
            text: py::prefix(text, 600).to_string(),
            evidence: evidence.map(str::to_string),
            must_act: false,
        });
    }

    /// `MA(...)`: a must-act-this-tick finding.
    pub fn ma(&mut self, sev: &str, scope: &str, code: &str, text: &str, evidence: Option<&str>) {
        self.f(sev, scope, code, text, evidence);
        if let Some(last) = self.findings.last_mut() {
            last.must_act = true;
        }
    }

    /// Finding codes in emission order (test helper).
    pub fn codes(&self) -> Vec<String> {
        self.findings.iter().map(|f| f.code.clone()).collect()
    }

    /// `run(args, timeout)`.
    pub fn run(&self, argv: &[&str], timeout_secs: u64) -> CmdOut {
        self.host
            .run(argv, Some(Duration::from_secs(timeout_secs)), None)
    }
}

/// `prev.get(key, {})` (raises when `prev` is not a dict).
pub fn prev_entry(prev: &J, key: &str) -> py::PyResult<J> {
    match prev {
        J::Obj(o) => Ok(o
            .get(key)
            .cloned()
            .unwrap_or_else(|| J::Obj(py::Obj::new()))),
        other => Err(py::PyErr::attr_get(other)),
    }
}

/// `{'hash': h, 'since': p.get('since') if p.get('hash') == h else now}`.
pub fn next_since(p: &J, hash: &J, now_iso: &str) -> py::PyResult<J> {
    if py::py_eq(p.pget("hash")?, hash) {
        Ok(p.pget("since")?.clone())
    } else {
        Ok(J::from(now_iso))
    }
}

/// `rj(p)`: JSON file or `None`.
pub fn rj(path: &str) -> Option<J> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    py::loads(&text)
}

/// `rj(p) or {}`-style helper returning an owned value.
pub fn rj_or_empty(path: &str) -> J {
    match rj(path) {
        Some(j) if j.truthy() => j,
        _ => J::Obj(py::Obj::new()),
    }
}

/// `tail_text(p, nbytes)`: the last `nbytes` bytes, decoded with replacement.
pub fn tail_text(path: &str, nbytes: u64) -> String {
    let read = || -> std::io::Result<Vec<u8>> {
        let mut f = std::fs::File::open(path)?;
        let n = f.seek(SeekFrom::End(0))?;
        f.seek(SeekFrom::Start(n.saturating_sub(nbytes)))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    };
    match read() {
        Ok(b) => String::from_utf8_lossy(&b).to_string(),
        Err(_) => String::new(),
    }
}

/// `glob.glob(dir + '/' + pattern)` for a pattern applied to names in `dir`.
pub fn glob_names(dir: &str, matches: impl Fn(&str) -> bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| !n.starts_with('.') && matches(n))
        .map(|n| format!("{dir}/{n}"))
        .collect();
    out.sort();
    out
}

/// Matches `<prefix>[0-9][0-9].json`.
pub fn two_digit_json(name: &str, prefix: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    let Some(digits) = rest.strip_suffix(".json") else {
        return false;
    };
    digits.len() == 2 && digits.bytes().all(|b| b.is_ascii_digit())
}

/// fnmatch-style `*` wildcard match (no other metacharacters).
pub fn star_match(name: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return name == pattern;
    }
    let mut rest = name;
    let Some(first) = parts.first() else {
        return false;
    };
    let Some(r) = rest.strip_prefix(first) else {
        return false;
    };
    rest = r;
    let last = parts.last().copied().unwrap_or("");
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// `hashlib.sha256(open(p,'rb').read()).hexdigest()` or `None` on OSError.
pub fn sha256_file(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_hex(&bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `os.path.isfile(p)`.
pub fn is_file(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_patterns() {
        assert!(star_match("I05.relabeled.json", "I05.relabeled*.json"));
        assert!(star_match("I05.relabeled-2.json", "I05.relabeled*.json"));
        assert!(!star_match("I05.json", "I05.relabeled*.json"));
        assert!(star_match("I05.fail.json", "I05.*fail*.json"));
        assert!(star_match("I05.x.failed.json", "I05.*fail*.json"));
        assert!(!star_match("I05fail.json", "I05.*fail*.json"));
        assert!(two_digit_json("I07.json", "I"));
        assert!(!two_digit_json("I7.json", "I"));
        assert!(two_digit_json("MR01.json", "MR"));
    }
}
