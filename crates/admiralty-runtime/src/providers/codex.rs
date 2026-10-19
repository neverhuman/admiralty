//! Bound, shell-free Codex exec launch data and conservative JSONL observations.
//! This module grants no effects, fallback, task acceptance or native qualification.
use super::{
    supervision::SupervisionScope,
    usage::{CumulativeUsage, UsageAccumulator, UsageDelta, UsageLimits},
};
use crate::{
    models::{ModelSpec, Provider},
    session::{
        CaptureState, LaunchSpec, MAX_LAUNCH_REQUEST_BYTES, OutputChunk, OutputStream,
        SessionStatus, validate_launch_envelope,
    },
};
use admiralty_core::{TaskContract, content_digest, limits::bounded_json_size};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

pub const CODEX_ADAPTER_VERSION: u32 = 1;
pub const SUPPORTED_CLI_VERSION: &str = "codex-cli 0.163.0";
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_HELP_BYTES: usize = 64 * 1024;
const MAX_EVENTS: usize = 8192;
const MAX_ITEMS: usize = 2048;

#[derive(Debug, Error)]
pub enum CodexError {
    #[error("invalid Codex adapter input: {0}")]
    Invalid(&'static str),
    #[error("Codex adapter input or output exceeds bounds")]
    Bounds,
    #[error("Codex JSONL protocol is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Codex model or effort differs from explicit selection")]
    ModelMismatch,
    #[error("recorded clock is unknown, expired or nonmonotonic")]
    Clock,
    #[error("output journal cursor has a gap, overlap or overflow")]
    JournalGap,
    #[error("parser failed; retain the journal and reconcile")]
    Failed,
    #[error("cumulative usage is invalid: {0}")]
    Usage(String),
}

fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn decimal(value: &str) -> bool {
    value.len() <= 20 && value.parse::<u64>().is_ok_and(|n| n.to_string() == value)
}
fn path(value: &Path) -> bool {
    value.is_absolute()
        && value.to_str().is_some_and(|s| {
            text(s, 2048)
                && !s.contains('\\')
                && s.strip_prefix('/').is_some_and(|tail| {
                    tail.split('/')
                        .all(|p| !p.is_empty() && p != "." && p != "..")
                })
        })
        && value
            .components()
            .any(|c| matches!(c, Component::Normal(_)))
        && value
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
fn digest<T: Serialize + ?Sized>(value: &T) -> Result<String, CodexError> {
    bounded_json_size(value, MAX_LAUNCH_REQUEST_BYTES).map_err(|_| CodexError::Bounds)?;
    content_digest(&value).map_err(|_| CodexError::Invalid("cannot digest launch input"))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexEffort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}
impl CodexEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexSandbox {
    ReadOnly,
    WorkspaceWrite,
}
impl CodexSandbox {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct CodexExecutionOptions {
    pub effort: CodexEffort,
    pub sandbox: CodexSandbox,
}

/// Read-only CLI observations supplied by the registered node's trusted collector.
/// Help text is dialect evidence, not an executable hash or an authentication grant.
#[derive(Clone, Debug, Serialize)]
pub struct CodexCliObservation {
    node_id: String,
    program: PathBuf,
    version: String,
    observed_tick: u64,
    help_digest: String,
    fingerprint: String,
}
impl CodexCliObservation {
    pub fn from_help(
        node_id: &str,
        program: PathBuf,
        version: &str,
        exec_help: &str,
        root_help: &str,
        observed_tick: Option<u64>,
    ) -> Result<Self, CodexError> {
        if !id(node_id)
            || !path(&program)
            || version.len() > 128
            || version.trim() != SUPPORTED_CLI_VERSION
        {
            return Err(CodexError::Invalid(
                "registered node, absolute program and supported CLI version required",
            ));
        }
        if exec_help.len() > MAX_HELP_BYTES || root_help.len() > MAX_HELP_BYTES {
            return Err(CodexError::Bounds);
        }
        for flag in [
            "--json",
            "--color",
            "--ignore-user-config",
            "--strict-config",
            "--ephemeral",
            "--model",
            "--sandbox",
            "--cd",
        ] {
            if !exec_help
                .split_ascii_whitespace()
                .any(|token| token == flag)
            {
                return Err(CodexError::Invalid(
                    "required exec flag absent from observed help",
                ));
            }
        }
        if ["--no-daemon", "--ask-for-approval"].iter().any(|flag| {
            !root_help
                .split_ascii_whitespace()
                .any(|token| token == *flag)
        }) {
            return Err(CodexError::Invalid(
                "required root flag absent from observed help",
            ));
        }
        let observed_tick = observed_tick.ok_or(CodexError::Clock)?;
        let help_digest =
            content_digest(&(exec_help, root_help)).map_err(|_| CodexError::Bounds)?;
        let fingerprint = digest(&(
            CODEX_ADAPTER_VERSION,
            node_id,
            &program,
            SUPPORTED_CLI_VERSION,
            observed_tick,
            &help_digest,
        ))?;
        Ok(Self {
            node_id: node_id.into(),
            program,
            version: SUPPORTED_CLI_VERSION.into(),
            observed_tick,
            help_digest,
            fingerprint,
        })
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Caller-admitted, exact bindings. These reference already checked policy/custody
/// decisions; constructing this value is not admission and grants no execution.
#[derive(Clone, Debug)]
pub struct CodexAdmission {
    pub scope: SupervisionScope,
    pub attempt_ordinal: u16,
    pub registry_revision: String,
    pub selected_model_digest: String,
    pub cli_observation_digest: String,
    pub node_id: String,
    pub working_dir: PathBuf,
    pub spool_root: PathBuf,
    pub lease_ref: String,
    pub resource_permit_ref: String,
    pub custody_ref: String,
    pub auth_context_ref: String,
    pub admitted_tick: u64,
    pub expires_tick: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodexLaunchTuple {
    pub adapter_version: u32,
    pub scope: SupervisionScope,
    pub attempt_ordinal: u16,
    pub registry_revision: String,
    pub selected_model_digest: String,
    pub registry_model_id: String,
    pub requested_model: String,
    pub requested_effort: CodexEffort,
    pub sandbox: CodexSandbox,
    pub node_id: String,
    pub working_dir: PathBuf,
    pub spool_dir: PathBuf,
    pub run_id: String,
    pub cli: CodexCliObservation,
    pub prompt_digest: String,
    pub argv_digest: String,
    pub lease_ref: String,
    pub resource_permit_ref: String,
    pub custody_ref: String,
    pub auth_context_ref: String,
    pub admitted_tick: String,
    pub expires_tick: String,
    native_execution_qualified: bool,
}
impl CodexLaunchTuple {
    pub fn native_execution_qualified(&self) -> bool {
        self.native_execution_qualified
    }
}

/// Data only. The session daemon must recheck owner/epoch/lease/permit/custody,
/// journal its intent, launch once, then explicitly close stdin using close_input.
#[derive(Clone, Debug)]
pub struct PreparedCodexLaunch {
    spec: LaunchSpec,
    tuple: CodexLaunchTuple,
    tuple_digest: String,
}
impl PreparedCodexLaunch {
    pub fn launch_spec(&self) -> &LaunchSpec {
        &self.spec
    }
    pub fn into_launch_spec(self) -> LaunchSpec {
        self.spec
    }
    pub fn tuple(&self) -> &CodexLaunchTuple {
        &self.tuple
    }
    pub fn tuple_digest(&self) -> &str {
        &self.tuple_digest
    }
    pub fn requires_stdin_eof(&self) -> bool {
        true
    }
}

pub fn prepare_launch(
    contract: &TaskContract,
    admission: CodexAdmission,
    model: &ModelSpec,
    options: CodexExecutionOptions,
    cli: &CodexCliObservation,
    task_capsule: &str,
    now: Option<u64>,
) -> Result<PreparedCodexLaunch, CodexError> {
    let CodexExecutionOptions { effort, sandbox } = options;
    bounded_json_size(contract, MAX_PROMPT_BYTES).map_err(|_| CodexError::Bounds)?;
    bounded_json_size(model, 16 * 1024).map_err(|_| CodexError::Bounds)?;
    contract
        .validate()
        .map_err(|_| CodexError::Invalid("invalid task contract"))?;
    let now = now.ok_or(CodexError::Clock)?;
    if admission.admitted_tick > now
        || now >= admission.expires_tick
        || cli.observed_tick > admission.admitted_tick
        || admission.admitted_tick - cli.observed_tick > 300
    {
        return Err(CodexError::Clock);
    }
    let scope = &admission.scope;
    if scope.project_id != contract.project_id
        || scope.root_id != contract.root_id
        || scope.task_id != contract.task_id
        || scope.revision != contract.revision
        || scope.authority_ref != contract.authority_ref
        || scope.contract_digest != digest(contract)?
        || !id(&scope.incarnation)
        || contract.simulated
    {
        return Err(CodexError::Invalid(
            "exact non-simulated contract/scope binding required",
        ));
    }
    if !decimal(&admission.registry_revision)
        || admission.attempt_ordinal == 0
        || admission.attempt_ordinal > contract.max_attempts
        || admission.attempt_ordinal > model.task_max_attempts
        || model.provider != Provider::CodexCli
        || !model.enabled
        || !id(&model.id)
        || !text(&model.label, 200)
        || !text(&model.provider_model, 256)
        || !(1..=1024).contains(&model.max_concurrency)
        || !(1..=20).contains(&model.task_max_attempts)
        || model.capabilities.is_empty()
        || model.capabilities.len() > 64
        || model.capabilities.iter().any(|capability| !id(capability))
        || model.endpoint.is_some()
        || model.credential_file_ref.is_some()
        || admission.selected_model_digest != digest(model)?
    {
        return Err(CodexError::Invalid(
            "explicit enabled Codex model, digest and attempt budget required; connection refs are not applied",
        ));
    }
    if !id(&admission.node_id)
        || admission.node_id != cli.node_id
        || model
            .node_id
            .as_ref()
            .is_some_and(|n| n != &admission.node_id)
        || admission.cli_observation_digest != cli.fingerprint
    {
        return Err(CodexError::Invalid(
            "CLI and selected model must bind the admitted node",
        ));
    }
    if !path(&admission.working_dir)
        || !path(&admission.spool_root)
        || admission.spool_root.starts_with(&admission.working_dir)
        || admission.working_dir.starts_with(&admission.spool_root)
    {
        return Err(CodexError::Invalid(
            "separate lexically normalized absolute worktree and spool root required",
        ));
    }
    for reference in [
        &admission.lease_ref,
        &admission.resource_permit_ref,
        &admission.custody_ref,
        &admission.auth_context_ref,
        &scope.authority_ref,
    ] {
        if !text(reference, 512) || reference.chars().any(char::is_whitespace) {
            return Err(CodexError::Invalid(
                "bounded immutable admission references required",
            ));
        }
    }
    if task_capsule.trim().is_empty()
        || task_capsule.len() > MAX_PROMPT_BYTES
        || task_capsule.contains('\0')
    {
        return Err(CodexError::Bounds);
    }
    #[derive(Serialize)]
    struct Prompt<'a> {
        schema_version: u32,
        task_contract: &'a TaskContract,
        task_capsule: &'a str,
    }
    let prompt = Prompt {
        schema_version: 1,
        task_contract: contract,
        task_capsule,
    };
    bounded_json_size(&prompt, MAX_PROMPT_BYTES).map_err(|_| CodexError::Bounds)?;
    let prompt = serde_json::to_string(&prompt)?;
    // Model/path changes cannot evade the same attempt/incarnation run reservation.
    let run_hash = digest(&(
        &scope.project_id,
        &scope.root_id,
        &scope.task_id,
        &scope.attempt_id,
        &scope.incarnation,
    ))?;
    let run_id = format!("cx-{}", run_hash.trim_start_matches("sha256:"));
    let spool_dir = admission.spool_root.join(&run_id);
    let cwd = admission
        .working_dir
        .to_str()
        .ok_or(CodexError::Invalid("UTF-8 cwd required"))?;
    let args: Vec<String> = [
        "--no-daemon",
        "--ask-for-approval",
        "never",
        "exec",
        "--json",
        "--color",
        "never",
        "--ignore-user-config",
        "--strict-config",
        "--ephemeral",
        "--model",
        &model.provider_model,
        "--sandbox",
        sandbox.as_str(),
        "--config",
        &format!("model_reasoning_effort=\"{}\"", effort.as_str()),
        "--cd",
        cwd,
        "--",
        &prompt,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let tuple = CodexLaunchTuple {
        adapter_version: CODEX_ADAPTER_VERSION,
        scope: admission.scope,
        attempt_ordinal: admission.attempt_ordinal,
        registry_revision: admission.registry_revision,
        selected_model_digest: admission.selected_model_digest,
        registry_model_id: model.id.clone(),
        requested_model: model.provider_model.clone(),
        requested_effort: effort,
        sandbox,
        node_id: admission.node_id,
        working_dir: admission.working_dir.clone(),
        spool_dir: spool_dir.clone(),
        run_id: run_id.clone(),
        cli: cli.clone(),
        prompt_digest: digest(&prompt)?,
        argv_digest: digest(&args)?,
        lease_ref: admission.lease_ref,
        resource_permit_ref: admission.resource_permit_ref,
        custody_ref: admission.custody_ref,
        auth_context_ref: admission.auth_context_ref,
        admitted_tick: admission.admitted_tick.to_string(),
        expires_tick: admission.expires_tick.to_string(),
        native_execution_qualified: false,
    };
    let tuple_digest = digest(&tuple)?;
    let spec = LaunchSpec {
        run_id,
        program: cli.program.clone(),
        args,
        cwd: admission.working_dir,
        spool_dir,
    };
    validate_launch_envelope(&spec).map_err(|_| {
        CodexError::Invalid("prepared launch exceeds shared session/daemon envelope")
    })?;
    Ok(PreparedCodexLaunch {
        spec,
        tuple,
        tuple_digest,
    })
}

/// Provider reports per turn; missing fields stay unknown and no aggregate is
/// fabricated. Cached input is a subset of input, not another input total.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CodexTurnUsage {
    pub input_tokens: Option<String>,
    pub cached_input_tokens: Option<String>,
    pub cache_write_input_tokens: Option<String>,
    pub output_tokens: Option<String>,
    pub reasoning_output_tokens: Option<String>,
    pub semantics: String,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CodexObservation {
    pub launch_tuple_digest: Option<String>,
    pub event_ordinal: String,
    pub journal_start: String,
    pub journal_end: String,
    pub event_digest: String,
    pub event_type: String,
    pub thread_id: Option<String>,
    pub item_id: Option<String>,
    pub item_type: Option<String>,
    pub tool_server: Option<String>,
    pub tool_name: Option<String>,
    pub tool_arguments_digest: Option<String>,
    pub tool_status: Option<String>,
    pub command_digest: Option<String>,
    pub tool_exit_code: Option<i32>,
    pub reported_model: Option<String>,
    pub reported_effort: Option<String>,
    pub usage: Option<CodexTurnUsage>,
    pub provider_error: bool,
    pub protocol_unknown: bool,
    // Sealed: neither agent text, tool exit nor provider termination can set it.
    trusted_acceptance: bool,
}
impl CodexObservation {
    pub fn trusted_acceptance(&self) -> bool {
        self.trusted_acceptance
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexTermination {
    ProviderTurnCompleted,
    ProviderFailed,
    ProcessFailed,
    Unknown,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CodexStreamSummary {
    pub launch_tuple_digest: Option<String>,
    pub termination: CodexTermination,
    pub process_exit_code: Option<i32>,
    pub process_exit_success: Option<bool>,
    pub producer_final_output_offset: Option<String>,
    pub capture_complete: bool,
    pub truncated_line: bool,
    pub parser_failed: bool,
    pub failure: Option<CodexParseFailure>,
    pub unknown_events: String,
    pub open_tools: Vec<String>,
    pub thread_id: Option<String>,
    pub requested_model: String,
    pub reported_model: Option<String>,
    pub requested_effort: CodexEffort,
    pub reported_effort: Option<String>,
    pub raw_bytes: String,
    pub stderr_bytes: String,
    pub events: String,
    trusted_acceptance: bool,
    native_execution_qualified: bool,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CodexParseFailure {
    pub reason: String,
    pub chunk_start: String,
    pub chunk_end: Option<String>,
}
impl CodexStreamSummary {
    pub fn trusted_acceptance(&self) -> bool {
        self.trusted_acceptance
    }
    pub fn native_execution_qualified(&self) -> bool {
        self.native_execution_qualified
    }
}

// serde_json::Value otherwise silently accepts duplicate object fields. Preserve
// no ambiguous counter/model/status records, including nested item/usage fields.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unambiguous bounded JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| StrictValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = seq.next_element::<StrictValue>()? {
                    if out.len() >= MAX_EVENTS {
                        return Err(de::Error::custom("JSON array bound"));
                    }
                    out.push(v.0);
                }
                Ok(StrictValue(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some(k) = map.next_key::<String>()? {
                    if out.len() >= MAX_EVENTS || out.contains_key(&k) {
                        return Err(de::Error::custom("duplicate field or JSON map bound"));
                    }
                    out.insert(k, map.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}
fn string_field(value: &Value, key: &str, max: usize) -> Result<Option<String>, CodexError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if text(s, max) => Ok(Some(s.clone())),
        _ => Err(CodexError::Invalid("malformed bounded string field")),
    }
}
fn required(value: &Value, key: &str) -> Result<String, CodexError> {
    string_field(value, key, 256)?.ok_or(CodexError::Invalid("required event string absent"))
}
fn counter(value: &Value, key: &str) -> Result<Option<String>, CodexError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|n| Some(n.to_string()))
            .ok_or(CodexError::Invalid("usage counter is not u64")),
        Some(Value::String(s)) if decimal(s) => Ok(Some(s.clone())),
        _ => Err(CodexError::Invalid("usage counter is not canonical u64")),
    }
}
fn data_field(value: &Value, key: &str) -> Result<Option<String>, CodexError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s))
            if !s.trim().is_empty() && s.len() <= MAX_EVENT_BYTES && !s.contains('\0') =>
        {
            Ok(Some(s.clone()))
        }
        _ => Err(CodexError::Invalid("malformed bounded tool data string")),
    }
}
fn required_data<'a>(value: &'a Value, key: &str) -> Result<&'a str, CodexError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| s.len() <= MAX_EVENT_BYTES && !s.contains('\0'))
        .ok_or(CodexError::Invalid(
            "required bounded provider data string absent",
        ))
}
fn tool_kind(kind: &str) -> bool {
    matches!(
        kind,
        "command_execution" | "mcp_tool_call" | "collab_tool_call" | "web_search" | "file_change"
    )
}
#[derive(Clone, Debug)]
struct ItemState {
    kind: String,
    complete: bool,
    identity: Option<String>,
}

/// Feed the combined, zero-based SessionHost journal in order, including stderr.
/// Only stdout is JSONL. Stderr bytes and repeated output are activity, not progress.
pub struct CodexNdjsonStream {
    launch_tuple_digest: Option<String>,
    requested_model: String,
    requested_effort: CodexEffort,
    reported_model: Option<String>,
    reported_effort: Option<String>,
    pending: Vec<u8>,
    line_start: Option<u64>,
    next_offset: u64,
    stderr_bytes: u64,
    events: usize,
    unknown_events: usize,
    failed: bool,
    thread_id: Option<String>,
    turn_active: bool,
    terminal: Option<bool>,
    items: BTreeMap<String, ItemState>,
    observations: Vec<CodexObservation>,
    failure: Option<CodexParseFailure>,
    cumulative: UsageAccumulator,
}
impl CodexNdjsonStream {
    pub fn new(requested_model: &str, requested_effort: CodexEffort) -> Result<Self, CodexError> {
        if !text(requested_model, 256) {
            return Err(CodexError::Invalid("explicit provider model required"));
        }
        Ok(Self {
            launch_tuple_digest: None,
            requested_model: requested_model.into(),
            requested_effort,
            reported_model: None,
            reported_effort: None,
            pending: Vec::new(),
            line_start: None,
            next_offset: 0,
            stderr_bytes: 0,
            events: 0,
            unknown_events: 0,
            failed: false,
            thread_id: None,
            turn_active: false,
            terminal: None,
            items: BTreeMap::new(),
            observations: Vec::new(),
            failure: None,
            cumulative: UsageAccumulator::new(UsageLimits::new(1, MAX_ITEMS, MAX_EVENTS))
                .map_err(|e| CodexError::Usage(e.to_string()))?,
        })
    }
    /// Bind observations to immutable prepared launch data. The caller must tail
    /// only that run's owned journal; OutputChunk itself contains no run identity.
    pub fn from_prepared(prepared: &PreparedCodexLaunch) -> Result<Self, CodexError> {
        let mut stream = Self::new(
            &prepared.tuple.requested_model,
            prepared.tuple.requested_effort,
        )?;
        stream.launch_tuple_digest = Some(prepared.tuple_digest.clone());
        Ok(stream)
    }
    pub fn push_chunk(&mut self, chunk: &OutputChunk) -> Result<Vec<CodexObservation>, CodexError> {
        if self.failed {
            return Err(CodexError::Failed);
        }
        let result = self.push_inner(chunk);
        if let Err(error) = &result {
            self.failed = true;
            self.failure = Some(CodexParseFailure {
                reason: error.to_string(),
                chunk_start: chunk.offset.to_string(),
                chunk_end: chunk
                    .offset
                    .checked_add(chunk.bytes.len() as u64)
                    .map(|n| n.to_string()),
            });
        }
        result
    }
    /// Valid prefix survives a later malformed line in the same chunk. Receipting
    /// by ordinal stays deterministic across fragmentation, including error paths.
    pub fn observations(&self) -> &[CodexObservation] {
        &self.observations
    }
    fn push_inner(&mut self, chunk: &OutputChunk) -> Result<Vec<CodexObservation>, CodexError> {
        if chunk.offset != self.next_offset {
            return Err(CodexError::JournalGap);
        }
        let end = self
            .next_offset
            .checked_add(u64::try_from(chunk.bytes.len()).map_err(|_| CodexError::Bounds)?)
            .ok_or(CodexError::JournalGap)?;
        if end > MAX_STREAM_BYTES as u64 {
            return Err(CodexError::Bounds);
        }
        self.next_offset = end;
        if chunk.stream == OutputStream::Stderr {
            self.stderr_bytes = self
                .stderr_bytes
                .checked_add(chunk.bytes.len() as u64)
                .ok_or(CodexError::Bounds)?;
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for (index, byte) in chunk.bytes.iter().enumerate() {
            self.line_start.get_or_insert(chunk.offset + index as u64);
            if *byte == b'\n' {
                let bytes = std::mem::take(&mut self.pending);
                let start = self.line_start.take().ok_or(CodexError::JournalGap)?;
                if !bytes.is_empty() {
                    out.push(self.parse_line(&bytes, start, chunk.offset + index as u64 + 1)?);
                }
            } else {
                if self.pending.len() >= MAX_EVENT_BYTES {
                    return Err(CodexError::Bounds);
                }
                self.pending.push(*byte);
            }
        }
        Ok(out)
    }
    fn parse_line(
        &mut self,
        bytes: &[u8],
        start: u64,
        end: u64,
    ) -> Result<CodexObservation, CodexError> {
        if self.events >= MAX_EVENTS {
            return Err(CodexError::Bounds);
        }
        let StrictValue(value) = serde_json::from_slice::<StrictValue>(bytes)?;
        if !value.is_object() {
            return Err(CodexError::Invalid("event must be an object"));
        }
        let event_type = required(&value, "type")?;
        let model = string_field(&value, "model", 256)?;
        let effort = string_field(&value, "reasoning_effort", 32)?;
        if model.as_ref().is_some_and(|m| m != &self.requested_model)
            || effort
                .as_ref()
                .is_some_and(|e| e != self.requested_effort.as_str())
        {
            return Err(CodexError::ModelMismatch);
        }
        let mut observation = CodexObservation {
            launch_tuple_digest: self.launch_tuple_digest.clone(),
            event_ordinal: (self.events + 1).to_string(),
            journal_start: start.to_string(),
            journal_end: end.to_string(),
            event_digest: content_digest(&value).map_err(|_| CodexError::Bounds)?,
            event_type: event_type.clone(),
            thread_id: self.thread_id.clone(),
            item_id: None,
            item_type: None,
            tool_server: None,
            tool_name: None,
            tool_arguments_digest: None,
            tool_status: None,
            command_digest: None,
            tool_exit_code: None,
            reported_model: model.clone(),
            reported_effort: effort.clone(),
            usage: None,
            provider_error: false,
            protocol_unknown: false,
            trusted_acceptance: false,
        };
        match event_type.as_str() {
            "thread.started" => {
                if self.thread_id.is_some() || self.terminal.is_some() {
                    return Err(CodexError::Invalid(
                        "duplicate thread or event after terminal",
                    ));
                }
                let thread = required(&value, "thread_id")?;
                self.thread_id = Some(thread.clone());
                observation.thread_id = Some(thread);
            }
            "turn.started" => {
                if self.thread_id.is_none() || self.turn_active || self.terminal.is_some() {
                    return Err(CodexError::Invalid("turn lifecycle is inconsistent"));
                }
                self.turn_active = true;
            }
            "item.started" | "item.updated" | "item.completed" => {
                if !self.turn_active || self.terminal.is_some() {
                    return Err(CodexError::Invalid("item outside active turn"));
                }
                let item = value
                    .get("item")
                    .filter(|v| v.is_object())
                    .ok_or(CodexError::Invalid("item object required"))?;
                let item_id = required(item, "id")?;
                let kind = required(item, "type")?;
                let completed = event_type == "item.completed";
                if let Some(previous) = self.items.get(&item_id) {
                    if previous.kind != kind || previous.complete || event_type == "item.started" {
                        return Err(CodexError::Invalid(
                            "item identity reused or lifecycle reversed",
                        ));
                    }
                } else if self.items.len() >= MAX_ITEMS {
                    return Err(CodexError::Bounds);
                }
                let tool = tool_kind(&kind);
                let mut identity = None;
                if tool {
                    if event_type != "item.started"
                        && !self.items.contains_key(&item_id)
                        && !(kind == "file_change" && completed)
                    {
                        observation.protocol_unknown = true;
                    }
                    let status = string_field(item, "status", 64)?;
                    if status.as_deref().is_some_and(|s| {
                        !(matches!(s, "in_progress" | "completed" | "failed")
                            || kind == "command_execution" && s == "declined")
                    }) {
                        observation.protocol_unknown = true;
                    }
                    if completed && status.as_deref() == Some("in_progress") {
                        return Err(CodexError::Invalid("completed tool reports in_progress"));
                    }
                    if status.is_none() && kind != "web_search" {
                        observation.protocol_unknown = true;
                    }
                    observation.tool_status = status.or_else(|| {
                        Some(
                            if completed {
                                "item_completed_status_unreported"
                            } else {
                                "item_active_status_unreported"
                            }
                            .into(),
                        )
                    });
                    observation.tool_name = match kind.as_str() {
                        "mcp_tool_call" | "collab_tool_call" => string_field(item, "tool", 256)?,
                        _ => Some(kind.clone()),
                    };
                    observation.tool_server = string_field(item, "server", 256)?;
                    match kind.as_str() {
                        "command_execution" => {
                            let command = required_data(item, "command")?;
                            required_data(item, "aggregated_output")?;
                            identity = Some(
                                content_digest(&(kind.as_str(), command))
                                    .map_err(|_| CodexError::Bounds)?,
                            );
                        }
                        "mcp_tool_call" => {
                            let server = required(item, "server")?;
                            let name = required(item, "tool")?;
                            for key in ["result", "error"] {
                                if item
                                    .get(key)
                                    .is_some_and(|v| !v.is_null() && !v.is_object())
                                {
                                    return Err(CodexError::Invalid(
                                        "MCP result/error must be object or unknown",
                                    ));
                                }
                            }
                            identity = Some(
                                content_digest(&(
                                    kind.as_str(),
                                    server,
                                    name,
                                    item.get("arguments"),
                                ))
                                .map_err(|_| CodexError::Bounds)?,
                            );
                        }
                        "collab_tool_call" => {
                            let name = required(item, "tool")?;
                            if !matches!(
                                name.as_str(),
                                "spawn_agent" | "send_input" | "wait" | "close_agent"
                            ) {
                                observation.protocol_unknown = true;
                            }
                            let sender = required(item, "sender_thread_id")?;
                            let receivers = item
                                .get("receiver_thread_ids")
                                .and_then(Value::as_array)
                                .filter(|v| v.len() <= MAX_ITEMS)
                                .ok_or(CodexError::Invalid(
                                    "bounded collab receiver IDs required",
                                ))?;
                            if receivers
                                .iter()
                                .any(|v| !v.as_str().is_some_and(|s| text(s, 256)))
                            {
                                return Err(CodexError::Invalid("invalid collab receiver ID"));
                            }
                            let states = item
                                .get("agents_states")
                                .and_then(Value::as_object)
                                .filter(|v| v.len() <= MAX_ITEMS)
                                .ok_or(CodexError::Invalid("bounded collab states required"))?;
                            for (agent, state) in states {
                                if !text(agent, 256) || !state.is_object() {
                                    return Err(CodexError::Invalid("invalid collab state"));
                                }
                                let status = required(state, "status")?;
                                if !matches!(
                                    status.as_str(),
                                    "pending_init"
                                        | "running"
                                        | "interrupted"
                                        | "completed"
                                        | "errored"
                                        | "shutdown"
                                        | "not_found"
                                ) {
                                    observation.protocol_unknown = true;
                                }
                                if state.get("message").is_some_and(|v| !v.is_null()) {
                                    required_data(state, "message")?;
                                }
                            }
                            if item.get("prompt").is_some_and(|v| !v.is_null()) {
                                required_data(item, "prompt")?;
                            }
                            identity = Some(
                                content_digest(&(
                                    kind.as_str(),
                                    name,
                                    sender,
                                    receivers,
                                    item.get("prompt"),
                                ))
                                .map_err(|_| CodexError::Bounds)?,
                            );
                        }
                        "web_search" => {
                            required_data(item, "query")?;
                            if !item.get("action").is_some_and(Value::is_object) {
                                return Err(CodexError::Invalid("web action object required"));
                            }
                        }
                        "file_change" => {
                            let changes = item
                                .get("changes")
                                .and_then(Value::as_array)
                                .filter(|v| v.len() <= MAX_ITEMS)
                                .ok_or(CodexError::Invalid("bounded file changes required"))?;
                            for change in changes {
                                required_data(change, "path")?;
                                let kind = required(change, "kind")?;
                                if !matches!(kind.as_str(), "add" | "delete" | "update") {
                                    observation.protocol_unknown = true;
                                }
                            }
                        }
                        _ => {}
                    }
                    if let Some(arguments) = item.get("arguments") {
                        observation.tool_arguments_digest =
                            Some(content_digest(arguments).map_err(|_| CodexError::Bounds)?);
                    }
                    if let Some(command) = data_field(item, "command")? {
                        observation.command_digest =
                            Some(content_digest(&command).map_err(|_| CodexError::Bounds)?);
                    }
                    if let Some(exit) = item.get("exit_code").filter(|v| !v.is_null()) {
                        observation.tool_exit_code = Some(
                            exit.as_i64()
                                .and_then(|n| i32::try_from(n).ok())
                                .ok_or(CodexError::Invalid("tool exit code is not i32"))?,
                        );
                    }
                } else if kind == "error" {
                    required_data(item, "message")?;
                    observation.provider_error = true;
                } else if matches!(kind.as_str(), "agent_message" | "reasoning") {
                    required_data(item, "text")?;
                } else if kind == "todo_list" {
                    let items = item
                        .get("items")
                        .and_then(Value::as_array)
                        .filter(|v| v.len() <= MAX_ITEMS)
                        .ok_or(CodexError::Invalid("bounded todo items required"))?;
                    for item in items {
                        required_data(item, "text")?;
                        if !item.get("completed").is_some_and(Value::is_boolean) {
                            return Err(CodexError::Invalid("todo completed boolean required"));
                        }
                    }
                } else {
                    observation.protocol_unknown = true;
                }
                if self.items.get(&item_id).is_some_and(|previous| {
                    previous.identity.is_some() && previous.identity != identity
                }) {
                    return Err(CodexError::Invalid(
                        "immutable tool identity changed under item ID",
                    ));
                }
                self.items.insert(
                    item_id.clone(),
                    ItemState {
                        kind: kind.clone(),
                        complete: completed,
                        identity,
                    },
                );
                observation.item_id = Some(item_id);
                observation.item_type = Some(kind);
            }
            "turn.completed" => {
                if !self.turn_active || self.terminal.is_some() {
                    return Err(CodexError::Invalid("turn completion outside active turn"));
                }
                if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
                    if !usage.is_object() {
                        return Err(CodexError::Invalid("usage must be object"));
                    }
                    let input = counter(usage, "input_tokens")?;
                    let cache = counter(usage, "cached_input_tokens")?;
                    if let (Some(i), Some(c)) = (&input, &cache)
                        && c.parse::<u64>().map_err(|_| CodexError::Bounds)?
                            > i.parse::<u64>().map_err(|_| CodexError::Bounds)?
                    {
                        return Err(CodexError::Invalid("cached input exceeds input"));
                    }
                    observation.usage = Some(CodexTurnUsage {
                        input_tokens: input,
                        cached_input_tokens: cache,
                        cache_write_input_tokens: counter(usage, "cache_write_input_tokens")?,
                        output_tokens: counter(usage, "output_tokens")?,
                        reasoning_output_tokens: counter(usage, "reasoning_output_tokens")?,
                        semantics:
                            "provider_reported_per_turn; not_cumulative; cached_input_subset".into(),
                    });
                } else {
                    observation.protocol_unknown = true;
                }
                self.turn_active = false;
                self.terminal = Some(true);
            }
            "turn.failed" => {
                if !self.turn_active || self.terminal.is_some() {
                    return Err(CodexError::Invalid("turn failure outside active turn"));
                }
                let error = value
                    .get("error")
                    .filter(|v| v.is_object())
                    .ok_or(CodexError::Invalid("turn error object required"))?;
                required_data(error, "message")?;
                self.turn_active = false;
                self.terminal = Some(false);
                observation.provider_error = true;
            }
            "error" => {
                required_data(&value, "message")?;
                observation.provider_error = true;
                self.terminal = Some(false);
                self.turn_active = false;
            }
            _ => {
                observation.protocol_unknown = true;
            }
        }
        self.events += 1;
        if observation.protocol_unknown {
            self.unknown_events += 1;
        }
        if model.is_some() {
            self.reported_model = model;
        }
        if effort.is_some() {
            self.reported_effort = effort;
        }
        self.observations.push(observation.clone());
        Ok(observation)
    }
    /// Only for an independently identified cumulative source. Exec turn usage is
    /// never fed here automatically. Reset/left censoring/dedup live in usage.rs.
    pub fn observe_cumulative(
        &mut self,
        sample: CumulativeUsage,
    ) -> Result<UsageDelta, CodexError> {
        if self.failed {
            return Err(CodexError::Failed);
        }
        if sample.model != self.requested_model
            || self.thread_id.as_ref() != Some(&sample.session_id)
        {
            return Err(CodexError::ModelMismatch);
        }
        self.cumulative
            .observe(sample)
            .map_err(|e| CodexError::Usage(e.to_string()))
    }
    /// EOF is explicit; a final unterminated record is truncated, never parsed as
    /// success. Process status and complete capture are supplied by SessionHost.
    pub fn finish_from_status(self, status: &SessionStatus) -> CodexStreamSummary {
        let state = if status.capture_failed
            || status
                .final_output_offset
                .is_some_and(|n| n != status.output_offset)
        {
            CaptureState::Failed
        } else {
            status.capture_state
        };
        let mut summary = self.finish_at_watermark(
            status.exit.and_then(|exit| exit.0),
            state,
            status.final_output_offset,
        );
        summary.process_exit_success = status.exit.map(|exit| exit.1);
        summary.producer_final_output_offset = status.final_output_offset.map(|n| n.to_string());
        if status.exit.is_some_and(|exit| !exit.1) {
            summary.termination = CodexTermination::ProcessFailed;
        }
        summary
    }
    /// Daemon clients validate canonical decimal offsets before passing recorded
    /// status evidence. Complete means both producer EOF and consumer drain here.
    pub fn finish_at_watermark(
        self,
        process_exit_code: Option<i32>,
        capture_state: CaptureState,
        final_output_offset: Option<u64>,
    ) -> CodexStreamSummary {
        let complete = capture_state == CaptureState::Complete
            && final_output_offset == Some(self.next_offset);
        let mut summary = self.finish(process_exit_code, complete);
        summary.producer_final_output_offset = final_output_offset.map(|n| n.to_string());
        summary
    }
    /// Offline/direct-host assertion API. Live daemon consumers should use
    /// finish_from_status, which also checks the fixed producer EOF watermark.
    pub fn finish(
        self,
        process_exit_code: Option<i32>,
        capture_complete: bool,
    ) -> CodexStreamSummary {
        let truncated_line = !self.pending.is_empty();
        let open_tools: Vec<_> = self
            .items
            .iter()
            .filter(|(_, item)| !item.complete && tool_kind(&item.kind))
            .map(|(id, _)| id.clone())
            .collect();
        let termination = if process_exit_code.is_some_and(|n| n != 0) {
            CodexTermination::ProcessFailed
        } else if self.failed
            || truncated_line
            || !capture_complete
            || process_exit_code.is_none()
            || self.unknown_events > 0
            || !open_tools.is_empty()
        {
            CodexTermination::Unknown
        } else {
            match self.terminal {
                Some(true) => CodexTermination::ProviderTurnCompleted,
                Some(false) => CodexTermination::ProviderFailed,
                None => CodexTermination::Unknown,
            }
        };
        CodexStreamSummary {
            launch_tuple_digest: self.launch_tuple_digest.clone(),
            termination,
            process_exit_code,
            process_exit_success: process_exit_code.map(|code| code == 0),
            producer_final_output_offset: None,
            capture_complete,
            truncated_line,
            parser_failed: self.failed,
            failure: self.failure.clone(),
            unknown_events: self.unknown_events.to_string(),
            open_tools,
            thread_id: self.thread_id.clone(),
            requested_model: self.requested_model.clone(),
            reported_model: self.reported_model.clone(),
            requested_effort: self.requested_effort,
            reported_effort: self.reported_effort.clone(),
            raw_bytes: self.next_offset.to_string(),
            stderr_bytes: self.stderr_bytes.to_string(),
            events: self.events.to_string(),
            trusted_acceptance: false,
            native_execution_qualified: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::usage::CounterKind;
    use super::*;
    use crate::session::SessionHost;
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskId,
    };
    use std::{collections::BTreeSet, fs};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn contract() -> TestResult<TaskContract> {
        Ok(TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("project")?,
            root_id: RootId::new("root")?,
            task_id: TaskId::new("task")?,
            revision: 2,
            title: "Scoped fixture".into(),
            objective: "Implement the scoped fixture".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec!["secrets".into()],
            read_first: vec!["README.md".into()],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "C1".into(),
                text: "exact check passes".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "fixture-check".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 1,
            }],
            authority_ref: "authority:fixture".into(),
            max_attempts: 2,
            simulated: false,
        })
    }
    fn model() -> ModelSpec {
        ModelSpec {
            id: "registry-codex".into(),
            label: "Selected Codex".into(),
            provider: Provider::CodexCli,
            provider_model: "native-model".into(),
            tier: 0,
            enabled: true,
            node_id: Some("node".into()),
            endpoint: None,
            credential_file_ref: None,
            max_concurrency: 1,
            capabilities: BTreeSet::from(["code".into()]),
            task_max_attempts: 2,
            escalate_to: None,
        }
    }
    fn cli(program: PathBuf) -> TestResult<CodexCliObservation> {
        Ok(CodexCliObservation::from_help(
            "node",
            program,
            SUPPORTED_CLI_VERSION,
            "--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd",
            "--no-daemon --ask-for-approval",
            Some(10),
        )?)
    }
    fn admission(
        contract: &TaskContract,
        model: &ModelSpec,
        cli: &CodexCliObservation,
        base: &Path,
    ) -> TestResult<CodexAdmission> {
        Ok(CodexAdmission {
            scope: SupervisionScope {
                project_id: contract.project_id.clone(),
                root_id: contract.root_id.clone(),
                task_id: contract.task_id.clone(),
                attempt_id: AttemptId::new("attempt")?,
                owner: PrincipalId::new("captain")?,
                incarnation: "incarnation".into(),
                authority_ref: contract.authority_ref.clone(),
                contract_digest: digest(contract)?,
                revision: contract.revision,
            },
            attempt_ordinal: 1,
            registry_revision: "17".into(),
            selected_model_digest: digest(model)?,
            cli_observation_digest: cli.fingerprint().into(),
            node_id: "node".into(),
            working_dir: base.join("work"),
            spool_root: base.join("spool"),
            lease_ref: "lease:one".into(),
            resource_permit_ref: "permit:one".into(),
            custody_ref: "custody:one".into(),
            auth_context_ref: "auth:node".into(),
            admitted_tick: 10,
            expires_tick: 20,
        })
    }
    fn prepare(
        contract: &TaskContract,
        admission: CodexAdmission,
        model: &ModelSpec,
        cli: &CodexCliObservation,
        capsule: &str,
    ) -> Result<PreparedCodexLaunch, CodexError> {
        prepare_launch(
            contract,
            admission,
            model,
            CodexExecutionOptions {
                effort: CodexEffort::Medium,
                sandbox: CodexSandbox::WorkspaceWrite,
            },
            cli,
            capsule,
            Some(10),
        )
    }
    fn chunk(
        parser: &mut CodexNdjsonStream,
        bytes: &[u8],
    ) -> Result<Vec<CodexObservation>, CodexError> {
        parser.push_chunk(&OutputChunk {
            offset: parser.next_offset,
            stream: OutputStream::Stdout,
            bytes: bytes.to_vec(),
        })
    }
    fn event(
        parser: &mut CodexNdjsonStream,
        value: Value,
    ) -> Result<Vec<CodexObservation>, CodexError> {
        chunk(parser, format!("{value}\n").as_bytes())
    }
    const START: &[u8] =
        b"{\"type\":\"thread.started\",\"thread_id\":\"thread\"}\n{\"type\":\"turn.started\"}\n";
    const COMPLETE:&[u8]=b"{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":100,\"cached_input_tokens\":90,\"output_tokens\":3}}\n";

    #[test]
    fn exact_argv_and_scope_are_bound_without_shell_interpretation() -> TestResult {
        let contract = contract()?;
        let model = model();
        let cli = cli("/usr/local/bin/codex".into())?;
        let admission = admission(&contract, &model, &cli, Path::new("/fixture"))?;
        let capsule = "literal $(touch /tmp/forbidden); `id`; --model other\nPASS";
        let prepared = prepare(&contract, admission.clone(), &model, &cli, capsule)?;
        let args = &prepared.launch_spec().args;
        assert_eq!(
            &args[..19],
            &[
                "--no-daemon",
                "--ask-for-approval",
                "never",
                "exec",
                "--json",
                "--color",
                "never",
                "--ignore-user-config",
                "--strict-config",
                "--ephemeral",
                "--model",
                "native-model",
                "--sandbox",
                "workspace-write",
                "--config",
                "model_reasoning_effort=\"medium\"",
                "--cd",
                "/fixture/work",
                "--"
            ]
        );
        assert_eq!(args.len(), 20);
        let prompt: Value = serde_json::from_str(&args[19])?;
        assert_eq!(prompt["task_capsule"], capsule);
        assert_eq!(prompt["task_contract"], serde_json::to_value(&contract)?);
        assert_eq!(prepared.tuple().scope, admission.scope);
        assert_eq!(prepared.tuple().selected_model_digest, digest(&model)?);
        assert_eq!(prepared.tuple().argv_digest, digest(args)?);
        assert!(!prepared.tuple().native_execution_qualified());
        assert!(prepared.requires_stdin_eof());
        let mut changed_model = model.clone();
        changed_model.provider_model = "other-native-model".into();
        let mut changed = admission.clone();
        changed.selected_model_digest = digest(&changed_model)?;
        assert_eq!(
            prepared.launch_spec().run_id,
            prepare(&contract, changed, &changed_model, &cli, capsule)?
                .launch_spec()
                .run_id
        );
        let mut wrong = admission.clone();
        wrong.scope.revision += 1;
        assert!(prepare(&contract, wrong, &model, &cli, capsule).is_err());
        let mut wrong = admission.clone();
        wrong.scope.project_id = ProjectId::new("foreign")?;
        assert!(prepare(&contract, wrong, &model, &cli, capsule).is_err());
        let mut wrong = contract.clone();
        wrong.objective.push_str(" changed");
        assert!(prepare(&wrong, admission, &model, &cli, capsule).is_err());
        Ok(())
    }
    #[test]
    fn unadmitted_nodes_models_clocks_paths_and_help_are_rejected() -> TestResult {
        let contract = contract()?;
        let model = model();
        let cli = cli("/usr/local/bin/codex".into())?;
        let initial = admission(&contract, &model, &cli, Path::new("/fixture"))?;
        let mut wrong = initial.clone();
        wrong.node_id = "foreign".into();
        assert!(prepare(&contract, wrong, &model, &cli, "task").is_err());
        let mut wrong = initial.clone();
        wrong.attempt_ordinal = 3;
        assert!(prepare(&contract, wrong, &model, &cli, "task").is_err());
        let mut wrong = initial.clone();
        wrong.selected_model_digest = "sha256:forged".into();
        assert!(prepare(&contract, wrong, &model, &cli, "task").is_err());
        let mut wrong = initial.clone();
        wrong.cli_observation_digest = "sha256:forged".into();
        assert!(prepare(&contract, wrong, &model, &cli, "task").is_err());
        let mut wrong = initial.clone();
        wrong.spool_root = wrong.working_dir.join("spool");
        assert!(prepare(&contract, wrong, &model, &cli, "task").is_err());
        for bad in [
            "/fixture/./work",
            "/fixture/../work",
            "/fixture//work",
            "relative",
            "/",
        ] {
            let mut wrong = initial.clone();
            wrong.working_dir = bad.into();
            assert!(
                prepare(&contract, wrong, &model, &cli, "task").is_err(),
                "{bad}"
            );
        }
        for now in [None, Some(9), Some(20)] {
            assert!(
                prepare_launch(
                    &contract,
                    initial.clone(),
                    &model,
                    CodexExecutionOptions {
                        effort: CodexEffort::Medium,
                        sandbox: CodexSandbox::ReadOnly
                    },
                    &cli,
                    "task",
                    now
                )
                .is_err()
            );
        }
        let mut wrong = initial.clone();
        wrong.expires_tick = 10;
        assert!(prepare(&contract, wrong, &model, &cli, "task").is_err());
        assert!(
            CodexCliObservation::from_help(
                "node",
                "/bin/codex".into(),
                "codex-cli 9.9.9",
                "--json",
                "",
                Some(0)
            )
            .is_err()
        );
        assert!(
            CodexCliObservation::from_help(
                "node",
                "/bin/codex".into(),
                SUPPORTED_CLI_VERSION,
                "--json",
                "",
                Some(0)
            )
            .is_err()
        );
        let mut connected = model.clone();
        connected.credential_file_ref = Some("credentials/codex".into());
        let mut admitted = initial;
        admitted.selected_model_digest = digest(&connected)?;
        assert!(prepare(&contract, admitted, &connected, &cli, "task").is_err());
        Ok(())
    }
    #[test]
    fn fragmented_jsonl_and_stderr_have_exact_journal_provenance() -> TestResult {
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        let mut observations = Vec::new();
        for part in START.chunks(7) {
            observations.extend(chunk(&mut parser, part)?);
        }
        parser.push_chunk(&OutputChunk {
            offset: parser.next_offset,
            stream: OutputStream::Stderr,
            bytes: b"diagnostic: PASS, not a provider event\n".to_vec(),
        })?;
        for part in COMPLETE.chunks(3) {
            observations.extend(chunk(&mut parser, part)?);
        }
        assert_eq!(observations.len(), 3);
        let usage = observations[2].usage.as_ref().ok_or("usage absent")?;
        assert_eq!(usage.cached_input_tokens.as_deref(), Some("90"));
        assert_eq!(usage.reasoning_output_tokens, None);
        assert!(usage.semantics.contains("not_cumulative"));
        let summary = parser.finish(Some(0), true);
        assert_eq!(summary.termination, CodexTermination::ProviderTurnCompleted);
        assert_eq!(summary.reported_model, None); // selected model is not observed model.
        assert_ne!(summary.stderr_bytes, "0");
        assert!(!summary.trusted_acceptance());
        assert!(!summary.native_execution_qualified());
        assert!(observations.iter().all(|o| !o.trusted_acceptance()));
        Ok(())
    }
    #[test]
    fn quiet_tools_and_noisy_output_do_not_invent_verified_progress() -> TestResult {
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        let start=chunk(&mut parser,b"{\"type\":\"item.started\",\"item\":{\"id\":\"tool\",\"type\":\"command_execution\",\"command\":\"test\",\"aggregated_output\":\"\",\"status\":\"in_progress\"}}\n")?;
        assert_eq!(start[0].tool_status.as_deref(), Some("in_progress"));
        assert!(start[0].command_digest.is_some());
        let update=b"{\"type\":\"item.updated\",\"item\":{\"id\":\"tool\",\"type\":\"command_execution\",\"command\":\"test\",\"status\":\"in_progress\",\"aggregated_output\":\"still running PASS PASS\"}}\n";
        for _ in 0..10 {
            let noisy = chunk(&mut parser, update)?;
            assert_eq!(noisy[0].command_digest, start[0].command_digest);
            assert!(!noisy[0].trusted_acceptance());
        }
        chunk(&mut parser,b"{\"type\":\"item.completed\",\"item\":{\"id\":\"message\",\"type\":\"agent_message\",\"text\":\"PASS; task verified\"}}\n")?;
        chunk(&mut parser, COMPLETE)?;
        let summary = parser.finish(Some(0), true);
        assert_eq!(summary.open_tools, vec!["tool"]);
        assert_eq!(summary.termination, CodexTermination::Unknown);
        assert!(!summary.trusted_acceptance());
        Ok(())
    }
    #[test]
    fn completed_tool_retains_exit_but_does_not_accept_task() -> TestResult {
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        chunk(&mut parser,b"{\"type\":\"item.started\",\"item\":{\"id\":\"tool\",\"type\":\"command_execution\",\"command\":\"test\",\"aggregated_output\":\"\",\"status\":\"in_progress\"}}\n")?;
        let obs=chunk(&mut parser,b"{\"type\":\"item.completed\",\"item\":{\"id\":\"tool\",\"type\":\"command_execution\",\"command\":\"test\",\"aggregated_output\":\"PASS\",\"status\":\"completed\",\"exit_code\":0}}\n")?;
        assert_eq!(obs[0].tool_exit_code, Some(0));
        assert!(!obs[0].trusted_acceptance());
        chunk(&mut parser, COMPLETE)?;
        assert_eq!(
            parser.finish(Some(0), true).termination,
            CodexTermination::ProviderTurnCompleted
        );
        Ok(())
    }
    #[test]
    fn malformed_duplicate_model_mismatch_gaps_and_unknown_events_fail_closed() -> TestResult {
        for bad in [
            b"not json\n".as_slice(),
            b"{\"type\":\"thread.started\",\"type\":\"error\"}\n",
            b"{\"type\":\"thread.started\",\"thread_id\":\"t\",\"model\":\"foreign\"}\n",
            b"{\"type\":\"thread.started\",\"thread_id\":\"t\",\"reasoning_effort\":\"high\"}\n",
        ] {
            let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
            assert!(chunk(&mut parser, bad).is_err());
            assert!(chunk(&mut parser, START).is_err());
            assert!(parser.finish(Some(0), true).parser_failed);
        }
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        assert!(
            parser
                .push_chunk(&OutputChunk {
                    offset: 1,
                    stream: OutputStream::Stdout,
                    bytes: START.to_vec()
                })
                .is_err()
        );
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        let obs = chunk(
            &mut parser,
            b"{\"type\":\"new.protocol\",\"data\":\"PASS\"}\n",
        )?;
        assert!(obs[0].protocol_unknown);
        chunk(&mut parser, COMPLETE)?;
        assert_eq!(
            parser.finish(Some(0), true).termination,
            CodexTermination::Unknown
        );
        Ok(())
    }
    #[test]
    fn eof_capture_unknown_and_process_failure_override_provider_completion() -> TestResult {
        for (exit, captured, expected) in [
            (Some(0), false, CodexTermination::Unknown),
            (None, true, CodexTermination::Unknown),
            (Some(7), true, CodexTermination::ProcessFailed),
        ] {
            let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
            chunk(&mut parser, START)?;
            chunk(&mut parser, COMPLETE)?;
            assert_eq!(parser.finish(exit, captured).termination, expected);
        }
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        chunk(&mut parser, &COMPLETE[..COMPLETE.len() - 1])?;
        let summary = parser.finish(Some(0), true);
        assert!(summary.truncated_line);
        assert_eq!(summary.termination, CodexTermination::Unknown);
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        assert_eq!(
            parser.finish(Some(0), true).termination,
            CodexTermination::Unknown
        );
        Ok(())
    }
    #[test]
    fn invalid_counters_and_nested_duplicate_fields_are_not_zeroed() -> TestResult {
        for usage in [
            "{\"input_tokens\":10,\"cached_input_tokens\":11}",
            "{\"output_tokens\":-1}",
            "{\"output_tokens\":\"01\"}",
            "{\"output_tokens\":1.5}",
            "{\"output_tokens\":1,\"output_tokens\":2}",
        ] {
            let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
            chunk(&mut parser, START)?;
            assert!(
                chunk(
                    &mut parser,
                    format!("{{\"type\":\"turn.completed\",\"usage\":{usage}}}\n").as_bytes()
                )
                .is_err()
            );
        }
        Ok(())
    }
    #[test]
    fn cumulative_usage_requires_explicit_segments_preserves_gaps_and_resets() -> TestResult {
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        let sample = |segment: &str, event: &str, value, reset| CumulativeUsage {
            session_id: "thread".into(),
            segment_id: segment.into(),
            source_event_id: event.into(),
            model: "native-model".into(),
            counter_kind: CounterKind::Input,
            cumulative_value: value,
            reset_baseline: reset,
        };
        assert_eq!(
            parser
                .observe_cumulative(sample("one", "missing", None, None))?
                .delta,
            None
        );
        assert_eq!(
            parser
                .observe_cumulative(sample("one", "opening", Some(10), None))?
                .delta,
            None
        );
        let next = parser.observe_cumulative(sample("one", "next", Some(12), None))?;
        assert_eq!(next.delta, Some(2));
        assert_eq!(next.segment_total, None);
        assert!(
            parser
                .observe_cumulative(sample("one", "next", Some(12), None))?
                .duplicate
        );
        assert!(
            parser
                .observe_cumulative(sample("one", "decreased", Some(1), None))
                .is_err()
        );
        assert!(
            parser
                .observe_cumulative(sample("one", "hidden-reset", Some(12), Some(0)))
                .is_err()
        );
        let reset = parser.observe_cumulative(sample("two", "explicit-reset", Some(3), Some(0)))?;
        assert_eq!(reset.delta, Some(3));
        assert_eq!(reset.segment_total, Some(3));
        let mut foreign = sample("two", "foreign", Some(4), None);
        foreign.session_id = "foreign".into();
        assert!(parser.observe_cumulative(foreign).is_err());
        Ok(())
    }
    #[test]
    fn parser_and_prompt_limits_are_enforced_incrementally() -> TestResult {
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        assert!(chunk(&mut parser, &vec![b'x'; MAX_EVENT_BYTES + 1]).is_err());
        let contract = contract()?;
        let model = model();
        let cli = cli("/usr/local/bin/codex".into())?;
        let admission = admission(&contract, &model, &cli, Path::new("/fixture"))?;
        assert!(
            prepare(
                &contract,
                admission,
                &model,
                &cli,
                &"x".repeat(MAX_PROMPT_BYTES + 1)
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn pinned_file_web_mcp_and_collab_shapes_keep_report_provenance() -> TestResult {
        use serde_json::json;
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        let file = event(
            &mut parser,
            json!({"type":"item.completed","item":{"id":"file","type":"file_change","changes":[{"path":"src/x.rs","kind":"update"}],"status":"completed"}}),
        )?;
        assert!(!file[0].protocol_unknown); // no fabricated missing-start incident.
        let web = json!({"id":"web","type":"web_search","query":"source","action":{"type":"search","query":"source"}});
        event(&mut parser, json!({"type":"item.started","item":web}))?;
        let web = event(&mut parser, json!({"type":"item.completed","item":web}))?;
        assert!(!web[0].protocol_unknown);
        assert_eq!(
            web[0].tool_status.as_deref(),
            Some("item_completed_status_unreported")
        );
        let mut mcp = json!({"id":"mcp","type":"mcp_tool_call","server":"docs","tool":"search","arguments":{"query":"source"},"result":null,"error":null,"status":"in_progress"});
        event(&mut parser, json!({"type":"item.started","item":mcp}))?;
        mcp["status"] = json!("completed");
        mcp["result"] = json!({"content":[],"structured_content":null});
        let mcp = event(&mut parser, json!({"type":"item.completed","item":mcp}))?;
        assert_eq!(mcp[0].tool_server.as_deref(), Some("docs"));
        assert_eq!(mcp[0].tool_name.as_deref(), Some("search"));
        assert!(mcp[0].tool_arguments_digest.is_some());
        let mut collab = json!({"id":"collab","type":"collab_tool_call","tool":"wait","sender_thread_id":"thread","receiver_thread_ids":["native-child"],"prompt":null,"agents_states":{"native-child":{"status":"running","message":null}},"status":"in_progress"});
        event(&mut parser, json!({"type":"item.started","item":collab}))?;
        collab["status"] = json!("completed");
        collab["agents_states"]["native-child"]["status"] = json!("completed");
        let collab = event(&mut parser, json!({"type":"item.completed","item":collab}))?;
        assert_eq!(collab[0].tool_name.as_deref(), Some("wait"));
        assert!(!collab[0].trusted_acceptance());
        let usage = event(
            &mut parser,
            json!({"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"cache_write_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}),
        )?;
        assert_eq!(
            usage[0]
                .usage
                .as_ref()
                .and_then(|u| u.cache_write_input_tokens.as_deref()),
            Some("2")
        );
        assert_eq!(
            parser.finish(Some(0), true).termination,
            CodexTermination::ProviderTurnCompleted
        );
        Ok(())
    }
    #[test]
    fn tool_identity_cannot_change_under_one_item_id() -> TestResult {
        use serde_json::json;
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        event(
            &mut parser,
            json!({"type":"item.started","item":{"id":"tool","type":"command_execution","command":"echo one\necho two","aggregated_output":"","status":"in_progress"}}),
        )?;
        assert!(event(&mut parser,json!({"type":"item.updated","item":{"id":"tool","type":"command_execution","command":"unrelated command","aggregated_output":"new output","status":"in_progress"}})).is_err());
        assert!(parser.finish(Some(0), true).parser_failed);
        Ok(())
    }
    #[test]
    fn malformed_known_payloads_are_not_recognized_as_valid_events() -> TestResult {
        use serde_json::json;
        for item in [
            json!({"id":"a","type":"agent_message"}),
            json!({"id":"a","type":"command_execution","status":"completed"}),
            json!({"id":"a","type":"file_change","status":"completed"}),
            json!({"id":"a","type":"mcp_tool_call","status":"completed"}),
            json!({"id":"a","type":"collab_tool_call","tool":"wait","status":"completed"}),
            json!({"id":"a","type":"todo_list","items":[{"text":"task","completed":"yes"}]}),
        ] {
            let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
            chunk(&mut parser, START)?;
            assert!(event(&mut parser, json!({"type":"item.completed","item":item})).is_err());
        }
        for failure in [
            json!({"type":"error"}),
            json!({"type":"turn.failed","error":{}}),
        ] {
            let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
            chunk(&mut parser, START)?;
            assert!(event(&mut parser, failure).is_err());
        }
        Ok(())
    }
    #[test]
    fn malformed_chunk_keeps_valid_prefix_for_deterministic_replay() -> TestResult {
        let mut bytes = START.to_vec();
        bytes.extend_from_slice(b"not-json\nignored-tail");
        let mut whole = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        assert!(chunk(&mut whole, &bytes).is_err());
        let mut fragmented = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        for part in bytes.chunks(1) {
            if chunk(&mut fragmented, part).is_err() {
                break;
            }
        }
        assert_eq!(whole.observations(), fragmented.observations());
        assert_eq!(whole.observations().len(), 2);
        assert!(whole.finish(Some(0), true).failure.is_some());
        Ok(())
    }
    #[test]
    fn matching_metadata_is_reported_and_clock_zero_is_valid() -> TestResult {
        use serde_json::json;
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        event(
            &mut parser,
            json!({"type":"thread.started","thread_id":"thread","model":"native-model","reasoning_effort":"medium"}),
        )?;
        event(&mut parser, json!({"type":"turn.started"}))?;
        chunk(&mut parser, COMPLETE)?;
        let summary = parser.finish(Some(0), true);
        assert_eq!(summary.reported_model.as_deref(), Some("native-model"));
        assert_eq!(summary.reported_effort.as_deref(), Some("medium"));
        assert!(!summary.native_execution_qualified());
        let contract = contract()?;
        let model = model();
        let mut cli = cli("/usr/local/bin/codex".into())?;
        // Trusted constructor still needs the same observation fingerprint after a new tick.
        cli = CodexCliObservation::from_help(
            "node",
            cli.program.clone(),
            SUPPORTED_CLI_VERSION,
            "--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd",
            "--no-daemon --ask-for-approval",
            Some(0),
        )?;
        let mut admitted = admission(&contract, &model, &cli, Path::new("/fixture"))?;
        admitted.admitted_tick = 0;
        admitted.expires_tick = 1;
        assert!(
            prepare_launch(
                &contract,
                admitted.clone(),
                &model,
                CodexExecutionOptions {
                    effort: CodexEffort::Medium,
                    sandbox: CodexSandbox::ReadOnly
                },
                &cli,
                "task",
                Some(0)
            )
            .is_ok()
        );
        admitted.admitted_tick = 301;
        admitted.expires_tick = 310;
        assert!(
            prepare_launch(
                &contract,
                admitted,
                &model,
                CodexExecutionOptions {
                    effort: CodexEffort::Medium,
                    sandbox: CodexSandbox::ReadOnly
                },
                &cli,
                "task",
                Some(301)
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn live_finalization_requires_final_producer_eof_and_consumed_watermark() -> TestResult {
        let final_offset = (START.len() + COMPLETE.len()) as u64;
        for state in [CaptureState::Capturing, CaptureState::Failed] {
            let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
            chunk(&mut parser, START)?;
            chunk(&mut parser, COMPLETE)?;
            let summary = parser.finish_at_watermark(Some(0), state, Some(final_offset));
            assert!(!summary.capture_complete);
            assert_eq!(summary.termination, CodexTermination::Unknown);
        }
        let mut unread = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut unread, START)?;
        let summary =
            unread.finish_at_watermark(Some(0), CaptureState::Complete, Some(final_offset));
        assert!(!summary.capture_complete);
        assert_eq!(summary.termination, CodexTermination::Unknown);
        let status = SessionStatus {
            pid: 1,
            start_identity: None,
            exit: Some((Some(0), true)),
            input_epoch: 1,
            input_outcome_unknown: false,
            input_closed: true,
            output_offset: final_offset,
            capture_failed: false,
            capture_state: CaptureState::Complete,
            final_output_offset: Some(final_offset),
        };
        let mut drained = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut drained, START)?;
        chunk(&mut drained, COMPLETE)?;
        let summary = drained.finish_from_status(&status);
        assert!(summary.capture_complete);
        assert_eq!(summary.termination, CodexTermination::ProviderTurnCompleted);
        assert_eq!(
            summary.producer_final_output_offset,
            Some(final_offset.to_string())
        );
        assert!(!summary.trusted_acceptance());
        let mut failed = status;
        failed.capture_failed = true;
        let mut parser = CodexNdjsonStream::new("native-model", CodexEffort::Medium)?;
        chunk(&mut parser, START)?;
        chunk(&mut parser, COMPLETE)?;
        assert!(!parser.finish_from_status(&failed).capture_complete);
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn fake_process_receives_literal_prompt_and_explicit_stdin_eof() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let base = dir.path().canonicalize()?;
        fs::create_dir(base.join("work"))?;
        fs::create_dir(base.join("spool"))?;
        let program = base.join("fake-codex");
        let args_file = base.join("argv.txt");
        let marker = base.join("forbidden-effect");
        let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\"'\"'"));
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\ncat > /dev/null\nprintf '%s\\n' '{{\"type\":\"thread.started\",\"thread_id\":\"thread\"}}' '{{\"type\":\"turn.started\"}}' '{{\"type\":\"item.completed\",\"item\":{{\"id\":\"reply\",\"type\":\"agent_message\",\"text\":\"PASS\"}}}}' '{{\"type\":\"turn.completed\",\"usage\":{{}}}}'\n",
            quote(&args_file)
        );
        fs::write(&program, script)?;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700))?;
        let contract = contract()?;
        let model = model();
        let cli = cli(program)?;
        let admission = admission(&contract, &model, &cli, &base)?;
        let prepared = prepare(
            &contract,
            admission,
            &model,
            &cli,
            &format!(
                "literal $(touch {}); `touch {}`",
                quote(&marker),
                quote(&marker)
            ),
        )?;
        let spec = prepared.launch_spec().clone();
        let host = SessionHost::new();
        host.launch(spec.clone())?;
        host.close_input_if_epoch(&spec.run_id, 0)?;
        let status = host.wait(&spec.run_id)?;
        assert!(status.success());
        assert!(!marker.exists());
        assert_eq!(
            fs::read_to_string(args_file)?,
            format!("{}\n", spec.args.join("\n"))
        );
        let tail = host.tail(&spec.run_id, 0, 256 * 1024)?;
        let mut parser = CodexNdjsonStream::from_prepared(&prepared)?;
        for record in tail.chunks {
            for observation in parser.push_chunk(&record)? {
                assert!(!observation.trusted_acceptance());
                assert_eq!(
                    observation.launch_tuple_digest.as_deref(),
                    Some(prepared.tuple_digest())
                );
            }
        }
        let summary = parser.finish(status.code(), true);
        assert_eq!(
            summary.launch_tuple_digest.as_deref(),
            Some(prepared.tuple_digest())
        );
        assert_eq!(summary.termination, CodexTermination::ProviderTurnCompleted);
        assert!(!summary.trusted_acceptance());
        Ok(())
    }
}
