//! Offline, unadmitted process launch data and caller-supplied observations.
//! No executable, Screen, transcript, or model flag authenticates a provider.
//! Custody, journal/watermark binding, billing and admission remain unqualified.
use crate::session::{LaunchSpec, validate_launch_envelope};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub use crate::session::{MAX_LAUNCH_ARG_BYTES as MAX_ARG_BYTES, MAX_LAUNCH_ARGS as MAX_ARGS};

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("process profile is invalid")]
    InvalidProfile,
}

#[derive(Clone, Debug)]
pub struct Launch {
    spec: LaunchSpec,
}

impl Launch {
    /// Pure validation of the caller's complete envelope, including JSON escaping.
    /// This does not examine the filesystem, reserve resources or grant execution.
    pub fn new(spec: LaunchSpec) -> Result<Self, ProcessError> {
        if [&spec.program, &spec.cwd, &spec.spool_dir]
            .into_iter()
            .any(|path| !lexical_absolute(path))
            || validate_launch_envelope(&spec).is_err()
        {
            return Err(ProcessError::InvalidProfile);
        }
        Ok(Self { spec })
    }

    pub fn spec(&self) -> &LaunchSpec {
        &self.spec
    }

    /// Consuming this data does not satisfy SessionHost custody or domain admission.
    pub fn into_spec(self) -> LaunchSpec {
        self.spec
    }

    pub fn native_execution_qualified(&self) -> bool {
        false
    }

    pub fn parent_acceptance(&self) -> bool {
        false
    }
}

fn lexical_absolute(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    if !path.is_absolute()
        || path.file_name().is_none()
        || value.chars().any(char::is_control)
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return false;
    }
    #[cfg(unix)]
    if value.contains('\\') {
        return false;
    }
    // Compare OsStr, not Path equality, which normalizes some aliases itself.
    let normalized: PathBuf = path.components().collect();
    normalized.as_os_str() == path.as_os_str()
}

pub(crate) fn valid_model(model: &str) -> bool {
    !model.trim().is_empty()
        && !model.starts_with('-')
        && model.len() <= 256
        && !model.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Running,
    Exited(i32),
    Signaled,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub lifecycle: Lifecycle,
    pub output_bytes: u64,
    pub output_truncated: bool,
    pub usage: Option<u64>,
    pub provider: Option<String>,
}

impl Observation {
    pub fn native_execution_qualified(&self) -> bool {
        false
    }

    pub fn parent_acceptance(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyIdentity {
    /// Unauthenticated argv[0] basename hint, never observed provider identity.
    pub executable_hint: Option<String>,
    /// One unambiguous requested model flag, never observed model identity.
    pub requested_model: Option<String>,
    pub observed_provider: Option<String>,
    pub observed_model: Option<String>,
    pub observed_only: bool,
}

/// Classify bounded caller-supplied argv as hints only. Screen names/transcripts
/// deliberately contribute nothing: quoted output and recycled names are not identity.
pub fn classify_legacy(
    argv: &[String],
    _screen_session: Option<&str>,
    _screen_text: Option<&str>,
) -> Result<LegacyIdentity, ProcessError> {
    if argv.len() > MAX_ARGS
        || argv
            .iter()
            .any(|arg| arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
        || argv
            .iter()
            .try_fold(0_usize, |total, arg| total.checked_add(arg.len()))
            .is_none_or(|total| total > crate::session::MAX_LAUNCH_ARGV_BYTES)
        || admiralty_core::limits::bounded_json_size(argv, crate::session::MAX_LAUNCH_SPEC_BYTES)
            .is_err()
    {
        return Err(ProcessError::InvalidProfile);
    }
    let executable = argv.first().map(|s| s.rsplit('/').next().unwrap_or(s));
    let executable_hint = match executable {
        Some("agy") => Some("agy"),
        Some("gemini") => Some("gemini"),
        Some("grok") => Some("grok"),
        _ => None,
    };
    let mut requested_model = None;
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let model = if arg == "--model" || arg == "-m" {
            Some(args.next().ok_or(ProcessError::InvalidProfile)?.as_str())
        } else {
            arg.strip_prefix("--model=")
        };
        if let Some(model) = model {
            if requested_model.is_some() || !valid_model(model) {
                return Err(ProcessError::InvalidProfile);
            }
            requested_model = Some(model.to_owned());
        }
    }
    Ok(LegacyIdentity {
        executable_hint: executable_hint.map(str::to_owned),
        requested_model,
        observed_provider: None,
        observed_model: None,
        observed_only: true,
    })
}

pub fn observe(
    lifecycle: Lifecycle,
    output: &[u8],
    max_output_bytes: usize,
) -> Result<Observation, ProcessError> {
    if max_output_bytes == 0 || max_output_bytes > super::observation::MAX_STREAM_BYTES {
        return Err(ProcessError::InvalidProfile);
    }
    Ok(Observation {
        lifecycle,
        output_bytes: output.len() as u64,
        output_truncated: output.len() > max_output_bytes,
        usage: None,
        provider: None,
    })
}

/// Provider-reported availability only. An error cannot authenticate an account,
/// settle usage, qualify a transport, or authorize model fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportedFailureKind {
    QuotaExhausted,
    PaymentRequired,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureObservation {
    pub kind: ReportedFailureKind,
    pub message: String,
    pub session_id: Option<String>,
    pub event_count: usize,
}

impl FailureObservation {
    pub fn native_execution_qualified(&self) -> bool {
        false
    }

    pub fn parent_acceptance(&self) -> bool {
        false
    }

    /// Error-path zero counters do not establish a qualified usage observation.
    pub fn usage(&self) -> Option<u64> {
        None
    }
}
