//! Bounded, authenticated read tools for native task agents.
//!
//! These helpers are deliberately read-only. Authenticated identity comes
//! from the coordination credential authority; request fields never name an
//! actor. Durable coordination reads use the scope-indexed inbox API.

use super::coordination::{
    AuthenticatedCoordinationActor, CoordinationCredentialAuthority, CoordinationService,
};
use admiralty_core::{
    ContextCapsule, MessagePayload, ProjectId, SchedulingState, TaskId, compile_task_capsule,
};
use admiralty_store::inbox::{InboxCursor, InboxScope};
use admiralty_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

pub const MAX_AGENT_TOOL_PAGE: usize = 16;
pub const MAX_AGENT_TOOL_REPLY_BYTES: usize = 64 * 1024;
pub const MAX_AGENT_CONTEXT_TOKENS: usize = 4_096;
pub const QUESTION_JOURNAL_REQUIREMENT: &str = "question details require a durable journal read scoped by root, project, task, authority, and authenticated recipient";

fn valid_contract_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value.as_bytes()[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentTaskContext {
    /// Recovery identity only; global event volume is never disclosed to an agent.
    pub recovery_generation: String,
    pub task_view_digest: String,
    pub task_id: TaskId,
    pub task_state: SchedulingState,
    pub revision: u32,
    pub contract_digest: String,
    pub context_epoch: String,
    /// Absent only when the caller supplied the exact current digest.
    pub capsule: Option<ContextCapsule>,
    pub pending_questions: Vec<PendingQuestion>,
    pub artifact_refs: Vec<String>,
    pub omitted_items: usize,
    pub continuation: Option<InboxCursor>,
    pub question_journal_requirement: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingQuestion {
    pub message_id: String,
    pub requirement: String,
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Debug)]
pub enum AgentToolError {
    AccessDenied,
    TaskNotFound,
    CapsuleBudgetExceeded,
    Backpressure,
    Store(StoreError),
}

impl std::fmt::Display for AgentToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::AccessDenied => "agent tool access denied or scope unavailable",
            Self::TaskNotFound => "task context is unavailable",
            Self::CapsuleBudgetExceeded => "task capsule exceeds the configured context budget",
            Self::Backpressure => "agent tool output exceeds its bounded response budget",
            Self::Store(_) => "agent tool storage read failed",
        })
    }
}

impl std::error::Error for AgentToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

/// Compact task/context retrieval. The inbox grant is checked for this exact
/// project, authenticated recipient, and epoch before any Store read. Only the
/// requested task and its bounded capsule are returned; the projection is
/// never serialized or returned wholesale.
#[allow(clippy::too_many_arguments)]
pub fn task_context_get(
    store: &Store,
    actor: &AuthenticatedCoordinationActor,
    credentials: &CoordinationCredentialAuthority,
    project_id: ProjectId,
    task_id: TaskId,
    context_epoch: String,
    known_contract_digest: Option<&str>,
    max_estimated_tokens: usize,
    inbox_cursor: Option<InboxCursor>,
    inbox_limit: usize,
) -> Result<AgentTaskContext, AgentToolError> {
    if inbox_limit == 0 || inbox_limit > MAX_AGENT_TOOL_PAGE {
        return Err(AgentToolError::Backpressure);
    }
    if known_contract_digest.is_some_and(|digest| !valid_contract_digest(digest)) {
        return Err(AgentToolError::Backpressure);
    }
    let scope = InboxScope {
        project_id: project_id.clone(),
        recipient: actor.principal().clone(),
        context_epoch: context_epoch.clone(),
    };
    let coordination = CoordinationService::new(store);
    let inbox = coordination
        .pending_messages_authenticated(actor, credentials, scope, inbox_cursor, inbox_limit)
        .map_err(|_| AgentToolError::AccessDenied)?;

    let snapshot = store.snapshot().map_err(AgentToolError::Store)?;
    let task = snapshot
        .projection
        .tasks
        .get(&task_id)
        .filter(|view| view.contract.project_id == project_id)
        .ok_or(AgentToolError::TaskNotFound)?;
    let contract_digest =
        admiralty_core::content_digest(&task.contract).map_err(|_| AgentToolError::TaskNotFound)?;
    let capsule = if known_contract_digest == Some(contract_digest.as_str()) {
        None
    } else {
        let budget = max_estimated_tokens.min(MAX_AGENT_CONTEXT_TOKENS);
        Some(
            compile_task_capsule(&task.contract, &context_epoch, budget)
                .map_err(|_| AgentToolError::CapsuleBudgetExceeded)?,
        )
    };

    let mut pending_questions = Vec::new();
    let mut artifact_refs = Vec::new();
    let mut omitted_items = 0;
    for message in &inbox.messages {
        match &message.payload {
            MessagePayload::Question {
                task_id: message_task,
                requirement,
                question,
                options,
            } if message_task == &task_id => {
                if pending_questions.len() < MAX_AGENT_TOOL_PAGE {
                    pending_questions.push(PendingQuestion {
                        message_id: message.message_id.to_string(),
                        requirement: requirement.clone(),
                        question: question.clone(),
                        options: options.clone(),
                    });
                } else {
                    omitted_items += 1;
                }
            }
            MessagePayload::ArtifactReady {
                task_id: message_task,
                artifact_ref,
                ..
            }
            | MessagePayload::Handoff {
                task_id: message_task,
                candidate_ref: artifact_ref,
                ..
            } if message_task == &task_id => {
                if artifact_refs.len() < MAX_AGENT_TOOL_PAGE {
                    artifact_refs.push(artifact_ref.clone());
                } else {
                    omitted_items += 1;
                }
            }
            _ => {}
        }
    }
    let reply = AgentTaskContext {
        recovery_generation: snapshot.cursor.recovery_generation,
        task_view_digest: admiralty_core::content_digest(task)
            .map_err(|_| AgentToolError::TaskNotFound)?,
        task_id,
        task_state: task.state.clone(),
        revision: task.contract.revision,
        contract_digest,
        context_epoch,
        capsule,
        pending_questions,
        artifact_refs,
        omitted_items,
        continuation: inbox.next_cursor,
        question_journal_requirement: QUESTION_JOURNAL_REQUIREMENT.to_owned(),
    };
    let size = serde_json::to_vec(&reply)
        .map_err(|_| AgentToolError::Backpressure)?
        .len();
    if size > MAX_AGENT_TOOL_REPLY_BYTES {
        return Err(AgentToolError::Backpressure);
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coordination::{
        CoordinationAction, CoordinationCredential, CoordinationCredentialGrant,
    };
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, CoordinationMessage, DomainEvent, MessageId, PrincipalId,
        RootId, SourceIdentity, TaskContract,
    };
    use std::{fs, path::PathBuf};

    fn task(project: ProjectId) -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: project,
            root_id: RootId::new("agent-tools-root").expect("valid root"),
            task_id: TaskId::new("agent-tools-task").expect("valid task"),
            revision: 1,
            title: "Bounded agent task".into(),
            objective: "Read only the authorized task capsule".into(),
            non_goals: vec!["No acceptance or launch authority".into()],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec!["secrets".into()],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "check-it".into(),
                text: "Preserve exact task scope".into(),
                check_ids: vec!["unit".into()],
            }],
            checks: vec![CheckSpec {
                id: "unit".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 30,
            }],
            authority_ref: "grant:agent-tools".into(),
            max_attempts: 1,
            simulated: true,
        }
    }

    fn temporary_database() -> PathBuf {
        std::env::temp_dir().join(format!(
            "admiralty-agent-tools-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn credential(scope: &InboxScope) -> (CoordinationCredentialAuthority, String) {
        let token = "agent-tools-test-token-with-sufficient-entropy".to_owned();
        let credential = CoordinationCredential::configured(
            "agent-tools-test".into(),
            token.clone(),
            scope.recipient.clone(),
            vec![CoordinationCredentialGrant {
                scope: scope.clone(),
                action: CoordinationAction::ReadPending,
                message_id: None,
                source: None,
            }],
        );
        (
            CoordinationCredentialAuthority::new(vec![credential]).expect("valid credential"),
            token,
        )
    }

    fn message(project: ProjectId, recipient: PrincipalId, task_id: TaskId) -> CoordinationMessage {
        CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new("agent-tools-question").expect("valid message ID"),
            project_id: project,
            sender: PrincipalId::new("captain").expect("valid sender"),
            recipient,
            causation_id: None,
            context_epoch: "epoch-agent-tools".into(),
            authority_ref: "grant:agent-tools".into(),
            payload: MessagePayload::Question {
                task_id,
                requirement: "Need owner guidance".into(),
                question: "Which bounded option applies?".into(),
                options: vec!["A".into(), "B".into()],
            },
        }
    }

    #[test]
    fn authenticated_task_context_is_bounded_scoped_and_read_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = temporary_database();
        let project = ProjectId::new("agent-tools-project").expect("valid project");
        let recipient = PrincipalId::new("agent-tools-worker").expect("valid recipient");
        let scope = InboxScope {
            project_id: project.clone(),
            recipient: recipient.clone(),
            context_epoch: "epoch-agent-tools".into(),
        };
        let (credentials, token) = credential(&scope);
        let actor = credentials
            .authenticate(&token)
            .expect("authenticated handle");
        {
            let store = Store::open(&path).expect("open actual Store");
            let contract = task(project.clone());
            store
                .append(
                    SourceIdentity {
                        producer_id: "agent-tools-seed".into(),
                        incarnation: "test-1".into(),
                        sequence: "1".into(),
                    },
                    DomainEvent::TaskSubmitted {
                        contract: contract.clone(),
                    },
                    "2026-10-18T20:00:00Z".into(),
                )
                .expect("store task through writer");
            store
                .append(
                    SourceIdentity {
                        producer_id: "agent-tools-message".into(),
                        incarnation: "test-1".into(),
                        sequence: "1".into(),
                    },
                    DomainEvent::MessageQueued {
                        message: message(project.clone(), recipient, contract.task_id.clone()),
                    },
                    "2026-10-18T20:00:01Z".into(),
                )
                .expect("store scoped inbox message");

            let cursor_before = store.snapshot()?.cursor;
            let reply = task_context_get(
                &store,
                &actor,
                &credentials,
                project.clone(),
                contract.task_id.clone(),
                "epoch-agent-tools".into(),
                None,
                MAX_AGENT_CONTEXT_TOKENS,
                None,
                16,
            )
            .expect("authorized task context");
            assert_eq!(
                reply.contract_digest,
                admiralty_core::content_digest(&contract)?
            );
            assert_eq!(reply.pending_questions.len(), 1);
            assert_eq!(reply.task_state, SchedulingState::Ready);
            assert!(reply.capsule.is_some());
            assert_eq!(
                reply.question_journal_requirement,
                QUESTION_JOURNAL_REQUIREMENT
            );
            assert!(serde_json::to_vec(&reply)?.len() <= MAX_AGENT_TOOL_REPLY_BYTES);
            assert_eq!(
                store.snapshot()?.cursor,
                cursor_before,
                "tool reads are non-mutating"
            );

            let wrong_project = ProjectId::new("foreign-project").expect("valid foreign project");
            assert!(matches!(
                task_context_get(
                    &store,
                    &actor,
                    &credentials,
                    wrong_project,
                    contract.task_id,
                    "epoch-agent-tools".into(),
                    None,
                    MAX_AGENT_CONTEXT_TOKENS,
                    None,
                    16,
                ),
                Err(AgentToolError::AccessDenied)
            ));
            assert!(matches!(
                task_context_get(
                    &store,
                    &actor,
                    &credentials,
                    project,
                    TaskId::new("agent-tools-task")?,
                    "epoch-agent-tools".into(),
                    Some("unbounded-and-invalid"),
                    MAX_AGENT_CONTEXT_TOKENS,
                    None,
                    16,
                ),
                Err(AgentToolError::Backpressure)
            ));
        }
        let _ = fs::remove_file(&path);
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".writer.lock");
        let _ = fs::remove_file(PathBuf::from(lock_path));
        Ok(())
    }
}
