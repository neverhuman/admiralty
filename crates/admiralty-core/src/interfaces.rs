//! Pure, bounded interface compatibility evidence. Callers authenticate grants,
//! durably append events and persist invalidation proposals atomically.
use crate::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_INTERFACE_EVENT_BYTES: usize = 1_048_576;
pub const MAX_INTERFACE_CONSUMERS: usize = 1_024;
pub const MAX_INTERFACE_EVENTS: usize = 1_024;
pub const MAX_INTERFACE_STRING_BYTES: usize = 4_096;

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub authority_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceContract {
    pub interface_id: InterfaceId,
    pub scope: InterfaceScope,
    pub producer: PrincipalId,
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

pub fn reduce_interface(
    current: &Option<InterfaceState>,
    event: &InterfaceEvent,
) -> Result<InterfaceReduction, InterfaceError> {
    validate_event(event)?;
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
    value.len() <= MAX_INTERFACE_STRING_BYTES
}
fn validate_event(event: &InterfaceEvent) -> Result<(), InterfaceError> {
    let mut counter = EventSize(MAX_INTERFACE_EVENT_BYTES);
    serde_json::to_writer(&mut counter, event)
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
struct EventSize(usize);
impl std::io::Write for EventSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("event size limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
}
