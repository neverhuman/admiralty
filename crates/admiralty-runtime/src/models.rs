//! Editable model policy and deterministic routing proposals. No provider calls,
//! credential reads, SSH, process launches or execution permits are issued here.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const MAX_REGISTRY_BYTES: usize = 1024 * 1024;
pub const MAX_MODELS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenaiCompatible,
    OpencodeSsh,
    CodexCli,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    /// Native provider identity, independent of this registry's stable id.
    pub provider_model: String,
    /// Lower integers are preferred. Values are policy order, never prices.
    pub tier: i32,
    pub enabled: bool,
    pub node_id: Option<String>,
    pub endpoint: Option<String>,
    /// Relative reference only. Contents are never opened by the registry.
    pub credential_file_ref: Option<String>,
    pub max_concurrency: u16,
    pub capabilities: BTreeSet<String>,
    pub task_max_attempts: u16,
    pub escalate_to: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelRegistry {
    pub schema_version: u32,
    /// Canonical decimal string remains exact in JavaScript.
    pub revision: String,
    pub models: Vec<ModelSpec>,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: "0".into(),
            models: vec![],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("invalid model registry: {0}")]
    Invalid(String),
    #[error("model registry revision conflict; current revision is {current}")]
    RevisionConflict { current: String },
    #[error("model registry writer is busy; retry after reading current revision")]
    Busy,
    #[error("model registry filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("model registry JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

fn invalid(message: &str) -> RegistryError {
    RegistryError::Invalid(message.into())
}
fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
fn revision(value: &str) -> Result<u64, RegistryError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|number| number.to_string() == value)
        .ok_or_else(|| invalid("revision must be a canonical decimal u64 string"))
}
fn secret_marker(value: &str) -> bool {
    value.contains("PRIVATE KEY-----")
        || value
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
            .any(|part| {
                part.len() >= 20
                    && [
                        "sk-",
                        "ghp_",
                        "github_pat_",
                        "xoxb-",
                        "xoxp-",
                        "AIza",
                        "AKIA",
                    ]
                    .iter()
                    .any(|prefix| part.starts_with(prefix))
            })
}

impl ModelRegistry {
    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.schema_version != 1 || self.models.len() > MAX_MODELS {
            return Err(invalid("schema version 1 and at most256 models required"));
        }
        revision(&self.revision)?;
        let mut ids = BTreeSet::new();
        let mut tier_caps = BTreeMap::new();
        for model in &self.models {
            if !identity(&model.id)
                || !ids.insert(model.id.as_str())
                || model.label.trim().is_empty()
                || model.label.len() > 200
                || model.label.chars().any(char::is_control)
                || model.provider_model.trim().is_empty()
                || model.provider_model.len() > 256
                || model.provider_model.chars().any(char::is_control)
                || !(1..=1024).contains(&model.max_concurrency)
                || !(1..=20).contains(&model.task_max_attempts)
                || model.capabilities.is_empty()
                || model.capabilities.len() > 64
                || model.capabilities.iter().any(|value| !identity(value))
                || model
                    .node_id
                    .as_deref()
                    .is_some_and(|value| !identity(value))
            {
                return Err(invalid(
                    "models need unique IDs, labels, capabilities and bounded concurrency/attempt limits",
                ));
            }
            if tier_caps
                .insert(model.tier, model.task_max_attempts)
                .is_some_and(|old| old != model.task_max_attempts)
            {
                return Err(invalid(
                    "models in the same tier must share the per-task attempt cap",
                ));
            }
            if let Some(reference) = &model.credential_file_ref
                && (!reference.starts_with("credentials/")
                    || reference.len() > 256
                    || !admiralty_core::safe_relative_path(reference)
                    || reference.chars().any(char::is_control))
            {
                return Err(invalid(
                    "credentials must be relative credentials/ file references, never values",
                ));
            }
            if let Some(endpoint) = &model.endpoint
                && (endpoint.len() > 2048
                    || endpoint.chars().any(char::is_whitespace)
                    || endpoint.chars().any(char::is_control)
                    || endpoint.contains(['@', '?', '#']))
            {
                return Err(invalid(
                    "endpoint must not contain userinfo, query credentials, whitespace or fragments",
                ));
            }
            match model.provider {
                Provider::OpenaiCompatible => {
                    let value = model
                        .endpoint
                        .as_deref()
                        .and_then(|url| {
                            url.strip_prefix("http://")
                                .or_else(|| url.strip_prefix("https://"))
                        })
                        .filter(|value| !value.is_empty() && !value.starts_with('/'));
                    if value.is_none() {
                        return Err(invalid(
                            "OpenAI-compatible provider requires an explicit HTTP(S) endpoint",
                        ));
                    }
                }
                Provider::OpencodeSsh => {
                    if model.node_id.is_none()
                        || model.endpoint.as_deref().is_some_and(|value| {
                            !value.strip_prefix("ssh://").is_some_and(identity)
                        })
                    {
                        return Err(invalid(
                            "opencode_ssh requires node_id and an optional ssh://profile reference, never a shell command",
                        ));
                    }
                }
                Provider::CodexCli => {
                    if model.endpoint.is_some() {
                        return Err(invalid(
                            "codex_cli endpoint must be unset; CLI qualification owns its transport",
                        ));
                    }
                }
            }
            for value in [&model.id, &model.label, &model.provider_model]
                .into_iter()
                .chain(model.endpoint.iter())
                .chain(model.credential_file_ref.iter())
                .chain(model.node_id.iter())
            {
                if secret_marker(value) {
                    return Err(invalid(
                        "secret values are forbidden; store credentials outside the registry",
                    ));
                }
            }
        }
        let by_id: BTreeMap<_, _> = self
            .models
            .iter()
            .map(|model| (model.id.as_str(), model))
            .collect();
        for model in &self.models {
            let mut visited = BTreeSet::new();
            let mut current = model;
            while let Some(next) = current.escalate_to.as_deref() {
                if !visited.insert(current.id.as_str()) {
                    return Err(invalid("escalation cycle"));
                }
                let target = by_id
                    .get(next)
                    .ok_or_else(|| invalid("escalation target does not exist"))?;
                if target.tier < current.tier {
                    return Err(invalid("escalation cannot decrease tier order"));
                }
                current = target;
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_REGISTRY_BYTES {
            return Err(invalid("serialized registry exceeds one MiB"));
        }
        Ok(())
    }

    pub fn select(
        &self,
        request: &SelectionRequest,
        capacity: &CapacitySnapshot,
    ) -> Result<SelectionOutcome, RegistryError> {
        self.validate()?;
        if request
            .required_capabilities
            .iter()
            .any(|value| !identity(value))
        {
            return Err(invalid("invalid required capability"));
        }
        let capable = |model: &&ModelSpec| {
            model.enabled && request.required_capabilities.is_subset(&model.capabilities)
        };
        let available = |model: &ModelSpec| {
            !capacity.unavailable.contains(&model.id)
                && capacity.in_use.get(&model.id).copied().unwrap_or(0) < model.max_concurrency
        };
        let attempts_left = |model: &ModelSpec| {
            request
                .attempts_by_tier
                .get(&model.tier)
                .copied()
                .unwrap_or(0)
                < model.task_max_attempts
        };
        let selected = |model: &ModelSpec| SelectionOutcome::Selected {
            model_id: model.id.clone(),
            usage_cost: UsageCost::Unknown,
        };
        let waiting = |model: Option<&ModelSpec>, reason| SelectionOutcome::Waiting {
            model_id: model.map(|value| value.id.clone()),
            reason,
        };
        let lookup = |id: &str| {
            self.models
                .iter()
                .find(|model| model.id == id)
                .ok_or_else(|| invalid("requested model does not exist"))
        };
        if let Some(reason) = match request.last_failure {
            Some(FailureReason::SetupError) => Some(WaitReason::SetupRequired),
            Some(FailureReason::BlockedInput) => Some(WaitReason::NeedsInput),
            _ => None,
        } {
            let model = request
                .pinned_model
                .as_deref()
                .or(request.current_model.as_deref())
                .map(lookup)
                .transpose()?;
            return Ok(waiting(model, reason));
        }
        if let Some(id) = request.pinned_model.as_deref() {
            if request
                .current_model
                .as_deref()
                .is_some_and(|current| current != id)
            {
                return Err(invalid("pinned model conflicts with current model"));
            }
            let model = lookup(id)?;
            return Ok(if !capable(&model) {
                waiting(Some(model), WaitReason::NoCapableModel)
            } else if !attempts_left(model) {
                waiting(Some(model), WaitReason::AttemptLimit)
            } else if !available(model) {
                waiting(Some(model), WaitReason::CapacityOrUnavailable)
            } else if matches!(
                request.last_failure,
                Some(FailureReason::ContextExhausted | FailureReason::ProviderUnavailable)
            ) {
                waiting(Some(model), WaitReason::PinnedModelUnavailable)
            } else {
                selected(model)
            });
        }
        if let Some(id) = request.current_model.as_deref() {
            let mut model = lookup(id)?;
            let force_escalation = matches!(
                request.last_failure,
                Some(FailureReason::ContextExhausted | FailureReason::ProviderUnavailable)
            );
            if request.last_failure.is_none() && !attempts_left(model) {
                return Ok(waiting(Some(model), WaitReason::AttemptLimit));
            }
            if force_escalation || !attempts_left(model) {
                if request.last_failure.is_none() || !request.allow_escalation {
                    return Ok(waiting(Some(model), WaitReason::EscalationNotAuthorized));
                }
                let Some(next) = model.escalate_to.as_deref() else {
                    return Ok(waiting(Some(model), WaitReason::EscalationExhausted));
                };
                model = lookup(next)?;
            }
            loop {
                if !capable(&model) {
                    return Ok(waiting(Some(model), WaitReason::NoCapableModel));
                }
                if !attempts_left(model) {
                    return Ok(waiting(Some(model), WaitReason::AttemptLimit));
                }
                if available(model) {
                    return Ok(selected(model));
                }
                if !request.allow_capacity_fallback {
                    return Ok(waiting(Some(model), WaitReason::CapacityOrUnavailable));
                }
                let Some(next) = model.escalate_to.as_deref() else {
                    return Ok(waiting(Some(model), WaitReason::CapacityOrUnavailable));
                };
                model = lookup(next)?;
            }
        }
        if request.last_failure.is_some() {
            return Err(invalid("failure evidence requires a current model"));
        }
        let mut candidates: Vec<_> = self.models.iter().filter(capable).collect();
        candidates.sort_by(|left, right| (left.tier, &left.id).cmp(&(right.tier, &right.id)));
        let Some(first) = candidates.first().copied() else {
            return Ok(waiting(None, WaitReason::NoCapableModel));
        };
        if !attempts_left(first) {
            return Ok(waiting(Some(first), WaitReason::AttemptLimit));
        }
        for model in candidates {
            if model.tier != first.tier && !request.allow_capacity_fallback {
                break;
            }
            if attempts_left(model) && available(model) {
                return Ok(selected(model));
            }
        }
        Ok(waiting(
            Some(first),
            if attempts_left(first) {
                WaitReason::CapacityOrUnavailable
            } else {
                WaitReason::AttemptLimit
            },
        ))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRequest {
    #[serde(default)]
    pub required_capabilities: BTreeSet<String>,
    pub pinned_model: Option<String>,
    pub current_model: Option<String>,
    #[serde(default)]
    pub attempts_by_tier: BTreeMap<i32, u16>,
    pub last_failure: Option<FailureReason>,
    #[serde(default)]
    pub allow_escalation: bool,
    #[serde(default)]
    pub allow_capacity_fallback: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacitySnapshot {
    #[serde(default)]
    pub in_use: BTreeMap<String, u16>,
    #[serde(default)]
    pub unavailable: BTreeSet<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    Failure,
    TestFail,
    ContextExhausted,
    ProviderUnavailable,
    SetupError,
    BlockedInput,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsageCost {
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    CapacityOrUnavailable,
    AttemptLimit,
    NoCapableModel,
    EscalationNotAuthorized,
    EscalationExhausted,
    PinnedModelUnavailable,
    SetupRequired,
    NeedsInput,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SelectionOutcome {
    Selected {
        model_id: String,
        usage_cost: UsageCost,
    },
    Waiting {
        model_id: Option<String>,
        reason: WaitReason,
    },
}

/// Private local configuration file. Reads do not create files or read secrets.
#[derive(Clone, Debug)]
pub struct RegistryFile {
    path: PathBuf,
}

impl RegistryFile {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, RegistryError> {
        let path = path.into();
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || path.file_name().is_none()
        {
            return Err(invalid("registry requires an absolute dedicated file path"));
        }
        check_parent(&path)?;
        Ok(Self { path })
    }

    pub fn read(&self) -> Result<ModelRegistry, RegistryError> {
        check_parent(&self.path)?;
        let mut file = match open_private(&self.path, false) {
            Ok(file) => file,
            Err(RegistryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ModelRegistry::default());
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_REGISTRY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err(invalid("registry exceeds one MiB"));
        }
        let registry: ModelRegistry = serde_json::from_slice(&bytes)?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn replace(
        &self,
        expected_revision: &str,
        models: Vec<ModelSpec>,
    ) -> Result<ModelRegistry, RegistryError> {
        check_parent(&self.path)?;
        revision(expected_revision)?;
        let mut lock_path = self.path.as_os_str().to_os_string();
        lock_path.push(".write.lock");
        let lock = open_private(&PathBuf::from(lock_path), true)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                RegistryError::Busy
            } else {
                RegistryError::Io(error)
            }
        })?;
        let current = self.read()?;
        if current.revision != expected_revision {
            return Err(RegistryError::RevisionConflict {
                current: current.revision,
            });
        }
        let next = revision(&current.revision)?
            .checked_add(1)
            .ok_or_else(|| invalid("registry revision exhausted"))?;
        let updated = ModelRegistry {
            schema_version: 1,
            revision: next.to_string(),
            models,
        };
        updated.validate()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("registry parent missing"))?;
        let mut stage = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut stage, &updated)?;
        stage.write_all(b"\n")?;
        stage.as_file().sync_all()?;
        // Recheck destination type before atomic replacement; a protected
        // private parent prevents unrelated users swapping this path.
        match self.path.symlink_metadata() {
            Ok(_) => drop(open_private(&self.path, false)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        stage
            .persist(&self.path)
            .map_err(|error| RegistryError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        Ok(updated)
    }
}

#[cfg(unix)]
fn check_parent(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::MetadataExt;
    for ancestor in path
        .parent()
        .ok_or_else(|| invalid("registry parent missing"))?
        .ancestors()
    {
        let info = ancestor.symlink_metadata()?;
        if info.file_type().is_symlink() {
            let alias = (ancestor == Path::new("/tmp")
                && ancestor.canonicalize()? == Path::new("/private/tmp"))
                || (ancestor == Path::new("/var")
                    && ancestor.canonicalize()? == Path::new("/private/var"));
            if !alias || info.uid() != 0 {
                return Err(invalid("registry parent symlinks are forbidden"));
            }
        }
    }
    let info = path
        .parent()
        .ok_or_else(|| invalid("registry parent missing"))?
        .metadata()?;
    if !info.is_dir()
        || info.uid() != rustix::process::geteuid().as_raw()
        || info.mode() & 0o077 != 0
    {
        return Err(invalid(
            "registry parent must be an owned private0700 directory",
        ));
    }
    Ok(())
}
#[cfg(not(unix))]
fn check_parent(_: &Path) -> Result<(), RegistryError> {
    Err(invalid(
        "private registry file permissions are not qualified on this platform",
    ))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    options
}
fn open_private(path: &Path, create: bool) -> Result<File, RegistryError> {
    let file = private_options()
        .read(true)
        .write(create)
        .create(create)
        .truncate(false)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.len() > MAX_REGISTRY_BYTES as u64 {
        return Err(invalid("registry must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if info.uid() != rustix::process::geteuid().as_raw()
            || info.mode() & 0o077 != 0
            || info.nlink() != 1
        {
            return Err(invalid(
                "registry and lock require owned0600 files with one link",
            ));
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, tier: i32, next: Option<&str>) -> ModelSpec {
        ModelSpec {
            id: id.into(),
            label: format!("Fixture {id}"),
            provider: Provider::OpenaiCompatible,
            provider_model: format!("fixture/{id}"),
            tier,
            enabled: true,
            node_id: Some("arbitrary-node".into()),
            endpoint: Some("http://127.0.0.1:8000/v1".into()),
            credential_file_ref: Some("credentials/fixture-provider".into()),
            max_concurrency: 16,
            capabilities: BTreeSet::from(["coding".into(), "tool_calls".into()]),
            task_max_attempts: 2,
            escalate_to: next.map(str::to_owned),
        }
    }
    fn registry() -> ModelRegistry {
        ModelRegistry {
            models: vec![
                model("own-model", 0, Some("external-model")),
                model("external-model", 1, None),
            ],
            ..Default::default()
        }
    }
    fn request() -> SelectionRequest {
        SelectionRequest {
            required_capabilities: BTreeSet::from(["coding".into()]),
            ..Default::default()
        }
    }
    fn selected(value: SelectionOutcome) -> String {
        match value {
            SelectionOutcome::Selected {
                model_id,
                usage_cost,
            } => {
                assert_eq!(usage_cost, UsageCost::Unknown);
                model_id
            }
            other => panic!("expected selection, got {other:?}"),
        }
    }
    fn file_fixture() -> (tempfile::TempDir, RegistryFile) {
        let temporary = tempfile::tempdir().expect("private fixture directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
                .expect("fixture registry directory is explicitly private");
        }
        let file = RegistryFile::new(temporary.path().join("models.json"))
            .expect("private fixture registry");
        (temporary, file)
    }

    #[test]
    fn cheapest_capable_model_is_deterministic_and_queue_never_silently_escalates() {
        let config = registry();
        assert_eq!(
            selected(
                config
                    .select(&request(), &CapacitySnapshot::default())
                    .expect("initial selection")
            ),
            "own-model"
        );
        let full = CapacitySnapshot {
            in_use: BTreeMap::from([("own-model".into(), 16)]),
            ..Default::default()
        };
        assert!(matches!(
            config.select(&request(), &full).expect("bounded capacity"),
            SelectionOutcome::Waiting {
                reason: WaitReason::CapacityOrUnavailable,
                ..
            }
        ));
        let mut allowed = request();
        allowed.allow_capacity_fallback = true;
        assert_eq!(
            selected(config.select(&allowed, &full).expect("explicit fallback")),
            "external-model"
        );
        allowed.attempts_by_tier.insert(0, 2);
        assert!(matches!(
            config
                .select(&allowed, &full)
                .expect("spent attempt budget"),
            SelectionOutcome::Waiting {
                reason: WaitReason::AttemptLimit,
                ..
            }
        ));
    }

    #[test]
    fn pinned_model_is_honored_even_with_cheaper_capacity_and_never_replaced() {
        let config = registry();
        let mut pinned = request();
        pinned.pinned_model = Some("external-model".into());
        assert_eq!(
            selected(
                config
                    .select(&pinned, &CapacitySnapshot::default())
                    .expect("pinned model")
            ),
            "external-model"
        );
        pinned.pinned_model = Some("own-model".into());
        pinned.last_failure = Some(FailureReason::ContextExhausted);
        pinned.allow_escalation = true;
        pinned.allow_capacity_fallback = true;
        assert!(
            matches!(config.select(&pinned, &CapacitySnapshot::default()).expect("pinned failure"), SelectionOutcome::Waiting { model_id: Some(id), .. } if id == "own-model")
        );
        pinned.current_model = Some("external-model".into());
        assert!(
            config
                .select(&pinned, &CapacitySnapshot::default())
                .is_err()
        );
    }

    #[test]
    fn failed_checks_retry_inside_tier_cap_and_escalate_only_with_explicit_policy() {
        let config = registry();
        for reason in [FailureReason::Failure, FailureReason::TestFail] {
            let mut failed = request();
            failed.current_model = Some("own-model".into());
            failed.last_failure = Some(reason);
            failed.attempts_by_tier.insert(0, 1);
            assert_eq!(
                selected(
                    config
                        .select(&failed, &CapacitySnapshot::default())
                        .expect("bounded retry")
                ),
                "own-model"
            );
            failed.attempts_by_tier.insert(0, 2);
            assert!(matches!(
                config
                    .select(&failed, &CapacitySnapshot::default())
                    .expect("policy needed"),
                SelectionOutcome::Waiting {
                    reason: WaitReason::EscalationNotAuthorized,
                    ..
                }
            ));
            failed.allow_escalation = true;
            assert_eq!(
                selected(
                    config
                        .select(&failed, &CapacitySnapshot::default())
                        .expect("failure escalation")
                ),
                "external-model"
            );
            failed.last_failure = None;
            assert!(matches!(
                config
                    .select(&failed, &CapacitySnapshot::default())
                    .expect("no failure evidence"),
                SelectionOutcome::Waiting {
                    reason: WaitReason::AttemptLimit,
                    ..
                }
            ));
        }
        for reason in [
            FailureReason::ContextExhausted,
            FailureReason::ProviderUnavailable,
        ] {
            let mut failed = request();
            failed.current_model = Some("own-model".into());
            failed.last_failure = Some(reason);
            failed.allow_escalation = true;
            assert_eq!(
                selected(
                    config
                        .select(&failed, &CapacitySnapshot::default())
                        .expect("explicit provider failure")
                ),
                "external-model"
            );
        }
    }

    #[test]
    fn disabled_incapable_or_unavailable_models_never_receive_an_execution_proposal() {
        let mut config = registry();
        config.models[0].enabled = false;
        assert_eq!(
            selected(
                config
                    .select(&request(), &CapacitySnapshot::default())
                    .expect("disabled model excluded")
            ),
            "external-model"
        );
        config.models[0].enabled = true;
        config.models[0].capabilities = BTreeSet::from(["vision".into()]);
        assert_eq!(
            selected(
                config
                    .select(&request(), &CapacitySnapshot::default())
                    .expect("capability filtered")
            ),
            "external-model"
        );
        let mut needs = request();
        needs
            .required_capabilities
            .insert("unavailable-capability".into());
        assert!(matches!(
            config
                .select(&needs, &CapacitySnapshot::default())
                .expect("unknown capability"),
            SelectionOutcome::Waiting {
                reason: WaitReason::NoCapableModel,
                ..
            }
        ));
        config.models[0].capabilities.insert("coding".into());
        let health = CapacitySnapshot {
            unavailable: BTreeSet::from(["own-model".into()]),
            ..Default::default()
        };
        assert!(matches!(
            config
                .select(&request(), &health)
                .expect("unavailable is not automatic escalation"),
            SelectionOutcome::Waiting {
                reason: WaitReason::CapacityOrUnavailable,
                ..
            }
        ));
    }

    #[test]
    fn infrastructure_and_input_failures_never_escalate_tokens() {
        for (failure, expected) in [
            (FailureReason::SetupError, WaitReason::SetupRequired),
            (FailureReason::BlockedInput, WaitReason::NeedsInput),
        ] {
            let mut blocked = request();
            blocked.current_model = Some("own-model".into());
            blocked.last_failure = Some(failure);
            blocked.allow_escalation = true;
            blocked.allow_capacity_fallback = true;
            blocked.attempts_by_tier.insert(0, 2);
            assert_eq!(
                registry()
                    .select(&blocked, &CapacitySnapshot::default())
                    .expect("infrastructure block is visible"),
                SelectionOutcome::Waiting {
                    model_id: Some("own-model".into()),
                    reason: expected
                }
            );
        }
    }

    #[test]
    fn graph_cycles_unknown_targets_and_secret_values_fail_validation() {
        let mut config = registry();
        config.models[0].tier = -5;
        config
            .validate()
            .expect("negative priority supports lower custom tiers");
        config.models[0].tier = 1;
        config.models[1].escalate_to = Some("own-model".into());
        assert!(config.validate().is_err());
        config = registry();
        config.models[0].escalate_to = Some("missing".into());
        assert!(config.validate().is_err());
        for endpoint in [
            "https://user:password@example.test/v1",
            "https://example.test/v1?api_key=private",
        ] {
            config = registry();
            config.models[0].endpoint = Some(endpoint.into());
            assert!(config.validate().is_err());
        }
        config = registry();
        config.models[0].credential_file_ref =
            Some("sk-this-is-a-synthetic-secret-marker-only".into());
        assert!(config.validate().is_err());
        config = registry();
        config.models[0].label = "sk-this-is-a-synthetic-secret-marker-only".into();
        assert!(config.validate().is_err());
        let mut value = serde_json::to_value(registry()).expect("registry fixture JSON");
        value["models"][0]["api_key"] = serde_json::json!("forbidden plaintext field");
        assert!(serde_json::from_value::<ModelRegistry>(value).is_err());
    }

    #[test]
    fn atomic_revision_update_preserves_previous_configuration_on_invalid_or_stale_write() {
        let (_temporary, file) = file_fixture();
        assert_eq!(
            file.read().expect("missing registry is empty").revision,
            "0"
        );
        let first = file
            .replace("0", registry().models)
            .expect("initial configuration");
        assert_eq!(first.revision, "1");
        assert!(
            matches!(file.replace("0", first.models.clone()), Err(RegistryError::RevisionConflict { current }) if current == "1")
        );
        let mut invalid = first.models.clone();
        invalid[0].max_concurrency = 0;
        assert!(file.replace("1", invalid).is_err());
        assert_eq!(file.read().expect("valid prior registry retained"), first);
        assert_eq!(
            file.replace("1", first.models)
                .expect("next configuration")
                .revision,
            "2"
        );
    }

    #[test]
    fn concurrent_writers_cannot_overwrite_a_single_expected_revision_twice() {
        let (_temporary, file) = file_fixture();
        file.replace("0", registry().models)
            .expect("initial configuration");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = [8, 12]
            .into_iter()
            .map(|limit| {
                let file = file.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut models = registry().models;
                    models[0].max_concurrency = limit;
                    barrier.wait();
                    file.replace("1", models)
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("fixture writer joins"))
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            file.read().expect("concurrent result readable").revision,
            "2"
        );
    }

    #[cfg(unix)]
    #[test]
    fn registry_rejects_symlinks_hardlinks_nonprivate_files_and_oversized_json() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (temporary, file) = file_fixture();
        file.replace("0", registry().models)
            .expect("private configuration");
        let alias = temporary.path().join("alias.json");
        symlink(&file.path, &alias).expect("fixture symlink");
        assert!(
            RegistryFile::new(alias)
                .expect("parent remains private")
                .read()
                .is_err()
        );
        let hardlink = temporary.path().join("hardlink.json");
        std::fs::hard_link(&file.path, &hardlink).expect("fixture hardlink");
        assert!(file.read().is_err());
        std::fs::remove_file(hardlink).expect("remove disposable hardlink");
        std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o644))
            .expect("nonprivate fixture mode");
        assert!(file.read().is_err());
        std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o600))
            .expect("restore private mode");
        std::fs::write(&file.path, vec![b' '; MAX_REGISTRY_BYTES + 1])
            .expect("oversized fixture input");
        assert!(file.read().is_err());
    }

    #[test]
    fn ornith_example_is_configuration_not_zero_price_or_native_qualification() {
        let example: ModelRegistry =
            serde_json::from_str(include_str!("../../../configs/examples/models.json"))
                .expect("example registry JSON");
        example.validate().expect("example model policy");
        assert_eq!(
            selected(
                example
                    .select(&request(), &CapacitySnapshot::default())
                    .expect("configured cheapest tier")
            ),
            "ornith"
        );
        assert_eq!(
            serde_json::to_value(UsageCost::Unknown).expect("unknown cost JSON"),
            serde_json::json!({"status":"unknown"})
        );
    }
}
