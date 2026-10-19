//! Full safe-turn context compiled from an actual scoped Store journal read.
//!
//! A source reference or matching digest never manufactures compiler custody.
//! Store returns opaque replay-validated originals under current recording AND
//! context-read policy. Independent expectations prevent accidentally accepting
//! another batch, baseline, render, task revision or epoch. Complete original
//! role capsules and updates are retained without prose summarization or delta
//! assumptions. This module performs no send, ACK, wake, baseline change, token
//! savings measurement, or execution/authority grant. Caller read permissions
//! and expectations must be independently configured; JSON is not an issuer.
use admiralty_core::{
    content_digest, coordination::context::MAX_ROLE_CAPSULE_BYTES, governor::DeliveryDecision,
};
use admiralty_store::{
    Store, StoreError,
    attention::{
        AttentionContextPin, AttentionOutcome, AttentionProof, AttentionScope,
        AuthorizedDeliveryContext, LoadDeliveryContext, MAX_DELIVERY_CONTEXT_SOURCES,
    },
    context_renders::ContextBaseline,
};
use serde::Serialize;

const MAX_EXPECTATION_BYTES: usize = 32 * 1024;

/// Explicit baseline observation. Unknown requires recovery/fresh input rather
/// than treating an unobserved baseline as absent or assuming opaque memory.
#[derive(Debug, Serialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum ExpectedBaseline {
    Absent,
    Exact { baseline: ContextBaseline },
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct SafeTurnSourcePin {
    pub order: String,
    pub update_digest: String,
    pub context: AttentionContextPin,
    pub contract_digest: String,
}

/// Independently retained expected bindings, not wire authentication. No
/// deserializer or default synthesizes an expected identity from source data.
#[derive(Debug, Serialize)]
pub struct SafeTurnExpectation {
    pub scope: AttentionScope,
    pub command_id: String,
    pub receipt_digest: String,
    pub batch_digest: String,
    pub request_context: AttentionContextPin,
    pub baseline: ExpectedBaseline,
    /// Exactly one entry per actual delivery ref, in its immutable batch order.
    pub sources: Vec<SafeTurnSourcePin>,
}

#[derive(Debug)]
pub enum SafeTurnContextError {
    InvalidExpectation,
    BindingMismatch,
    BaselineUnknown,
    BudgetExceeded,
    Store(StoreError),
}
impl std::fmt::Display for SafeTurnContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidExpectation => "invalid or oversized safe-turn expectations",
            Self::BindingMismatch => {
                "safe-turn evidence differs from independently retained bindings"
            }
            Self::BaselineUnknown => {
                "safe-turn baseline is unknown; recover context before compiling"
            }
            Self::BudgetExceeded => {
                "complete safe-turn context exceeds budget; split or expand under policy"
            }
            Self::Store(_) => "safe-turn source retrieval failed",
        })
    }
}
impl std::error::Error for SafeTurnContextError {}

/// Immutable compiler output only. Its digest describes the exact rendered
/// bytes; possession never attests transport, provider memory, ACK or authority.
#[derive(Debug)]
pub struct CompiledSafeTurnContext {
    text: String,
    rendered_digest: String,
    batch_digest: String,
    receipt_digest: String,
}
impl CompiledSafeTurnContext {
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn rendered_digest(&self) -> &str {
        &self.rendered_digest
    }
    pub fn batch_digest(&self) -> &str {
        &self.batch_digest
    }
    pub fn receipt_digest(&self) -> &str {
        &self.receipt_digest
    }
    pub fn estimated_tokens(&self) -> usize {
        self.text.len().div_ceil(4)
    }
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn positive_decimal(value: &str) -> bool {
    value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0)
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

#[derive(Serialize)]
struct Document<'a> {
    protocol_version: u8,
    kind: &'static str,
    /// Full context even when a baseline exists: opaque CLI memory is unknown.
    mode: &'static str,
    batch_digest: &'a str,
    evidence: &'a AuthorizedDeliveryContext,
    delivered: bool,
    application_acknowledged: bool,
    authority_granted: bool,
}

/// Reads once on Store's sole writer, then compiles the exact returned immutable
/// snapshot. Current-policy changes after that read do not grant later delivery;
/// an eventual sender must independently authorize its own operation. No Store
/// mutations occur on either success or failure of this compiler.
pub fn compile_safe_turn_context(
    store: &Store,
    expected: &SafeTurnExpectation,
    proof: AttentionProof,
    max_bytes: usize,
) -> Result<CompiledSafeTurnContext, SafeTurnContextError> {
    if max_bytes == 0 || max_bytes > MAX_ROLE_CAPSULE_BYTES {
        return Err(SafeTurnContextError::BudgetExceeded);
    }
    admiralty_core::limits::bounded_json_size(expected, MAX_EXPECTATION_BYTES)
        .map_err(|_| SafeTurnContextError::InvalidExpectation)?;
    if !id(&expected.command_id)
        || !digest(&expected.receipt_digest)
        || !digest(&expected.batch_digest)
        || expected.sources.is_empty()
        || expected.sources.len() > MAX_DELIVERY_CONTEXT_SOURCES
        || expected.sources.iter().any(|p| {
            !positive_decimal(&p.order) || !digest(&p.update_digest) || !digest(&p.contract_digest)
        })
    {
        return Err(SafeTurnContextError::InvalidExpectation);
    }
    if matches!(expected.baseline, ExpectedBaseline::Unknown) {
        return Err(SafeTurnContextError::BaselineUnknown);
    }
    let evidence = store
        .attention_delivery_context(
            LoadDeliveryContext {
                scope: expected.scope.clone(),
                command_id: expected.command_id.clone(),
                receipt_digest: expected.receipt_digest.clone(),
            },
            proof,
        )
        .map_err(SafeTurnContextError::Store)?;
    let AttentionOutcome::Delivery {
        decision: DeliveryDecision::Wake { delivery },
    } = &evidence.result().receipt.outcome
    else {
        return Err(SafeTurnContextError::BindingMismatch);
    };
    if evidence.wake_request().binding.context != expected.request_context
        || content_digest(delivery).ok().as_deref() != Some(&expected.batch_digest)
        || evidence.sources().len() != expected.sources.len()
        || delivery.refs.len() != expected.sources.len()
    {
        return Err(SafeTurnContextError::BindingMismatch);
    }
    match (&expected.baseline, evidence.baseline()) {
        (ExpectedBaseline::Absent, None) => {}
        (ExpectedBaseline::Exact { baseline: expected }, Some(actual)) if expected == actual => {}
        _ => return Err(SafeTurnContextError::BindingMismatch),
    }
    for ((source, pin), reference) in evidence
        .sources()
        .iter()
        .zip(&expected.sources)
        .zip(&delivery.refs)
    {
        if source.reference() != reference
            || reference.order != pin.order
            || reference.update_digest != pin.update_digest
            || source.observe_request().binding.context != pin.context
            || content_digest(source.current_contract()).ok().as_deref()
                != Some(&pin.contract_digest)
        {
            return Err(SafeTurnContextError::BindingMismatch);
        }
    }
    let document = Document {
        protocol_version: 1,
        kind: "admiralty_safe_turn_context",
        mode: "complete_protected_sources",
        batch_digest: &expected.batch_digest,
        evidence: &evidence,
        delivered: false,
        application_acknowledged: false,
        authority_granted: false,
    };
    admiralty_core::limits::bounded_json_size(&document, max_bytes)
        .map_err(|_| SafeTurnContextError::BudgetExceeded)?;
    let text =
        serde_json::to_string(&document).map_err(|_| SafeTurnContextError::BudgetExceeded)?;
    let rendered_digest =
        content_digest(&document).map_err(|_| SafeTurnContextError::BindingMismatch)?;
    Ok(CompiledSafeTurnContext {
        text,
        rendered_digest,
        batch_digest: expected.batch_digest.clone(),
        receipt_digest: expected.receipt_digest.clone(),
    })
}
