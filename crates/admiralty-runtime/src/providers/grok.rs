//! Strict offline Grok ACP observations. ACP remains the provider's protocol;
//! observations are not Admiralty coordination, MCP, admission or acceptance.
use super::observation::{JsonlDecoder, ObservationError};
use super::process;
use crate::session::LaunchSpec;
use serde_json::Value;
use thiserror::Error;

pub const VERSION: &str = "1.0.62";
pub const BUILD: &str = "1474f9cbdd91";
pub const MAX_HELP_BYTES: usize = 64 * 1024;
pub use super::observation::{MAX_EVENT_BYTES, MAX_EVENTS, MAX_STREAM_BYTES};

#[derive(Debug, Error)]
pub enum GrokError {
    #[error("Grok CLI version is not qualified by this fixture")]
    UnsupportedVersion,
    #[error("Grok observation or launch profile is invalid")]
    InvalidProfile,
    #[error(transparent)]
    Stream(#[from] ObservationError),
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

impl Capabilities {
    pub fn native_execution_qualified(&self) -> bool {
        false
    }
}

/// Help/version matching reports advertised syntax only, not transport conformance.
pub fn qualify(version: &str, help: &str) -> Result<Capabilities, GrokError> {
    if version.len() > 128 || help.len() > MAX_HELP_BYTES {
        return Err(GrokError::InvalidProfile);
    }
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

#[derive(Clone, Debug)]
pub struct Launch {
    envelope: process::Launch,
    pub requested_model: Option<String>,
    pub observed_model: Option<String>,
    pub usage: Option<u64>,
}

impl Launch {
    pub fn spec(&self) -> &LaunchSpec {
        self.envelope.spec()
    }

    pub fn into_spec(self) -> LaunchSpec {
        self.envelope.into_spec()
    }

    pub fn native_execution_qualified(&self) -> bool {
        false
    }

    pub fn parent_acceptance(&self) -> bool {
        false
    }
}

/// Fill a complete caller-owned, unadmitted envelope. Empty input argv is required.
/// Configuration/auth/custody remain unchecked and no provider is invoked here.
pub fn launch(
    mut spec: LaunchSpec,
    model: Option<&str>,
    prompt: &str,
) -> Result<Launch, GrokError> {
    if !spec.args.is_empty()
        || prompt.trim().is_empty()
        || prompt.starts_with('-')
        || prompt.len() > crate::session::MAX_LAUNCH_ARG_BYTES
        || prompt.contains('\0')
        || model.is_some_and(|m| !process::valid_model(m))
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
    spec.args = args;
    Ok(Launch {
        envelope: process::Launch::new(spec).map_err(|_| GrokError::InvalidProfile)?,
        requested_model: model.map(str::to_owned),
        observed_model: None,
        usage: None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub method: String,
    /// Untrusted provider payload; fields never become managed coordination events.
    pub raw: Value,
}

fn observation(raw: Value) -> Result<Observation, ObservationError> {
    if raw.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || raw.get("params").is_some_and(|value| !value.is_object())
    {
        return Err(ObservationError::Json);
    }
    let method = raw
        .get("method")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
        .ok_or(ObservationError::Json)?
        .to_owned();
    Ok(Observation { method, raw })
}

/// Provisional objects, not full ACP qualification. The caller must separately
/// bind session identity, journal continuity and a final capture watermark.
#[derive(Default)]
pub struct StreamDecoder {
    jsonl: JsonlDecoder,
    failure: Option<ObservationError>,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Observation>, GrokError> {
        if let Some(error) = self.failure {
            return Err(error.into());
        }
        self.jsonl
            .push(bytes)
            .and_then(|values| values.into_iter().map(observation).collect())
            .map_err(|error| {
                self.failure = Some(error);
                error.into()
            })
    }

    pub fn finish(&mut self) -> Result<(), GrokError> {
        if let Some(error) = self.failure {
            return Err(error.into());
        }
        self.jsonl.finish().map_err(|error| {
            self.failure = Some(error);
            error.into()
        })
    }
}

pub fn parse_stream(bytes: &[u8]) -> Result<Vec<Observation>, GrokError> {
    let mut decoder = StreamDecoder::new();
    let observations = decoder.push(bytes)?;
    decoder.finish()?;
    Ok(observations)
}
