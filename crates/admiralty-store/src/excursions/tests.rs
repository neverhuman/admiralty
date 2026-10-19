use super::*;
use crate::{Store, StoreAuthorities};
use admiralty_core::{ProjectId, RootId, TaskId, excursions::IncidentEvent};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn scope() -> ExcursionScope {
    ExcursionScope {
        project: ProjectId::new("p").expect("project"),
        root: RootId::new("r").expect("root"),
        task: TaskId::new("t").expect("task"),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
    }
}
struct Policy {
    active: AtomicBool,
    writes: bool,
}
impl ExcursionAuthority for Policy {
    fn authorize(
        &self,
        s: &ExcursionScope,
        action: ExcursionAction,
        _: Option<&ExcursionCommand>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        self.active.load(Ordering::SeqCst)
            && s == &scope()
            && proof.downcast_ref::<bool>() == Some(&true)
            && (self.writes || action == ExcursionAction::Read)
    }
}
fn policy(writes: bool) -> Arc<Policy> {
    Arc::new(Policy {
        active: AtomicBool::new(true),
        writes,
    })
}
fn proof() -> ExcursionProof {
    Arc::new(true)
}
fn command(id: &str, revision: u32, incidents: Vec<ScopedIncident>) -> ExcursionCommand {
    ExcursionCommand {
        scope: scope(),
        command_id: id.into(),
        expected_revision: revision,
        operation: ExcursionOperation::Observe { incidents },
    }
}
fn incident(id: &str, parent: Option<&str>) -> ScopedIncident {
    ScopedIncident {
        scope: scope(),
        event: IncidentEvent {
            event_id: id.into(),
            family_signature: "storage-timeout".into(),
            cause_parent: parent.map(str::to_owned),
            kind: if parent.is_some() {
                "escalation"
            } else {
                "failure"
            }
            .into(),
            evidence_digests: vec![],
            observed_tick: None,
        },
    }
}
fn reproduction() -> ReproductionContract {
    ReproductionContract {
        fields: BTreeMap::from([
            ("scope".into(), "p/r/t".into()),
            ("steps".into(), "Replay retained evidence".into()),
            (
                "expected".into(),
                "One incident for explicit lineage".into(),
            ),
            ("actual".into(), "Failure then escalation".into()),
            (
                "tried_approaches".into(),
                "Do not increase the timeout; retry failed".into(),
            ),
            (
                "candidate_fix".into(),
                "Inspect bounded writer contention".into(),
            ),
            (
                "measurable_outcome".into(),
                "No duplicate episodes for a replay".into(),
            ),
        ]),
    }
}
fn private_dir() -> tempfile::TempDir {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private scratch")
    }
    #[cfg(not(unix))]
    {
        tempfile::tempdir().expect("scratch")
    }
}
fn open(path: &std::path::Path, p: Arc<Policy>) -> Store {
    Store::open_with_startup_authorities(
        path,
        StoreAuthorities {
            excursions: Some(p),
            ..Default::default()
        },
    )
    .expect("store")
}

#[test]
fn durable_restart_replay_lineage_recurrence_and_sanitized_proposal() {
    let dir = private_dir();
    let path = dir.path().join("incidents.db");
    let p = policy(true);
    let store = open(&path, p.clone());
    let first = command(
        "first",
        0,
        vec![
            incident("failure", None),
            incident("escalation", Some("failure")),
        ],
    );
    let original = store
        .record_excursion(first.clone(), proof())
        .expect("initial");
    let before = store.read_excursions(scope(), proof()).expect("read");
    assert_eq!(before.state.as_ref().expect("state").episodes.len(), 1);
    let proposal = ExcursionCommand {
        scope: scope(),
        command_id: "proposal".into(),
        expected_revision: 1,
        operation: ExcursionOperation::Propose {
            episode_id: before.state.expect("state").episodes[0].episode_id.clone(),
            reproduction: reproduction(),
        },
    };
    store
        .record_excursion(proposal.clone(), proof())
        .expect("proposal");
    store
        .record_excursion(
            command("recurrence", 2, vec![incident("recurrence", None)]),
            proof(),
        )
        .expect("recurrence");
    let expected = store.read_excursions(scope(), proof()).expect("snapshot");
    assert_eq!(expected.state.as_ref().expect("state").episodes.len(), 2);
    let local = &expected.proposals[0].proposal;
    assert!(!local.execution_granted && !local.external_publication_allowed);
    assert_eq!(local.proposed_investigator_max_model_turns, 1);
    assert_eq!(
        local.reproduction_contract.fields["tried_approaches"],
        "Do not increase the timeout; retry failed"
    );
    assert!(local.uncertainty.contains("unknown"));
    drop(store);
    let store = open(&path, p);
    assert_eq!(
        store.read_excursions(scope(), proof()).expect("restart"),
        expected
    );
    let retry = store
        .record_excursion(first, proof())
        .expect("lost reply retry");
    assert!(retry.replayed);
    assert_eq!(retry.receipt, original.receipt);
    assert!(
        store
            .record_excursion(proposal, proof())
            .expect("proposal retry")
            .replayed
    );
    assert_eq!(
        store.read_excursions(scope(), proof()).expect("unchanged"),
        expected
    );
}

#[test]
fn default_deny_read_only_foreign_scope_and_revoked_retry() {
    let dir = private_dir();
    let path = dir.path().join("incidents.db");
    let store = Store::open(&path).expect("no authority");
    let c = command("first", 0, vec![incident("failure", None)]);
    assert!(store.record_excursion(c.clone(), proof()).is_err());
    assert!(store.read_excursions(scope(), proof()).is_err());
    drop(store);
    let p = policy(false);
    let store = open(&path, p);
    assert_eq!(
        store
            .read_excursions(scope(), proof())
            .expect("read grant")
            .revision,
        0
    );
    assert!(store.record_excursion(c.clone(), proof()).is_err());
    drop(store);
    let p = policy(true);
    let store = open(&path, p.clone());
    store.record_excursion(c.clone(), proof()).expect("write");
    let mut foreign = scope();
    foreign.root = RootId::new("foreign").expect("root");
    assert!(store.read_excursions(foreign, proof()).is_err());
    assert!(store.read_excursions(scope(), Arc::new(false)).is_err());
    p.active.store(false, Ordering::SeqCst);
    assert!(store.record_excursion(c, proof()).is_err());
    assert!(store.read_excursions(scope(), proof()).is_err());
}

#[test]
fn stale_colliding_secret_and_foreign_events_leave_history_unchanged() {
    let mut c = Connection::open_in_memory().expect("db");
    initialize(&c).expect("schema");
    let p = policy(true);
    let first = command("first", 0, vec![incident("failure", None)]);
    record(&mut c, &first, Some(p.as_ref()), proof().as_ref()).expect("first");
    let snapshot = read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref()).expect("snapshot");
    assert!(matches!(
        record(
            &mut c,
            &command("stale", 0, vec![incident("other", None)]),
            Some(p.as_ref()),
            proof().as_ref()
        ),
        Err(ExcursionStoreError::Stale)
    ));
    let mut collision = first;
    collision.expected_revision = 1;
    assert!(matches!(
        record(&mut c, &collision, Some(p.as_ref()), proof().as_ref()),
        Err(ExcursionStoreError::Collision)
    ));
    let mut fields = reproduction();
    fields
        .fields
        .insert("actual".into(), "token=do-not-copy".into());
    let proposal = ExcursionCommand {
        scope: scope(),
        command_id: "unsafe".into(),
        expected_revision: 1,
        operation: ExcursionOperation::Propose {
            episode_id: snapshot.state.as_ref().expect("state").episodes[0]
                .episode_id
                .clone(),
            reproduction: fields,
        },
    };
    assert!(record(&mut c, &proposal, Some(p.as_ref()), proof().as_ref()).is_err());
    let mut foreign = incident("foreign", None);
    foreign.scope.task = TaskId::new("elsewhere").expect("task");
    assert!(
        record(
            &mut c,
            &command("foreign", 1, vec![foreign]),
            Some(p.as_ref()),
            proof().as_ref()
        )
        .is_err()
    );
    assert_eq!(
        read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref()).expect("after"),
        snapshot
    );
}

#[test]
fn late_authority_or_sql_failure_rolls_back_complete_command() {
    struct RevokeOnCommit(AtomicUsize);
    impl ExcursionAuthority for RevokeOnCommit {
        fn authorize(
            &self,
            _: &ExcursionScope,
            _: ExcursionAction,
            _: Option<&ExcursionCommand>,
            _: &(dyn Any + Send + Sync),
        ) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst) == 0
        }
    }
    let mut c = Connection::open_in_memory().expect("db");
    initialize(&c).expect("schema");
    let p = policy(true);
    let first = command("first", 0, vec![incident("failure", None)]);
    assert!(matches!(
        record(
            &mut c,
            &first,
            Some(&RevokeOnCommit(AtomicUsize::new(0))),
            proof().as_ref()
        ),
        Err(ExcursionStoreError::Denied)
    ));
    assert_eq!(
        read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref())
            .expect("empty")
            .revision,
        0
    );
    c.execute_batch("CREATE TRIGGER fail_incident AFTER INSERT ON excursion_commands BEGIN SELECT RAISE(ABORT,'late SQL fault'); END;").expect("fault");
    assert!(record(&mut c, &first, Some(p.as_ref()), proof().as_ref()).is_err());
    assert_eq!(
        read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref())
            .expect("rollback")
            .revision,
        0
    );
    c.execute_batch("DROP TRIGGER fail_incident")
        .expect("remove owned fault");
    record(&mut c, &first, Some(p.as_ref()), proof().as_ref()).expect("same retry");
}

#[test]
fn receipt_or_identity_corruption_is_rejected_before_reuse() {
    for corruption in [
        "UPDATE excursion_commands SET receipt_json='{}'",
        "UPDATE excursion_commands SET command_id='forged'",
    ] {
        let mut c = Connection::open_in_memory().expect("db");
        initialize(&c).expect("schema");
        let p = policy(true);
        let first = command("first", 0, vec![incident("failure", None)]);
        record(&mut c, &first, Some(p.as_ref()), proof().as_ref()).expect("first");
        c.execute_batch(corruption).expect("inject");
        assert!(read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref()).is_err());
        assert!(record(&mut c, &first, Some(p.as_ref()), proof().as_ref()).is_err());
    }
}

#[test]
fn proposal_capacity_preserves_history_and_allows_exact_retry() {
    let mut c = Connection::open_in_memory().expect("db");
    initialize(&c).expect("schema");
    let p = policy(true);
    record(
        &mut c,
        &command("first", 0, vec![incident("failure", None)]),
        Some(p.as_ref()),
        proof().as_ref(),
    )
    .expect("first");
    let id = read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref())
        .expect("read")
        .state
        .expect("state")
        .episodes[0]
        .episode_id
        .clone();
    let mut last = None;
    for i in 0..MAX_PROPOSALS {
        let proposal = ExcursionCommand {
            scope: scope(),
            command_id: format!("proposal-{i}"),
            expected_revision: i as u32 + 1,
            operation: ExcursionOperation::Propose {
                episode_id: id.clone(),
                reproduction: reproduction(),
            },
        };
        record(&mut c, &proposal, Some(p.as_ref()), proof().as_ref()).expect("bounded proposal");
        last = Some(proposal);
    }
    let last = last.expect("last");
    assert!(
        record(&mut c, &last, Some(p.as_ref()), proof().as_ref())
            .expect("retry at capacity")
            .replayed
    );
    let mut over = last;
    over.command_id = "overflow".into();
    over.expected_revision += 1;
    assert!(matches!(
        record(&mut c, &over, Some(p.as_ref()), proof().as_ref()),
        Err(ExcursionStoreError::Capacity)
    ));
    assert_eq!(
        read(&mut c, &scope(), Some(p.as_ref()), proof().as_ref())
            .expect("retained")
            .proposals
            .len(),
        MAX_PROPOSALS
    );
}
