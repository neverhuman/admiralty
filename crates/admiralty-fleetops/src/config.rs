//! Host-specific constants of the retired scripts, gathered into one typed
//! config. `FleetConfig::default()` reproduces the scripts' hard-coded values
//! exactly; a JSON file (`--config`), the `CDASH_RUNTIME` /
//! `ADMIRALTY_KICKER_BASE` environment variables and per-command flags
//! override them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One captain screen (from `send_add2.sh` / `send_compute.sh`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptainTarget {
    /// Full GNU screen id (`PID.name`).
    pub screen: String,
    /// Coordination log that fallback pointers are appended to.
    pub coord_log: String,
    /// Optional `flock` file guarding `coord_log` (empty = no lock).
    #[serde(default)]
    pub lock: String,
}

/// All overridable host specifics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FleetConfig {
    /// `BASE` of `tools/bin/*` (`/home/ubuntu/xbabe2_kicker`).
    pub kicker_base: PathBuf,
    /// `DEFAULT_SESSIONS` of `kick` / `SESSIONS` of `status`.
    pub monitored_sessions: Vec<String>,
    /// Hard-coded list of `tools/peek_all.sh`.
    pub peek_all_sessions: Vec<String>,
    /// Scratch directory used by the scripts (`/tmp`).
    pub tmp_dir: PathBuf,
    /// Default output directory of `mr_probe.sh`.
    pub probe_models_outdir: PathBuf,
    /// Screen names `mr_probe.sh` never hardcopies.
    pub probe_models_skip: Vec<String>,
    /// `CDASH_RUNTIME` default of `dashboard/globalres.py`.
    pub dashboard_runtime: PathBuf,
    /// `HOSTS` of `globalres.py` (xbabe0 is always refused).
    pub resource_hosts: Vec<String>,
    /// Host probed locally with `bash -c` instead of over ssh.
    pub resource_local_host: String,
    pub ssh_user: String,
    pub ssh_connect_timeout_secs: u64,
    pub ssh_server_alive_interval_secs: u64,
    /// `subprocess.run(..., timeout=25)` of `globalres.py`.
    pub probe_timeout_secs: u64,
    /// Upper bound for any single `screen` client invocation.
    pub screen_timeout_secs: u64,
    pub screen_bin: String,
    pub ssh_bin: String,
    pub bash_bin: String,
    pub codex_model: String,
    pub codex_reasoning_effort: String,
    /// procfs root (tests point this at a fixture tree).
    pub proc_root: PathBuf,
    /// `PR_COOLDOWN_SEC` of `kick`.
    pub pr_cooldown_secs: i64,
    /// Captain screens used as defaults by `fleet deliver` plans.
    pub captains: BTreeMap<String, CaptainTarget>,
    /// Host survey of `fleet sweep` (was `tools/sweep.sh`).
    pub sweep: crate::sweep::SweepConfig,
}

pub const DEFAULT_KICKER_BASE: &str = "/home/ubuntu/xbabe2_kicker";
pub const DEFAULT_DASHBOARD_RUNTIME: &str = "/home/ubuntu/xbabe2_kicker/captains-dashboard";

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

impl Default for FleetConfig {
    fn default() -> Self {
        let captain = |screen: &str, log: &str, lock: &str| CaptainTarget {
            screen: screen.to_string(),
            coord_log: log.to_string(),
            lock: lock.to_string(),
        };
        let mut captains = BTreeMap::new();
        captains.insert(
            "joil".to_string(),
            captain(
                "2708475.captain_joil",
                "/home/ubuntu/joil/COORD.md",
                "/home/ubuntu/joil/.private/v5-workers/coord.lock",
            ),
        );
        captains.insert(
            "dope".to_string(),
            captain("2716151.captain_dope", "/home/ubuntu/dope/COMMS.md", ""),
        );
        captains.insert(
            "jopedime".to_string(),
            captain(
                "2724936.captain_jopedime",
                "/home/ubuntu/JopeDime/COMMS.md",
                "",
            ),
        );
        captains.insert(
            "jarambit".to_string(),
            captain(
                "2740538.captain_jarambit",
                "/home/ubuntu/jarambit/COMMS.md",
                "",
            ),
        );
        Self {
            kicker_base: PathBuf::from(DEFAULT_KICKER_BASE),
            monitored_sessions: strings(&[
                "grok_red",
                "jopedime_codex_cleanup",
                "jopeDime_cleanup_claude",
                "jopedime_codex_training",
                "jankurai",
            ]),
            peek_all_sessions: strings(&[
                "jopedime_lane_b_codex",
                "jopedime_lane_c_codex",
                "jopedime_kernel2_codex",
                "jopedime_codex_cleanup",
                "jopeDime_cleanup_claude",
                "redline_codex",
                "redclaude",
                "dope",
                "grok_dope",
                "dope_paper_grok",
                "grok_sit_watch",
                "grok_jeryu_audit",
                "xbabe2_storage",
                "jankurai",
                "grok_red",
            ]),
            tmp_dir: PathBuf::from("/tmp"),
            probe_models_outdir: PathBuf::from("/tmp/mr_probe"),
            probe_models_skip: strings(&["captains_dash", "disk_janitor"]),
            dashboard_runtime: PathBuf::from(DEFAULT_DASHBOARD_RUNTIME),
            resource_hosts: strings(&["xbabe1", "xbabe2", "xbabe3"]),
            resource_local_host: "xbabe2".to_string(),
            ssh_user: "ubuntu".to_string(),
            ssh_connect_timeout_secs: 6,
            ssh_server_alive_interval_secs: 5,
            probe_timeout_secs: 25,
            screen_timeout_secs: 15,
            screen_bin: "screen".to_string(),
            ssh_bin: "ssh".to_string(),
            bash_bin: "bash".to_string(),
            codex_model: "gpt-6-luna".to_string(),
            codex_reasoning_effort: "medium".to_string(),
            proc_root: PathBuf::from("/proc"),
            pr_cooldown_secs: 15 * 60,
            captains,
            sweep: crate::sweep::SweepConfig::default(),
        }
    }
}

impl FleetConfig {
    /// Defaults, then an optional JSON file, then environment overrides.
    pub fn load(
        config_file: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut cfg = match config_file {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
                serde_json::from_str::<FleetConfig>(&text)
                    .map_err(|e| format!("invalid config {}: {e}", path.display()))?
            }
            None => FleetConfig::default(),
        };
        if let Some(base) = env("ADMIRALTY_KICKER_BASE").filter(|v| !v.is_empty()) {
            cfg.kicker_base = PathBuf::from(base);
        }
        // `os.environ.get('CDASH_RUNTIME', default)`: an empty value is used as-is.
        if let Some(runtime) = env("CDASH_RUNTIME") {
            cfg.dashboard_runtime = PathBuf::from(runtime);
        }
        Ok(cfg)
    }

    pub fn state_dir(&self) -> PathBuf {
        self.kicker_base.join("state")
    }

    pub fn snaps_dir(&self) -> PathBuf {
        self.state_dir().join("snaps")
    }

    pub fn kick_log(&self) -> PathBuf {
        self.kicker_base.join("logs").join("kick.log")
    }

    pub fn screen_timeout(&self) -> Duration {
        Duration::from_secs(self.screen_timeout_secs.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_equal_script_constants() {
        let cfg = FleetConfig::default();
        assert_eq!(cfg.kicker_base, PathBuf::from("/home/ubuntu/xbabe2_kicker"));
        assert_eq!(
            cfg.state_dir(),
            PathBuf::from("/home/ubuntu/xbabe2_kicker/state")
        );
        assert_eq!(
            cfg.kick_log(),
            PathBuf::from("/home/ubuntu/xbabe2_kicker/logs/kick.log")
        );
        assert_eq!(cfg.monitored_sessions.len(), 5);
        assert_eq!(cfg.peek_all_sessions.len(), 15);
        assert_eq!(
            cfg.peek_all_sessions.first().map(String::as_str),
            Some("jopedime_lane_b_codex")
        );
        assert_eq!(
            cfg.peek_all_sessions.last().map(String::as_str),
            Some("grok_red")
        );
        assert_eq!(cfg.pr_cooldown_secs, 900);
        assert_eq!(cfg.resource_hosts, ["xbabe1", "xbabe2", "xbabe3"]);
        assert!(!cfg.resource_hosts.iter().any(|h| h == "xbabe0"));
        assert_eq!(cfg.captains["joil"].screen, "2708475.captain_joil");
        assert_eq!(
            cfg.captains["joil"].lock,
            "/home/ubuntu/joil/.private/v5-workers/coord.lock"
        );
        assert_eq!(
            cfg.captains["jarambit"].coord_log,
            "/home/ubuntu/jarambit/COMMS.md"
        );
        assert!(cfg.captains["dope"].lock.is_empty());
    }

    #[test]
    fn json_and_env_overrides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fleet.json");
        std::fs::write(
            &path,
            r#"{"monitored_sessions": ["a"], "pr_cooldown_secs": 5}"#,
        )
        .expect("write");
        let env = |k: &str| match k {
            "CDASH_RUNTIME" => Some("/rt".to_string()),
            "ADMIRALTY_KICKER_BASE" => Some("/kb".to_string()),
            _ => None,
        };
        let cfg = FleetConfig::load(Some(&path), &env).expect("load");
        assert_eq!(cfg.monitored_sessions, ["a"]);
        assert_eq!(cfg.pr_cooldown_secs, 5);
        assert_eq!(cfg.dashboard_runtime, PathBuf::from("/rt"));
        assert_eq!(cfg.kicker_base, PathBuf::from("/kb"));
        // Untouched fields keep their defaults.
        assert_eq!(cfg.resource_local_host, "xbabe2");

        std::fs::write(&path, r#"{"bogus": 1}"#).expect("write");
        assert!(FleetConfig::load(Some(&path), &|_| None).is_err());
    }
}
