//! Pure parsing and precedence resolution for public configuration layers.
//! Private policy is supplied by a trusted caller and is never part of the
//! public layer format.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};
use thiserror::Error;

const MAX_LAYER_BYTES: usize = 256 * 1024;
const MAX_ITEMS: usize = 256;
const MAX_TEXT: usize = 4096;
const MAX_PATH_COMPONENTS: usize = 128;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ConfigLayerName {
    Defaults,
    User,
    Project,
    Runtime,
}

impl ConfigLayerName {
    fn rank(self) -> u8 {
        match self {
            Self::Defaults => 0,
            Self::User => 1,
            Self::Project => 2,
            Self::Runtime => 3,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub id: String,
    pub label: String,
    pub address: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub id: String,
    pub provider: String,
    pub model_ref: String,
    pub node_id: Option<String>,
    pub max_concurrency: u16,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigPatch {
    pub listen: Option<String>,
    pub max_agents: Option<u16>,
    pub max_context_tokens: Option<u32>,
    pub nodes: Option<Vec<NodeConfig>>,
    pub models: Option<Vec<ModelConfig>>,
    pub denied_paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigLayer {
    pub layer: ConfigLayerName,
    pub source_ref: String,
    pub values: ConfigPatch,
}

/// Trusted immutable limits and denials, supplied independently from public config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivatePolicy {
    pub max_agents: u16,
    pub max_context_tokens: u32,
    pub denied_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceOrigin {
    Defaults,
    User,
    Project,
    Runtime,
    Private,
}

impl From<ConfigLayerName> for ProvenanceOrigin {
    fn from(value: ConfigLayerName) -> Self {
        match value {
            ConfigLayerName::Defaults => Self::Defaults,
            ConfigLayerName::User => Self::User,
            ConfigLayerName::Project => Self::Project,
            ConfigLayerName::Runtime => Self::Runtime,
        }
    }
}

/// Ordered contributors remain available even when a later public value wins.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvenanceContribution {
    pub origin: ProvenanceOrigin,
    /// Private policy has no public source identity and is never relabeled Runtime.
    pub source_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigLayerReceipt {
    pub layer: ConfigLayerName,
    pub source_ref: String,
    /// Binds the complete typed public layer, including values later overridden.
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub listen: String,
    pub max_agents: u16,
    pub max_context_tokens: u32,
    pub nodes: Vec<NodeConfig>,
    pub models: Vec<ModelConfig>,
    pub denied_paths: Vec<String>,
    pub provenance: BTreeMap<String, ProvenanceOrigin>,
    pub provenance_history: BTreeMap<String, Vec<ProvenanceContribution>>,
    pub source_layers: Vec<ConfigLayerReceipt>,
    pub private_policy: PrivatePolicy,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("configuration layer exceeds 256 KiB")]
    LayerTooLarge,
    #[error("invalid configuration JSON: {0}")]
    Json(String),
    #[error("invalid configuration: {0}")]
    Invalid(&'static str),
    #[error("configuration layer order is invalid")]
    LayerOrder,
    #[error("defaults layer is missing or incomplete")]
    DefaultsIncomplete,
    #[error("public limit exceeds immutable private policy")]
    PrivateCapExceeded,
    #[error("duplicate identifier")]
    DuplicateId,
    #[error("model references an unknown node")]
    UnknownNode,
    #[error("could not digest configuration layer: {0}")]
    Digest(String),
}

/// Parse one bounded JSON layer. Unknown keys, including any attempted private
/// policy override, fail closed through `deny_unknown_fields`.
pub fn parse_layer(bytes: &[u8]) -> Result<ConfigLayer, ConfigError> {
    if bytes.len() > MAX_LAYER_BYTES {
        return Err(ConfigError::LayerTooLarge);
    }
    let layer: ConfigLayer =
        serde_json::from_slice(bytes).map_err(|e| ConfigError::Json(e.to_string()))?;
    validate_layer(&layer)?;
    Ok(layer)
}

/// Resolve ordered public patches under caller-owned immutable private policy.
/// Node and model lists replace as whole values; denied paths only accumulate.
pub fn resolve_config(
    layers: &[ConfigLayer],
    private_policy: &PrivatePolicy,
) -> Result<ResolvedConfig, ConfigError> {
    if layers.is_empty() || layers.len() > 4 || layers[0].layer != ConfigLayerName::Defaults {
        return Err(ConfigError::DefaultsIncomplete);
    }
    let mut prior = None;
    for layer in layers {
        validate_layer(layer)?;
        let rank = layer.layer.rank();
        if prior.is_some_and(|p| rank <= p) {
            return Err(ConfigError::LayerOrder);
        }
        prior = Some(rank);
    }
    if !defaults_complete(&layers[0].values) {
        return Err(ConfigError::DefaultsIncomplete);
    }
    validate_policy(private_policy)?;
    for layer in layers {
        if layer
            .values
            .max_agents
            .is_some_and(|v| v > private_policy.max_agents)
            || layer
                .values
                .max_context_tokens
                .is_some_and(|v| v > private_policy.max_context_tokens)
            || layer.values.models.as_ref().is_some_and(|models| {
                models
                    .iter()
                    .any(|model| model.max_concurrency > private_policy.max_agents)
            })
        {
            return Err(ConfigError::PrivateCapExceeded);
        }
    }

    let mut listen = None;
    let mut max_agents = None;
    let mut max_context_tokens = None;
    let mut nodes = None;
    let mut models = None;
    let mut denied = BTreeSet::new();
    let mut provenance = BTreeMap::new();
    let mut provenance_history = BTreeMap::new();
    let mut source_layers = Vec::with_capacity(layers.len());

    for layer in layers {
        let values = &layer.values;
        let origin = ProvenanceOrigin::from(layer.layer);
        source_layers.push(ConfigLayerReceipt {
            layer: layer.layer,
            source_ref: layer.source_ref.clone(),
            digest: crate::content_digest(layer)
                .map_err(|error| ConfigError::Digest(error.to_string()))?,
        });
        let mut record = |field: &str| {
            record_contribution(
                &mut provenance,
                &mut provenance_history,
                field.to_owned(),
                origin,
                Some(&layer.source_ref),
            );
        };
        if let Some(value) = &values.listen {
            listen = Some(value.clone());
            record("listen");
        }
        if let Some(value) = values.max_agents {
            max_agents = Some(value);
            record("max_agents");
        }
        if let Some(value) = values.max_context_tokens {
            max_context_tokens = Some(value);
            record("max_context_tokens");
        }
        if let Some(value) = &values.nodes {
            nodes = Some(value.clone());
            record("nodes");
        }
        if let Some(value) = &values.models {
            models = Some(value.clone());
            record("models");
        }
        if let Some(paths) = &values.denied_paths {
            record("denied_paths");
            for path in paths {
                denied.insert(path.clone());
                record(&format!("denied_paths.{path}"));
            }
        }
    }

    let listen = listen.ok_or(ConfigError::DefaultsIncomplete)?;
    let max_agents = max_agents.ok_or(ConfigError::DefaultsIncomplete)?;
    let max_context_tokens = max_context_tokens.ok_or(ConfigError::DefaultsIncomplete)?;
    let nodes = nodes.ok_or(ConfigError::DefaultsIncomplete)?;
    let models = models.ok_or(ConfigError::DefaultsIncomplete)?;
    if max_agents == 0 || max_context_tokens == 0 {
        return Err(ConfigError::Invalid("limits must be nonzero"));
    }
    if max_agents > private_policy.max_agents
        || max_context_tokens > private_policy.max_context_tokens
    {
        return Err(ConfigError::PrivateCapExceeded);
    }

    validate_effective_nodes_models(&nodes, &models, max_agents)?;
    for path in &private_policy.denied_paths {
        denied.insert(path.clone());
        record_contribution(
            &mut provenance,
            &mut provenance_history,
            format!("denied_paths.{path}"),
            ProvenanceOrigin::Private,
            None,
        );
    }
    if !private_policy.denied_paths.is_empty() {
        record_contribution(
            &mut provenance,
            &mut provenance_history,
            "denied_paths".into(),
            ProvenanceOrigin::Private,
            None,
        );
    }

    Ok(ResolvedConfig {
        listen,
        max_agents,
        max_context_tokens,
        nodes,
        models,
        denied_paths: denied.into_iter().collect(),
        provenance,
        provenance_history,
        source_layers,
        private_policy: private_policy.clone(),
    })
}

fn record_contribution(
    provenance: &mut BTreeMap<String, ProvenanceOrigin>,
    history: &mut BTreeMap<String, Vec<ProvenanceContribution>>,
    key: String,
    origin: ProvenanceOrigin,
    source_ref: Option<&str>,
) {
    provenance.insert(key.clone(), origin);
    history
        .entry(key)
        .or_default()
        .push(ProvenanceContribution {
            origin,
            source_ref: source_ref.map(str::to_owned),
        });
}

fn defaults_complete(patch: &ConfigPatch) -> bool {
    patch.listen.is_some()
        && patch.max_agents.is_some()
        && patch.max_context_tokens.is_some()
        && patch.nodes.is_some()
        && patch.models.is_some()
        && patch.denied_paths.is_some()
}

fn validate_layer(layer: &ConfigLayer) -> Result<(), ConfigError> {
    valid_text(&layer.source_ref, 128, false)?;
    let p = &layer.values;
    if let Some(s) = &p.listen {
        valid_text(s, MAX_TEXT, false)?;
    }
    if p.max_agents == Some(0) || p.max_context_tokens == Some(0) {
        return Err(ConfigError::Invalid("limits must be nonzero"));
    }
    if let Some(nodes) = &p.nodes {
        bounded(nodes.len())?;
        let mut ids = BTreeSet::new();
        for node in nodes {
            valid_id(&node.id)?;
            if !ids.insert(&node.id) {
                return Err(ConfigError::DuplicateId);
            }
            valid_text(&node.label, MAX_TEXT, false)?;
            if let Some(address) = &node.address {
                valid_text(address, MAX_TEXT, false)?;
            }
        }
    }
    if let Some(models) = &p.models {
        bounded(models.len())?;
        let mut ids = BTreeSet::new();
        for model in models {
            valid_id(&model.id)?;
            if !ids.insert(&model.id) {
                return Err(ConfigError::DuplicateId);
            }
            valid_text(&model.provider, MAX_TEXT, false)?;
            valid_text(&model.model_ref, MAX_TEXT, false)?;
            if let Some(node_id) = &model.node_id {
                valid_id(node_id)?;
            }
            if model.max_concurrency == 0 {
                return Err(ConfigError::Invalid("model concurrency must be nonzero"));
            }
        }
    }
    if let Some(paths) = &p.denied_paths {
        bounded(paths.len())?;
        for path in paths {
            valid_path(path)?;
        }
    }
    // Programmatic callers get the same aggregate bound as parsed public layers.
    serde_json::to_writer(LayerByteLimit(MAX_LAYER_BYTES), layer)
        .map_err(|_| ConfigError::LayerTooLarge)?;
    Ok(())
}

struct LayerByteLimit(usize);

impl Write for LayerByteLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_policy(policy: &PrivatePolicy) -> Result<(), ConfigError> {
    if policy.max_agents == 0 || policy.max_context_tokens == 0 {
        return Err(ConfigError::Invalid("private limits must be nonzero"));
    }
    bounded(policy.denied_paths.len())?;
    for path in &policy.denied_paths {
        valid_path(path)?;
    }
    Ok(())
}

fn validate_effective_nodes_models(
    nodes: &[NodeConfig],
    models: &[ModelConfig],
    max_agents: u16,
) -> Result<(), ConfigError> {
    let mut node_ids = BTreeSet::new();
    for node in nodes {
        if !node_ids.insert(&node.id) {
            return Err(ConfigError::DuplicateId);
        }
    }
    let mut model_ids = BTreeSet::new();
    for model in models {
        if !model_ids.insert(&model.id) {
            return Err(ConfigError::DuplicateId);
        }
        if model.max_concurrency == 0 || model.max_concurrency > max_agents {
            return Err(ConfigError::Invalid(
                "model concurrency is outside effective agent limit",
            ));
        }
        if model
            .node_id
            .as_ref()
            .is_some_and(|id| !node_ids.contains(id))
        {
            return Err(ConfigError::UnknownNode);
        }
    }
    Ok(())
}

fn bounded(length: usize) -> Result<(), ConfigError> {
    if length > MAX_ITEMS {
        Err(ConfigError::Invalid("list exceeds 256 entries"))
    } else {
        Ok(())
    }
}

fn valid_id(value: &str) -> Result<(), ConfigError> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(ConfigError::Invalid("identifier is not portable ASCII"));
    }
    Ok(())
}

fn valid_text(value: &str, max: usize, allow_empty: bool) -> Result<(), ConfigError> {
    if value.len() > max
        || (!allow_empty && value.trim().is_empty())
        || value.chars().any(char::is_control)
    {
        return Err(ConfigError::Invalid(
            "text is empty, oversized, or contains control characters",
        ));
    }
    Ok(())
}

fn valid_path(value: &str) -> Result<(), ConfigError> {
    if value.is_empty()
        || value.len() > MAX_TEXT
        || !value.is_ascii()
        || value.starts_with('/')
        || value.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
        || value.chars().any(char::is_control)
        || value.split('/').count() > MAX_PATH_COMPONENTS
        || value.split('/').any(|part| {
            let base = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ((base.starts_with("COM") || base.starts_with("LPT"))
                    && base.len() == 4
                    && matches!(base.as_bytes()[3], b'1'..=b'9'))
        })
    {
        return Err(ConfigError::Invalid(
            "denied path must be a safe relative component path",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> PrivatePolicy {
        PrivatePolicy {
            max_agents: 8,
            max_context_tokens: 32_000,
            denied_paths: vec!["secrets".into()],
        }
    }

    fn layer(layer: ConfigLayerName, values: ConfigPatch) -> ConfigLayer {
        ConfigLayer {
            layer,
            source_ref: format!("{layer:?}-test"),
            values,
        }
    }

    fn defaults() -> ConfigLayer {
        layer(
            ConfigLayerName::Defaults,
            ConfigPatch {
                listen: Some("127.0.0.1:7000".into()),
                max_agents: Some(4),
                max_context_tokens: Some(16_000),
                nodes: Some(vec![NodeConfig {
                    id: "arbitrary-node.7".into(),
                    label: "Build host".into(),
                    address: Some("host.example:7".into()),
                }]),
                models: Some(vec![ModelConfig {
                    id: "model-x".into(),
                    provider: "provider-z".into(),
                    model_ref: "vendor/custom-42".into(),
                    node_id: Some("arbitrary-node.7".into()),
                    max_concurrency: 2,
                }]),
                denied_paths: Some(vec!["build/cache".into()]),
            },
        )
    }

    #[test]
    fn four_layer_precedence_and_field_provenance() {
        let layers = vec![
            defaults(),
            layer(
                ConfigLayerName::User,
                ConfigPatch {
                    listen: Some("127.0.0.1:7001".into()),
                    ..Default::default()
                },
            ),
            layer(
                ConfigLayerName::Project,
                ConfigPatch {
                    max_agents: Some(6),
                    nodes: Some(vec![]),
                    denied_paths: Some(vec!["workspace/private".into()]),
                    ..Default::default()
                },
            ),
            layer(
                ConfigLayerName::Runtime,
                ConfigPatch {
                    max_context_tokens: Some(24_000),
                    models: Some(vec![]),
                    ..Default::default()
                },
            ),
        ];
        let resolved = resolve_config(&layers, &policy()).unwrap();
        assert_eq!(resolved.listen, "127.0.0.1:7001");
        assert_eq!(resolved.max_agents, 6);
        assert_eq!(resolved.max_context_tokens, 24_000);
        assert!(resolved.nodes.is_empty());
        assert!(resolved.models.is_empty());
        assert_eq!(resolved.provenance["listen"], ProvenanceOrigin::User);
        assert_eq!(resolved.provenance["max_agents"], ProvenanceOrigin::Project);
        assert_eq!(
            resolved.provenance["max_context_tokens"],
            ProvenanceOrigin::Runtime
        );
        assert_eq!(resolved.provenance["nodes"], ProvenanceOrigin::Project);
        assert_eq!(resolved.provenance["models"], ProvenanceOrigin::Runtime);
        assert_eq!(
            resolved.provenance["denied_paths.build/cache"],
            ProvenanceOrigin::Defaults
        );
        assert_eq!(
            resolved.provenance["denied_paths.workspace/private"],
            ProvenanceOrigin::Project
        );
        assert_eq!(
            resolved.provenance["denied_paths.secrets"],
            ProvenanceOrigin::Private
        );
    }

    #[test]
    fn partial_layers_and_defaults_order_errors() {
        let partial = layer(
            ConfigLayerName::User,
            ConfigPatch {
                listen: Some("x".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            resolve_config(std::slice::from_ref(&partial), &policy()).unwrap_err(),
            ConfigError::DefaultsIncomplete
        );
        assert_eq!(
            resolve_config(&[defaults(), partial.clone(), partial], &policy()).unwrap_err(),
            ConfigError::LayerOrder
        );
        assert_eq!(
            resolve_config(
                &[layer(ConfigLayerName::Defaults, ConfigPatch::default())],
                &policy()
            )
            .unwrap_err(),
            ConfigError::DefaultsIncomplete
        );
        let mut missing = defaults();
        missing.values.models = None;
        assert_eq!(
            resolve_config(&[missing], &policy()).unwrap_err(),
            ConfigError::DefaultsIncomplete
        );
        let runtime = layer(ConfigLayerName::Runtime, ConfigPatch::default());
        let project = layer(ConfigLayerName::Project, ConfigPatch::default());
        let user = layer(ConfigLayerName::User, ConfigPatch::default());
        assert_eq!(
            resolve_config(&[defaults(), runtime, project.clone()], &policy()).unwrap_err(),
            ConfigError::LayerOrder
        );
        assert_eq!(
            resolve_config(
                &[
                    defaults(),
                    user,
                    project,
                    layer(ConfigLayerName::Runtime, ConfigPatch::default()),
                    layer(ConfigLayerName::Runtime, ConfigPatch::default())
                ],
                &policy()
            )
            .unwrap_err(),
            ConfigError::DefaultsIncomplete
        );
    }

    #[test]
    fn parser_rejects_unknown_private_keys_and_oversize_input() {
        let bytes = br#"{"layer":"defaults","source_ref":"d","private_policy":{"max_agents":9},"values":{}}"#;
        assert!(matches!(parse_layer(bytes), Err(ConfigError::Json(_))));
        let nested = br#"{"layer":"defaults","source_ref":"d","values":{"private_policy":{}}}"#;
        assert!(matches!(parse_layer(nested), Err(ConfigError::Json(_))));
        assert_eq!(
            parse_layer(&vec![b' '; MAX_LAYER_BYTES + 1]).unwrap_err(),
            ConfigError::LayerTooLarge
        );
        let unknown_node = br#"{"layer":"defaults","source_ref":"d","values":{"nodes":[{"id":"n","label":"N","address":null,"extra":true}]}}"#;
        assert!(matches!(
            parse_layer(unknown_node),
            Err(ConfigError::Json(_))
        ));
    }

    #[test]
    fn private_caps_and_denials_survive_public_overrides() {
        let mut high = defaults();
        high.values.max_agents = Some(9);
        assert_eq!(
            resolve_config(&[high], &policy()).unwrap_err(),
            ConfigError::PrivateCapExceeded
        );
        let mut shadowed_high = defaults();
        shadowed_high.values.max_agents = Some(9);
        let lowered = layer(
            ConfigLayerName::Runtime,
            ConfigPatch {
                max_agents: Some(4),
                ..Default::default()
            },
        );
        assert_eq!(
            resolve_config(&[shadowed_high, lowered], &policy()).unwrap_err(),
            ConfigError::PrivateCapExceeded
        );
        let mut high = defaults();
        high.values.max_context_tokens = Some(32_001);
        assert_eq!(
            resolve_config(&[high], &policy()).unwrap_err(),
            ConfigError::PrivateCapExceeded
        );
        let resolved = resolve_config(
            &[
                defaults(),
                layer(
                    ConfigLayerName::Runtime,
                    ConfigPatch {
                        max_agents: Some(8),
                        max_context_tokens: Some(32_000),
                        denied_paths: Some(vec![]),
                        ..Default::default()
                    },
                ),
            ],
            &policy(),
        )
        .unwrap();
        assert_eq!(resolved.private_policy, policy());
        assert!(resolved.denied_paths.contains(&"secrets".to_owned()));
        assert_eq!(
            resolved.provenance["denied_paths.secrets"],
            ProvenanceOrigin::Private
        );
    }

    #[test]
    fn arbitrary_data_supported_and_effective_references_are_checked() {
        let mut data = defaults();
        data.values.nodes = Some(vec![NodeConfig {
            id: "worker.custom_12".into(),
            label: "Remote builder".into(),
            address: None,
        }]);
        data.values.models = Some(vec![ModelConfig {
            id: "m.anything-2".into(),
            provider: "provider-a".into(),
            model_ref: "model/from-catalog".into(),
            node_id: Some("worker.custom_12".into()),
            max_concurrency: 4,
        }]);
        assert!(resolve_config(&[data.clone()], &policy()).is_ok());
        data.values.models.as_mut().unwrap()[0].node_id = Some("missing-node".into());
        assert_eq!(
            resolve_config(&[data.clone()], &policy()).unwrap_err(),
            ConfigError::UnknownNode
        );
        data.values.models = Some(vec![
            data.values.models.as_ref().unwrap()[0].clone(),
            data.values.models.as_ref().unwrap()[0].clone(),
        ]);
        data.values.models.as_mut().unwrap()[0].node_id = None;
        data.values.models.as_mut().unwrap()[1].node_id = None;
        assert_eq!(
            resolve_config(&[data], &policy()).unwrap_err(),
            ConfigError::DuplicateId
        );
    }

    #[test]
    fn public_denies_union_monotonically_and_replay_is_deterministic() {
        let layers = vec![
            defaults(),
            layer(
                ConfigLayerName::User,
                ConfigPatch {
                    denied_paths: Some(vec!["user/cache".into(), "build/cache".into()]),
                    ..Default::default()
                },
            ),
        ];
        let before = layers.clone();
        let a = resolve_config(&layers, &policy()).unwrap();
        let b = resolve_config(&layers, &policy()).unwrap();
        assert_eq!(a, b);
        assert_eq!(layers, before);
        assert_eq!(a.denied_paths, vec!["build/cache", "secrets", "user/cache"]);
        assert_eq!(
            a.provenance["denied_paths.build/cache"],
            ProvenanceOrigin::User
        );
        let erased = vec![
            layers[0].clone(),
            layer(
                ConfigLayerName::User,
                ConfigPatch {
                    denied_paths: Some(vec![]),
                    ..Default::default()
                },
            ),
        ];
        assert!(
            resolve_config(&erased, &policy())
                .unwrap()
                .denied_paths
                .contains(&"build/cache".into())
        );
    }

    #[test]
    fn duplicate_ids_bounds_and_unsafe_denied_paths_rejected() {
        let mut duplicate = defaults();
        duplicate.values.nodes = Some(vec![
            NodeConfig {
                id: "n".into(),
                label: "a".into(),
                address: None,
            },
            NodeConfig {
                id: "n".into(),
                label: "b".into(),
                address: None,
            },
        ]);
        assert_eq!(
            resolve_config(&[duplicate], &policy()).unwrap_err(),
            ConfigError::DuplicateId
        );
        for path in ["../escape", "/absolute", "a\\b", "a//b", "./x", "a\0b"] {
            let mut layer = defaults();
            layer.values.denied_paths = Some(vec![path.into()]);
            assert!(
                resolve_config(&[layer], &policy()).is_err(),
                "accepted {path:?}"
            );
        }
        let too_many = vec!["a".to_owned(); MAX_ITEMS + 1];
        let mut layer = defaults();
        layer.values.denied_paths = Some(too_many);
        assert!(resolve_config(&[layer], &policy()).is_err());
        let mut bad_model = defaults();
        bad_model.values.models.as_mut().unwrap()[0].max_concurrency = 5;
        assert!(resolve_config(&[bad_model], &policy()).is_err());
    }

    #[test]
    fn text_identifier_and_each_list_bound_are_enforced() {
        let mut long_label = defaults();
        long_label.values.nodes.as_mut().unwrap()[0].label = "x".repeat(MAX_TEXT + 1);
        assert!(resolve_config(&[long_label], &policy()).is_err());

        let mut long_address = defaults();
        long_address.values.nodes.as_mut().unwrap()[0].address = Some("x".repeat(MAX_TEXT + 1));
        assert!(resolve_config(&[long_address], &policy()).is_err());

        let mut long_provider = defaults();
        long_provider.values.models.as_mut().unwrap()[0].provider = "x".repeat(MAX_TEXT + 1);
        assert!(resolve_config(&[long_provider], &policy()).is_err());

        let mut long_ref = defaults();
        long_ref.values.models.as_mut().unwrap()[0].model_ref = "x".repeat(MAX_TEXT + 1);
        assert!(resolve_config(&[long_ref], &policy()).is_err());

        let mut long_id = defaults();
        long_id.values.nodes.as_mut().unwrap()[0].id = "a".repeat(129);
        assert!(resolve_config(&[long_id], &policy()).is_err());

        let mut long_source = defaults();
        long_source.source_ref = "x".repeat(129);
        assert!(resolve_config(&[long_source], &policy()).is_err());

        let mut many_nodes = defaults();
        many_nodes.values.nodes = Some(
            (0..=MAX_ITEMS)
                .map(|n| NodeConfig {
                    id: format!("node-{n}"),
                    label: "node".into(),
                    address: None,
                })
                .collect(),
        );
        assert!(resolve_config(&[many_nodes], &policy()).is_err());

        let mut many_models = defaults();
        many_models.values.models = Some(
            (0..=MAX_ITEMS)
                .map(|n| ModelConfig {
                    id: format!("model-{n}"),
                    provider: "provider".into(),
                    model_ref: "model".into(),
                    node_id: None,
                    max_concurrency: 1,
                })
                .collect(),
        );
        assert!(resolve_config(&[many_models], &policy()).is_err());
    }

    #[test]
    fn layer_and_source_shape_are_validated() {
        let mut bad = defaults();
        bad.source_ref = "\nsecret".into();
        assert!(resolve_config(&[bad], &policy()).is_err());
        let raw = json!({"layer":"runtime", "source_ref":"r", "values":{"listen":"0.0.0.0:1"}});
        assert_eq!(
            parse_layer(raw.to_string().as_bytes()).unwrap().layer,
            ConfigLayerName::Runtime
        );
    }

    #[test]
    fn shadowed_model_duplicates_zero_and_private_cap_violations_are_rejected() {
        let erased = || {
            layer(
                ConfigLayerName::Runtime,
                ConfigPatch {
                    models: Some(vec![]),
                    nodes: Some(vec![]),
                    ..Default::default()
                },
            )
        };
        let mut high = defaults();
        high.values.models.as_mut().unwrap()[0].max_concurrency = policy().max_agents + 1;
        assert_eq!(
            resolve_config(&[high], &policy()).unwrap_err(),
            ConfigError::PrivateCapExceeded
        );
        let mut high = defaults();
        high.values.models.as_mut().unwrap()[0].max_concurrency = policy().max_agents + 1;
        assert_eq!(
            resolve_config(&[high, erased()], &policy()).unwrap_err(),
            ConfigError::PrivateCapExceeded
        );
        let mut zero = defaults();
        zero.values.models.as_mut().unwrap()[0].max_concurrency = 0;
        assert!(matches!(
            resolve_config(&[zero, erased()], &policy()),
            Err(ConfigError::Invalid(_))
        ));

        let mut duplicate_models = defaults();
        let model = duplicate_models.values.models.as_ref().unwrap()[0].clone();
        duplicate_models.values.models.as_mut().unwrap().push(model);
        assert_eq!(
            resolve_config(&[duplicate_models, erased()], &policy()).unwrap_err(),
            ConfigError::DuplicateId
        );
        let mut duplicate_nodes = defaults();
        let node = duplicate_nodes.values.nodes.as_ref().unwrap()[0].clone();
        duplicate_nodes.values.nodes.as_mut().unwrap().push(node);
        assert_eq!(
            resolve_config(&[duplicate_nodes, erased()], &policy()).unwrap_err(),
            ConfigError::DuplicateId
        );
    }

    #[test]
    fn portable_denials_and_programmatic_layer_sizes_are_bounded() {
        let paths = [
            "a\nb".to_owned(),
            "a\u{7f}b".to_owned(),
            "a/child.".to_owned(),
            "a/child ".to_owned(),
            "a/*".to_owned(),
            "a/CON.txt".to_owned(),
            "a/com1".to_owned(),
            "a/lpt9.log".to_owned(),
            "a/résumé".to_owned(),
            "x".repeat(MAX_TEXT + 1),
            vec!["a"; MAX_PATH_COMPONENTS + 1].join("/"),
        ];
        for path in paths {
            let mut public = defaults();
            public.values.denied_paths = Some(vec![path.clone()]);
            assert!(
                resolve_config(&[public], &policy()).is_err(),
                "public: {path:?}"
            );
            let mut private = policy();
            private.denied_paths = vec![path.clone()];
            assert!(
                resolve_config(&[defaults()], &private).is_err(),
                "private: {path:?}"
            );
        }
        let mut large = defaults();
        large.values.nodes = Some(
            (0..100)
                .map(|index| NodeConfig {
                    id: format!("node-{index}"),
                    label: "x".repeat(MAX_TEXT),
                    address: None,
                })
                .collect(),
        );
        large.values.models = Some(vec![]);
        assert_eq!(
            resolve_config(&[large], &policy()).unwrap_err(),
            ConfigError::LayerTooLarge
        );
    }

    #[test]
    fn provenance_retains_source_identities_and_overwritten_layer_receipts() {
        let mut first = defaults();
        first.values.denied_paths = Some(vec!["secrets".into()]);
        let public = layer(
            ConfigLayerName::User,
            ConfigPatch {
                listen: Some("127.0.0.1:7444".into()),
                denied_paths: Some(vec!["secrets".into()]),
                ..Default::default()
            },
        );
        let layers = vec![first, public];
        let before = layers.clone();
        let private = policy();
        let resolved = resolve_config(&layers, &private).unwrap();
        assert_eq!(layers, before);
        assert_eq!(private, policy());
        assert_eq!(resolved.private_policy, private);
        assert_eq!(resolved.provenance["listen"], ProvenanceOrigin::User);
        assert_eq!(
            resolved.provenance_history["listen"],
            [
                ProvenanceContribution {
                    origin: ProvenanceOrigin::Defaults,
                    source_ref: Some(layers[0].source_ref.clone()),
                },
                ProvenanceContribution {
                    origin: ProvenanceOrigin::User,
                    source_ref: Some(layers[1].source_ref.clone()),
                },
            ]
        );
        assert_eq!(
            resolved.provenance_history["denied_paths.secrets"]
                .iter()
                .map(|entry| entry.origin)
                .collect::<Vec<_>>(),
            [
                ProvenanceOrigin::Defaults,
                ProvenanceOrigin::User,
                ProvenanceOrigin::Private
            ]
        );
        assert_eq!(
            resolved.provenance_history["denied_paths.secrets"][2].source_ref,
            None
        );
        assert_eq!(resolved.source_layers.len(), 2);
        assert_eq!(
            resolved.source_layers[0].digest,
            crate::content_digest(&layers[0]).unwrap()
        );
        let mut changed = layers;
        changed[0].values.listen = Some("127.0.0.1:7555".into());
        let other = resolve_config(&changed, &private).unwrap();
        assert_eq!(resolved.listen, other.listen);
        assert_ne!(
            resolved.source_layers[0].digest,
            other.source_layers[0].digest
        );
        assert_eq!(resolved.source_layers[1], other.source_layers[1]);
    }

    #[test]
    fn duplicate_json_fields_and_private_invalid_limits_fail_closed() {
        for raw in [
            br#"{"layer":"defaults","layer":"runtime","source_ref":"d","values":{}}"#.as_slice(),
            br#"{"layer":"defaults","source_ref":"d","values":{"max_agents":1,"max_agents":2}}"#
                .as_slice(),
        ] {
            assert!(matches!(parse_layer(raw), Err(ConfigError::Json(_))));
        }
        let mut private = policy();
        private.max_agents = 0;
        assert!(matches!(
            resolve_config(&[defaults()], &private),
            Err(ConfigError::Invalid(_))
        ));
        let mut private = policy();
        private.max_context_tokens = 0;
        assert!(matches!(
            resolve_config(&[defaults()], &private),
            Err(ConfigError::Invalid(_))
        ));
    }
}
