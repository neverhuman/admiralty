//! Host specifics that `tools/sc_audit.py` hard-coded, gathered into one
//! typed struct. `AuditConfig::default()` reproduces the Python constants
//! exactly (with `HOME` taken from `$HOME`, like `os.path.expanduser('~')`).
//! An optional JSON file may override any top-level field (shallow merge).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One captain-managed project (`CAPTAINS`/`LOGS`/`REPOS`/`GHREPO`/`SLICES`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    /// Captain screen name.
    pub captain: String,
    /// Coordination log (LANE/STATUS lines).
    pub log: String,
    /// Read-only clone used for git queries.
    pub repo: String,
    /// `owner/name` on GitHub.
    pub gh_repo: String,
    /// CPU slice on xbabe2 (CPU_SLICES.md).
    pub slice: String,
}

/// Everything the audit reads, with Python-identical defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    pub home: String,
    /// `KICK`: captains coordination directory.
    pub kick: String,
    /// `OUT_DEFAULT`.
    pub out_default: String,
    /// `GH`: gh CLI path.
    pub gh: String,
    /// Projects in `CAPTAINS` order.
    pub projects: Vec<ProjectConfig>,
    /// `ADM`: Admiralty v2 rollout state.
    pub adm: String,
    pub agent_range: String,
    pub overflow: String,
    pub reserved_non_joil: String,
    /// `JE`: JopeDime evidence directory.
    pub je: String,
    /// `JE_MR`: JopeDime major-release evidence directory.
    pub je_mr: String,
    /// `DOPE_P`: DOPE packet directory.
    pub dope_packet: String,
    /// `JOIL_WT`: Joil v5 engineering-plan worktree.
    pub joil_wt: String,
    /// Joil gorkbot validator LATEST.md.
    pub joil_validator: String,
    /// Native Codex queue database.
    pub queue_db: String,
    /// Directory for `sc_audit_hc_*` / `sc_watch_hc_*` screen hardcopies.
    pub hc_dir: String,
    /// Host whose screens are local (default lane host).
    pub local_host: String,
    /// Remote lane hosts reached over ssh.
    pub remote_lane_hosts: Vec<String>,
    /// P1 GPU hosts (`GPU_HOSTS`).
    pub gpu_hosts: Vec<String>,
    /// Host the audit must never touch.
    pub forbidden_host: String,
    /// ssh login user for remote hosts.
    pub ssh_user: String,
}

/// `os.path.expanduser('~')`: `$HOME`, else the passwd entry of the current uid.
pub fn home_dir() -> String {
    if let Ok(h) = std::env::var("HOME") {
        return h;
    }
    let uid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(str::to_string))
        });
    if let (Some(uid), Ok(passwd)) = (uid, std::fs::read_to_string("/etc/passwd")) {
        for line in passwd.lines() {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() >= 6 && f[2] == uid {
                return f[5].to_string();
            }
        }
    }
    "~".to_string()
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self::with_home(&home_dir())
    }
}

impl AuditConfig {
    /// The Python constants for a given `HOME`.
    pub fn with_home(home: &str) -> Self {
        let kick = format!("{home}/xbabe2_kicker/captains-20261017");
        let p = |name: &str, captain: &str, log: &str, repo: &str, gh: &str, slice: &str| {
            ProjectConfig {
                name: name.to_string(),
                captain: captain.to_string(),
                log: format!("{home}{log}"),
                repo: format!("{home}{repo}"),
                gh_repo: gh.to_string(),
                slice: slice.to_string(),
            }
        };
        Self {
            home: home.to_string(),
            out_default: format!("{kick}/sc_audit"),
            gh: format!("{home}/.local/bin/gh"),
            projects: vec![
                p(
                    "joil",
                    "captain_joil",
                    "/joil/COORD.md",
                    "/joil",
                    "neverhuman/joil",
                    "36-39,90-91,110-111,32-35,0-5,16-21,40-43",
                ),
                p(
                    "dope",
                    "captain_dope",
                    "/dope/COMMS.md",
                    "/dope",
                    "neverhuman/dope",
                    "78-89",
                ),
                p(
                    "jopedime",
                    "captain_jopedime",
                    "/JopeDime/COMMS.md",
                    "/JopeDime",
                    "neverhuman/JopeDime",
                    "22-31,92-101",
                ),
                p(
                    "jarambit",
                    "captain_jarambit",
                    "/jarambit/COMMS.md",
                    "/jarambit",
                    "neverhuman/jarambit",
                    "6-15",
                ),
                // agy captain (Jepson 5:25 AM MDT Oct 18); the Codex screen captain_admiralty is its worker.
                // The repo is the captain's own clone; never ~/admiralty (Codex working tree).
                p(
                    "admiralty",
                    "agy_captain_admiralty",
                    "/admiralty-v2-build/COMMS.md",
                    "/.admiralty-captain",
                    "neverhuman/admiralty",
                    "102-109,112-113,64-65",
                ),
            ],
            adm: format!("{home}/admiralty-v2-build"),
            agent_range: "0-50,64-114".to_string(),
            overflow: "44-50,64-67,102-109,112-114".to_string(),
            reserved_non_joil: "0-5,16-21,32-43,68-77,90-91,110-111".to_string(),
            je: format!("{home}/jope-evidence/spec-integration-20261017"),
            je_mr: format!("{home}/jope-evidence/major-release-20261018"),
            dope_packet: format!(
                "{home}/dope/target/unified-spec-workplan-20261017/packet-v2-9e11fd166f5f"
            ),
            joil_wt: format!("{home}/joil/.private/worktrees/v5-engineering-plan"),
            joil_validator: format!("{home}/joil/.private/v5-workers/gorkbot/validator/LATEST.md"),
            queue_db: format!("{home}/.codex/queue_1.sqlite"),
            hc_dir: "/tmp".to_string(),
            local_host: "xbabe2".to_string(),
            remote_lane_hosts: vec!["xbabe1".to_string(), "xbabe3".to_string()],
            gpu_hosts: vec![
                "xbabe1".to_string(),
                "xbabe2".to_string(),
                "xbabe3".to_string(),
            ],
            forbidden_host: "xbabe0".to_string(),
            ssh_user: "ubuntu".to_string(),
            kick,
        }
    }

    /// Apply a JSON object of top-level overrides.
    pub fn merged(&self, overrides: &serde_json::Value) -> Result<Self, String> {
        let serde_json::Value::Object(over) = overrides else {
            return Err("config override must be a JSON object".to_string());
        };
        let mut base = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let serde_json::Value::Object(map) = &mut base {
            for (k, v) in over {
                if !map.contains_key(k) {
                    return Err(format!("unknown config key: {k}"));
                }
                map.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base).map_err(|e| e.to_string())
    }

    /// Load defaults and apply an optional override file.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let base = Self::default();
        match path {
            None => Ok(base),
            Some(p) => {
                let text =
                    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
                let value: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
                base.merged(&value)
            }
        }
    }

    pub fn project(&self, name: &str) -> Option<&ProjectConfig> {
        self.projects.iter().find(|p| p.name == name)
    }
    /// `CAPTAINS.get(proj, proj)`.
    pub fn captain_or(&self, name: &str) -> String {
        self.project(name)
            .map_or_else(|| name.to_string(), |p| p.captain.clone())
    }
    /// `proj in CAPTAINS`.
    pub fn is_captain_project(&self, name: &str) -> bool {
        self.project(name).is_some()
    }
    pub fn log(&self, name: &str) -> String {
        self.project(name)
            .map(|p| p.log.clone())
            .unwrap_or_default()
    }
    pub fn repo(&self, name: &str) -> String {
        self.project(name)
            .map(|p| p.repo.clone())
            .unwrap_or_default()
    }
    pub fn gh_repo(&self, name: &str) -> Option<String> {
        self.project(name).map(|p| p.gh_repo.clone())
    }
    pub fn slice(&self, name: &str) -> String {
        self.project(name)
            .map(|p| p.slice.clone())
            .unwrap_or_default()
    }
    pub fn status_json(&self) -> String {
        format!("{}/super_captain_status.json", self.kick)
    }
    /// `/tmp/sc_audit_hc_<name>.txt`.
    pub fn audit_hc(&self, name: &str) -> String {
        format!("{}/sc_audit_hc_{name}.txt", self.hc_dir)
    }
    /// `/tmp/sc_watch_hc_<name>.txt`.
    pub fn watch_hc(&self, name: &str) -> String {
        format!("{}/sc_watch_hc_{name}.txt", self.hc_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_equal_python_constants() {
        let c = AuditConfig::with_home("/home/ubuntu");
        assert_eq!(c.kick, "/home/ubuntu/xbabe2_kicker/captains-20261017");
        assert_eq!(
            c.out_default,
            "/home/ubuntu/xbabe2_kicker/captains-20261017/sc_audit"
        );
        assert_eq!(c.gh, "/home/ubuntu/.local/bin/gh");
        let names: Vec<&str> = c.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["joil", "dope", "jopedime", "jarambit", "admiralty"]);
        assert_eq!(c.captain_or("admiralty"), "agy_captain_admiralty");
        assert_eq!(c.repo("admiralty"), "/home/ubuntu/.admiralty-captain");
        assert_eq!(c.log("joil"), "/home/ubuntu/joil/COORD.md");
        assert_eq!(
            c.gh_repo("jopedime").as_deref(),
            Some("neverhuman/JopeDime")
        );
        assert_eq!(c.slice("joil"), "36-39,90-91,110-111,32-35,0-5,16-21,40-43");
        assert_eq!(c.captain_or("super_captain"), "super_captain");
        assert!(!c.gpu_hosts.contains(&"xbabe0".to_string()));
    }

    #[test]
    fn overrides_merge_and_reject_unknown_keys() {
        let c = AuditConfig::with_home("/h");
        let m = c
            .merged(&serde_json::json!({"hc_dir": "/x"}))
            .expect("merge");
        assert_eq!(m.hc_dir, "/x");
        assert_eq!(m.kick, c.kick);
        assert!(c.merged(&serde_json::json!({"nope": 1})).is_err());
    }
}
