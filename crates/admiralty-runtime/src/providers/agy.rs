//! Version-pinned help evidence and strict offline stream-json observations.
//! Neither a launch envelope nor provider output grants native admission,
//! acceptance, identity, auth, usage, resume or permission qualification.
use super::observation::{JsonlDecoder, ObservationError};
use super::process;
use crate::session::LaunchSpec;
use serde_json::Value;
use thiserror::Error;

pub const VERSION: &str = "1.4.2";
pub const MAX_HELP_BYTES: usize = 64 * 1024;
pub use super::observation::{MAX_EVENT_BYTES, MAX_EVENTS, MAX_STREAM_BYTES};

#[derive(Debug, Error)]
pub enum AgyError {
    #[error("agy CLI version is not qualified by this fixture")]
    UnsupportedVersion,
    #[error("agy observation or launch profile is invalid")]
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
    pub stream_json: Capability,
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
pub fn qualify(version: &str, help: &str) -> Result<Capabilities, AgyError> {
    if version.len() > 128 || help.len() > MAX_HELP_BYTES {
        return Err(AgyError::InvalidProfile);
    }
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

/// Fill a complete caller-owned, unadmitted envelope. Empty input argv is required
/// so no supplied options can silently disappear. Custody/configuration are unchecked.
pub fn launch(mut spec: LaunchSpec, model: Option<&str>, prompt: &str) -> Result<Launch, AgyError> {
    if !spec.args.is_empty()
        || prompt.trim().is_empty()
        || prompt.starts_with('-')
        || prompt.len() > crate::session::MAX_LAUNCH_ARG_BYTES
        || prompt.contains('\0')
        || model.is_some_and(|m| !process::valid_model(m))
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
    spec.args = args;
    Ok(Launch {
        envelope: process::Launch::new(spec).map_err(|_| AgyError::InvalidProfile)?,
        requested_model: model.map(str::to_owned),
        observed_model: None,
        usage: None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub event_type: String,
    /// Untrusted provider payload. No fields are promoted to identity or authority.
    pub raw: Value,
}

fn observation(raw: Value) -> Result<Observation, ObservationError> {
    let event_type = raw
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
        .ok_or(ObservationError::Json)?
        .to_owned();
    Ok(Observation { event_type, raw })
}

/// Provisional observations only. No session journal, offsets or final capture
/// watermark are bound here; `finish` means JSONL framing completeness only.
#[derive(Default)]
pub struct StreamDecoder {
    jsonl: JsonlDecoder,
    failure: Option<ObservationError>,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Observation>, AgyError> {
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

    pub fn finish(&mut self) -> Result<(), AgyError> {
        if let Some(error) = self.failure {
            return Err(error.into());
        }
        self.jsonl.finish().map_err(|error| {
            self.failure = Some(error);
            error.into()
        })
    }
}

pub fn parse_stream(bytes: &[u8]) -> Result<Vec<Observation>, AgyError> {
    let mut decoder = StreamDecoder::new();
    let observations = decoder.push(bytes)?;
    decoder.finish()?;
    Ok(observations)
}
