//! Bounded OpenCode JSON observations and a reviewable SSH launch specification.
//! No process launches, credentials, node enrollment or acceptance authority here.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf};
use thiserror::Error;

const MAX_PROMPT_BYTES: usize = 16 * 1024;
const MAX_EVENT_BYTES: usize = 256 * 1024;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 8192;

#[derive(Debug, Error)]
pub enum OpenCodeError {
    #[error("invalid OpenCode SSH profile: {0}")]
    InvalidProfile(&'static str),
    #[error("OpenCode output exceeds parser bounds")]
    Bounds,
    #[error("OpenCode JSON event is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("OpenCode parser failed; retain raw output and reconcile")]
    Failed,
    #[error("model identity differs from the explicitly selected provider/model")]
    ModelMismatch,
}

/// References configuration on an already registered node. Files are never read
/// by this module. The caller must establish remote directory existence/custody.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenCodeSshProfile {
    pub node_id: String,
    pub ssh_alias: String,
    pub ssh_program: PathBuf,
    pub remote_program: String,
    pub remote_config: Option<String>,
    pub remote_worktree: String,
    pub model_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenCodeSshLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub node_id: String,
    pub requested_model: String,
    pub remote_worktree: String,
    pub native_execution_qualified: bool,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn remote_path(value: &str) -> bool {
    value.starts_with('/')
        && value != "/"
        && value.len() <= 2048
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|part| part == "..")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl OpenCodeSshProfile {
    pub fn launch_spec(&self, prompt: &str) -> Result<OpenCodeSshLaunch, OpenCodeError> {
        if !identifier(&self.node_id)
            || !identifier(&self.ssh_alias)
            || !self.ssh_program.is_absolute()
        {
            return Err(OpenCodeError::InvalidProfile(
                "registered node, safe SSH alias and absolute SSH executable required",
            ));
        }
        if !remote_path(&self.remote_program)
            || !remote_path(&self.remote_worktree)
            || self
                .remote_config
                .as_deref()
                .is_some_and(|p| !remote_path(p))
        {
            return Err(OpenCodeError::InvalidProfile(
                "paths must be existing absolute remote paths without traversal",
            ));
        }
        if !self.model_ref.contains('/')
            || self.model_ref.len() > 256
            || !self
                .model_ref
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
            || self.model_ref.starts_with('/')
            || self.model_ref.ends_with('/')
        {
            return Err(OpenCodeError::InvalidProfile(
                "explicit provider/model reference required",
            ));
        }
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0') {
            return Err(OpenCodeError::InvalidProfile(
                "bounded nonempty task capsule required",
            ));
        }
        let mut remote = vec!["env".to_owned(), "OPENCODE_DISABLE_AUTOUPDATE=true".into()];
        if let Some(config) = &self.remote_config {
            remote.push(format!("OPENCODE_CONFIG={config}"));
        }
        remote.extend([
            self.remote_program.clone(),
            "run".into(),
            "--model".into(),
            self.model_ref.clone(),
            "--agent".into(),
            "build".into(),
            "--dir".into(),
            self.remote_worktree.clone(),
            "--format".into(),
            "json".into(),
            prompt.into(),
        ]);
        let command = remote
            .iter()
            .map(|part| shell_quote(part))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(OpenCodeSshLaunch {
            program: self.ssh_program.clone(),
            args: vec![
                "-T".into(),
                "-o".into(),
                "BatchMode=yes".into(),
                "-o".into(),
                "ConnectTimeout=8".into(),
                "-o".into(),
                "ServerAliveInterval=15".into(),
                "-o".into(),
                "ServerAliveCountMax=2".into(),
                self.ssh_alias.clone(),
                command,
            ],
            node_id: self.node_id.clone(),
            requested_model: self.model_ref.clone(),
            remote_worktree: self.remote_worktree.clone(),
            native_execution_qualified: false,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelIdentity {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepUsage {
    pub input_tokens: Option<String>,
    pub output_tokens: Option<String>,
    pub reasoning_tokens: Option<String>,
    pub cache_read_tokens: Option<String>,
    pub cache_write_tokens: Option<String>,
    /// Step fields are provider reports, not a qualified aggregate or money claim.
    pub semantics: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenCodeObservation {
    pub event_type: String,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_status: Option<String>,
    pub finish_reason: Option<String>,
    pub usage: Option<StepUsage>,
    pub model_identity: Option<ModelIdentity>,
    pub provider_error: bool,
    /// A step ending does not establish task completion, verification or acceptance.
    pub trusted_acceptance: bool,
}

fn field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .map(str::to_owned)
}

fn counter(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(number.to_string());
    }
    let text = value.as_str()?;
    let number = text.parse::<u64>().ok()?;
    (number.to_string() == text).then(|| text.to_owned())
}

fn parse_event(bytes: &[u8]) -> Result<OpenCodeObservation, OpenCodeError> {
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(OpenCodeError::Bounds);
    }
    let value: Value = serde_json::from_slice(bytes)?;
    let event_type = field(&value, "type").ok_or(OpenCodeError::InvalidProfile(
        "event requires a bounded type",
    ))?;
    let part = &value["part"];
    let tokens = &part["tokens"];
    let usage = if event_type == "step_finish" {
        Some(StepUsage {
            input_tokens: counter(tokens.get("input")),
            output_tokens: counter(tokens.get("output")),
            reasoning_tokens: counter(tokens.get("reasoning")),
            cache_read_tokens: counter(tokens["cache"].get("read")),
            cache_write_tokens: counter(tokens["cache"].get("write")),
            semantics: "unqualified_provider_step_report".into(),
        })
    } else {
        None
    };
    Ok(OpenCodeObservation {
        session_id: field(&value, "sessionID"),
        tool_name: if event_type == "tool_use" {
            field(part, "tool")
        } else {
            None
        },
        tool_status: if event_type == "tool_use" {
            field(&part["state"], "status")
        } else {
            None
        },
        finish_reason: if event_type == "step_finish" {
            field(part, "reason")
        } else {
            None
        },
        model_identity: match (field(&value, "providerID"), field(&value, "modelID")) {
            (Some(provider_id), Some(model_id)) => Some(ModelIdentity {
                provider_id,
                model_id,
            }),
            _ => None,
        },
        provider_error: event_type == "error",
        event_type,
        usage,
        trusted_acceptance: false,
    })
}

#[derive(Default)]
pub struct OpenCodeJsonStream {
    pending: Vec<u8>,
    total_bytes: usize,
    events: usize,
    failed: bool,
}

impl OpenCodeJsonStream {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<OpenCodeObservation>, OpenCodeError> {
        if self.failed {
            return Err(OpenCodeError::Failed);
        }
        let result = self.push_inner(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn push_inner(&mut self, bytes: &[u8]) -> Result<Vec<OpenCodeObservation>, OpenCodeError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or(OpenCodeError::Bounds)?;
        if self.total_bytes > MAX_STREAM_BYTES {
            return Err(OpenCodeError::Bounds);
        }
        self.pending.extend_from_slice(bytes);
        let mut observations = Vec::new();
        if let Some(last) = self.pending.iter().rposition(|byte| *byte == b'\n') {
            let completed: Vec<u8> = self.pending.drain(..=last).collect();
            for line in completed
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            {
                self.events += 1;
                if self.events > MAX_EVENTS {
                    return Err(OpenCodeError::Bounds);
                }
                observations.push(parse_event(line)?);
            }
        }
        if self.pending.len() > MAX_EVENT_BYTES {
            return Err(OpenCodeError::Bounds);
        }
        Ok(observations)
    }

    pub fn finish(&mut self) -> Result<Vec<OpenCodeObservation>, OpenCodeError> {
        if self.failed {
            return Err(OpenCodeError::Failed);
        }
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        self.push(b"\n")
    }
}

/// Parse metadata from an explicit export of this task's session. This does not
/// read any provider global state or establish transport/resume qualification.
pub fn model_identities_from_export(bytes: &[u8]) -> Result<Vec<ModelIdentity>, OpenCodeError> {
    if bytes.len() > MAX_STREAM_BYTES {
        return Err(OpenCodeError::Bounds);
    }
    let value: Value = serde_json::from_slice(bytes)?;
    let mut identities = BTreeSet::new();
    if let Some(messages) = value["messages"].as_array() {
        for message in messages {
            let info = &message["info"];
            if info["role"] == "assistant"
                && let (Some(provider_id), Some(model_id)) =
                    (field(info, "providerID"), field(info, "modelID"))
            {
                identities.insert(ModelIdentity {
                    provider_id,
                    model_id,
                });
            }
        }
    }
    Ok(identities.into_iter().collect())
}

pub fn ensure_selected_model(
    expected: &str,
    observed: &[ModelIdentity],
) -> Result<bool, OpenCodeError> {
    if observed.is_empty() {
        return Ok(false);
    } // unknown is not selected-model evidence
    if observed
        .iter()
        .any(|identity| format!("{}/{}", identity.provider_id, identity.model_id) != expected)
    {
        return Err(OpenCodeError::ModelMismatch);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> OpenCodeSshProfile {
        OpenCodeSshProfile {
            node_id: "node-uuid-123".into(),
            ssh_alias: "enrolled-lab-node".into(),
            ssh_program: "/usr/bin/ssh".into(),
            remote_program: "/opt/agent/opencode".into(),
            remote_config: Some("/private/agent/opencode.json".into()),
            remote_worktree: "/work/private/task-1".into(),
            model_ref: "local/model-a".into(),
        }
    }

    #[test]
    fn reviewable_argv_quotes_capsules_and_rejects_injection_paths() -> Result<(), OpenCodeError> {
        let launch = profile().launch_spec("Implement 'quoted'; $(do not execute) text")?;
        assert!(!launch.native_execution_qualified);
        assert!(launch.args.last().is_some_and(
            |command| command.contains("'\"'\"'") && command.contains("--format' 'json")
        ));
        let mut unsafe_profile = profile();
        unsafe_profile.ssh_alias = "-oProxyCommand=evil".into();
        assert!(unsafe_profile.launch_spec("task").is_err());
        unsafe_profile = profile();
        unsafe_profile.remote_worktree = "/work/../foreign".into();
        assert!(unsafe_profile.launch_spec("task").is_err());
        unsafe_profile = profile();
        unsafe_profile.model_ref = "provider/model;evil".into();
        assert!(unsafe_profile.launch_spec("task").is_err());
        Ok(())
    }

    #[test]
    fn split_ndjson_preserves_unknown_usage_and_never_accepts() -> Result<(), OpenCodeError> {
        let input = include_bytes!("../../../../tests/fixtures/providers/opencode/success.ndjson");
        let mut parser = OpenCodeJsonStream::default();
        let mut events = Vec::new();
        for chunk in input.chunks(7) {
            events.extend(parser.push(chunk)?);
        }
        events.extend(parser.finish()?);
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| !event.trusted_acceptance));
        assert_eq!(events[1].tool_status.as_deref(), Some("completed"));
        let usage = events[2].usage.as_ref().ok_or(OpenCodeError::Failed)?;
        assert_eq!(usage.input_tokens.as_deref(), Some("9007199254740993"));
        assert_eq!(usage.cache_read_tokens, None);
        assert_eq!(events[3].event_type, "future_event");
        assert!(events[3].usage.is_none());
        Ok(())
    }

    #[test]
    fn malformed_and_oversized_events_fail_closed() {
        let mut parser = OpenCodeJsonStream::default();
        assert!(parser.push(b"{bad}\n").is_err());
        assert!(matches!(parser.push(b"{}\n"), Err(OpenCodeError::Failed)));
        let mut parser = OpenCodeJsonStream::default();
        assert!(parser.push(&vec![b'x'; MAX_EVENT_BYTES + 1]).is_err());
        let mut parser = OpenCodeJsonStream::default();
        assert!(parser.push(b"{\"type\":").is_ok());
        assert!(parser.finish().is_err());
    }

    #[test]
    fn actual_model_requires_explicit_session_evidence() -> Result<(), OpenCodeError> {
        let identities = model_identities_from_export(include_bytes!(
            "../../../../tests/fixtures/providers/opencode/session-export.json"
        ))?;
        assert!(ensure_selected_model("local/model-a", &identities)?);
        assert!(!ensure_selected_model("local/model-a", &[])?);
        assert!(matches!(
            ensure_selected_model("local/model-b", &identities),
            Err(OpenCodeError::ModelMismatch)
        ));
        Ok(())
    }
}
