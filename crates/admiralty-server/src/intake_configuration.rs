//! Explicit, owner-only intake startup policy. Never an HTTP/request issuer.
//!
//! This is a static startup snapshot, not a file watcher. Registry revocation is
//! still checked by the existing bridge on every service/writer/replay turn.
//! The private-file reader bounds bytes before decode; collection visitors cap
//! allocations while decoding. Individual strings are bounded after decode
//! within that hard file limit. No protection against root/same-UID mutation is
//! claimed, and best-effort token clearing is not cryptographic zeroization.
use crate::{
    services::{
        work_order_intake::{
            IntakeScopeGrant, MAX_GRANTS_PER_CREDENTIAL, MAX_INTAKE_CREDENTIALS,
            MAX_TOTAL_INTAKE_GRANTS, WorkOrderIntakeAction, WorkOrderIntakeAuthRegistry,
            WorkOrderIntakeBinding, WorkOrderIntakeCredential, WorkOrderIntakePolicy,
        },
        work_order_routes::ExactLoopbackOrigins,
    },
    startup::{TrustedIntakeComposition, read_private_file, read_private_token},
};
use admiralty_core::{PrincipalId, ProjectId, RootId, intake::IntakePolicy};
use serde::{
    Deserialize, Deserializer,
    de::{self, IgnoredAny, SeqAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_POLICIES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntakeConfigurationError {
    Unavailable,
    Invalid,
}
impl fmt::Display for IntakeConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "intake-configuration-unavailable",
            Self::Invalid => "intake-configuration-invalid",
        })
    }
}
impl std::error::Error for IntakeConfigurationError {}
type Result<T> = std::result::Result<T, IntakeConfigurationError>;

struct Items<T, const N: usize>(Vec<T>);
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Items<T, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Bounded<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for Bounded<T, N> {
            type Value = Items<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded array")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                if seq.size_hint().is_some_and(|n| n > N) {
                    return Err(de::Error::custom("array limit"));
                }
                let mut values = Vec::new();
                while values.len() < N {
                    match seq.next_element()? {
                        Some(value) => values.push(value),
                        None => return Ok(Items(values)),
                    }
                }
                if seq.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("array limit"));
                }
                Ok(Items(values))
            }
        }
        d.deserialize_seq(Bounded::<T, N>(std::marker::PhantomData))
    }
}

struct Positive(u64);
impl<'de> Deserialize<'de> for Positive {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() > 20
            || s.is_empty()
            || s.starts_with('0')
            || !s.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(de::Error::custom("positive decimal string required"));
        }
        s.parse::<u64>()
            .map(Self)
            .map_err(|_| de::Error::custom("decimal overflow"))
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    PreviewRaw,
    CaptureRaw,
    RecordEnriched,
    ReadCaptured,
    ReadWorkOrder,
    QueryDuplicates,
}
impl From<Action> for WorkOrderIntakeAction {
    fn from(a: Action) -> Self {
        match a {
            Action::PreviewRaw => Self::PreviewRaw,
            Action::CaptureRaw => Self::CaptureRaw,
            Action::RecordEnriched => Self::RecordEnriched,
            Action::ReadCaptured => Self::ReadCaptured,
            Action::ReadWorkOrder => Self::ReadWorkOrder,
            Action::QueryDuplicates => Self::QueryDuplicates,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    schema_version: u32,
    origins: Items<String, 8>,
    policies: Items<Policy, MAX_POLICIES>,
    credentials: Items<Credential, MAX_INTAKE_CREDENTIALS>,
    action_rules: Items<ActionRule, MAX_TOTAL_INTAKE_GRANTS>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    id: String,
    revision: String,
    max_source_bytes: Positive,
    max_requests: Positive,
    max_outcomes: Positive,
    max_json_item_text_bytes: Positive,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    credential_id: String,
    token_file: PathBuf,
    actor: PrincipalId,
    grants: Items<Grant, MAX_GRANTS_PER_CREDENTIAL>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    project_id: ProjectId,
    root_id: RootId,
    authority_ref: String,
    authority_generation: Positive,
    policy_id: String,
    actions: Items<Action, 6>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionRule {
    credential_id: String,
    actor: PrincipalId,
    project_id: ProjectId,
    root_id: RootId,
    authority_ref: String,
    authority_generation: Positive,
    policy_id: String,
    actions: Items<Action, 6>,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    credential_id: String,
    project_id: ProjectId,
    root_id: RootId,
}
struct Scope {
    actor: PrincipalId,
    grant: IntakeScopeGrant,
    policy_id: String,
    policy_digest: String,
}
struct Rule {
    key: Key,
    expected: Scope,
    actions: BTreeSet<WorkOrderIntakeAction>,
}
struct ConfiguredPolicy {
    rules: Vec<Rule>,
}
fn same_policy(a: &IntakePolicy, b: &IntakePolicy) -> bool {
    a.revision == b.revision
        && a.max_source_bytes == b.max_source_bytes
        && a.max_requests == b.max_requests
        && a.max_outcomes == b.max_outcomes
        && a.max_json_item_text_bytes == b.max_json_item_text_bytes
}
impl WorkOrderIntakePolicy for ConfiguredPolicy {
    fn authorize_current(&self, b: &WorkOrderIntakeBinding<'_>) -> bool {
        // The paired bridge supplies the actual current opaque registry actor
        // and rechecks it after this callback. No I/O or registry/Store reentry.
        self.rules.iter().any(|r| {
            b.credential_id == r.key.credential_id
                && b.authenticated_actor == &r.expected.actor
                && b.scope.principal == r.expected.actor
                && b.scope.project_id.as_ref() == Some(&r.key.project_id)
                && b.scope.root_id.as_ref() == Some(&r.key.root_id)
                && b.scope.authority_ref == r.expected.grant.authority_ref
                && b.scope.authority_generation == r.expected.grant.authority_generation
                && b.policy_digest == r.expected.policy_digest
                && same_policy(b.policy, &r.expected.grant.policy)
                && r.actions.contains(&b.action)
        })
    }
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn actions(values: Items<Action, 6>) -> Result<BTreeSet<WorkOrderIntakeAction>> {
    let len = values.0.len();
    let set: BTreeSet<_> = values.0.into_iter().map(Into::into).collect();
    if len == 0 || set.len() != len {
        return Err(IntakeConfigurationError::Invalid);
    }
    Ok(set)
}
fn policy(p: Policy) -> Result<IntakePolicy> {
    let value = IntakePolicy {
        revision: p.revision,
        max_source_bytes: p
            .max_source_bytes
            .0
            .try_into()
            .map_err(|_| IntakeConfigurationError::Invalid)?,
        max_requests: p
            .max_requests
            .0
            .try_into()
            .map_err(|_| IntakeConfigurationError::Invalid)?,
        max_outcomes: p
            .max_outcomes
            .0
            .try_into()
            .map_err(|_| IntakeConfigurationError::Invalid)?,
        max_json_item_text_bytes: p
            .max_json_item_text_bytes
            .0
            .try_into()
            .map_err(|_| IntakeConfigurationError::Invalid)?,
    };
    value
        .identity_digest()
        .map_err(|_| IntakeConfigurationError::Invalid)?;
    Ok(value)
}

/// Load a trusted owner-file snapshot, not request JSON. This function performs
/// no Store open, file creation, hot reload, provider call or transport effect.
/// The startup caller MUST reject reuse of owner/inbox tokens before DB open.
/// No format restriction is claimed: the existing binding carries no format.
pub fn load_private_intake_configuration(path: &Path) -> Result<TrustedIntakeComposition> {
    let bytes = read_private_file(path, MAX_CONFIG_BYTES)
        .map_err(|_| IntakeConfigurationError::Unavailable)?;
    let config: Configuration =
        serde_json::from_slice(&bytes).map_err(|_| IntakeConfigurationError::Invalid)?;
    if config.schema_version != 1 || config.credentials.0.is_empty() || config.policies.0.is_empty()
    {
        return Err(IntakeConfigurationError::Invalid);
    }
    let origins = ExactLoopbackOrigins::new(config.origins.0)
        .map_err(|_| IntakeConfigurationError::Invalid)?;
    let mut policies = BTreeMap::new();
    for p in config.policies.0 {
        if !id(&p.id) || policies.contains_key(&p.id) {
            return Err(IntakeConfigurationError::Invalid);
        }
        let key = p.id.clone();
        policies.insert(key, policy(p)?);
    }
    let mut scopes = BTreeMap::new();
    let mut credential_ids = BTreeSet::new();
    let mut pending = Vec::new();
    let mut total = 0usize;
    for c in config.credentials.0 {
        if !id(&c.credential_id)
            || !credential_ids.insert(c.credential_id.clone())
            || !c.token_file.is_absolute()
            || c.token_file.as_os_str().len() > 4096
        {
            return Err(IntakeConfigurationError::Invalid);
        }
        let mut grants = Vec::new();
        for g in c.grants.0 {
            total = total
                .checked_add(1)
                .ok_or(IntakeConfigurationError::Invalid)?;
            if total > MAX_TOTAL_INTAKE_GRANTS {
                return Err(IntakeConfigurationError::Invalid);
            }
            let p = policies
                .get(&g.policy_id)
                .ok_or(IntakeConfigurationError::Invalid)?;
            let grant = IntakeScopeGrant {
                project_id: g.project_id.clone(),
                root_id: g.root_id.clone(),
                authority_ref: g.authority_ref,
                authority_generation: g.authority_generation.0,
                policy: p.clone(),
                allowed_actions: actions(g.actions)?,
            };
            let key = Key {
                credential_id: c.credential_id.clone(),
                project_id: g.project_id,
                root_id: g.root_id,
            };
            let scope = Scope {
                actor: c.actor.clone(),
                policy_id: g.policy_id,
                policy_digest: p
                    .identity_digest()
                    .map_err(|_| IntakeConfigurationError::Invalid)?,
                grant: grant.clone(),
            };
            if scopes.insert(key, scope).is_some() {
                return Err(IntakeConfigurationError::Invalid);
            }
            grants.push(grant);
        }
        pending.push((c.credential_id, c.actor, c.token_file, grants));
    }
    let mut rules = Vec::new();
    let mut seen = BTreeSet::new();
    for r in config.action_rules.0 {
        let key = Key {
            credential_id: r.credential_id,
            project_id: r.project_id,
            root_id: r.root_id,
        };
        let current = scopes.get(&key).ok_or(IntakeConfigurationError::Invalid)?;
        let allowed = actions(r.actions)?;
        if current.actor != r.actor
            || current.grant.authority_ref != r.authority_ref
            || current.grant.authority_generation != r.authority_generation.0
            || current.policy_id != r.policy_id
            || !allowed.is_subset(&current.grant.allowed_actions)
            || !seen.insert((
                key.credential_id.clone(),
                key.project_id.clone(),
                key.root_id.clone(),
            ))
        {
            return Err(IntakeConfigurationError::Invalid);
        }
        rules.push(Rule {
            expected: Scope {
                actor: current.actor.clone(),
                grant: current.grant.clone(),
                policy_id: current.policy_id.clone(),
                policy_digest: current.policy_digest.clone(),
            },
            key,
            actions: allowed,
        });
    }
    let mut credentials = Vec::new();
    for (credential_id, actor, token_file, grants) in pending {
        let token =
            read_private_token(&token_file).map_err(|_| IntakeConfigurationError::Unavailable)?;
        credentials.push(WorkOrderIntakeCredential::configured(
            credential_id,
            token,
            actor,
            grants,
        ));
    }
    let registry = WorkOrderIntakeAuthRegistry::new(credentials)
        .map_err(|_| IntakeConfigurationError::Invalid)?;
    Ok(TrustedIntakeComposition {
        registry: Arc::new(registry),
        policy: Arc::new(ConfiguredPolicy { rules }),
        origins,
    })
}
