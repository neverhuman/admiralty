use crate::{PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_COLLECTION: usize = 1024;
const MAX_ID: usize = 128;
const MAX_REGISTRY_BYTES: usize = 8 * MAX_JSON_BYTES;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct NodeId(String);
impl NodeId {
    pub fn new(value: impl Into<String>) -> Result<Self, NodeError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_ID
            || value == "."
            || value == ".."
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(NodeError::Invalid("invalid node ID"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for NodeId {
    type Error = NodeError;
    fn try_from(v: String) -> Result<Self, Self::Error> {
        Self::new(v)
    }
}
impl From<NodeId> for String {
    fn from(v: NodeId) -> Self {
        v.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub authority: PrincipalId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentToken {
    pub token_id: String,
    /// Digest of the one-time secret; secret material is never stored here.
    pub token_digest: String,
    pub scope: NodeScope,
    pub expires_tick: DecimalU64,
    pub consumed_binding: Option<EnrollmentBinding>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentBinding {
    pub node_id: NodeId,
    pub boot_id: String,
    pub public_key_digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeIdentity {
    pub node_id: NodeId,
    pub boot_id: String,
    pub public_key_digest: String,
    pub generation: DecimalU64,
    pub capability_digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct DecimalU64(String);
impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value.to_string())
    }
    pub fn parse(&self) -> Result<u64, NodeError> {
        let n = self
            .0
            .parse::<u64>()
            .map_err(|_| NodeError::Invalid("invalid decimal u64"))?;
        if n.to_string() != self.0 {
            return Err(NodeError::Invalid("noncanonical decimal u64"));
        }
        Ok(n)
    }
}
impl TryFrom<String> for DecimalU64 {
    type Error = NodeError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let d = Self(s);
        d.parse()?;
        Ok(d)
    }
}
impl From<DecimalU64> for String {
    fn from(v: DecimalU64) -> Self {
        v.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnrollmentEvent {
    TokenIssued {
        event_id: String,
        token: EnrollmentToken,
    },
    Enrolled {
        event_id: String,
        token_id: String,
        scope: NodeScope,
        identity: NodeIdentity,
    },
    Rotated {
        event_id: String,
        node_id: NodeId,
        expected_generation: DecimalU64,
        new_public_key_digest: String,
    },
    Revoked {
        event_id: String,
        node_id: NodeId,
        expected_generation: DecimalU64,
    },
    CapacityObserved {
        event_id: String,
        node_id: NodeId,
        boot_id: String,
        generation: DecimalU64,
        observed_tick: Option<DecimalU64>,
        fresh_for_ticks: DecimalU64,
        capacity_digest: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct NodeRegistry {
    pub tokens: BTreeMap<String, EnrollmentToken>,
    pub nodes: BTreeMap<NodeId, NodeIdentity>,
    pub revoked_generations: BTreeMap<NodeId, DecimalU64>,
    pub capacity: BTreeMap<NodeId, CapacityObservation>,
    pub applied_events: BTreeMap<String, String>,
    #[serde(default)]
    pub scopes: BTreeMap<NodeId, NodeScope>,
    #[serde(default)]
    pub last_tick: Option<DecimalU64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapacityObservation {
    pub boot_id: String,
    pub generation: DecimalU64,
    pub observed_tick: DecimalU64,
    pub fresh_for_ticks: DecimalU64,
    pub capacity_digest: String,
}
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum NodeError {
    #[error("invalid enrollment data: {0}")]
    Invalid(&'static str),
    #[error("event identity collision")]
    Collision,
    #[error("unknown enrollment token")]
    UnknownToken,
    #[error("token scope mismatch")]
    ForeignScope,
    #[error("token expired or clock unknown")]
    TokenExpired,
    #[error("token already consumed by a different binding")]
    Consumed,
    #[error("unknown node or stale generation/boot")]
    StaleNode,
    #[error("checked arithmetic overflow")]
    Overflow,
}

/// Decode bounded, strict serialized input before passing data to the pure reducer.
pub fn decode_event(bytes: &[u8]) -> Result<EnrollmentEvent, NodeError> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(NodeError::Invalid("JSON exceeds 1 MiB"));
    }
    let event: EnrollmentEvent = serde_json::from_slice(bytes)
        .map_err(|_| NodeError::Invalid("malformed, missing, or unknown JSON fields"))?;
    validate_event(&event)?;
    Ok(event)
}
fn validate_text(value: &str, max: usize) -> Result<(), NodeError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(NodeError::Invalid("invalid or oversized string"))
    } else {
        Ok(())
    }
}
fn validate_digest(value: &str) -> Result<(), NodeError> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or(NodeError::Invalid("digest must use sha256"))?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(NodeError::Invalid("malformed SHA-256 digest"));
    }
    Ok(())
}
fn validate_scope(scope: &NodeScope) -> Result<(), NodeError> {
    validate_text(scope.authority.as_str(), MAX_ID)?;
    Ok(())
}
fn validate_event(event: &EnrollmentEvent) -> Result<(), NodeError> {
    bounded_json_size(event, MAX_JSON_BYTES)
        .map_err(|_| NodeError::Invalid("event exceeds 1 MiB"))?;
    let id = match event {
        EnrollmentEvent::TokenIssued { event_id, .. }
        | EnrollmentEvent::Enrolled { event_id, .. }
        | EnrollmentEvent::Rotated { event_id, .. }
        | EnrollmentEvent::Revoked { event_id, .. }
        | EnrollmentEvent::CapacityObserved { event_id, .. } => event_id,
    };
    validate_text(id, MAX_ID)?;
    crate::MessageId::new(id.clone())
        .map_err(|_| NodeError::Invalid("invalid event identifier"))?;
    match event {
        EnrollmentEvent::TokenIssued { token, .. } => {
            validate_text(&token.token_id, MAX_ID)?;
            crate::MessageId::new(token.token_id.clone())
                .map_err(|_| NodeError::Invalid("invalid token identifier"))?;
            validate_digest(&token.token_digest)?;
            validate_scope(&token.scope)?;
            token.expires_tick.parse()?;
            if let Some(binding) = &token.consumed_binding {
                validate_binding(binding)?;
                return Err(NodeError::Invalid("new token cannot be pre-consumed"));
            }
        }
        EnrollmentEvent::Enrolled {
            token_id,
            scope,
            identity,
            ..
        } => {
            validate_text(token_id, MAX_ID)?;
            validate_scope(scope)?;
            validate_identity(identity)?;
        }
        EnrollmentEvent::Rotated {
            new_public_key_digest,
            expected_generation,
            ..
        } => {
            validate_digest(new_public_key_digest)?;
            expected_generation.parse()?;
        }
        EnrollmentEvent::Revoked {
            expected_generation,
            ..
        } => {
            expected_generation.parse()?;
        }
        EnrollmentEvent::CapacityObserved {
            boot_id,
            generation,
            observed_tick,
            fresh_for_ticks,
            capacity_digest,
            ..
        } => {
            validate_text(boot_id, MAX_ID)?;
            generation.parse()?;
            if let Some(t) = observed_tick {
                t.parse()?;
            }
            fresh_for_ticks.parse()?;
            validate_digest(capacity_digest)?;
        }
    }
    Ok(())
}
fn validate_binding(binding: &EnrollmentBinding) -> Result<(), NodeError> {
    validate_text(&binding.boot_id, MAX_ID)?;
    validate_digest(&binding.public_key_digest)
}
fn validate_identity(identity: &NodeIdentity) -> Result<(), NodeError> {
    validate_text(&identity.boot_id, MAX_ID)?;
    if identity.generation.parse()? == 0 {
        return Err(NodeError::Invalid("generation must be positive"));
    }
    validate_digest(&identity.public_key_digest)?;
    validate_digest(&identity.capability_digest)
}

/// Controller-side admission under an independently authenticated authority.
/// This context must never be copied from the same untrusted enrollment event.
pub fn reduce_enrollment_for(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    expected_scope: &NodeScope,
    authenticated_actor: &PrincipalId,
    explicit_now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    let scope = match event {
        EnrollmentEvent::TokenIssued { token, .. } => &token.scope,
        EnrollmentEvent::Enrolled { scope, .. } => scope,
        EnrollmentEvent::Rotated { node_id, .. }
        | EnrollmentEvent::Revoked { node_id, .. }
        | EnrollmentEvent::CapacityObserved { node_id, .. } => {
            state.scopes.get(node_id).ok_or(NodeError::ForeignScope)?
        }
    };
    if scope != expected_scope || authenticated_actor != &expected_scope.authority {
        return Err(NodeError::ForeignScope);
    }
    reduce_enrollment(state, event, explicit_now)
}

fn validate_registry(state: &NodeRegistry) -> Result<(), NodeError> {
    if [
        state.tokens.len(),
        state.nodes.len(),
        state.capacity.len(),
        state.revoked_generations.len(),
        state.scopes.len(),
        state.applied_events.len(),
    ]
    .into_iter()
    .any(|length| length > MAX_COLLECTION)
    {
        return Err(NodeError::Invalid("collection exceeds 1024 entries"));
    }
    bounded_json_size(state, MAX_REGISTRY_BYTES)
        .map_err(|_| NodeError::Invalid("registry exceeds 8 MiB"))?;
    Ok(())
}

/// Apply one recorded enrollment event. Time is explicit; `None` never means tick zero.
pub fn reduce_enrollment(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    explicit_now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    validate_event(event)?;
    validate_registry(state)?;
    let event_id = match event {
        EnrollmentEvent::TokenIssued { event_id, .. }
        | EnrollmentEvent::Enrolled { event_id, .. }
        | EnrollmentEvent::Rotated { event_id, .. }
        | EnrollmentEvent::Revoked { event_id, .. }
        | EnrollmentEvent::CapacityObserved { event_id, .. } => event_id,
    };
    let digest = content_digest(&(event, explicit_now))
        .map_err(|_| NodeError::Invalid("event serialization failed"))?;
    if let Some(previous) = state.applied_events.get(event_id) {
        return if previous == &digest {
            Ok(state.clone())
        } else {
            Err(NodeError::Collision)
        };
    }
    if state.last_tick.is_none()
        && (!state.tokens.is_empty()
            || !state.nodes.is_empty()
            || !state.applied_events.is_empty()
            || !state.revoked_generations.is_empty()
            || !state.capacity.is_empty()
            || !state.scopes.is_empty())
    {
        return Err(NodeError::Invalid("recorded clock history missing"));
    }
    let now = explicit_now.ok_or(if matches!(event, EnrollmentEvent::Enrolled { .. }) {
        NodeError::TokenExpired
    } else {
        NodeError::Invalid("clock unknown")
    })?;
    if state
        .last_tick
        .as_ref()
        .map(DecimalU64::parse)
        .transpose()?
        .is_some_and(|previous| now < previous)
    {
        return Err(NodeError::Invalid("clock rollback"));
    }
    let mut next = state.clone();
    match event {
        EnrollmentEvent::TokenIssued { token, .. } => {
            if now >= token.expires_tick.parse()? {
                return Err(NodeError::TokenExpired);
            }
            if next
                .tokens
                .get(&token.token_id)
                .is_some_and(|old| old != token)
            {
                return Err(NodeError::Collision);
            }
            next.tokens.insert(token.token_id.clone(), token.clone());
        }
        EnrollmentEvent::Enrolled {
            token_id,
            scope,
            identity,
            ..
        } => {
            let now = explicit_now.ok_or(NodeError::TokenExpired)?;
            let token = next.tokens.get(token_id).ok_or(NodeError::UnknownToken)?;
            if &token.scope != scope {
                return Err(NodeError::ForeignScope);
            }
            if next
                .scopes
                .get(&identity.node_id)
                .is_some_and(|old| old != scope)
            {
                return Err(NodeError::ForeignScope);
            }
            if now >= token.expires_tick.parse()? {
                return Err(NodeError::TokenExpired);
            }
            let binding = EnrollmentBinding {
                node_id: identity.node_id.clone(),
                boot_id: identity.boot_id.clone(),
                public_key_digest: identity.public_key_digest.clone(),
            };
            if let Some(consumed) = &token.consumed_binding
                && (consumed != &binding || next.nodes.get(&identity.node_id) != Some(identity))
            {
                return Err(NodeError::Consumed);
            }
            if next
                .nodes
                .get(&identity.node_id)
                .is_some_and(|old| old != identity)
            {
                return Err(NodeError::Consumed);
            }
            if !next.nodes.contains_key(&identity.node_id)
                && !next.revoked_generations.contains_key(&identity.node_id)
                && identity.generation.parse()? != 1
            {
                return Err(NodeError::StaleNode);
            }
            if next
                .revoked_generations
                .get(&identity.node_id)
                .is_some_and(|generation| generation != &identity.generation)
            {
                return Err(NodeError::StaleNode);
            }
            let mut consumed_token = token.clone();
            consumed_token.consumed_binding = Some(binding);
            next.tokens.insert(token_id.clone(), consumed_token);
            next.nodes
                .insert(identity.node_id.clone(), identity.clone());
            next.scopes.insert(identity.node_id.clone(), scope.clone());
        }
        EnrollmentEvent::Rotated {
            node_id,
            expected_generation,
            new_public_key_digest,
            ..
        } => {
            let node = next.nodes.get_mut(node_id).ok_or(NodeError::StaleNode)?;
            let expected = expected_generation.parse()?;
            if node.generation.parse()? != expected {
                return Err(NodeError::StaleNode);
            }
            let generation = expected.checked_add(1).ok_or(NodeError::Overflow)?;
            node.generation = DecimalU64::new(generation);
            node.public_key_digest = new_public_key_digest.clone();
            next.capacity.remove(node_id);
        }
        EnrollmentEvent::Revoked {
            node_id,
            expected_generation,
            ..
        } => {
            let node = next.nodes.get(node_id).ok_or(NodeError::StaleNode)?;
            let expected = expected_generation.parse()?;
            if node.generation.parse()? != expected {
                return Err(NodeError::StaleNode);
            }
            let generation = expected.checked_add(1).ok_or(NodeError::Overflow)?;
            next.revoked_generations
                .insert(node_id.clone(), DecimalU64::new(generation));
            next.nodes.remove(node_id);
            next.capacity.remove(node_id);
        }
        EnrollmentEvent::CapacityObserved {
            node_id,
            boot_id,
            generation,
            observed_tick,
            fresh_for_ticks,
            capacity_digest,
            ..
        } => {
            let node = next.nodes.get(node_id).ok_or(NodeError::StaleNode)?;
            if &node.boot_id != boot_id || node.generation != *generation {
                return Err(NodeError::StaleNode);
            }
            let at = observed_tick
                .as_ref()
                .ok_or(NodeError::Invalid("capacity timestamp unknown"))?
                .parse()?;
            let now = explicit_now.ok_or(NodeError::Invalid("clock unknown"))?;
            let freshness = fresh_for_ticks.parse()?;
            let expiry = at.checked_add(freshness).ok_or(NodeError::Overflow)?;
            if at > now || now > expiry {
                return Err(NodeError::Invalid(
                    "capacity observation stale or from future",
                ));
            }
            let observation = CapacityObservation {
                boot_id: boot_id.clone(),
                generation: generation.clone(),
                observed_tick: DecimalU64::new(at),
                fresh_for_ticks: fresh_for_ticks.clone(),
                capacity_digest: capacity_digest.clone(),
            };
            if let Some(previous) = next.capacity.get(node_id) {
                let previous_tick = previous.observed_tick.parse()?;
                if at < previous_tick {
                    return Err(NodeError::Invalid("capacity observation rollback"));
                }
                if at == previous_tick && previous != &observation {
                    return Err(NodeError::Collision);
                }
            }
            next.capacity.insert(node_id.clone(), observation);
        }
    }
    next.applied_events.insert(event_id.clone(), digest);
    next.last_tick = Some(DecimalU64::new(now));
    validate_registry(&next)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }
    fn scope() -> NodeScope {
        NodeScope {
            project_id: ProjectId::new("p").expect("project ID fixture"),
            root_id: RootId::new("r").expect("root ID fixture"),
            task_id: TaskId::new("t").expect("task ID fixture"),
            authority: PrincipalId::new("owner").expect("authority fixture"),
        }
    }
    fn identity() -> NodeIdentity {
        NodeIdentity {
            node_id: NodeId::new("node-a").expect("node ID fixture"),
            boot_id: "boot-a".into(),
            public_key_digest: d('a'),
            generation: DecimalU64::new(1),
            capability_digest: d('b'),
        }
    }
    fn token() -> EnrollmentToken {
        EnrollmentToken {
            token_id: "token-a".into(),
            token_digest: d('c'),
            scope: scope(),
            expires_tick: DecimalU64::new(10),
            consumed_binding: None,
        }
    }
    fn issue() -> NodeRegistry {
        reduce_enrollment(
            &NodeRegistry::default(),
            &EnrollmentEvent::TokenIssued {
                event_id: "issue".into(),
                token: token(),
            },
            Some(0),
        )
        .expect("issue token")
    }
    fn enroll(
        state: &NodeRegistry,
        event_id: &str,
        identity: NodeIdentity,
        scope: NodeScope,
        token_id: &str,
        now: Option<u64>,
    ) -> Result<NodeRegistry, NodeError> {
        reduce_enrollment(
            state,
            &EnrollmentEvent::Enrolled {
                event_id: event_id.into(),
                token_id: token_id.into(),
                scope,
                identity,
            },
            now,
        )
    }
    #[test]
    fn duplicate_event_and_exact_consumption_replay_are_idempotent_but_changed_binding_fails() {
        let start = issue();
        let event = EnrollmentEvent::Enrolled {
            event_id: "enroll".into(),
            token_id: "token-a".into(),
            scope: scope(),
            identity: identity(),
        };
        let enrolled = reduce_enrollment(&start, &event, Some(2)).expect("enroll");
        assert_eq!(
            reduce_enrollment(&enrolled, &event, Some(2)).expect("event replay"),
            enrolled
        );
        assert!(matches!(
            enroll(
                &enrolled,
                "different-event",
                NodeIdentity {
                    boot_id: "boot-b".into(),
                    ..identity()
                },
                scope(),
                "token-a",
                Some(3)
            ),
            Err(NodeError::Consumed)
        ));
        assert!(matches!(
            enroll(
                &enrolled,
                "different-scope",
                identity(),
                NodeScope {
                    task_id: TaskId::new("other").expect("task fixture"),
                    ..scope()
                },
                "token-a",
                Some(3)
            ),
            Err(NodeError::ForeignScope)
        ));
    }
    #[test]
    fn clock_unknown_deadline_and_foreign_scope_fail_closed() {
        let start = issue();
        assert!(matches!(
            enroll(
                &start,
                "unknown-clock",
                identity(),
                scope(),
                "token-a",
                None
            ),
            Err(NodeError::TokenExpired)
        ));
        assert!(matches!(
            enroll(&start, "boundary", identity(), scope(), "token-a", Some(10)),
            Err(NodeError::TokenExpired)
        ));
        assert!(matches!(
            enroll(
                &start,
                "foreign",
                identity(),
                NodeScope {
                    project_id: ProjectId::new("other").expect("project fixture"),
                    ..scope()
                },
                "token-a",
                Some(1)
            ),
            Err(NodeError::ForeignScope)
        ));
        assert!(matches!(
            enroll(&start, "expired", identity(), scope(), "token-a", Some(11)),
            Err(NodeError::TokenExpired)
        ));
    }
    #[test]
    fn rotation_revocation_stale_boot_and_generation_are_checked() {
        let enrolled = enroll(&issue(), "enroll", identity(), scope(), "token-a", Some(1))
            .expect("enrollment");
        let rotated = reduce_enrollment(
            &enrolled,
            &EnrollmentEvent::Rotated {
                event_id: "rotate".into(),
                node_id: identity().node_id.clone(),
                expected_generation: DecimalU64::new(1),
                new_public_key_digest: d('d'),
            },
            Some(2),
        )
        .expect("rotation");
        assert_eq!(
            rotated.nodes[&identity().node_id]
                .generation
                .parse()
                .expect("generation"),
            2
        );
        assert!(matches!(
            reduce_enrollment(
                &rotated,
                &EnrollmentEvent::Rotated {
                    event_id: "stale-rotate".into(),
                    node_id: identity().node_id.clone(),
                    expected_generation: DecimalU64::new(1),
                    new_public_key_digest: d('e')
                },
                Some(2)
            ),
            Err(NodeError::StaleNode)
        ));
        assert!(matches!(
            reduce_enrollment(
                &rotated,
                &EnrollmentEvent::CapacityObserved {
                    event_id: "stale-boot".into(),
                    node_id: identity().node_id.clone(),
                    boot_id: "old-boot".into(),
                    generation: DecimalU64::new(2),
                    observed_tick: Some(DecimalU64::new(1)),
                    fresh_for_ticks: DecimalU64::new(10),
                    capacity_digest: d('f')
                },
                Some(2)
            ),
            Err(NodeError::StaleNode)
        ));
        let revoked = reduce_enrollment(
            &rotated,
            &EnrollmentEvent::Revoked {
                event_id: "revoke".into(),
                node_id: identity().node_id.clone(),
                expected_generation: DecimalU64::new(2),
            },
            Some(3),
        )
        .expect("revoke");
        assert!(revoked.nodes.is_empty());
        assert!(
            revoked
                .revoked_generations
                .contains_key(&identity().node_id)
        );
        let replacement = EnrollmentToken {
            token_id: "token-b".into(),
            token_digest: d('e'),
            scope: scope(),
            expires_tick: DecimalU64::new(20),
            consumed_binding: None,
        };
        let issued = reduce_enrollment(
            &revoked,
            &EnrollmentEvent::TokenIssued {
                event_id: "issue-b".into(),
                token: replacement,
            },
            Some(4),
        )
        .expect("issue replacement token");
        assert!(matches!(
            enroll(
                &issued,
                "old-generation",
                NodeIdentity {
                    generation: DecimalU64::new(1),
                    ..identity()
                },
                scope(),
                "token-b",
                Some(5)
            ),
            Err(NodeError::StaleNode)
        ));
        assert!(
            enroll(
                &issued,
                "new-generation",
                NodeIdentity {
                    generation: DecimalU64::new(3),
                    ..identity()
                },
                scope(),
                "token-b",
                Some(5)
            )
            .is_ok()
        );
    }
    #[test]
    fn capacity_unknown_stale_and_overflow_are_not_free_capacity() {
        let enrolled = enroll(&issue(), "enroll", identity(), scope(), "token-a", Some(1))
            .expect("enrollment");
        let cap = |event_id: &str, at: Option<u64>, fresh: u64| EnrollmentEvent::CapacityObserved {
            event_id: event_id.into(),
            node_id: identity().node_id.clone(),
            boot_id: "boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: at.map(DecimalU64::new),
            fresh_for_ticks: DecimalU64::new(fresh),
            capacity_digest: d('f'),
        };
        assert!(matches!(
            reduce_enrollment(&enrolled, &cap("unknown", None, 4), Some(2)),
            Err(NodeError::Invalid(_))
        ));
        assert!(matches!(
            reduce_enrollment(&enrolled, &cap("stale", Some(1), 1), Some(3)),
            Err(NodeError::Invalid(_))
        ));
        assert!(matches!(
            reduce_enrollment(
                &enrolled,
                &cap("overflow", Some(u64::MAX), 1),
                Some(u64::MAX),
            ),
            Err(NodeError::Overflow)
        ));
        assert!(
            reduce_enrollment(&enrolled, &cap("fresh", Some(2), 2), Some(4))
                .expect("fresh capacity")
                .capacity
                .contains_key(&identity().node_id)
        );
    }
    #[test]
    fn bounds_ids_digests_unknown_fields_and_counter_overflow_are_rejected() {
        assert!(NodeId::new("../host").is_err());
        assert!(NodeId::new("x".repeat(129)).is_err());
        assert!(DecimalU64("01".into()).parse().is_err());
        assert!(
            decode_event(br#"{"kind":"token_issued","event_id":"x","token":{},"surprise":1}"#)
                .is_err()
        );
        assert!(decode_event(&vec![b' '; MAX_JSON_BYTES + 1]).is_err());
        assert!(validate_digest("sha256:ABC").is_err());
        let max = NodeIdentity {
            generation: DecimalU64::new(u64::MAX),
            ..identity()
        };
        let mut state = NodeRegistry::default();
        state.nodes.insert(max.node_id.clone(), max.clone());
        state.last_tick = Some(DecimalU64::new(0));
        assert!(matches!(
            reduce_enrollment(
                &state,
                &EnrollmentEvent::Rotated {
                    event_id: "overflow".into(),
                    node_id: max.node_id,
                    expected_generation: DecimalU64::new(u64::MAX),
                    new_public_key_digest: d('d')
                },
                Some(0)
            ),
            Err(NodeError::Overflow)
        ));
    }

    #[test]
    fn recorded_clock_is_part_of_event_identity_and_new_mutations_cannot_roll_back() {
        let start = issue();
        let enrolled =
            enroll(&start, "enroll", identity(), scope(), "token-a", Some(2)).expect("enroll");
        let issued = EnrollmentEvent::TokenIssued {
            event_id: "issue".into(),
            token: token(),
        };
        assert_eq!(
            reduce_enrollment(&enrolled, &issued, Some(0)).expect("exact historical replay"),
            enrolled
        );
        assert_eq!(
            reduce_enrollment(&enrolled, &issued, Some(1)),
            Err(NodeError::Collision)
        );
        let rotate = EnrollmentEvent::Rotated {
            event_id: "rotate".into(),
            node_id: identity().node_id,
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: d('d'),
        };
        assert!(matches!(
            reduce_enrollment(&enrolled, &rotate, Some(1)),
            Err(NodeError::Invalid("clock rollback"))
        ));
        assert!(reduce_enrollment(&enrolled, &rotate, None).is_err());
        assert_eq!(start.last_tick, Some(DecimalU64::new(0)));
        assert_eq!(enrolled.last_tick, Some(DecimalU64::new(2)));
        let mut missing_history = enrolled.clone();
        missing_history.last_tick = None;
        assert_eq!(
            reduce_enrollment(&missing_history, &rotate, Some(0)),
            Err(NodeError::Invalid("recorded clock history missing"))
        );
    }

    #[test]
    fn revoked_identity_requires_fresh_token_exact_scope_and_next_generation() {
        let enrolled =
            enroll(&issue(), "enroll", identity(), scope(), "token-a", Some(1)).expect("enroll");
        let revoke = EnrollmentEvent::Revoked {
            event_id: "revoke".into(),
            node_id: identity().node_id,
            expected_generation: DecimalU64::new(1),
        };
        assert_eq!(
            reduce_enrollment_for(
                &enrolled,
                &revoke,
                &scope(),
                &PrincipalId::new("stranger").expect("principal"),
                Some(2)
            ),
            Err(NodeError::ForeignScope)
        );
        let revoked =
            reduce_enrollment_for(&enrolled, &revoke, &scope(), &scope().authority, Some(2))
                .expect("authorized revoke");
        let next_identity = NodeIdentity {
            generation: DecimalU64::new(2),
            ..identity()
        };
        assert_eq!(
            enroll(
                &revoked,
                "resurrect",
                next_identity.clone(),
                scope(),
                "token-a",
                Some(3)
            ),
            Err(NodeError::Consumed)
        );
        let replacement = EnrollmentToken {
            token_id: "new-token".into(),
            expires_tick: DecimalU64::new(30),
            ..token()
        };
        let issued = reduce_enrollment(
            &revoked,
            &EnrollmentEvent::TokenIssued {
                event_id: "new-issue".into(),
                token: replacement,
            },
            Some(3),
        )
        .expect("new token");
        let renewed = enroll(
            &issued,
            "renew",
            next_identity.clone(),
            scope(),
            "new-token",
            Some(4),
        )
        .expect("new generation enrollment");
        let observation = EnrollmentEvent::CapacityObserved {
            event_id: "new-capacity".into(),
            node_id: next_identity.node_id.clone(),
            boot_id: next_identity.boot_id,
            generation: next_identity.generation,
            observed_tick: Some(DecimalU64::new(5)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: d('f'),
        };
        let capacity = reduce_enrollment_for(
            &renewed,
            &observation,
            &scope(),
            &scope().authority,
            Some(5),
        )
        .expect("renewed identity capacity");
        assert!(capacity.capacity.contains_key(&identity().node_id));
        let foreign = NodeScope {
            root_id: RootId::new("foreign").expect("root"),
            ..scope()
        };
        assert_eq!(
            reduce_enrollment_for(
                &renewed,
                &observation,
                &foreign,
                &scope().authority,
                Some(5)
            ),
            Err(NodeError::ForeignScope)
        );
    }

    #[test]
    fn capacity_rollback_equivocation_and_preconsumed_tokens_fail_without_changes() {
        let enrolled =
            enroll(&issue(), "enroll", identity(), scope(), "token-a", Some(1)).expect("enroll");
        let cap = |id: &str, at: u64, digest| EnrollmentEvent::CapacityObserved {
            event_id: id.into(),
            node_id: identity().node_id,
            boot_id: identity().boot_id,
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(at)),
            fresh_for_ticks: DecimalU64::new(10),
            capacity_digest: d(digest),
        };
        let state =
            reduce_enrollment(&enrolled, &cap("cap", 3, 'a'), Some(3)).expect("observation");
        let before = state.clone();
        assert!(matches!(
            reduce_enrollment(&state, &cap("old", 2, 'b'), Some(4)),
            Err(NodeError::Invalid("capacity observation rollback"))
        ));
        assert_eq!(
            reduce_enrollment(&state, &cap("changed", 3, 'b'), Some(4)),
            Err(NodeError::Collision)
        );
        assert_eq!(state, before);
        let mut consumed = token();
        consumed.consumed_binding = Some(EnrollmentBinding {
            node_id: identity().node_id,
            boot_id: identity().boot_id,
            public_key_digest: identity().public_key_digest,
        });
        assert!(
            reduce_enrollment(
                &NodeRegistry::default(),
                &EnrollmentEvent::TokenIssued {
                    event_id: "preconsumed".into(),
                    token: consumed
                },
                Some(0)
            )
            .is_err()
        );
        let mut oversized = cap("large", 3, 'b');
        if let EnrollmentEvent::CapacityObserved { boot_id, .. } = &mut oversized {
            *boot_id = "x".repeat(MAX_JSON_BYTES);
        }
        assert!(reduce_enrollment(&state, &oversized, Some(4)).is_err());
        let duplicate = serde_json::to_string(&cap("duplicate", 3, 'b'))
            .expect("JSON")
            .replacen("{", "{\"kind\":\"capacity_observed\",", 1);
        assert!(decode_event(duplicate.as_bytes()).is_err());
    }
}
