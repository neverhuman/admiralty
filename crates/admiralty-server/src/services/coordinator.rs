//! Read-only fair-round composition against the actual Store projection.
use admiralty_core::{
    SchedulingState, content_digest,
    scheduler::{
        Knowledge, LeaseObservation, TaskSchedulingState,
        fairness::{
            FairRound, FairnessError, FairnessPolicy, FairnessState, ProjectQueue, WorkClass,
            propose_fair_round,
        },
    },
};
use admiralty_store::Store;

#[derive(Debug)]
pub enum CoordinationError {
    Unavailable,
    Stale,
    Fairness(FairnessError),
}

impl std::fmt::Display for CoordinationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("Store scheduling snapshot is unavailable"),
            Self::Stale => {
                f.write_str("queue is incomplete or differs from the current Store projection")
            }
            Self::Fairness(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CoordinationError {}
impl From<FairnessError> for CoordinationError {
    fn from(value: FairnessError) -> Self {
        Self::Fairness(value)
    }
}

#[derive(Clone, Debug)]
pub struct CoordinatedRound {
    pub store_read_digest: String,
    pub round: FairRound,
}

/// Validates reported task states against an actual single-writer snapshot.
/// The returned selection still requires current budget, resource, model and
/// authority proofs at native admission; it cannot start or claim work.
pub fn propose_store_round(
    store: &Store,
    queues: &[ProjectQueue],
    state: &FairnessState,
    policy: &FairnessPolicy,
    inference: Knowledge<u16>,
    compute: Knowledge<u16>,
    now: u64,
) -> Result<CoordinatedRound, CoordinationError> {
    let round = propose_fair_round(queues, state, policy, inference, compute, now)?;
    let snapshot = store
        .snapshot()
        .map_err(|_| CoordinationError::Unavailable)?;
    for queue in queues {
        let scope = &queue.snapshot.scope;
        let actual_count = snapshot
            .projection
            .tasks
            .values()
            .filter(|task| {
                task.contract.project_id == scope.project_id
                    && task.contract.root_id == scope.root_id
            })
            .count();
        if actual_count != queue.snapshot.tasks.len() {
            return Err(CoordinationError::Stale);
        }
        let mut active_features = 0;
        for reported in &queue.snapshot.tasks {
            let task = snapshot
                .projection
                .tasks
                .get(&reported.contract.task_id)
                .ok_or(CoordinationError::Stale)?;
            let actual_state = match task.state {
                SchedulingState::Ready => TaskSchedulingState::Pending,
                SchedulingState::Claimed
                | SchedulingState::Running
                | SchedulingState::AwaitingVerification => TaskSchedulingState::Active,
                SchedulingState::Accepted => TaskSchedulingState::Completed,
                _ => TaskSchedulingState::Blocked,
            };
            if task.contract != reported.contract
                || reported.state != actual_state
                || task.owner.is_some() && matches!(reported.lease, LeaseObservation::Absent)
            {
                return Err(CoordinationError::Stale);
            }
            if actual_state == TaskSchedulingState::Active
                && queue
                    .tasks
                    .get(&reported.contract.task_id)
                    .is_some_and(|m| m.class == WorkClass::Feature)
            {
                active_features += 1;
            }
        }
        if active_features != usize::from(queue.active_feature_wip) {
            return Err(CoordinationError::Stale);
        }
    }
    let store_read_digest =
        content_digest(&snapshot).map_err(|_| CoordinationError::Unavailable)?;
    Ok(CoordinatedRound {
        store_read_digest,
        round,
    })
}

/// Advisory stale-read detection. Actual admission must still recheck proofs
/// inside the Store transaction; a successful read is not a lease or authority.
pub fn read_set_is_current(
    store: &Store,
    round: &CoordinatedRound,
) -> Result<bool, CoordinationError> {
    let current = store
        .snapshot()
        .map_err(|_| CoordinationError::Unavailable)?;
    Ok(
        content_digest(&current).map_err(|_| CoordinationError::Unavailable)?
            == round.store_read_digest,
    )
}
