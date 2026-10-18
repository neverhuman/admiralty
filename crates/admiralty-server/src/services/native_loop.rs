//! First service-local loop seam: persist an actually observed native start.
//! This is not a dispatcher, admission authority, recovery launcher or activation
//! route. Only trusted startup may compose it with `NativeExecutor`.

use super::{
    controller::DispatchIntent,
    native_executor::{StartedSink, StartedSinkError},
};
use admiralty_core::{
    AttemptId, DomainEvent, PrincipalId, ProjectId, RootId, SchedulingState, SourceIdentity,
    TaskId, content_digest, nodes::NodeId,
};
use admiralty_runtime::resource_gate::RecoveryIdentity;
use admiralty_store::Store;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_STARTED_BINDINGS: usize = 1024;

/// Immutable trusted startup scope, not a wire format or an execution grant.
#[derive(Clone)]
pub struct StartedScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub node_id: NodeId,
    /// Keep both namespace fields stable across a retry/restart of this service.
    pub source_producer: String,
    pub source_incarnation: String,
}

/// Input and retained run bindings are bounded. Store calls use its existing
/// serialized writer/snapshot API; they do not create an independent writer or
/// promise a wall-clock deadline. Never call this from a Store authority callback.
pub struct StoreStartedSink {
    store: Store,
    scope: StartedScope,
    limit: usize,
    bindings: Mutex<BTreeMap<String, String>>,
}

impl StoreStartedSink {
    pub fn new(store: Store, scope: StartedScope, limit: usize) -> Result<Self, StartedSinkError> {
        if !(1..=MAX_STARTED_BINDINGS).contains(&limit)
            || !bounded(&scope.source_producer)
            || !bounded(&scope.source_incarnation)
        {
            return Err(StartedSinkError::InvalidBinding);
        }
        Ok(Self {
            store,
            scope,
            limit,
            bindings: Mutex::new(BTreeMap::new()),
        })
    }

    fn binding(
        &self,
        intent: &DispatchIntent,
        recovery: &RecoveryIdentity,
    ) -> Result<(TaskId, AttemptId, String), StartedSinkError> {
        if intent.schema_version != 1
            || intent.contract_revision == 0
            || ![
                &intent.task_id,
                &intent.attempt_id,
                &intent.owner,
                &intent.run_id,
                &intent.provider,
                &intent.model,
                &recovery.unit,
                &recovery.permit_id,
                &recovery.reservation_ref,
                &recovery.host_incarnation,
                &recovery.identity.lease_id,
            ]
            .into_iter()
            .all(|s| bounded(s))
            || intent.run_id.len() > 128
            || !intent
                .run_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || ![
                &intent.contract_digest,
                &intent.context_digest,
                &intent.policy_digest,
                &recovery.launch_spec_digest,
                &recovery.worker_environment_digest,
                &recovery.backend_config_digest,
            ]
            .into_iter()
            .all(|s| digest(s))
            || recovery.main_pid.is_none_or(|pid| pid <= 1)
            || recovery.invocation_id.as_ref().is_none_or(|s| !bounded(s))
            || recovery.process_start.as_ref().is_none_or(|s| !bounded(s))
            || recovery.run_id != intent.run_id
            || recovery.identity.project_id != self.scope.project_id
            || recovery.identity.node_id != self.scope.node_id
        {
            return Err(StartedSinkError::InvalidBinding);
        }
        let task_id =
            TaskId::new(intent.task_id.clone()).map_err(|_| StartedSinkError::InvalidBinding)?;
        let attempt_id = AttemptId::new(intent.attempt_id.clone())
            .map_err(|_| StartedSinkError::InvalidBinding)?;
        let owner =
            PrincipalId::new(intent.owner.clone()).map_err(|_| StartedSinkError::InvalidBinding)?;
        if recovery.identity.task_id != task_id || recovery.identity.attempt_id != attempt_id {
            return Err(StartedSinkError::InvalidBinding);
        }
        let snapshot = self
            .store
            .snapshot()
            .map_err(|_| StartedSinkError::Unavailable)?;
        let task = snapshot
            .projection
            .tasks
            .get(&task_id)
            .ok_or(StartedSinkError::InvalidBinding)?;
        if task.contract.simulated
            || task.contract.project_id != self.scope.project_id
            || task.contract.root_id != self.scope.root_id
            || task.contract.revision != intent.contract_revision
            || task.attempt_id.as_ref() != Some(&attempt_id)
            || task.owner.as_ref() != Some(&owner)
            || !matches!(
                task.state,
                SchedulingState::Claimed | SchedulingState::Running
            )
            || content_digest(&task.contract).map_err(|_| StartedSinkError::InvalidBinding)?
                != intent.contract_digest
        {
            return Err(StartedSinkError::InvalidBinding);
        }
        // Claimed contract identity/owner cannot be revised. Store rechecks the
        // exact current attempt in its atomic reducer, and rejects racing changes.
        let binding = content_digest(&serde_json::json!({
            "schema_version": 1,
            "source_namespace": self.scope.source_incarnation,
            "root_id": self.scope.root_id,
            "intent": {"schema_version":intent.schema_version,"task_id":intent.task_id,
                "contract_revision":intent.contract_revision,"contract_digest":intent.contract_digest,
                "attempt_id":intent.attempt_id,"owner":intent.owner,"run_id":intent.run_id,
                "context_digest":intent.context_digest,"policy_digest":intent.policy_digest,
                "provider":intent.provider,"model":intent.model},
            "recovery": recovery,
        })).map_err(|_| StartedSinkError::InvalidBinding)?;
        Ok((task_id, attempt_id, binding))
    }
}

impl StartedSink for StoreStartedSink {
    fn record_started(
        &self,
        intent: &DispatchIntent,
        recovery: &RecoveryIdentity,
    ) -> Result<(), StartedSinkError> {
        let (task_id, attempt_id, binding) = self.binding(intent, recovery)?;
        let mut bindings = self
            .bindings
            .lock()
            .map_err(|_| StartedSinkError::Unavailable)?;
        match bindings.get(&intent.run_id) {
            Some(original) if original != &binding => return Err(StartedSinkError::InvalidBinding),
            None if bindings.len() >= self.limit => return Err(StartedSinkError::Capacity),
            _ => {}
        }
        // Retain even on an uncertain Store reply. Only this exact binding can
        // be retried in-process; never evict an old run to admit a conflicting one.
        bindings.insert(intent.run_id.clone(), binding.clone());
        self.store
            .append(
                SourceIdentity {
                    producer_id: self.scope.source_producer.clone(),
                    incarnation: binding,
                    sequence: "1".into(),
                },
                DomainEvent::TaskStarted {
                    task_id,
                    attempt_id,
                },
                observed_utc()?,
            )
            .map_err(|_| StartedSinkError::Unavailable)?;
        Ok(())
    }
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn observed_utc() -> Result<String, StartedSinkError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StartedSinkError::Unavailable)?;
    let seconds = duration.as_secs();
    // Bound arithmetic and refuse clocks outside representable RFC3339 years.
    if seconds > 253_402_300_799 {
        return Err(StartedSinkError::Unavailable);
    }
    let shifted = (seconds / 86_400) as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let sod = seconds % 86_400;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:09}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        duration.subsec_nanos()
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use admiralty_core::{AcceptanceCriterion, CheckSpec, TaskContract};
    use admiralty_runtime::resource_gate::AdmissionIdentity;
    use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};

    fn directory() -> PathBuf {
        let parent = fs::canonicalize(std::env::temp_dir()).expect("canonical private temp");
        let root = parent.join(format!("native-started-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("private Store parent");
        root
    }
    fn scope() -> StartedScope {
        StartedScope {
            project_id: ProjectId::new("project").expect("project"),
            root_id: RootId::new("root").expect("root"),
            node_id: NodeId::new("node").expect("node"),
            source_producer: "native-started-test".into(),
            source_incarnation: "stable-service-generation".into(),
        }
    }
    fn claimed(store: &Store, name: &str) -> (DispatchIntent, RecoveryIdentity) {
        let scope = scope();
        let task_id = TaskId::new(name).expect("task");
        let attempt_id = AttemptId::new(format!("{name}-attempt")).expect("attempt");
        let owner = PrincipalId::new("worker").expect("owner");
        let contract = TaskContract {
            schema_version: 1,
            project_id: scope.project_id.clone(),
            root_id: scope.root_id.clone(),
            task_id: task_id.clone(),
            revision: 1,
            title: "Observed start fixture".into(),
            objective: "Persist an observed start exactly once".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "c1".into(),
                text: "bounded".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "true".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 1,
            }],
            authority_ref: "fixture:scope".into(),
            max_attempts: 1,
            simulated: false,
        };
        let contract_digest = content_digest(&contract).expect("contract digest");
        let source = |sequence: &str| SourceIdentity {
            producer_id: "fixture".into(),
            incarnation: name.into(),
            sequence: sequence.into(),
        };
        store
            .append(
                source("1"),
                DomainEvent::TaskSubmitted { contract },
                "2026-10-18T00:00:00Z".into(),
            )
            .expect("submit");
        store
            .append(
                source("2"),
                DomainEvent::TaskClaimed {
                    task_id: task_id.clone(),
                    revision: 1,
                    attempt_id: attempt_id.clone(),
                    owner: owner.clone(),
                },
                "2026-10-18T00:00:01Z".into(),
            )
            .expect("claim");
        let hash = || format!("sha256:{}", "a".repeat(64));
        let intent = DispatchIntent {
            schema_version: 1,
            task_id: name.into(),
            contract_revision: 1,
            contract_digest,
            attempt_id: attempt_id.to_string(),
            owner: owner.to_string(),
            run_id: format!("{name}-run"),
            context_digest: hash(),
            policy_digest: hash(),
            provider: "declared-provider".into(),
            model: "declared-model".into(),
        };
        let recovery = RecoveryIdentity {
            identity: AdmissionIdentity {
                project_id: scope.project_id,
                task_id,
                attempt_id,
                node_id: scope.node_id,
                lease_id: "lease".into(),
            },
            launch_spec_digest: hash(),
            worker_environment_digest: hash(),
            unit: "actual-unit.service".into(),
            run_id: intent.run_id.clone(),
            permit_id: "permit".into(),
            reservation_ref: "reservation".into(),
            host_incarnation: "actual-boot".into(),
            backend_config_digest: hash(),
            invocation_id: Some("actual-invocation".into()),
            main_pid: Some(1234),
            process_start: Some("actual-process-start".into()),
        };
        (intent, recovery)
    }

    #[test]
    fn exact_start_survives_lost_ack_and_store_reopen_without_duplicate_event() {
        let root = directory();
        let path = root.join("store.sqlite");
        let store = Store::open(&path).expect("Store");
        let (intent, recovery) = claimed(&store, "task");
        let sink = StoreStartedSink::new(store.clone(), scope(), 2).expect("sink");
        // Deliberately discard the success, as if persistence's reply was lost.
        sink.record_started(&intent, &recovery)
            .expect("actual durable TaskStarted");
        let first = store.snapshot().expect("first state");
        assert_eq!(first.cursor.event_seq, "3");
        assert_eq!(
            first
                .projection
                .tasks
                .get(&recovery.identity.task_id)
                .expect("task")
                .state,
            SchedulingState::Running
        );
        sink.record_started(&intent, &recovery)
            .expect("same source replay while Running");
        assert_eq!(store.snapshot().expect("unchanged state"), first);
        drop(sink);
        drop(store);
        let store = Store::open(&path).expect("reopened Store");
        let sink = StoreStartedSink::new(store.clone(), scope(), 2)
            .expect("new sink with stable namespace");
        sink.record_started(&intent, &recovery)
            .expect("durable exact replay after lost process-local ACK");
        assert_eq!(store.snapshot().expect("unchanged reopened state"), first);
        assert_eq!(
            store
                .events_after(0, 10)
                .expect("actual events")
                .iter()
                .filter(|r| matches!(r.event, DomainEvent::TaskStarted { .. }))
                .count(),
            1
        );
        drop(sink);
        drop(store);
        fs::remove_dir_all(root).expect("owned fixture cleanup");
    }

    #[test]
    fn intent_recovery_contract_and_observation_mismatches_never_start_the_task() {
        let root = directory();
        let store = Store::open(root.join("store.sqlite")).expect("Store");
        let (intent, recovery) = claimed(&store, "task");
        let sink = StoreStartedSink::new(store.clone(), scope(), 2).expect("sink");
        let before = store.snapshot().expect("claimed state");
        type Mutation = fn(&mut DispatchIntent, &mut RecoveryIdentity);
        let mutations: &[Mutation] = &[
            |i, _| i.schema_version = 2,
            |i, _| i.contract_revision += 1,
            |i, _| i.contract_digest = format!("sha256:{}", "b".repeat(64)),
            |i, _| i.owner = "other-owner".into(),
            |i, _| i.attempt_id = "other-attempt".into(),
            |i, _| i.task_id = "other-task".into(),
            |_, r| r.identity.project_id = ProjectId::new("other-project").expect("project"),
            |_, r| r.identity.node_id = NodeId::new("other-node").expect("node"),
            |_, r| r.run_id = "other-run".into(),
            |_, r| r.main_pid = None,
            |_, r| r.invocation_id = None,
            |_, r| r.process_start = Some(String::new()),
            |_, r| r.worker_environment_digest = "not-a-digest".into(),
        ];
        for mutate in mutations {
            let mut changed_intent = intent.clone();
            let mut changed_recovery = recovery.clone();
            mutate(&mut changed_intent, &mut changed_recovery);
            assert_eq!(
                sink.record_started(&changed_intent, &changed_recovery),
                Err(StartedSinkError::InvalidBinding)
            );
            assert_eq!(store.snapshot().expect("unchanged claimed state"), before);
        }
        assert!(sink.bindings.lock().expect("registry").is_empty());
        sink.record_started(&intent, &recovery)
            .expect("original actual binding still admitted");
        let after = store.snapshot().expect("running state");
        let mut changed = recovery.clone();
        changed.process_start = Some("replacement-process".into());
        assert_eq!(
            sink.record_started(&intent, &changed),
            Err(StartedSinkError::InvalidBinding)
        );
        assert_eq!(
            store.snapshot().expect("no duplicate from replacement"),
            after
        );
        drop(sink);
        drop(store);
        fs::remove_dir_all(root).expect("owned fixture cleanup");
    }

    #[test]
    fn bounded_registry_retains_first_binding_and_denies_new_run_without_writes() {
        let root = directory();
        let store = Store::open(root.join("store.sqlite")).expect("Store");
        let (intent, recovery) = claimed(&store, "task");
        let (other_intent, other_recovery) = claimed(&store, "task-two");
        let sink = StoreStartedSink::new(store.clone(), scope(), 1).expect("one retained binding");
        sink.record_started(&intent, &recovery)
            .expect("first start");
        let before = store.snapshot().expect("first running, second claimed");
        assert_eq!(
            sink.record_started(&other_intent, &other_recovery),
            Err(StartedSinkError::Capacity)
        );
        assert_eq!(store.snapshot().expect("no writes at capacity"), before);
        sink.record_started(&intent, &recovery)
            .expect("exact replay still works at capacity");
        assert_eq!(store.snapshot().expect("unchanged replay"), before);
        drop(sink);
        drop(store);
        fs::remove_dir_all(root).expect("owned fixture cleanup");
    }
}
