//! Conservative shell-free generic process path. It observes direct-child
//! lifecycle and bounded output only; provider identity, billing, auth, resume,
//! and permission state remain unknown.
use std::path::PathBuf;
use thiserror::Error;

pub const MAX_ARGS: usize = 128;
pub const MAX_ARG_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("process profile is invalid")]
    InvalidProfile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl Launch {
    pub fn new(program: PathBuf, args: Vec<String>, cwd: PathBuf) -> Result<Self, ProcessError> {
        if !program.is_absolute()
            || !cwd.is_absolute()
            || args.len() > MAX_ARGS
            || args
                .iter()
                .any(|a| a.len() > MAX_ARG_BYTES || a.contains('\0'))
        {
            return Err(ProcessError::InvalidProfile);
        }
        Ok(Self { program, args, cwd })
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyIdentity {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub observed_only: bool,
}

/// Mirrors the installed legacy process classifier's executable/model signals.
/// Screen text can corroborate a Grok identity but never grants control.
pub fn classify_legacy(
    argv: &[String],
    screen_session: Option<&str>,
    screen_text: Option<&str>,
) -> LegacyIdentity {
    let executable = argv.first().map(|s| s.rsplit('/').next().unwrap_or(s));
    let provider = match executable {
        Some("agy") => Some("agy"),
        Some("gemini") => Some("gemini"),
        Some("grok") => Some("grok"),
        _ => None,
    }
    .or_else(|| {
        let evidence = format!(
            "{} {}",
            screen_session.unwrap_or_default(),
            screen_text.unwrap_or_default()
        )
        .to_ascii_lowercase();
        (evidence.contains("grok") || screen_text.is_some_and(|t| t.contains("Grok ")))
            .then_some("grok")
    });
    let model = argv
        .windows(2)
        .find(|w| w[0] == "-m" || w[0] == "--model")
        .map(|w| w[1].clone());
    LegacyIdentity {
        provider: provider.map(str::to_owned),
        model,
        observed_only: true,
    }
}

pub fn observe(
    lifecycle: Lifecycle,
    output: &[u8],
    max_output_bytes: usize,
) -> Result<Observation, ProcessError> {
    if max_output_bytes == 0 {
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
