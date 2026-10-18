//! Pure, bounded interface compatibility evidence. Callers authenticate grants,
//! durably append events and persist invalidation proposals atomically.
use crate::{PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_INTERFACE_EVENT_BYTES: usize = 1_048_576;
pub const MAX_INTERFACE_CONSUMERS: usize = 1_024;
pub const MAX_INTERFACE_EVENTS: usize = 1_024;
pub const MAX_INTERFACE_STRING_BYTES: usize = 4_096;
pub const MAX_INTERFACE_STATE_BYTES: usize = 8 * 1_048_576;

/// Canonical decimal string encoding for revisions that may exceed JS precision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64(u64);
impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
impl JsonSchema for DecimalU64 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DecimalU64".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        // Match canonical decimal strings exactly through u64::MAX. Numeric
        // JSON values lose precision in JS and are never this wire format.
        let maximum = u64::MAX.to_string();
        let mut alternatives = vec!["0".to_owned(), "[1-9][0-9]{0,18}".to_owned()];
        for (index, digit) in maximum.bytes().enumerate() {
            let lower = u8::from(index == 0);
            let upper = digit - b'0';
            if upper <= lower {
                continue;
            }
            let remainder = maximum.len() - index - 1;
            let suffix = if remainder == 0 {
                String::new()
            } else {
                format!("[0-9]{{{remainder}}}")
            };
            alternatives.push(format!(
                "{}[{lower}-{}]{suffix}",
                &maximum[..index],
                upper - 1
            ));
        }
        alternatives.push(maximum);
        json_schema!({"type":"string","pattern":format!(r"^(?:{})(?![\s\S])",alternatives.join("|"))})
    }
}
impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let parsed = value.parse::<u64>().map_err(de::Error::custom)?;
        if parsed.to_string() != value {
            return Err(de::Error::custom("expected canonical decimal u64"));
        }
        Ok(Self(parsed))
    }
}

#[derive(
    Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct InterfaceId(String);
impl InterfaceId {
    pub fn new(value: impl Into<String>) -> Result<Self, InterfaceError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || value == "."
            || value == ".."
        {
            return Err(InterfaceError("invalid interface identifier"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for InterfaceId {
    type Error = InterfaceError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}
impl From<InterfaceId> for String {
    fn from(id: InterfaceId) -> Self {
        id.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterfaceScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub authority_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterfaceContract {
    pub interface_id: InterfaceId,
    pub scope: InterfaceScope,
    pub producer: PrincipalId,
    #[schemars(length(max = MAX_INTERFACE_CONSUMERS))]
    pub consumers: Vec<PrincipalId>,
    pub revision: DecimalU64,
    pub artifact_digest: String,
    pub schema_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecipientEpoch {
    pub recipient: PrincipalId,
    pub epoch: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InterfaceEvent {
    Publish {
        event_id: crate::MessageId,
        contract: InterfaceContract,
        actor: PrincipalId,
        /// Principal bound to the authority grant validated by the caller.
        authority_owner: PrincipalId,
        recipient_epochs: Vec<RecipientEpoch>,
        expected_state_revision: Option<DecimalU64>,
    },
    Acknowledge {
        event_id: crate::MessageId,
        interface_id: InterfaceId,
        scope: InterfaceScope,
        actor: PrincipalId,
        recipient_epoch: String,
        interface_revision: DecimalU64,
        artifact_digest: String,
        schema_digest: String,
        expected_state_revision: DecimalU64,
        authority_ref: String,
    },
    Reject {
        event_id: crate::MessageId,
        interface_id: InterfaceId,
        scope: InterfaceScope,
        actor: PrincipalId,
        recipient_epoch: String,
        interface_revision: DecimalU64,
        artifact_digest: String,
        schema_digest: String,
        expected_state_revision: DecimalU64,
        authority_ref: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum CompatibilityEvidence {
    ProvisionalAck {
        artifact_digest: String,
        schema_digest: String,
    },
    Rejected {
        artifact_digest: String,
        schema_digest: String,
        reason: String,
    },
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InterfaceState {
    pub contract: InterfaceContract,
    pub authority_owner: PrincipalId,
    pub revision: DecimalU64,
    pub recipient_epochs: BTreeMap<PrincipalId, String>,
    pub evidence: BTreeMap<PrincipalId, CompatibilityEvidence>,
    applied_events: BTreeMap<crate::MessageId, String>,
}
impl InterfaceState {
    pub fn retained_event_count(&self) -> usize {
        self.applied_events.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidationProposal {
    pub interface_id: InterfaceId,
    pub scope: InterfaceScope,
    pub consumer: PrincipalId,
    pub recipient_epoch: String,
    pub previous_revision: DecimalU64,
    pub previous_artifact_digest: String,
    pub previous_schema_digest: String,
    pub new_revision: DecimalU64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceReduction {
    pub state: InterfaceState,
    pub invalidations: Vec<InvalidationProposal>,
    pub replayed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InterfaceError(pub &'static str);

pub fn decode_event(bytes: &[u8]) -> Result<InterfaceEvent, InterfaceError> {
    if bytes.len() > MAX_INTERFACE_EVENT_BYTES {
        return Err(InterfaceError("interface JSON exceeds 1 MiB"));
    }
    let event = serde_json::from_slice(bytes)
        .map_err(|_| InterfaceError("invalid or unknown interface JSON fields"))?;
    validate_event(&event)?;
    Ok(event)
}

/// Bind to caller-authenticated context, never to authority fields decoded from
/// the same worker request. The lower-level reducer remains replay-only policy.
pub fn reduce_interface_for(
    current: &Option<InterfaceState>,
    event: &InterfaceEvent,
    expected_scope: &InterfaceScope,
    authenticated_actor: &PrincipalId,
    authority_owner: &PrincipalId,
) -> Result<InterfaceReduction, InterfaceError> {
    let (scope, actor) = match event {
        InterfaceEvent::Publish {
            contract,
            actor,
            authority_owner: recorded_owner,
            ..
        } => {
            if recorded_owner != authority_owner {
                return Err(InterfaceError("authenticated authority owner mismatch"));
            }
            (&contract.scope, actor)
        }
        InterfaceEvent::Acknowledge { scope, actor, .. }
        | InterfaceEvent::Reject { scope, actor, .. } => (scope, actor),
    };
    if scope != expected_scope
        || actor != authenticated_actor
        || current
            .as_ref()
            .is_some_and(|state| &state.authority_owner != authority_owner)
    {
        return Err(InterfaceError(
            "authenticated actor or exact interface scope mismatch",
        ));
    }
    reduce_interface(current, event)
}

pub fn reduce_interface(
    current: &Option<InterfaceState>,
    event: &InterfaceEvent,
) -> Result<InterfaceReduction, InterfaceError> {
    validate_event(event)?;
    if let Some(state) = current {
        if state.contract.consumers.len() > MAX_INTERFACE_CONSUMERS
            || state.recipient_epochs.len() > MAX_INTERFACE_CONSUMERS
            || state.evidence.len() > MAX_INTERFACE_CONSUMERS
            || state.applied_events.len() > MAX_INTERFACE_EVENTS
        {
            return Err(InterfaceError("interface state collection bound exceeded"));
        }
        bounded_json_size(state, MAX_INTERFACE_STATE_BYTES)
            .map_err(|_| InterfaceError("interface state exceeds byte bound"))?;
    }
    let digest =
        content_digest(event).map_err(|_| InterfaceError("cannot fingerprint interface event"))?;
    let id = event_id(event);
    if let Some(state) = current
        && let Some(previous) = state.applied_events.get(id)
    {
        if previous == &digest {
            return Ok(InterfaceReduction {
                state: state.clone(),
                invalidations: Vec::new(),
                replayed: true,
            });
        }
        return Err(InterfaceError("event ID reused with different content"));
    }
    match event {
        InterfaceEvent::Publish {
            contract,
            actor,
            authority_owner,
            recipient_epochs,
            expected_state_revision,
            ..
        } => {
            if contract.revision.get() == 0 {
                return Err(InterfaceError("interface revision must be positive"));
            }
            let (next_revision, invalidations) = if let Some(old) = current {
                if expected_state_revision.as_ref() != Some(&old.revision) {
                    return Err(InterfaceError("stale expected state revision"));
                }
                if contract.interface_id != old.contract.interface_id
                    || contract.scope != old.contract.scope
                    || contract.producer != old.contract.producer
                    || (actor != &old.contract.producer && actor != &old.authority_owner)
                    || authority_owner != &old.authority_owner
                {
                    return Err(InterfaceError(
                        "publisher, interface scope or authority mismatch",
                    ));
                }
                if contract.revision.get() <= old.contract.revision.get() {
                    return Err(InterfaceError("interface revision must increase"));
                }
                let next = DecimalU64::new(
                    old.revision
                        .get()
                        .checked_add(1)
                        .ok_or(InterfaceError("state revision overflow"))?,
                );
                if old.applied_events.len() >= MAX_INTERFACE_EVENTS {
                    return Err(InterfaceError("interface event bound reached"));
                }
                let mut proposals = Vec::new();
                for (consumer, evidence) in &old.evidence {
                    if let CompatibilityEvidence::ProvisionalAck {
                        artifact_digest,
                        schema_digest,
                    } = evidence
                    {
                        proposals.push(InvalidationProposal {
                            interface_id: old.contract.interface_id.clone(),
                            scope: old.contract.scope.clone(),
                            consumer: consumer.clone(),
                            recipient_epoch: old
                                .recipient_epochs
                                .get(consumer)
                                .cloned()
                                .ok_or(InterfaceError("ACK recipient epoch missing from state"))?,
                            previous_revision: old.contract.revision.clone(),
                            previous_artifact_digest: artifact_digest.clone(),
                            previous_schema_digest: schema_digest.clone(),
                            new_revision: contract.revision.clone(),
                        });
                    }
                }
                (next, proposals)
            } else {
                if expected_state_revision.is_some()
                    || actor != &contract.producer && actor != authority_owner
                {
                    return Err(InterfaceError(
                        "initial publisher or state revision invalid",
                    ));
                }
                (DecimalU64::new(1), Vec::new())
            };
            let epochs = recipient_epochs
                .iter()
                .map(|v| (v.recipient.clone(), v.epoch.clone()))
                .collect();
            let mut applied_events = current
                .as_ref()
                .map_or_else(BTreeMap::new, |state| state.applied_events.clone());
            applied_events.insert(id.clone(), digest);
            let state = InterfaceState {
                contract: contract.clone(),
                authority_owner: authority_owner.clone(),
                revision: next_revision,
                recipient_epochs: epochs,
                evidence: BTreeMap::new(),
                applied_events,
            };
            Ok(InterfaceReduction {
                state,
                invalidations,
                replayed: false,
            })
        }
        InterfaceEvent::Acknowledge {
            interface_id,
            scope,
            actor,
            recipient_epoch,
            interface_revision,
            artifact_digest,
            schema_digest,
            expected_state_revision,
            authority_ref,
            ..
        }
        | InterfaceEvent::Reject {
            interface_id,
            scope,
            actor,
            recipient_epoch,
            interface_revision,
            artifact_digest,
            schema_digest,
            expected_state_revision,
            authority_ref,
            ..
        } => {
            let old = current
                .as_ref()
                .ok_or(InterfaceError("interface does not exist"))?;
            if interface_id != &old.contract.interface_id
                || scope != &old.contract.scope
                || authority_ref != &old.contract.scope.authority_ref
            {
                return Err(InterfaceError("foreign interface scope or authority"));
            }
            if expected_state_revision != &old.revision
                || interface_revision != &old.contract.revision
            {
                return Err(InterfaceError("stale interface or state revision"));
            }
            if !old.contract.consumers.contains(actor) {
                return Err(InterfaceError("actor is not an authorized consumer"));
            }
            if old.recipient_epochs.get(actor) != Some(recipient_epoch) {
                return Err(InterfaceError("stale recipient epoch"));
            }
            if artifact_digest != &old.contract.artifact_digest
                || schema_digest != &old.contract.schema_digest
            {
                return Err(InterfaceError("artifact or schema digest mismatch"));
            }
            if old.applied_events.len() >= MAX_INTERFACE_EVENTS {
                return Err(InterfaceError("interface event bound reached"));
            }
            let mut state = old.clone();
            let evidence = match event {
                InterfaceEvent::Acknowledge { .. } => CompatibilityEvidence::ProvisionalAck {
                    artifact_digest: artifact_digest.clone(),
                    schema_digest: schema_digest.clone(),
                },
                InterfaceEvent::Reject { reason, .. } => CompatibilityEvidence::Rejected {
                    artifact_digest: artifact_digest.clone(),
                    schema_digest: schema_digest.clone(),
                    reason: reason.clone(),
                },
                _ => unreachable!(),
            };
            state.evidence.insert(actor.clone(), evidence);
            state.revision = DecimalU64::new(
                state
                    .revision
                    .get()
                    .checked_add(1)
                    .ok_or(InterfaceError("state revision overflow"))?,
            );
            state.applied_events.insert(id.clone(), digest);
            Ok(InterfaceReduction {
                state,
                invalidations: Vec::new(),
                replayed: false,
            })
        }
    }
}

fn event_id(event: &InterfaceEvent) -> &crate::MessageId {
    match event {
        InterfaceEvent::Publish { event_id, .. }
        | InterfaceEvent::Acknowledge { event_id, .. }
        | InterfaceEvent::Reject { event_id, .. } => event_id,
    }
}
fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn bounded(value: &str) -> bool {
    value.len() <= MAX_INTERFACE_STRING_BYTES && !value.chars().any(char::is_control)
}
fn validate_event(event: &InterfaceEvent) -> Result<(), InterfaceError> {
    bounded_json_size(event, MAX_INTERFACE_EVENT_BYTES)
        .map_err(|_| InterfaceError("oversized interface event or serialization failure"))?;
    match event {
        InterfaceEvent::Publish {
            contract,
            recipient_epochs,
            ..
        } => {
            if contract.consumers.len() > MAX_INTERFACE_CONSUMERS
                || recipient_epochs.len() > MAX_INTERFACE_CONSUMERS
            {
                return Err(InterfaceError("consumer bound exceeded"));
            }
            if contract.scope.authority_ref.trim().is_empty()
                || !bounded(&contract.scope.authority_ref)
                || !valid_digest(&contract.artifact_digest)
                || !valid_digest(&contract.schema_digest)
            {
                return Err(InterfaceError("invalid authority or digest"));
            }
            let consumers: BTreeSet<_> = contract.consumers.iter().collect();
            let recipients: BTreeSet<_> = recipient_epochs.iter().map(|v| &v.recipient).collect();
            if consumers.len() != contract.consumers.len()
                || recipients.len() != recipient_epochs.len()
                || consumers != recipients
            {
                return Err(InterfaceError(
                    "recipient epochs must exactly match unique consumers",
                ));
            }
            if recipient_epochs
                .iter()
                .any(|v| v.epoch.trim().is_empty() || !bounded(&v.epoch))
            {
                return Err(InterfaceError("missing or oversized recipient epoch"));
            }
        }
        InterfaceEvent::Acknowledge {
            scope,
            authority_ref,
            recipient_epoch,
            artifact_digest,
            schema_digest,
            ..
        } => validate_evidence(
            scope,
            authority_ref,
            recipient_epoch,
            artifact_digest,
            schema_digest,
        )?,
        InterfaceEvent::Reject {
            scope,
            authority_ref,
            recipient_epoch,
            artifact_digest,
            schema_digest,
            reason,
            ..
        } => {
            validate_evidence(
                scope,
                authority_ref,
                recipient_epoch,
                artifact_digest,
                schema_digest,
            )?;
            if reason.trim().is_empty() || !bounded(reason) {
                return Err(InterfaceError("missing or oversized rejection reason"));
            }
        }
    }
    Ok(())
}
fn validate_evidence(
    scope: &InterfaceScope,
    authority: &str,
    epoch: &str,
    artifact: &str,
    schema: &str,
) -> Result<(), InterfaceError> {
    if scope.authority_ref.trim().is_empty()
        || authority.trim().is_empty()
        || !bounded(authority)
        || !bounded(&scope.authority_ref)
        || epoch.trim().is_empty()
        || !bounded(epoch)
    {
        return Err(InterfaceError(
            "missing or oversized scope, authority or epoch",
        ));
    }
    if !valid_digest(artifact) || !valid_digest(schema) {
        return Err(InterfaceError("invalid artifact or schema digest"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn principal(s: &str) -> PrincipalId {
        PrincipalId::new(s).expect("fixture principal")
    }
    fn id(s: &str) -> crate::MessageId {
        crate::MessageId::new(s).expect("fixture event ID")
    }
    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }
    fn scope() -> InterfaceScope {
        InterfaceScope {
            project_id: ProjectId::new("p").expect("fixture project"),
            root_id: RootId::new("r").expect("fixture root"),
            task_id: TaskId::new("producer-task").expect("fixture task"),
            authority_ref: "grant-1".into(),
        }
    }
    fn publish() -> InterfaceEvent {
        let consumers = vec![principal("consumer-a"), principal("consumer-b")];
        InterfaceEvent::Publish {
            event_id: id("publish-1"),
            contract: InterfaceContract {
                interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
                scope: scope(),
                producer: principal("producer"),
                consumers: consumers.clone(),
                revision: DecimalU64::new(1),
                artifact_digest: digest('a'),
                schema_digest: digest('b'),
            },
            actor: principal("producer"),
            authority_owner: principal("owner"),
            recipient_epochs: consumers
                .into_iter()
                .map(|recipient| RecipientEpoch {
                    recipient,
                    epoch: "epoch-1".into(),
                })
                .collect(),
            expected_state_revision: None,
        }
    }
    fn ack(event_id: &str, consumer: &str) -> InterfaceEvent {
        InterfaceEvent::Acknowledge {
            event_id: id(event_id),
            interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
            scope: scope(),
            actor: principal(consumer),
            recipient_epoch: "epoch-1".into(),
            interface_revision: DecimalU64::new(1),
            artifact_digest: digest('a'),
            schema_digest: digest('b'),
            expected_state_revision: DecimalU64::new(1),
            authority_ref: "grant-1".into(),
        }
    }
    #[test]
    fn direct_consumer_ack_is_provisional_and_never_acceptance() {
        let published = reduce_interface(&None, &publish()).expect("publish proposal");
        let acknowledged = reduce_interface(&Some(published.state), &ack("ack-1", "consumer-a"))
            .expect("consumer ACK");
        assert!(matches!(
            acknowledged.state.evidence.get(&principal("consumer-a")),
            Some(CompatibilityEvidence::ProvisionalAck { .. })
        ));
        assert!(acknowledged.invalidations.is_empty());
    }
    #[test]
    fn wrong_recipient_authority_scope_revision_epoch_and_digests_fail() {
        let state = reduce_interface(&None, &publish()).expect("publish").state;
        let mut e = ack("bad", "stranger");
        assert!(reduce_interface(&Some(state.clone()), &e).is_err());
        e = ack("bad", "consumer-a");
        if let InterfaceEvent::Acknowledge { authority_ref, .. } = &mut e {
            *authority_ref = "foreign".into();
        }
        assert!(reduce_interface(&Some(state.clone()), &e).is_err());
        e = ack("bad", "consumer-a");
        if let InterfaceEvent::Acknowledge { scope, .. } = &mut e {
            scope.project_id = ProjectId::new("other").expect("fixture project");
        }
        assert!(reduce_interface(&Some(state.clone()), &e).is_err());
        e = ack("bad", "consumer-a");
        if let InterfaceEvent::Acknowledge {
            expected_state_revision,
            ..
        } = &mut e
        {
            *expected_state_revision = DecimalU64::new(9);
        }
        assert!(reduce_interface(&Some(state.clone()), &e).is_err());
        e = ack("bad", "consumer-a");
        if let InterfaceEvent::Acknowledge {
            recipient_epoch, ..
        } = &mut e
        {
            *recipient_epoch = "stale".into();
        }
        assert!(reduce_interface(&Some(state.clone()), &e).is_err());
        e = ack("bad", "consumer-a");
        if let InterfaceEvent::Acknowledge {
            artifact_digest, ..
        } = &mut e
        {
            *artifact_digest = digest('c');
        }
        assert!(reduce_interface(&Some(state), &e).is_err());
    }
    #[test]
    fn changed_interface_invalidates_exact_prior_ack_and_replay_is_idempotent() {
        let state = reduce_interface(&None, &publish()).expect("publish").state;
        let acked = reduce_interface(&Some(state), &ack("ack", "consumer-a")).expect("ACK");
        let replay = reduce_interface(&Some(acked.state.clone()), &ack("ack", "consumer-a"))
            .expect("exact replay");
        assert!(replay.replayed);
        assert!(replay.invalidations.is_empty());
        let mut update = publish();
        if let InterfaceEvent::Publish {
            event_id,
            contract,
            actor,
            expected_state_revision,
            ..
        } = &mut update
        {
            *event_id = id("publish-2");
            contract.revision = DecimalU64::new(2);
            contract.artifact_digest = digest('c');
            *actor = principal("owner");
            *expected_state_revision = Some(acked.state.revision.clone());
        }
        let changed = reduce_interface(&Some(acked.state.clone()), &update).expect("new revision");
        assert_eq!(changed.invalidations.len(), 1);
        assert_eq!(changed.invalidations[0].consumer, principal("consumer-a"));
        assert_eq!(
            changed.invalidations[0].previous_revision,
            DecimalU64::new(1)
        );
        assert!(changed.state.evidence.is_empty());
        let old_publish = reduce_interface(&Some(changed.state.clone()), &publish())
            .expect("retained publish replay");
        assert!(old_publish.replayed);
        let mut collision = ack("ack", "consumer-a");
        if let InterfaceEvent::Acknowledge {
            artifact_digest, ..
        } = &mut collision
        {
            *artifact_digest = digest('c');
        }
        assert!(reduce_interface(&Some(acked.state), &collision).is_err());
    }
    #[test]
    fn rejection_is_retained_and_bounds_are_explicit() {
        let state = reduce_interface(&None, &publish()).expect("publish").state;
        let reject = InterfaceEvent::Reject {
            event_id: id("reject"),
            interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
            scope: scope(),
            actor: principal("consumer-a"),
            recipient_epoch: "epoch-1".into(),
            interface_revision: DecimalU64::new(1),
            artifact_digest: digest('a'),
            schema_digest: digest('b'),
            expected_state_revision: DecimalU64::new(1),
            authority_ref: "grant-1".into(),
            reason: "schema mismatch".into(),
        };
        let reduced =
            reduce_interface(&Some(state.clone()), &reject).expect("structured rejection");
        assert!(
            matches!(reduced.state.evidence.get(&principal("consumer-a")), Some(CompatibilityEvidence::Rejected { reason, .. }) if reason == "schema mismatch")
        );
        let mut too_many = publish();
        if let InterfaceEvent::Publish {
            contract,
            recipient_epochs,
            ..
        } = &mut too_many
        {
            contract.consumers = (0..MAX_INTERFACE_CONSUMERS + 1)
                .map(|i| principal(&format!("c{i}")))
                .collect();
            *recipient_epochs = contract
                .consumers
                .iter()
                .cloned()
                .map(|recipient| RecipientEpoch {
                    recipient,
                    epoch: "e".into(),
                })
                .collect();
        }
        assert!(reduce_interface(&None, &too_many).is_err());
        assert!(serde_json::from_str::<DecimalU64>("\"01\"").is_err());
        let unknown = r#"{"kind":"publish","extra":true}"#;
        assert!(serde_json::from_str::<InterfaceEvent>(unknown).is_err());
    }

    #[test]
    fn authenticated_context_cannot_be_substituted_by_worker_scope_or_actor_fields() {
        let published = reduce_interface_for(
            &None,
            &publish(),
            &scope(),
            &principal("producer"),
            &principal("owner"),
        )
        .expect("authenticated publish");
        let before = published.state.clone();
        assert!(
            reduce_interface_for(
                &Some(before.clone()),
                &ack("a", "consumer-a"),
                &scope(),
                &principal("consumer-b"),
                &principal("owner")
            )
            .is_err()
        );
        for field in ["project", "root", "task", "authority"] {
            let mut foreign = scope();
            match field {
                "project" => foreign.project_id = ProjectId::new("foreign").expect("project"),
                "root" => foreign.root_id = RootId::new("foreign").expect("root"),
                "task" => foreign.task_id = TaskId::new("foreign").expect("task"),
                _ => foreign.authority_ref = "foreign".into(),
            }
            assert!(
                reduce_interface_for(
                    &Some(before.clone()),
                    &ack("a", "consumer-a"),
                    &foreign,
                    &principal("consumer-a"),
                    &principal("owner")
                )
                .is_err()
            );
        }
        assert!(
            reduce_interface_for(
                &Some(before.clone()),
                &ack("a", "consumer-a"),
                &scope(),
                &principal("consumer-a"),
                &principal("foreign-owner")
            )
            .is_err()
        );
        let after = reduce_interface_for(
            &Some(before.clone()),
            &ack("a", "consumer-a"),
            &scope(),
            &principal("consumer-a"),
            &principal("owner"),
        )
        .expect("exact peer grant");
        assert_eq!(published.state, before);
        assert!(matches!(
            after.state.evidence[&principal("consumer-a")],
            CompatibilityEvidence::ProvisionalAck { .. }
        ));
    }

    #[test]
    fn strict_parser_and_typed_bounds_reject_control_context_and_oversized_events() {
        let bytes = serde_json::to_vec(&publish()).expect("JSON");
        assert_eq!(decode_event(&bytes).expect("bounded event"), publish());
        let duplicate =
            String::from_utf8(bytes)
                .expect("UTF8")
                .replacen("{", "{\"kind\":\"publish\",", 1);
        assert!(decode_event(duplicate.as_bytes()).is_err());
        assert!(decode_event(&vec![b' '; MAX_INTERFACE_EVENT_BYTES + 1]).is_err());
        let mut oversized = publish();
        if let InterfaceEvent::Publish { contract, .. } = &mut oversized {
            contract.scope.authority_ref = "x".repeat(MAX_INTERFACE_EVENT_BYTES);
        }
        assert!(reduce_interface(&None, &oversized).is_err());
        let mut control = publish();
        if let InterfaceEvent::Publish {
            recipient_epochs, ..
        } = &mut control
        {
            recipient_epochs[0].epoch = "epoch\nforged".into();
        }
        assert!(reduce_interface(&None, &control).is_err());
    }

    #[test]
    fn checked_state_revision_overflow_and_event_capacity_do_not_mutate_evidence() {
        let mut state = reduce_interface(&None, &publish()).expect("publish").state;
        state.revision = DecimalU64::new(u64::MAX);
        let mut event = ack("overflow", "consumer-a");
        if let InterfaceEvent::Acknowledge {
            expected_state_revision,
            ..
        } = &mut event
        {
            *expected_state_revision = state.revision.clone();
        }
        let before = state.clone();
        assert!(reduce_interface(&Some(state.clone()), &event).is_err());
        assert_eq!(state, before);
        state.revision = DecimalU64::new(1);
        for index in 1..MAX_INTERFACE_EVENTS {
            state
                .applied_events
                .insert(id(&format!("event-{index}")), digest('a'));
        }
        assert_eq!(state.applied_events.len(), MAX_INTERFACE_EVENTS);
        assert!(reduce_interface(&Some(state), &ack("new", "consumer-a")).is_err());
    }
}
