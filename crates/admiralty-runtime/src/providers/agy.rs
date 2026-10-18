//! Version-pinned agy CLI evidence and bounded stream-json observations.
//! This is a candidate parser; it does not claim auth, billing, resume, or
//! permission semantics and does not map provider events to Admiralty events.
use serde_json::Value;
use std::path::PathBuf;
use thiserror::Error;

pub const VERSION: &str = "1.4.2";
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EVENTS: usize = 8192;

#[derive(Debug, Error)]
pub enum AgyError {
    #[error("agy CLI version is not qualified by this fixture")]
    UnsupportedVersion,
    #[error("agy launch profile is invalid")]
    InvalidProfile,
    #[error("agy stream exceeds parser bounds")]
    Bounds,
    #[error("agy stream JSON is malformed: {0}")]
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
    pub stream_json: Capability,
    pub auth_status: Capability,
    pub usage: Capability,
    pub resume: Capability,
    pub permission_semantics: Capability,
}

pub fn qualify(version: &str, help: &str) -> Result<Capabilities, AgyError> {
    if version.trim() != VERSION {
        return Err(AgyError::UnsupportedVersion);
    }
    let stream_json = if help.contains("--input-format")
        && help.contains("--output-format")
        && help.contains("stream-json")
    {
        Capability::Advertised
    } else {
        Capability::Unsupported("observed help lacks the stream-json input/output flags")
    };
    Ok(Capabilities {
        version: VERSION,
        stream_json,
        auth_status: Capability::Unsupported(
            "version/help probes do not establish authentication state",
        ),
        usage: Capability::Unsupported("no usage payload has been qualified for this version"),
        resume: Capability::Unsupported("continuation behavior has not been qualified"),
        permission_semantics: Capability::Unsupported("permission behavior has not been qualified"),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub model: Option<String>,
    pub usage: Option<u64>,
}

pub fn launch(program: PathBuf, model: Option<&str>, prompt: &str) -> Result<Launch, AgyError> {
    if !program.is_absolute()
        || prompt.trim().is_empty()
        || prompt.len() > 64 * 1024
        || prompt.contains('\0')
        || model.is_some_and(|m| {
            m.trim().is_empty() || m.len() > 256 || m.chars().any(char::is_control)
        })
    {
        return Err(AgyError::InvalidProfile);
    }
    let mut args = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
    ];
    if let Some(m) = model {
        args.extend(["--model".into(), m.into()]);
    }
    args.push(prompt.into());
    Ok(Launch {
        program,
        args,
        model: model.map(str::to_owned),
        usage: None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub event_type: Option<String>,
    pub raw: Value,
}

pub fn parse_stream(bytes: &[u8]) -> Result<Vec<Observation>, AgyError> {
    if bytes.len() > MAX_STREAM_BYTES {
        return Err(AgyError::Bounds);
    }
    let mut out = Vec::new();
    for line in bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        if line.len() > MAX_EVENT_BYTES || out.len() == MAX_EVENTS {
            return Err(AgyError::Bounds);
        }
        let raw: Value = serde_json::from_slice(line)?;
        let event_type = raw.get("type").and_then(Value::as_str).map(str::to_owned);
        out.push(Observation { event_type, raw });
    }
    Ok(out)
}
