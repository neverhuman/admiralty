//! Conservative Claude Code CLI launch data and stream observations.
//! Parsing is observational: it grants no admission, effects, or managed-peer status.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const ADAPTER_VERSION: u32 = 1;
pub const QUALIFIED_CLI_VERSION: &str = "2.1.302";
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 8192;
const MAX_PROMPT_BYTES: usize = 64 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClaudeError {
    #[error("invalid Claude adapter input: {0}")]
    Invalid(&'static str),
    #[error("Claude stream exceeds configured bounds")]
    Bounds,
    #[error("malformed Claude JSONL event")]
    Json,
    #[error("resume session does not match observed session")]
    ResumeMismatch,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeLaunch {
    pub program: PathBuf,
    pub cwd: PathBuf,
    pub model: String,
    pub effort: Effort,
    pub max_budget_usd: String,
    pub permission_mode: String,
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

impl ClaudeLaunch {
    pub fn validate(&self) -> Result<(), ClaudeError> {
        if !safe_abs(&self.program) || !safe_abs(&self.cwd) || self.program.file_name().is_none() {
            return Err(ClaudeError::Invalid("absolute program and cwd required"));
        }
        if self.model.trim().is_empty()
            || self.model.len() > 128
            || self.model.chars().any(char::is_control)
        {
            return Err(ClaudeError::Invalid("explicit model required"));
        }
        let budget = self
            .max_budget_usd
            .parse::<f64>()
            .map_err(|_| ClaudeError::Invalid("invalid budget"))?;
        if !budget.is_finite() || budget <= 0.0 || budget.to_string() != self.max_budget_usd {
            return Err(ClaudeError::Invalid("canonical positive budget required"));
        }
        if self.permission_mode != "default"
            && self.permission_mode != "dontAsk"
            && self.permission_mode != "acceptEdits"
        {
            return Err(ClaudeError::Invalid("unsupported permission mode"));
        }
        if self.resume != self.session_id.is_some()
            || self
                .session_id
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err(ClaudeError::Invalid("resume requires a bounded session ID"));
        }
        Ok(())
    }

    /// Exact argv shape, with stream framing selected explicitly and shell expansion absent.
    pub fn args(&self) -> Result<Vec<String>, ClaudeError> {
        self.validate()?;
        let mut a = vec![
            "--output-format".into(),
            "stream-json".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--model".into(),
            self.model.clone(),
            "--effort".into(),
            self.effort.as_str().into(),
            "--permission-mode".into(),
            self.permission_mode.clone(),
            "--max-budget-usd".into(),
            self.max_budget_usd.clone(),
            "--tools".into(),
            String::new(),
            "--restricted".into(),
            "--strict-mcp-config".into(),
        ];
        if let Some(id) = &self.session_id {
            a.push(
                if self.resume {
                    "--resume"
                } else {
                    "--session-id"
                }
                .into(),
            );
            a.push(id.clone());
        }
        Ok(a)
    }

    /// Encode one bounded user turn for `--input-format stream-json`.
    pub fn input_frame(prompt: &str) -> Result<Vec<u8>, ClaudeError> {
        if prompt.is_empty()
            || prompt.len() > MAX_PROMPT_BYTES
            || prompt.chars().any(char::is_control)
        {
            return Err(ClaudeError::Invalid(
                "prompt must be bounded and control-free",
            ));
        }
        let mut frame = serde_json::to_vec(&serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prompt }
        }))
        .map_err(|_| ClaudeError::Invalid("cannot encode user frame"))?;
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
fn safe_abs(p: &Path) -> bool {
    p.is_absolute()
        && p.to_str()
            .is_some_and(|s| !s.contains('\\') && !s.chars().any(char::is_control))
        && p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thinking_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub session_id: Option<String>,
    pub version: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub event_types: Vec<String>,
    pub permission_denials: usize,
    pub usage: Usage,
    pub total_cost_usd: Option<f64>,
    pub opaque_subagents: bool,
    pub result_subtype: Option<String>,
    pub is_error: Option<bool>,
}

/// Parse a bounded complete stream. Result usage is authoritative for the turn; assistant
/// message usage is retained only when no result usage was supplied, preventing double count.
pub fn parse_stream(
    bytes: &[u8],
    expected_resume: Option<&str>,
) -> Result<Observation, ClaudeError> {
    if bytes.len() > MAX_STREAM_BYTES {
        return Err(ClaudeError::Bounds);
    }
    let mut out = Observation {
        session_id: None,
        version: None,
        model: None,
        permission_mode: None,
        event_types: Vec::new(),
        permission_denials: 0,
        usage: Usage::default(),
        total_cost_usd: None,
        opaque_subagents: false,
        result_subtype: None,
        is_error: None,
    };
    let mut result_usage = None;
    let mut message_usage = None;
    let mut count = 0;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        if line.len() > MAX_EVENT_BYTES {
            return Err(ClaudeError::Bounds);
        }
        count += 1;
        if count > MAX_EVENTS {
            return Err(ClaudeError::Bounds);
        }
        let v: Value = serde_json::from_slice(line).map_err(|_| ClaudeError::Json)?;
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .ok_or(ClaudeError::Json)?;
        if !out.event_types.iter().any(|x| x == ty) {
            out.event_types.push(ty.to_owned());
        }
        match ty {
            "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                out.session_id = str_field(&v, "session_id");
                out.version = str_field(&v, "claude_code_version");
                out.model = str_field(&v, "model");
                out.permission_mode = str_field(&v, "permissionMode");
                if expected_resume.is_some_and(|e| Some(e) != out.session_id.as_deref()) {
                    return Err(ClaudeError::ResumeMismatch);
                }
            }
            "assistant" => {
                if v.get("parent_tool_use_id").is_some_and(|x| !x.is_null()) {
                    out.opaque_subagents = true;
                }
                if let Some(u) = v.pointer("/message/usage") {
                    message_usage = parse_usage(u);
                }
            }
            "result" => {
                out.result_subtype = str_field(&v, "subtype");
                out.is_error = v.get("is_error").and_then(Value::as_bool);
                out.permission_denials = v
                    .get("permission_denials")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                out.total_cost_usd = v.get("total_cost_usd").and_then(Value::as_f64);
                result_usage = v.get("usage").and_then(parse_usage);
            }
            _ => {}
        }
    }
    out.usage = result_usage
        .or(message_usage)
        .ok_or(ClaudeError::Invalid("stream has no usage-bearing event"))?;
    Ok(out)
}
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn parse_usage(v: &Value) -> Option<Usage> {
    let n = |k| v.get(k).and_then(Value::as_u64);
    if [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "output_tokens",
    ]
    .iter()
    .all(|k| v.get(*k).is_none() || v.get(*k).is_some_and(Value::is_u64))
    {
        Some(Usage {
            input_tokens: n("input_tokens"),
            cache_read_input_tokens: n("cache_read_input_tokens"),
            cache_creation_input_tokens: n("cache_creation_input_tokens"),
            output_tokens: n("output_tokens"),
            thinking_tokens: v
                .pointer("/output_tokens_details/thinking_tokens")
                .and_then(Value::as_u64),
        })
    } else {
        None
    }
}

/// Capability/economics are deliberately explicit: coordination and native child
/// accounting remain unqualified even when CLI subagents are observed.
pub fn qualification() -> Value {
    serde_json::json!({
        "provider":"claude-code", "cli_version":QUALIFIED_CLI_VERSION, "adapter_version":ADAPTER_VERSION,
        "structured_stream":true, "launch_input_resume":true, "configuration_bound":true,
        "native_usage_categories":["input","cache_read","cache_write","output","thinking"],
        "parent_child_usage_deduplicated":true, "subagents":"opaque_native_not_managed_peers",
        "coordination_tools":"unqualified", "approval_and_permission_denials":"observable",
        "context_floor":"unknown", "version_canary":"2.1.302 only", "cost":"reported CLI cost; not settlement"
    })
}
