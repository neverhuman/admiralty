//! Bounded Grok ACP stream observations. ACP remains the provider's protocol;
//! observations are not Admiralty coordination or MCP messages.
use serde_json::Value;
use std::path::PathBuf;
use thiserror::Error;

pub const VERSION: &str = "1.0.62";
pub const BUILD: &str = "1474f9cbdd91";
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EVENTS: usize = 8192;

#[derive(Debug, Error)]
pub enum GrokError {
    #[error("Grok CLI version is not qualified by this fixture")]
    UnsupportedVersion,
    #[error("Grok launch profile is invalid")]
    InvalidProfile,
    #[error("Grok ACP stream exceeds parser bounds")]
    Bounds,
    #[error("Grok ACP JSON is malformed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    Advertised,
    Unsupported(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub version: &'static str,
    pub acp_stream: Capability,
    pub auth_status: Capability,
    pub usage: Capability,
    pub resume: Capability,
    pub permission_semantics: Capability,
}

pub fn qualify(version: &str, help: &str) -> Result<Capabilities, GrokError> {
    if version.trim() != format!("grok {VERSION} ({BUILD}) [stable]") {
        return Err(GrokError::UnsupportedVersion);
    }
    let acp_stream =
        if help.contains("streaming-json") && help.contains("ACP session update per line") {
            Capability::Advertised
        } else {
            Capability::Unsupported("observed help lacks the documented ACP streaming-json format")
        };
    Ok(Capabilities {
        version: VERSION,
        acp_stream,
        auth_status: Capability::Unsupported(
            "version/help probes do not establish authentication state",
        ),
        usage: Capability::Unsupported("no ACP usage payload has been qualified for this version"),
        resume: Capability::Unsupported(
            "resume flag exists, but continuation behavior has not been qualified",
        ),
        permission_semantics: Capability::Unsupported(
            "permission modes exist, but enforcement semantics have not been qualified",
        ),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub model: Option<String>,
    pub usage: Option<u64>,
}

pub fn launch(program: PathBuf, model: Option<&str>, prompt: &str) -> Result<Launch, GrokError> {
    if !program.is_absolute()
        || prompt.trim().is_empty()
        || prompt.len() > 64 * 1024
        || prompt.contains('\0')
        || model.is_some_and(|m| {
            m.trim().is_empty() || m.len() > 256 || m.chars().any(char::is_control)
        })
    {
        return Err(GrokError::InvalidProfile);
    }
    let mut args = vec![
        "--single".into(),
        prompt.into(),
        "--output-format".into(),
        "streaming-json".into(),
    ];
    if let Some(m) = model {
        args.extend(["--model".into(), m.into()]);
    }
    Ok(Launch {
        program,
        args,
        model: model.map(str::to_owned),
        usage: None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub method: Option<String>,
    pub raw: Value,
}

pub fn parse_stream(bytes: &[u8]) -> Result<Vec<Observation>, GrokError> {
    if bytes.len() > MAX_STREAM_BYTES {
        return Err(GrokError::Bounds);
    }
    let mut out = Vec::new();
    for line in bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        if line.len() > MAX_EVENT_BYTES || out.len() == MAX_EVENTS {
            return Err(GrokError::Bounds);
        }
        let raw: Value = serde_json::from_slice(line)?;
        let method = raw.get("method").and_then(Value::as_str).map(str::to_owned);
        out.push(Observation { method, raw });
    }
    Ok(out)
}
