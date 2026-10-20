use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObserveConfig {
    pub schema_version: u32,
    pub node_id: String,
    pub output_dir: PathBuf,
    #[serde(default = "interval")]
    pub interval_seconds: u64,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
    pub coordination_dir: Option<PathBuf>,
    pub rollout_manifest: Option<PathBuf>,
    pub rollout_lanes_file: Option<PathBuf>,
    pub codex_sessions: Option<PathBuf>,
    pub claude_projects: Option<PathBuf>,
    pub rates_dir: Option<PathBuf>,
    pub history_source: Option<PathBuf>,
    pub admiral_screen: Option<String>,
    pub owner_name: Option<String>,
    pub gh_program: Option<PathBuf>,
}

fn interval() -> u64 {
    15
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub repository: PathBuf,
    /// Trusted operator-selected adapter kind and source paths. No shell code.
    pub tracker: Value,
    pub manager_screen: Option<String>,
    #[serde(default)]
    pub worker_prefixes: Vec<String>,
    pub coordination_log: Option<PathBuf>,
    /// Explicit integration policy; absent means health checks are disabled.
    #[serde(default)]
    pub git_health: Option<GitHealthPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHealthPolicy {
    pub integration_ref: String,
    #[serde(default = "divergence_threshold")]
    pub divergence_threshold: u32,
}

fn divergence_threshold() -> u32 {
    20
}

/// Match Git's full-ref restrictions without accepting revision expressions.
fn integration_ref_valid(name: &str) -> bool {
    (name.starts_with("refs/heads/") || name.starts_with("refs/remotes/"))
        && name.len() <= 512
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && name
            .split('/')
            .all(|s| !s.is_empty() && !s.starts_with('.') && !s.ends_with(".lock"))
        && name.chars().all(|c| {
            !c.is_control()
                && !c.is_whitespace()
                && !matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub id: String,
    /// Explicit SSH target, never discovered from project text or HTTP input.
    pub ssh: Option<String>,
    #[serde(default = "linux")]
    pub os: String,
}
fn linux() -> String {
    "linux".into()
}

impl ObserveConfig {
    pub fn load(path: &std::path::Path) -> crate::Result<Self> {
        if !path.is_absolute() {
            return Err("observer configuration requires an absolute path".into());
        }
        validate_private(path, false)?;
        let config: Self = serde_json::from_value(crate::io::read_json(path, 256 * 1024)?)
            .map_err(|_| "observer configuration has invalid fields or types")?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> crate::Result<()> {
        use std::collections::BTreeSet;
        fn id(s: &str) -> bool {
            !s.is_empty()
                && s.len() <= 100
                && !matches!(s, "." | "..")
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        }
        fn absolute(p: &std::path::Path) -> bool {
            p.as_os_str().len() <= 4096
                && p.is_absolute()
                && !p
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
        }
        if self.schema_version != 1
            || !id(&self.node_id)
            || !(5..=300).contains(&self.interval_seconds)
            || !absolute(&self.output_dir)
            || self.projects.len() > 64
            || self.nodes.len() > 32
        {
            return Err("observer configuration exceeds bounds".into());
        }
        let mut projects = BTreeSet::new();
        let mut nodes = BTreeSet::new();
        for path in [
            &self.coordination_dir,
            &self.rollout_manifest,
            &self.rollout_lanes_file,
            &self.codex_sessions,
            &self.claude_projects,
            &self.rates_dir,
            &self.history_source,
            &self.gh_program,
        ]
        .into_iter()
        .flatten()
        {
            if !absolute(path) {
                return Err("observation source paths must be absolute and bounded".into());
            }
        }
        let screen = |s: &str| {
            !s.is_empty()
                && s.len() <= 200
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        };
        if self.admiral_screen.as_ref().is_some_and(|s| !screen(s)) {
            return Err("Admiral screen identity is invalid".into());
        }
        if self
            .owner_name
            .as_ref()
            .is_some_and(|s| s.len() > 100 || s.chars().any(char::is_control))
        {
            return Err("owner display name is invalid".into());
        }
        for p in &self.projects {
            if p.git_health.as_ref().is_some_and(|policy| {
                !integration_ref_valid(&policy.integration_ref)
                    || policy.divergence_threshold > 100_000
            }) {
                return Err("Git health requires a full branch ref and a bounded threshold".into());
            }
            if !id(&p.id)
                || !projects.insert(&p.id)
                || !absolute(&p.repository)
                || p.worker_prefixes.len() > 64
                || p.worker_prefixes
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
            {
                return Err("project observation configuration invalid".into());
            }
            if p.manager_screen.as_ref().is_some_and(|s| !screen(s))
                || p.coordination_log.as_ref().is_some_and(|p| !absolute(p))
            {
                return Err("project observation paths or manager identity are invalid".into());
            }
        }
        for n in &self.nodes {
            if !id(&n.id) || !nodes.insert(&n.id) || !matches!(n.os.as_str(), "linux" | "macos") {
                return Err("node observation configuration invalid".into());
            }
            if (n.ssh.is_none() && n.id != self.node_id)
                || (n.ssh.is_some() && n.id == self.node_id)
            {
                return Err("node identity does not match its local or remote transport".into());
            }
            if let Some(target) = &n.ssh
                && (target.is_empty()
                    || target.len() > 200
                    || target.starts_with('-')
                    || !target.bytes().all(|b| {
                        b.is_ascii_alphanumeric()
                            || matches!(b, b'@' | b'.' | b'-' | b'_' | b':' | b'[' | b']')
                    }))
            {
                return Err("SSH target must be an explicit hostname or configured alias".into());
            }
        }
        Ok(())
    }
}

pub fn validate_output(path: &std::path::Path) -> crate::Result<()> {
    validate_private(path, true)
}
fn validate_private(path: &std::path::Path, directory: bool) -> crate::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err("private observer path has an invalid type".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = rustix::process::geteuid().as_raw();
        if metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
            || (!directory && metadata.nlink() != 1)
        {
            return Err("observer paths must be owned and private".into());
        }
        for ancestor in path.ancestors().skip(1) {
            let m = std::fs::symlink_metadata(ancestor)?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || !(m.uid() == 0 || m.uid() == uid)
                || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
            {
                return Err("observer ancestor is not protected".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn git_health_policy_requires_explicit_full_ref_and_bounded_threshold() {
        let mut value = json!({"schema_version":1,"node_id":"local","output_dir":"/owned",
            "projects":[{"id":"project","name":"Project","repository":"/repo","tracker":{},
                "git_health":{"integration_ref":"refs/heads/main"}}]});
        let config: ObserveConfig = serde_json::from_value(value.clone()).expect("config");
        assert!(config.validate().is_ok());
        assert_eq!(
            config.projects[0]
                .git_health
                .as_ref()
                .expect("policy")
                .divergence_threshold,
            20
        );
        for name in [
            "main",
            "HEAD",
            "refs/heads/main~1",
            "refs/heads/main..other",
            "refs/heads/a.lock",
            "refs/heads/a\\b",
            "refs/heads/a b",
            "refs/heads/.hidden",
            "refs/heads/a//b",
            "refs/heads/a@{1}",
        ] {
            value["projects"][0]["git_health"]["integration_ref"] = json!(name);
            assert!(
                serde_json::from_value::<ObserveConfig>(value.clone())
                    .expect("config")
                    .validate()
                    .is_err(),
                "{name}"
            );
        }
        value["projects"][0]["git_health"] =
            json!({"integration_ref":"refs/remotes/origin/trunk","divergence_threshold":100_001});
        assert!(
            serde_json::from_value::<ObserveConfig>(value)
                .expect("config")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn transports_cannot_impersonate_local_or_duplicate_node_identity() {
        let mut value = json!({"schema_version":1,"node_id":"local","output_dir":"/srv/observed"});
        for nodes in [
            json!([{"id":"other"}]),
            json!([{"id":"local","ssh":"other"}]),
            json!([{"id":"remote","ssh":"host"},{"id":"remote","ssh":"host2"}]),
            json!([{"id":"remote","ssh":"-oProxyCommand=anything"}]),
        ] {
            value["nodes"] = nodes;
            let config: ObserveConfig =
                serde_json::from_value(value.clone()).expect("valid fixture");
            assert!(config.validate().is_err());
        }
        value["nodes"] = json!([{"id":"local"},{"id":"remote","ssh":"operator@configured-host"}]);
        assert!(
            serde_json::from_value::<ObserveConfig>(value)
                .expect("valid fixture")
                .validate()
                .is_ok()
        );
    }
}
