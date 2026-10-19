//! Additive v2 coordination headers. These are untrusted wire data, never grants.
//!
//! A validated header binds routing, immutable references and bytes. It does not
//! authenticate a principal, establish artifact custody, decode a kind's body,
//! authorize an effect, or acknowledge delivery/application. Those checks belong
//! to the independently installed caller policy and the selected body decoder.
//! The closed v1 `CoordinationMessage` remains a separate, unchanged protocol.

use crate::interfaces::DecimalU64;
use crate::limits::bounded_json_size;
use crate::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

pub const MAX_ENVELOPE_BYTES: usize = 16 * 1024;
pub const MAX_ENVELOPE_DEPTH: usize = 16;
pub const MAX_AUTHORITY_REFERENCE_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("coordination header exceeds its byte/depth bound")]
    BoundExceeded,
    #[error("invalid coordination header: {0}")]
    Invalid(&'static str),
    #[error("coordination JSON is malformed, duplicated, unknown or unsupported")]
    Json,
    #[error("coordination binding differs: {0}")]
    Binding(&'static str),
    #[error("referenced payload bytes differ")]
    PayloadMismatch,
}

/// A constant encoded as the integer 2, including in its derived JSON schema.
/// Deserializing another version never silently interprets it as this protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtocolVersion2;
impl Serialize for ProtocolVersion2 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(2)
    }
}
impl<'de> Deserialize<'de> for ProtocolVersion2 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if u8::deserialize(deserializer)? == 2 {
            Ok(Self)
        } else {
            Err(de::Error::custom(
                "expected coordination protocol version 2",
            ))
        }
    }
}
impl JsonSchema for ProtocolVersion2 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CoordinationProtocolVersion2".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"integer", "const":2})
    }
}

macro_rules! identifier {
    ($name:ident, $max:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, EnvelopeError> {
                let value = value.into();
                if value.is_empty() || value.len() > $max || value == "." || value == ".."
                    || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) {
                    return Err(EnvelopeError::Invalid(stringify!($name)));
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl TryFrom<String> for $name {
            type Error = EnvelopeError;
            fn try_from(value: String) -> Result<Self, Self::Error> { Self::new(value) }
        }
        impl From<$name> for String { fn from(value: $name) -> Self { value.0 } }
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"type":"string", "minLength":1, "maxLength":$max,
                    "pattern":r"^(?!\.{1,2}(?![\s\S]))[A-Za-z0-9_.-]+(?![\s\S])"})
            }
        }
    };
}
identifier!(TopicId, 256);
identifier!(CorrelationId, 128);
identifier!(ContextEpoch, 128);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);
impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, EnvelopeError> {
        let value = value.into();
        if value.len() != 71
            || !value.starts_with("sha256:")
            || !value.as_bytes()[7..]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            return Err(EnvelopeError::Invalid("canonical sha256 digest"));
        }
        Ok(Self(value))
    }
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Sha256Digest {
    type Error = EnvelopeError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Sha256Digest> for String {
    fn from(value: Sha256Digest) -> Self {
        value.0
    }
}
impl JsonSchema for Sha256Digest {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CoordinationSha256Digest".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string", "pattern":r"^sha256:[0-9a-f]{64}(?![\s\S])", "minLength":71, "maxLength":71})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskRevisionRef {
    #[schemars(schema_with = "canonical_id_schema")]
    pub task_id: TaskId,
    #[schemars(range(min = 1))]
    pub revision: u32,
    pub contract_digest: Sha256Digest,
}

/// This component resolves only a concrete principal, never an implicit group.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecipientScope {
    Principal {
        #[schemars(schema_with = "canonical_id_schema")]
        principal_id: PrincipalId,
    },
}

/// Versioned projection watermark; this is deliberately not a legacy cursor alias.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub recovery_generation: DecimalU64,
    pub event_seq: DecimalU64,
    #[schemars(range(min = 1))]
    pub projection_schema: u32,
    pub authorization_scope_digest: Sha256Digest,
}

/// Positive canonical decimal counter, sharing the existing u64 wire codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorityGeneration(DecimalU64);
impl AuthorityGeneration {
    pub fn new(value: u64) -> Result<Self, EnvelopeError> {
        if value == 0 {
            return Err(EnvelopeError::Invalid("authority generation"));
        }
        Ok(Self(DecimalU64::new(value)))
    }
    pub fn get(&self) -> u64 {
        self.0.get()
    }
}
impl Serialize for AuthorityGeneration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for AuthorityGeneration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(DecimalU64::deserialize(deserializer)?.get()).map_err(de::Error::custom)
    }
}
impl JsonSchema for AuthorityGeneration {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CoordinationAuthorityGeneration".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let decimal = generator.subschema_for::<DecimalU64>();
        json_schema!({"allOf":[decimal,{"not":{"const":"0"}}]})
    }
}

/// A claimed reference to externally checked policy; JSON cannot install policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityRef {
    #[schemars(length(min = 1, max = MAX_AUTHORITY_REFERENCE_BYTES), pattern(r"^[!-~]+(?![\s\S])"))]
    pub reference: String,
    pub generation: AuthorityGeneration,
    pub policy_digest: Sha256Digest,
    pub scope_digest: Sha256Digest,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PayloadRef {
    pub sha256: Sha256Digest,
    pub size_bytes: DecimalU64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationKind {
    TaskAssigned,
    TaskChanged,
    ProgressChanged,
    ArtifactReady,
    InterfaceProposed,
    InterfaceAccepted,
    QuestionAsked,
    QuestionAnswered,
    DecisionRequested,
    DecisionRecorded,
    VerificationFailed,
    HandoffReady,
    ClaimConflict,
    BudgetWarning,
    CancelRequested,
    SnapshotRequired,
}

/// Queue classification only. Even `Control` confers no action capability.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Control,
    Decision,
    Usage,
    Work,
    Progress,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclaredScope {
    #[schemars(schema_with = "canonical_id_schema")]
    pub project_id: ProjectId,
    #[schemars(schema_with = "canonical_id_schema")]
    pub root_id: RootId,
    #[schemars(schema_with = "canonical_id_schema")]
    pub sender: PrincipalId,
    pub recipient: RecipientScope,
    pub topic: TopicId,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(schema_with = "nullable_schema::<TaskRevisionRef>")]
    pub task_revision: Option<TaskRevisionRef>,
}
impl DeclaredScope {
    pub fn digest(&self) -> Result<Sha256Digest, EnvelopeError> {
        digest_json(&("coordination_scope_v2", self))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoordinationEnvelopeV2 {
    pub protocol_version: ProtocolVersion2,
    #[schemars(schema_with = "canonical_id_schema")]
    pub message_id: MessageId,
    #[schemars(schema_with = "canonical_id_schema")]
    pub project_id: ProjectId,
    #[schemars(schema_with = "canonical_id_schema")]
    pub root_id: RootId,
    #[schemars(schema_with = "canonical_id_schema")]
    pub sender: PrincipalId,
    pub recipient: RecipientScope,
    pub topic: TopicId,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(schema_with = "nullable_schema::<TaskRevisionRef>")]
    pub task_revision: Option<TaskRevisionRef>,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(schema_with = "nullable_id_schema")]
    pub causation_id: Option<MessageId>,
    pub correlation_id: CorrelationId,
    pub source_cursor: Cursor,
    pub authority: AuthorityRef,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(schema_with = "nullable_schema::<ContextEpoch>")]
    pub context_epoch: Option<ContextEpoch>,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(schema_with = "nullable_schema::<Sha256Digest>")]
    pub baseline_digest: Option<Sha256Digest>,
    pub protected_facts_digest: Sha256Digest,
    pub payload_ref: PayloadRef,
    pub kind: CoordinationKind,
    pub priority: Priority,
    /// Unix milliseconds, supplied as a canonical decimal string, never a float.
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(schema_with = "nullable_schema::<DecimalU64>")]
    pub expires_at_unix_ms: Option<DecimalU64>,
}

// These fields must be present, even when explicitly null. This prevents a
// compressed/reduced wire representation from silently omitting protected
// epoch/baseline/lineage metadata. The custom schema retains nullability while
// making the field required, matching the custom deserializer.
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
fn nullable_schema<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<Option<T>>()
}
fn canonical_id_schema(_: &mut SchemaGenerator) -> Schema {
    // Match the reused ProjectId/RootId/TaskId/PrincipalId/MessageId constructors
    // without changing their legacy schemas or creating a second ID codec.
    json_schema!({"type":"string", "minLength":1, "maxLength":128,
        "pattern":r"^(?!\.{1,2}(?![\s\S]))[A-Za-z0-9_.-]+(?![\s\S])"})
}
fn nullable_id_schema(generator: &mut SchemaGenerator) -> Schema {
    json_schema!({"anyOf":[canonical_id_schema(generator), {"type":"null"}]})
}

/// Independently supplied comparison data, deliberately not deserializable.
/// Equality with these values is NOT authentication or proof of current grants.
pub struct ExpectedBindings<'a> {
    pub scope: &'a DeclaredScope,
    pub source_cursor: &'a Cursor,
    pub authority: &'a AuthorityRef,
    pub context_epoch: Option<&'a ContextEpoch>,
    pub baseline_digest: Option<&'a Sha256Digest>,
    pub protected_facts_digest: &'a Sha256Digest,
    /// Independently retained exact header identity, covering every header field.
    pub envelope_digest: &'a Sha256Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expiry {
    NoExpiry,
    UnknownObservation,
    Live,
    Expired,
}

impl CoordinationEnvelopeV2 {
    /// Bound raw bytes and nesting BEFORE serde allocates strings/objects. Serde
    /// then rejects duplicate struct fields, unknown fields and unknown variants.
    pub fn parse(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        preflight(bytes)?;
        let envelope: Self = serde_json::from_slice(bytes).map_err(|_| EnvelopeError::Json)?;
        envelope.validate()?;
        Ok(envelope)
    }
    pub fn scope(&self) -> DeclaredScope {
        DeclaredScope {
            project_id: self.project_id.clone(),
            root_id: self.root_id.clone(),
            sender: self.sender.clone(),
            recipient: self.recipient.clone(),
            topic: self.topic.clone(),
            task_revision: self.task_revision.clone(),
        }
    }
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        bounded_json_size(self, MAX_ENVELOPE_BYTES).map_err(|_| EnvelopeError::BoundExceeded)?;
        if self.authority.reference.is_empty()
            || self.authority.reference.len() > MAX_AUTHORITY_REFERENCE_BYTES
            || !self
                .authority
                .reference
                .bytes()
                .all(|b| b.is_ascii_graphic())
        {
            return Err(EnvelopeError::Invalid("authority reference"));
        }
        if self.authority.generation.get() == 0 {
            return Err(EnvelopeError::Invalid("authority generation"));
        }
        if self.source_cursor.projection_schema == 0 {
            return Err(EnvelopeError::Invalid("projection schema"));
        }
        if self
            .task_revision
            .as_ref()
            .is_some_and(|task| task.revision == 0)
        {
            return Err(EnvelopeError::Invalid("task revision"));
        }
        if self.baseline_digest.is_some() && self.context_epoch.is_none() {
            return Err(EnvelopeError::Invalid("baseline requires context epoch"));
        }
        if self.causation_id.as_ref() == Some(&self.message_id) {
            return Err(EnvelopeError::Invalid("self causation"));
        }
        let scope_digest = self.scope().digest()?;
        if self.authority.scope_digest != scope_digest
            || self.source_cursor.authorization_scope_digest != scope_digest
        {
            return Err(EnvelopeError::Binding("declared routing scope digest"));
        }
        Ok(())
    }
    /// Canonical fixed-field JSON, not a signature or a custody proof.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EnvelopeError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| EnvelopeError::Json)
    }
    pub fn digest(&self) -> Result<Sha256Digest, EnvelopeError> {
        self.validate()?;
        digest_json(self)
    }
    pub fn validate_for(&self, expected: &ExpectedBindings<'_>) -> Result<(), EnvelopeError> {
        self.validate()?;
        if self.scope() != *expected.scope {
            return Err(EnvelopeError::Binding("routing/task scope"));
        }
        if &self.source_cursor != expected.source_cursor {
            return Err(EnvelopeError::Binding("source cursor"));
        }
        if &self.authority != expected.authority {
            return Err(EnvelopeError::Binding("authority reference"));
        }
        if self.context_epoch.as_ref() != expected.context_epoch {
            return Err(EnvelopeError::Binding("context epoch"));
        }
        if self.baseline_digest.as_ref() != expected.baseline_digest {
            return Err(EnvelopeError::Binding("baseline"));
        }
        if &self.protected_facts_digest != expected.protected_facts_digest {
            return Err(EnvelopeError::Binding("protected facts"));
        }
        if &self.digest()? != expected.envelope_digest {
            return Err(EnvelopeError::Binding("exact envelope digest"));
        }
        Ok(())
    }
    /// Caller bounds artifact retrieval separately. This verifies the exact raw
    /// bytes (not reserialized JSON); it cannot turn a reference into custody.
    pub fn verify_payload_bytes(&self, bytes: &[u8]) -> Result<(), EnvelopeError> {
        self.validate()?;
        if u64::try_from(bytes.len()).ok() != Some(self.payload_ref.size_bytes.get())
            || Sha256Digest::of_bytes(bytes) != self.payload_ref.sha256
        {
            return Err(EnvelopeError::PayloadMismatch);
        }
        Ok(())
    }
    /// Pure observation only; callers must durably record an expiry disposition.
    /// Missing observation stays unknown. Nothing is silently dropped or decided.
    pub fn expiry_at(&self, observed_unix_ms: Option<&DecimalU64>) -> Expiry {
        match (&self.expires_at_unix_ms, observed_unix_ms) {
            (None, _) => Expiry::NoExpiry,
            (Some(_), None) => Expiry::UnknownObservation,
            (Some(expiry), Some(observed)) if observed.get() >= expiry.get() => Expiry::Expired,
            (Some(_), Some(_)) => Expiry::Live,
        }
    }
}

fn digest_json(value: &impl Serialize) -> Result<Sha256Digest, EnvelopeError> {
    bounded_json_size(value, MAX_ENVELOPE_BYTES).map_err(|_| EnvelopeError::BoundExceeded)?;
    Sha256Digest::new(crate::content_digest(value).map_err(|_| EnvelopeError::Json)?)
}

fn preflight(bytes: &[u8]) -> Result<(), EnvelopeError> {
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(EnvelopeError::BoundExceeded);
    }
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth = depth.checked_add(1).ok_or(EnvelopeError::BoundExceeded)?;
                    if depth > MAX_ENVELOPE_DEPTH {
                        return Err(EnvelopeError::BoundExceeded);
                    }
                }
                b'}' | b']' => depth = depth.checked_sub(1).ok_or(EnvelopeError::Json)?,
                _ => {}
            }
        }
    }
    if in_string || depth != 0 {
        return Err(EnvelopeError::Json);
    }
    Ok(())
}
