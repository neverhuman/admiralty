//! Bounded fair scheduling proposals. Admission still needs the Store writer.
use super::{Knowledge, SchedulerError, SchedulerSnapshot, propose_ready_tasks};
use crate::{ProjectId, TaskId, content_digest, limits::bounded_json_size};
use serde::Serialize;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkClass {
    Repair,
    Verification,
    Feature,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueuedTask {
    pub queued_at: u64,
    pub class: WorkClass,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectPolicy {
    pub weight: u16,
    pub feature_wip_limit: u16,
    pub bootstrap_wip_limit: u16,
    pub finish_min_wip: u16,
    pub red_enter_secs: u64,
    pub red_exit_secs: u64,
    pub no_accept_enter_secs: u64,
    pub no_accept_exit_secs: u64,
}
impl Default for ProjectPolicy {
    fn default() -> Self {
        Self {
            weight: 1,
            feature_wip_limit: 8,
            bootstrap_wip_limit: 1,
            finish_min_wip: 2,
            red_enter_secs: 1200,
            red_exit_secs: 300,
            no_accept_enter_secs: 3600,
            no_accept_exit_secs: 300,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectQueue {
    pub snapshot: SchedulerSnapshot,
    pub tasks: BTreeMap<TaskId, QueuedTask>,
    pub policy: ProjectPolicy,
    pub active_feature_wip: u16,
    pub throughput_known: bool,
    /// Zero means an observed green train. Unknown holds new feature work.
    pub red_train_age_secs: Knowledge<u64>,
    pub since_last_accept_secs: Knowledge<u64>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct FairnessState {
    pub generation: u64,
    pub deficits: BTreeMap<ProjectId, i64>,
    pub finish_mode: BTreeSet<ProjectId>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FairnessPolicy {
    pub starvation_secs: u64,
    pub verification_percent: u8,
    pub repair_percent: u8,
}
impl Default for FairnessPolicy {
    fn default() -> Self {
        Self {
            starvation_secs: 600,
            verification_percent: 20,
            repair_percent: 10,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FairSelection {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub class: WorkClass,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FairRound {
    pub source_digest: String,
    pub expected_generation: u64,
    pub next_state: FairnessState,
    pub selected: Vec<FairSelection>,
    pub execution_granted: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FairnessError {
    #[error("invalid or unbounded fairness policy, state, task metadata or observation")]
    Invalid,
    #[error("fairness generation or credit overflow")]
    Overflow,
    #[error(transparent)]
    Scheduler(#[from] SchedulerError),
    #[error("could not bind the scheduling read set")]
    Digest,
}

fn finish_mode(queue: &ProjectQueue, was_finishing: bool) -> bool {
    let p = &queue.policy;
    let (Knowledge::Known(red), Knowledge::Known(no_accept)) =
        (&queue.red_train_age_secs, &queue.since_last_accept_secs)
    else {
        return true;
    };
    if was_finishing {
        *red > p.red_exit_secs || *no_accept > p.no_accept_exit_secs
    } else {
        *red >= p.red_enter_secs
            || (queue.active_feature_wip >= p.finish_min_wip
                && *no_accept >= p.no_accept_enter_secs)
    }
}

fn critical_paths(snapshot: &SchedulerSnapshot) -> BTreeMap<TaskId, usize> {
    // Snapshot has already passed cycle/missing-dependency validation.
    let mut heights: BTreeMap<_, _> = snapshot
        .tasks
        .iter()
        .map(|t| (t.contract.task_id.clone(), 1))
        .collect();
    let mut indegrees: BTreeMap<_, _> = snapshot
        .tasks
        .iter()
        .map(|t| (t.contract.task_id.clone(), t.contract.dependencies.len()))
        .collect();
    let mut children: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for task in &snapshot.tasks {
        for parent in &task.contract.dependencies {
            children
                .entry(parent.clone())
                .or_default()
                .push(task.contract.task_id.clone());
        }
    }
    let mut ready: BTreeSet<_> = indegrees
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut order = vec![];
    while let Some(id) = ready.pop_first() {
        if let Some(dependents) = children.get(&id) {
            for child in dependents {
                if let Some(n) = indegrees.get_mut(child) {
                    *n -= 1;
                    if *n == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
        order.push(id);
    }
    for id in order.into_iter().rev() {
        if let Some(dependents) = children.get(&id) {
            let height = 1 + dependents.iter().map(|c| heights[c]).max().unwrap_or(0);
            heights.insert(id, height);
        }
    }
    heights
}

/// Every read participates in the digest, including clock, permits, queues and
/// previous credits. Persist the returned state with the admission transaction
/// only after rechecking this read set and expected generation. This API cannot
/// consume a permit, claim a task, or authenticate caller-provided observations.
pub fn propose_fair_round(
    queues: &[ProjectQueue],
    state: &FairnessState,
    policy: &FairnessPolicy,
    inference_slots: Knowledge<u16>,
    compute_slots: Knowledge<u16>,
    now: u64,
) -> Result<FairRound, FairnessError> {
    if queues.len() > 64
        || state.deficits.len() > 64
        || state.finish_mode.len() > 64
        || policy.starvation_secs == 0
        || policy.verification_percent as u16 + policy.repair_percent as u16 > 100
        || state
            .deficits
            .values()
            .any(|v| v.unsigned_abs() > 1_000_000_000)
        || queues.iter().map(|q| q.snapshot.tasks.len()).sum::<usize>() > 1000
        || bounded_json_size(&(queues, state, policy), 8 * 1024 * 1024).is_err()
    {
        return Err(FairnessError::Invalid);
    }
    let source_digest =
        content_digest(&(queues, state, policy, &inference_slots, &compute_slots, now))
            .map_err(|_| FairnessError::Digest)?;
    let capacity = match (inference_slots, compute_slots) {
        (Knowledge::Known(a), Knowledge::Known(b)) => a.min(b).min(1000),
        _ => 0,
    };
    let mut next = FairnessState {
        generation: state
            .generation
            .checked_add(1)
            .ok_or(FairnessError::Overflow)?,
        ..FairnessState::default()
    };
    let mut ready = BTreeMap::new();
    let mut by_id = BTreeMap::new();
    for queue in queues {
        let id = &queue.snapshot.scope.project_id;
        let p = &queue.policy;
        if by_id.insert(id.clone(), queue).is_some()
            || p.weight == 0
            || p.weight > 1000
            || p.bootstrap_wip_limit == 0
            || p.bootstrap_wip_limit > p.feature_wip_limit
            || p.finish_min_wip == 0
            || p.red_enter_secs == 0
            || p.red_exit_secs >= p.red_enter_secs
            || p.no_accept_enter_secs == 0
            || p.no_accept_exit_secs >= p.no_accept_enter_secs
            || queue.tasks.len() != queue.snapshot.tasks.len()
            || queue.snapshot.tasks.iter().any(|t| {
                queue
                    .tasks
                    .get(&t.contract.task_id)
                    .is_none_or(|m| m.queued_at > now)
            })
        {
            return Err(FairnessError::Invalid);
        }
        // Validate and retain unknown states, but rank before allocating a
        // project's finite slots: lexical input order must not consume them.
        let mut snapshot = queue.snapshot.clone();
        let project_slots = match (&snapshot.available_quota, &snapshot.available_capacity) {
            (Knowledge::Known(a), Knowledge::Known(b)) => (*a).min(*b) as usize,
            _ => 0,
        };
        if matches!(snapshot.available_quota, Knowledge::Known(_)) {
            snapshot.available_quota = Knowledge::Known(u16::MAX);
        }
        if matches!(snapshot.available_capacity, Knowledge::Known(_)) {
            snapshot.available_capacity = Knowledge::Known(u16::MAX);
        }
        let proposal = propose_ready_tasks(&snapshot, &snapshot.scope)?;
        if finish_mode(queue, state.finish_mode.contains(id)) {
            next.finish_mode.insert(id.clone());
        }
        next.deficits
            .insert(id.clone(), state.deficits.get(id).copied().unwrap_or(0));
        let heights = critical_paths(&queue.snapshot);
        let limit = if queue.throughput_known {
            p.feature_wip_limit
        } else {
            p.bootstrap_wip_limit
        };
        let mut features = limit.saturating_sub(queue.active_feature_wip) as usize;
        let mut tasks = proposal.ready;
        tasks.sort_by_key(|t| {
            (
                queue.tasks[t].class,
                Reverse(heights[t]),
                queue.tasks[t].queued_at,
                t.clone(),
            )
        });
        tasks.retain(|t| {
            if queue.tasks[t].class != WorkClass::Feature {
                return true;
            }
            if next.finish_mode.contains(id) || features == 0 {
                return false;
            }
            features -= 1;
            true
        });
        tasks.truncate(project_slots);
        ready.insert(id.clone(), tasks);
    }
    let mut verify_reserve = capacity as usize * policy.verification_percent as usize / 100;
    let mut repair_reserve = capacity as usize * policy.repair_percent as usize / 100;
    let mut selected = vec![];
    for slot in 0..capacity as usize {
        let remaining = capacity as usize - slot;
        let eligible: Vec<_> = ready
            .iter()
            .filter(|(id, tasks)| {
                tasks.first().is_some_and(|t| {
                    by_id[*id].tasks[t].class != WorkClass::Feature
                        || remaining > verify_reserve + repair_reserve
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        if eligible.is_empty() {
            break;
        }
        let total: i64 = eligible
            .iter()
            .map(|id| by_id[id].policy.weight as i64)
            .sum();
        for id in &eligible {
            let credit = next.deficits.get_mut(id).ok_or(FairnessError::Invalid)?;
            *credit = credit
                .checked_add(by_id[id].policy.weight as i64)
                .ok_or(FairnessError::Overflow)?;
        }
        let winner = eligible
            .into_iter()
            .max_by_key(|id| {
                let age = now - by_id[id].tasks[&ready[id][0]].queued_at;
                (
                    age / policy.starvation_secs,
                    next.deficits[id],
                    Reverse(id.clone()),
                )
            })
            .ok_or(FairnessError::Invalid)?;
        let task = ready
            .get_mut(&winner)
            .ok_or(FairnessError::Invalid)?
            .remove(0);
        let class = by_id[&winner].tasks[&task].class;
        if class == WorkClass::Verification {
            verify_reserve = verify_reserve.saturating_sub(1);
        }
        if class == WorkClass::Repair {
            repair_reserve = repair_reserve.saturating_sub(1);
        }
        *next
            .deficits
            .get_mut(&winner)
            .ok_or(FairnessError::Invalid)? -= total;
        selected.push(FairSelection {
            project_id: winner,
            task_id: task,
            class,
        });
    }
    Ok(FairRound {
        source_digest,
        expected_generation: state.generation,
        next_state: next,
        selected,
        execution_granted: false,
    })
}
