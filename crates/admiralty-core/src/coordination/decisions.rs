//! Scoped question and decision lifecycles with expected revisions, deadlines, and supersession.
pub use crate::coordination::questions::{
    QuestionError, QuestionEvent, QuestionScope, QuestionState, QuestionStatus, reduce_question,
};

pub fn is_terminal_decision(state: &QuestionState) -> bool {
    matches!(
        state.status,
        QuestionStatus::Answered { .. }
            | QuestionStatus::Superseded { .. }
            | QuestionStatus::Expired { .. }
            | QuestionStatus::Cancelled
    )
}
