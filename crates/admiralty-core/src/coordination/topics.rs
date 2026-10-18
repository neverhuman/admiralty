//! Project-scoped topic subscriptions. Validates project/task authority before routing.
use crate::{PrincipalId, ProjectId, TaskId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScopedTopic {
    pub project_id: ProjectId,
    pub task_id: Option<TaskId>,
    pub channel: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopicError {
    EmptyChannel,
    UnauthorizedProject,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopicDirectory {
    subscriptions: BTreeMap<ScopedTopic, BTreeSet<PrincipalId>>,
}

impl TopicDirectory {
    pub fn subscribe(
        &mut self,
        caller_project: &ProjectId,
        topic: ScopedTopic,
        subscriber: PrincipalId,
    ) -> Result<bool, TopicError> {
        if topic.channel.trim().is_empty() {
            return Err(TopicError::EmptyChannel);
        }
        if &topic.project_id != caller_project {
            return Err(TopicError::UnauthorizedProject);
        }
        Ok(self
            .subscriptions
            .entry(topic)
            .or_default()
            .insert(subscriber))
    }

    pub fn resolve(
        &self,
        caller_project: &ProjectId,
        topic: &ScopedTopic,
    ) -> Result<Vec<PrincipalId>, TopicError> {
        if &topic.project_id != caller_project {
            return Err(TopicError::UnauthorizedProject);
        }
        Ok(self
            .subscriptions
            .get(topic)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default())
    }
}
