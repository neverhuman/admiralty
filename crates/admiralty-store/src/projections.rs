//! Pure projection rebuild (`rebuild_projections`) and indexed bounded read queries (`P03`).
//!
//! Rebuilding projections replays committed `DomainEvent` records through the pure
//! reducer with zero external effect execution and computes a deterministic
//! canonical `projection_digest`.

use crate::{Cursor, Result, Snapshot, Store, StoreError};
use admiralty_core::{
    PrincipalId, ProjectId, Projection, SchedulingState, TaskId, TaskView, content_digest,
};
use serde::{Deserialize, Serialize};
use std::ops::Bound;

pub const MAX_PROJECTION_QUERY_PAGE: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRebuildReport {
    pub cursor: Cursor,
    pub projection: Projection,
    pub projection_digest: String,
    pub integrity_incidents: String,
    pub effects_executed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskProjectionQuery {
    pub project_id: Option<ProjectId>,
    pub state: Option<SchedulingState>,
    pub owner: Option<PrincipalId>,
    pub after_task_id: Option<TaskId>,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskProjectionPage {
    pub cursor: Cursor,
    pub tasks: Vec<TaskView>,
    pub next_after_task_id: Option<TaskId>,
}

/// Rebuild projections purely from durable domain events with zero effect execution,
/// returning the updated recovery cursor, projection, and canonical projection hash.
pub fn rebuild_projections(store: &Store) -> Result<ProjectionRebuildReport> {
    let snapshot = store.rebuild()?;
    let projection_digest = content_digest(&snapshot.projection)
        .map_err(|e| StoreError::Invalid(format!("cannot digest rebuilt projection: {e}")))?;
    Ok(ProjectionRebuildReport {
        cursor: snapshot.cursor,
        projection: snapshot.projection,
        projection_digest,
        integrity_incidents: snapshot.integrity_incidents,
        effects_executed: false,
    })
}

/// Compute the canonical content digest of a `Snapshot`'s `Projection`.
pub fn snapshot_projection_digest(snapshot: &Snapshot) -> Result<String> {
    content_digest(&snapshot.projection)
        .map_err(|e| StoreError::Invalid(format!("cannot digest snapshot projection: {e}")))
}

/// Execute an indexed, bounded query over the task projection using `BTreeMap` range
/// bounds and deterministic cursor pagination (`1..=256` tasks per page).
pub fn query_tasks_bounded(
    snapshot: &Snapshot,
    query: &TaskProjectionQuery,
) -> Result<TaskProjectionPage> {
    if query.limit == 0 || query.limit > MAX_PROJECTION_QUERY_PAGE {
        return Err(StoreError::Invalid(
            "task projection query limit must be in 1..=256".into(),
        ));
    }
    let start_bound = match &query.after_task_id {
        Some(after) => Bound::Excluded(after.clone()),
        None => Bound::Unbounded,
    };
    let mut matching = snapshot
        .projection
        .tasks
        .range((start_bound, Bound::Unbounded))
        .filter(|(_, view)| {
            query
                .project_id
                .as_ref()
                .is_none_or(|p| &view.contract.project_id == p)
                && query.state.as_ref().is_none_or(|s| &view.state == s)
                && query
                    .owner
                    .as_ref()
                    .is_none_or(|o| view.owner.as_ref() == Some(o))
        });

    let mut tasks = Vec::with_capacity(query.limit.min(16));
    for _ in 0..query.limit {
        let Some((_, view)) = matching.next() else {
            break;
        };
        tasks.push(view.clone());
    }
    let has_more = matching.next().is_some();
    let next_after_task_id = if has_more {
        tasks.last().map(|t| t.contract.task_id.clone())
    } else {
        None
    };

    Ok(TaskProjectionPage {
        cursor: snapshot.cursor.clone(),
        tasks,
        next_after_task_id,
    })
}
