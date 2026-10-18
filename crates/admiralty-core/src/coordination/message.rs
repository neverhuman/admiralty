//! Scoped coordination message validation and immutable causation envelopes.
pub use crate::{CoordinationMessage, MessageId, MessagePayload};
use crate::{PrincipalId, ProjectId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageValidationError {
    EmptyEpoch,
    EmptyAuthorityRef,
    CrossProject {
        expected: ProjectId,
        actual: ProjectId,
    },
    WrongRecipient {
        expected: PrincipalId,
        actual: PrincipalId,
    },
}

pub fn validate_scoped_envelope(
    message: &CoordinationMessage,
    expected_project: &ProjectId,
    expected_recipient: Option<&PrincipalId>,
) -> Result<(), MessageValidationError> {
    if message.context_epoch.trim().is_empty() {
        return Err(MessageValidationError::EmptyEpoch);
    }
    if message.authority_ref.trim().is_empty() {
        return Err(MessageValidationError::EmptyAuthorityRef);
    }
    if &message.project_id != expected_project {
        return Err(MessageValidationError::CrossProject {
            expected: expected_project.clone(),
            actual: message.project_id.clone(),
        });
    }
    if let Some(expected) = expected_recipient
        && &message.recipient != expected
    {
        return Err(MessageValidationError::WrongRecipient {
            expected: expected.clone(),
            actual: message.recipient.clone(),
        });
    }
    Ok(())
}
