//! Complete wire-envelope validation plus exact project/recipient labels.
//! Actor authentication, task membership and grant checks belong to the caller;
//! a matching authority string is not an authorization decision.
pub use crate::{CoordinationMessage, MessageId, MessagePayload};
use crate::{PrincipalId, ProjectId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageValidationError {
    EmptyEpoch,
    EmptyAuthorityRef,
    InvalidEnvelope(String),
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
    message
        .validate()
        .map_err(MessageValidationError::InvalidEnvelope)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskId;

    fn fixture() -> CoordinationMessage {
        CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new("envelope-1").expect("fixture message ID"),
            project_id: ProjectId::new("project-1").expect("fixture project ID"),
            sender: PrincipalId::new("worker").expect("fixture sender"),
            recipient: PrincipalId::new("captain").expect("fixture recipient"),
            causation_id: None,
            context_epoch: "epoch-1".into(),
            authority_ref: "grant:fixture".into(),
            payload: MessagePayload::Question {
                task_id: TaskId::new("task-1").expect("fixture task"),
                requirement: "Do NOT publish; timeout <= 5 ms".into(),
                question: "Which interface revision applies?".into(),
                options: vec!["v1".into(), "v2".into()],
            },
        }
    }

    #[test]
    fn scoped_helper_preserves_exact_envelope_and_rejects_foreign_labels() {
        let message = fixture();
        let before = message.clone();
        assert_eq!(
            validate_scoped_envelope(&message, &message.project_id, Some(&message.recipient)),
            Ok(())
        );
        assert_eq!(message, before);
        assert!(matches!(
            validate_scoped_envelope(
                &message,
                &ProjectId::new("foreign").expect("fixture project"),
                None
            ),
            Err(MessageValidationError::CrossProject { .. })
        ));
        assert!(matches!(
            validate_scoped_envelope(&message, &message.project_id, Some(&message.sender)),
            Err(MessageValidationError::WrongRecipient { .. })
        ));
    }

    #[test]
    fn full_schema_payload_and_escaped_envelope_bounds_cannot_be_bypassed() {
        let mut cases = Vec::new();
        let mut version = fixture();
        version.schema_version = 99;
        cases.push(version);
        let mut payload = fixture();
        if let MessagePayload::Question { question, .. } = &mut payload.payload {
            question.clear();
        }
        cases.push(payload);
        let mut escaped = fixture();
        if let MessagePayload::Question { question, .. } = &mut escaped.payload {
            *question = "\"".repeat(8192);
        }
        cases.push(escaped);
        let mut epoch = fixture();
        epoch.context_epoch = "e".repeat(16_384);
        cases.push(epoch);
        let mut authority = fixture();
        authority.authority_ref = "a".repeat(16_384);
        cases.push(authority);
        for message in cases {
            assert!(matches!(
                validate_scoped_envelope(&message, &message.project_id, Some(&message.recipient)),
                Err(MessageValidationError::InvalidEnvelope(_))
            ));
        }
        let mut blank = fixture();
        blank.context_epoch = " \n".into();
        assert_eq!(
            validate_scoped_envelope(&blank, &blank.project_id, None),
            Err(MessageValidationError::EmptyEpoch)
        );
        blank = fixture();
        blank.authority_ref = " \n".into();
        assert_eq!(
            validate_scoped_envelope(&blank, &blank.project_id, None),
            Err(MessageValidationError::EmptyAuthorityRef)
        );
    }
}
