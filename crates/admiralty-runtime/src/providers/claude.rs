//! Offline Claude Code launch proposals and strict, complete-turn observations.
//!
//! No API here admits a launch, verifies filesystem custody, attests a CLI binary,
//! proves journal/capture continuity, settles usage, or grants task acceptance.
pub use super::observation::{MAX_EVENT_BYTES, MAX_EVENTS, MAX_STREAM_BYTES};
use super::observation::{ObservationError, decode_complete};
use crate::session::{MAX_INPUT_BYTES, MAX_LAUNCH_SPEC_BYTES};
use admiralty_core::{content_digest, limits::bounded_json_size};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const ADAPTER_VERSION: u32 = 2;
/// Captured fixture dialect, not a qualified executable/version attestation.
pub const OBSERVED_CLI_VERSION: &str = "2.1.302";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClaudeError {
    #[error("invalid Claude adapter input: {0}")]
    Invalid(&'static str),
    #[error("Claude stream or input exceeds configured bounds")]
    Bounds,
    #[error("malformed or ambiguous Claude JSONL event")]
    Json,
    #[error("Claude stream has an unterminated frame")]
    Framing,
    #[error("expected session does not match observed session")]
    ResumeMismatch,
    #[error("Claude fixture dialect version is unsupported")]
    VersionMismatch,
}

impl From<ObservationError> for ClaudeError {
    fn from(error: ObservationError) -> Self {
        match error {
            ObservationError::Bounds => Self::Bounds,
            ObservationError::Json => Self::Json,
            ObservationError::Framing | ObservationError::Closed => Self::Framing,
        }
    }
}

/// An UNADMITTED argv proposal. Validation is lexical only. Passing it cannot
/// establish contract, model registry, resource, credential or custody authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClaudeLaunch {
    pub program: PathBuf,
    pub cwd: PathBuf,
    pub model: String,
    pub effort: Effort,
    pub max_budget_usd: String,
    /// `default` explicitly means omit the permission flag. Other accepted
    /// choices are a conservative subset of captured help, not a grant.
    pub permission_mode: String,
    /// Fresh explicit IDs must be UUIDs; resume selectors are bounded identifiers.
    pub session_id: Option<String>,
    pub resume: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy)]
enum PermissionChoice {
    Omit,
    Explicit(&'static str),
}

fn permission_choice(value: &str) -> Result<PermissionChoice, ClaudeError> {
    match value {
        "default" => Ok(PermissionChoice::Omit),
        "acceptEdits" => Ok(PermissionChoice::Explicit("acceptEdits")),
        "dontAsk" => Ok(PermissionChoice::Explicit("dontAsk")),
        "manual" => Ok(PermissionChoice::Explicit("manual")),
        "plan" => Ok(PermissionChoice::Explicit("plan")),
        _ => Err(ClaudeError::Invalid("unsupported permission mode")),
    }
}

impl ClaudeLaunch {
    pub fn validate(&self) -> Result<(), ClaudeError> {
        if !safe_abs(&self.program) || !safe_abs(&self.cwd) || self.program.file_name().is_none() {
            return Err(ClaudeError::Invalid(
                "absolute bounded program and cwd required",
            ));
        }
        if !text(&self.model, 128) || self.model.starts_with('-') {
            return Err(ClaudeError::Invalid("explicit bounded model required"));
        }
        if self.max_budget_usd.len() > 32 {
            return Err(ClaudeError::Bounds);
        }
        let budget = self
            .max_budget_usd
            .parse::<f64>()
            .map_err(|_| ClaudeError::Invalid("invalid budget"))?;
        if !budget.is_finite() || budget <= 0.0 || budget.to_string() != self.max_budget_usd {
            return Err(ClaudeError::Invalid("canonical positive budget required"));
        }
        permission_choice(&self.permission_mode)?;
        match (&self.session_id, self.resume) {
            (None, true) => {
                return Err(ClaudeError::Invalid(
                    "resume requires an explicit session selector",
                ));
            }
            (Some(id), true) if !session_identifier(id) || id.starts_with('-') => {
                return Err(ClaudeError::Invalid("invalid resume selector"));
            }
            (Some(id), false) if !uuid(id) => {
                return Err(ClaudeError::Invalid("fresh session ID must be a UUID"));
            }
            _ => {}
        }
        bounded_json_size(self, MAX_LAUNCH_SPEC_BYTES).map_err(|_| ClaudeError::Bounds)?;
        Ok(())
    }

    pub fn is_admitted(&self) -> bool {
        false
    }

    /// Shell-free UNADMITTED proposal. Managed launch envelopes, immutable
    /// admission/configuration binding and actual CLI behavior remain unqualified.
    pub fn args(&self) -> Result<Vec<String>, ClaudeError> {
        self.validate()?;
        let mut args = vec![
            "--print".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--model".into(),
            self.model.clone(),
            "--effort".into(),
            self.effort.as_str().into(),
            "--max-budget-usd".into(),
            self.max_budget_usd.clone(),
            "--tools".into(),
            String::new(),
            "--restricted".into(),
            "--strict-mcp-config".into(),
        ];
        if let PermissionChoice::Explicit(mode) = permission_choice(&self.permission_mode)? {
            args.extend(["--permission-mode".into(), mode.into()]);
        }
        if let Some(id) = &self.session_id {
            args.extend([
                if self.resume {
                    "--resume"
                } else {
                    "--session-id"
                }
                .into(),
                id.clone(),
            ]);
        }
        Ok(args)
    }

    /// Encode one JSONL input within the session-host write bound, including LF
    /// and escaping. Larger capsules require retrieval or separately qualified
    /// chunking; this API never truncates or retries them.
    pub fn input_frame(prompt: &str) -> Result<Vec<u8>, ClaudeError> {
        if prompt.trim().is_empty()
            || prompt
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(ClaudeError::Invalid(
                "nonempty prompt with supported whitespace required",
            ));
        }
        if prompt.len() > MAX_INPUT_BYTES {
            return Err(ClaudeError::Bounds);
        }
        #[derive(Serialize)]
        struct Message<'a> {
            role: &'static str,
            content: &'a str,
        }
        #[derive(Serialize)]
        struct Frame<'a> {
            #[serde(rename = "type")]
            kind: &'static str,
            message: Message<'a>,
        }
        let value = Frame {
            kind: "user",
            message: Message {
                role: "user",
                content: prompt,
            },
        };
        bounded_json_size(&value, MAX_INPUT_BYTES - 1).map_err(|_| ClaudeError::Bounds)?;
        let mut frame = serde_json::to_vec(&value).map_err(|_| ClaudeError::Json)?;
        frame.push(b'\n');
        Ok(frame)
    }
}

impl Effort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}
fn text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn session_identifier(value: &str) -> bool {
    text(value, 128) && !value.chars().any(char::is_whitespace)
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
fn safe_abs(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| text(s, 4096) && !s.contains('\\'))
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thinking_tokens: Option<u64>,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Provider-reported turn result only; never task completion or acceptance.
/// A permission denial remains a separate observation even on a successful turn.
pub enum TurnOutcome {
    Succeeded,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    ParentTerminal,
    Unknown,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventScope {
    Parent,
    OpaqueChild,
}

/// Ordinal/digest identify ordered observations, not durable producer IDs,
/// journal offsets, proof of process EOF or permission to replay input.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EventObservation {
    pub ordinal: usize,
    pub event_digest: String,
    pub event_type: String,
    pub scope: EventScope,
    pub usage: Option<Usage>,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Observation {
    pub session_id: Option<String>,
    pub version: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub event_types: Vec<String>,
    pub events: Vec<EventObservation>,
    pub permission_denials: usize,
    pub permission_denials_known: bool,
    /// Only parent terminal usage; missing categories stay unknown. Never an
    /// aggregate of assistant/opaque child records, and never settled billing.
    pub usage: Usage,
    pub usage_source: UsageSource,
    pub total_cost_usd: Option<f64>,
    pub opaque_subagents: bool,
    pub result_subtype: Option<String>,
    pub is_error: Option<bool>,
    pub outcome: TurnOutcome,
    pub protocol_unknown: bool,
    native_execution_qualified: bool,
    trusted_acceptance: bool,
}
impl Observation {
    pub fn native_execution_qualified(&self) -> bool {
        self.native_execution_qualified
    }
    pub fn trusted_acceptance(&self) -> bool {
        self.trusted_acceptance
    }
}

/// Parse one LF-terminated initialized parent turn and one terminal result.
/// Session matching is lexical observation, not launch/configuration binding or
/// proof of owned process/capture completion. The expected ID can name a fresh
/// session or a resume; no resume is executed here.
pub fn parse_stream(
    bytes: &[u8],
    expected_session: Option<&str>,
) -> Result<Observation, ClaudeError> {
    if expected_session.is_some_and(|id| !session_identifier(id)) {
        return Err(ClaudeError::Invalid("invalid expected session"));
    }
    let values = decode_complete(bytes)?;
    let mut out = Observation {
        session_id: None,
        version: None,
        model: None,
        permission_mode: None,
        event_types: Vec::new(),
        events: Vec::new(),
        permission_denials: 0,
        permission_denials_known: false,
        usage: Usage::default(),
        usage_source: UsageSource::Unknown,
        total_cost_usd: None,
        opaque_subagents: false,
        result_subtype: None,
        is_error: None,
        outcome: TurnOutcome::Unknown,
        protocol_unknown: false,
        native_execution_qualified: false,
        trusted_acceptance: false,
    };
    let mut terminal = false;
    for value in values {
        if terminal {
            return Err(ClaudeError::Invalid("event after parent terminal result"));
        }
        let kind = required(&value, "type", 128)?;
        let scope = match value.get("parent_tool_use_id") {
            None | Some(Value::Null) => EventScope::Parent,
            Some(Value::String(id)) if text(id, 256) => EventScope::OpaqueChild,
            _ => return Err(ClaudeError::Invalid("malformed parent tool identity")),
        };
        let usage;
        if kind == "system" && optional(&value, "subtype", 128)?.as_deref() == Some("init") {
            if out.session_id.is_some() || scope != EventScope::Parent {
                return Err(ClaudeError::Invalid("duplicate or child init event"));
            }
            let session = required(&value, "session_id", 128)?;
            if !session_identifier(&session) {
                return Err(ClaudeError::Invalid("invalid observed session"));
            }
            if expected_session.is_some_and(|expected| expected != session) {
                return Err(ClaudeError::ResumeMismatch);
            }
            let version = required(&value, "claude_code_version", 64)?;
            if version != OBSERVED_CLI_VERSION {
                return Err(ClaudeError::VersionMismatch);
            }
            out.model = Some(required(&value, "model", 256)?);
            let mode = required(&value, "permissionMode", 64)?;
            if !matches!(
                mode.as_str(),
                "default"
                    | "acceptEdits"
                    | "auto"
                    | "bypassPermissions"
                    | "manual"
                    | "dontAsk"
                    | "plan"
            ) {
                return Err(ClaudeError::Invalid("unsupported observed permission mode"));
            }
            out.session_id = Some(session);
            out.version = Some(version);
            out.permission_mode = Some(mode);
            usage = supplied_usage(&value)?;
            if usage.is_some() {
                return Err(ClaudeError::Invalid("usage on init is unsupported"));
            }
        } else {
            let session = out
                .session_id
                .as_deref()
                .ok_or(ClaudeError::Invalid("init must precede all events"))?;
            if let Some(id) = optional(&value, "session_id", 128)?
                && id != session
            {
                return Err(ClaudeError::ResumeMismatch);
            }
            out.opaque_subagents |= scope == EventScope::OpaqueChild;
            usage = match kind.as_str() {
                "assistant" => {
                    let message = value
                        .get("message")
                        .filter(|v| v.is_object())
                        .ok_or(ClaudeError::Invalid("assistant message object required"))?;
                    supplied_usage(message)?
                }
                "result" => {
                    if required(&value, "session_id", 128)? != session {
                        return Err(ClaudeError::ResumeMismatch);
                    }
                    let result_usage = supplied_usage(&value)?;
                    let subtype = required(&value, "subtype", 128)?;
                    let is_error = match value.get("is_error") {
                        None => None,
                        Some(Value::Bool(flag)) => Some(*flag),
                        _ => return Err(ClaudeError::Invalid("malformed result error flag")),
                    };
                    let denials = match value.get("permission_denials") {
                        None => None,
                        Some(Value::Array(entries)) if entries.iter().all(Value::is_object) => {
                            Some(entries.len())
                        }
                        _ => return Err(ClaudeError::Invalid("malformed permission denials")),
                    };
                    let cost = match value.get("total_cost_usd") {
                        None => None,
                        Some(cost) => Some(
                            cost.as_f64()
                                .filter(|n| n.is_finite() && *n >= 0.0)
                                .ok_or(ClaudeError::Invalid("malformed reported cost"))?,
                        ),
                    };
                    let error_subtype = matches!(
                        subtype.as_str(),
                        "error_during_execution"
                            | "error_max_turns"
                            | "error_max_budget_usd"
                            | "error_max_structured_output_retries"
                    );
                    if (subtype == "success" && is_error == Some(true))
                        || (error_subtype && is_error == Some(false))
                    {
                        return Err(ClaudeError::Invalid("contradictory terminal outcome"));
                    }
                    if scope == EventScope::Parent {
                        out.protocol_unknown |= subtype != "success" && !error_subtype;
                        out.outcome = if is_error == Some(true) || error_subtype {
                            TurnOutcome::Failed
                        } else if subtype == "success"
                            && is_error == Some(false)
                            && !out.protocol_unknown
                        {
                            TurnOutcome::Succeeded
                        } else {
                            TurnOutcome::Unknown
                        };
                        out.usage_source = if result_usage.is_some() {
                            UsageSource::ParentTerminal
                        } else {
                            UsageSource::Unknown
                        };
                        out.usage = result_usage.clone().unwrap_or_default();
                        out.result_subtype = Some(subtype);
                        out.is_error = is_error;
                        out.permission_denials_known = denials.is_some();
                        out.permission_denials = denials.unwrap_or_default();
                        out.total_cost_usd = cost;
                        terminal = true;
                    }
                    result_usage
                }
                _ => {
                    out.protocol_unknown = true;
                    supplied_usage(&value)?
                }
            };
        }
        if !out.event_types.contains(&kind) {
            out.event_types.push(kind.clone());
        }
        out.events.push(EventObservation {
            ordinal: out.events.len() + 1,
            event_digest: content_digest(&value).map_err(|_| ClaudeError::Json)?,
            event_type: kind,
            scope,
            usage,
        });
    }
    if !terminal {
        return Err(ClaudeError::Invalid(
            "complete parent terminal result required",
        ));
    }
    Ok(out)
}
fn optional(value: &Value, key: &str, maximum: usize) -> Result<Option<String>, ClaudeError> {
    match value.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if text(s, maximum) => Ok(Some(s.clone())),
        _ => Err(ClaudeError::Invalid("malformed bounded string field")),
    }
}
fn required(value: &Value, key: &str, maximum: usize) -> Result<String, ClaudeError> {
    optional(value, key, maximum)?.ok_or(ClaudeError::Invalid("required string field absent"))
}
fn supplied_usage(value: &Value) -> Result<Option<Usage>, ClaudeError> {
    value.get("usage").map(parse_usage).transpose()
}
fn parse_usage(value: &Value) -> Result<Usage, ClaudeError> {
    if !value.is_object() {
        return Err(ClaudeError::Invalid("usage must be an object"));
    }
    let counter = |object: &Value, key: &str| match object.get(key) {
        None => Ok(None),
        Some(number) => number
            .as_u64()
            .map(Some)
            .ok_or(ClaudeError::Invalid("malformed native usage counter")),
    };
    let thinking_tokens = match value.get("output_tokens_details") {
        None => None,
        Some(details) if details.is_object() => counter(details, "thinking_tokens")?,
        _ => return Err(ClaudeError::Invalid("malformed output usage details")),
    };
    Ok(Usage {
        input_tokens: counter(value, "input_tokens")?,
        cache_read_input_tokens: counter(value, "cache_read_input_tokens")?,
        cache_creation_input_tokens: counter(value, "cache_creation_input_tokens")?,
        output_tokens: counter(value, "output_tokens")?,
        thinking_tokens,
    })
}

/// Offline parser facts only. Configuration, resume, billing, launch admission
/// and native execution need evidence that this pure component does not hold.
pub fn qualification() -> Value {
    serde_json::json!({
        "provider":"claude-code", "fixture_cli_version":OBSERVED_CLI_VERSION, "adapter_version":ADAPTER_VERSION,
        "admission":"unadmitted_offline_proposal", "offline_structured_parser":true,
        "structured_stream":false, "launch_input_resume":false, "configuration_bound":false,
        "runtime_activation":false, "native_execution_qualified":false, "trusted_acceptance":false,
        "native_usage_categories":["input","cache_read","cache_write","output","thinking"],
        "parent_child_usage_deduplicated":false, "subagents":"opaque_native_not_managed_peers",
        "coordination_tools":"unqualified", "approval_and_permission_denials":"observations_only",
        "context_floor":"unknown", "version_canary":"offline fixture dialect only", "cost":"reported CLI cost; not settlement"
    })
}
