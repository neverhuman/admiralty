use admiralty_core::{
    DomainEvent, ProjectId, SourceIdentity, TaskContract, content_digest,
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, StoreError, workplans::RegisterWorkplanRequest};
use std::{
    sync::{Arc, Barrier},
    thread,
};

fn private_root() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private root")
}

fn task(title: &str) -> TaskContract {
    serde_json::from_value(serde_json::json!({
        "schema_version":1,"project_id":"project","root_id":"root","task_id":"task",
        "revision":1,"title":title,"objective":"Keep registration distinct from execution",
        "non_goals":[],"owned_paths":["src"],"forbidden_paths":[],"read_first":[],
        "dependencies":[],"criteria":[{"id":"c","text":"Check passes","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":30}],
        "authority_ref":"fixture","max_attempts":1,"simulated":true
    }))
    .expect("fixture contract")
}

fn request(revision: u32, title: &str) -> RegisterWorkplanRequest {
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision,
        project_id: ProjectId::new("project").expect("id"),
        tasks: vec![task(title)],
    };
    let source = serde_json::to_vec(&plan).expect("source");
    RegisterWorkplanRequest {
        command_id: format!("register-{revision}"),
        project_id: plan.project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: (revision - 1).to_string(),
        source_digest: preview_workplan(&source).expect("preview").source_digest,
        canonical_digest: content_digest(&plan).expect("digest"),
        source,
    }
}

fn submit(
    store: &Store,
    contract: TaskContract,
) -> admiralty_store::Result<admiralty_store::AppendReceipt> {
    store.append(
        SourceIdentity {
            producer_id: "fixture".into(),
            incarnation: "one".into(),
            sequence: "1".into(),
        },
        DomainEvent::TaskSubmitted { contract },
        "2026-10-18T13:30:00Z".into(),
    )
}

#[test]
fn writer_registry_survives_backup_reopen_and_old_retry_without_events_or_wakes() {
    let root = private_root();
    let database = root.path().join("work.sqlite");
    let backup = root.path().join("backup.sqlite");
    let store = Store::open(&database).expect("store");
    let changes = store.subscribe_changes();
    let before = store.snapshot().expect("snapshot");
    let first = request(1, "Original");
    let receipt = store.register_workplan(first.clone()).expect("register");
    store
        .register_workplan(request(2, "Original"))
        .expect("advance");
    assert_eq!(
        receipt,
        store
            .register_workplan(first.clone())
            .expect("historical retry")
    );
    assert_eq!(store.snapshot().expect("snapshot"), before);
    assert!(!changes.has_changed().expect("watch"));
    assert!(matches!(
        Store::open(&database),
        Err(StoreError::AlreadyOwned)
    ));
    store.backup(&backup).expect("backup");
    drop(store);
    for path in [&database, &backup] {
        let reopened = Store::open(path).expect("reopen");
        let current = reopened
            .workplan(ProjectId::new("project").expect("id"), "plan".into(), None)
            .expect("read")
            .expect("plan");
        assert_eq!(current.plan.revision, 2);
        assert_eq!(
            reopened.register_workplan(first.clone()).expect("replay"),
            receipt
        );
        assert_eq!(
            reopened
                .snapshot()
                .expect("snapshot")
                .projection
                .tasks
                .len(),
            0
        );
    }
}

#[test]
fn domain_and_registry_cannot_rebind_task_identity_in_either_order() {
    let root = private_root();
    let store = Store::open(root.path().join("registry-first.sqlite")).expect("store");
    store
        .register_workplan(request(1, "Original"))
        .expect("register");
    assert!(submit(&store, task("Changed")).is_err());
    assert!(
        store
            .snapshot()
            .expect("snapshot")
            .projection
            .tasks
            .is_empty()
    );
    submit(&store, task("Original")).expect("matching task is allowed");
    let store = Store::open(root.path().join("domain-first.sqlite")).expect("store");
    submit(&store, task("Original")).expect("submit");
    let before = store.snapshot().expect("snapshot");
    assert!(store.register_workplan(request(1, "Changed")).is_err());
    assert!(
        store
            .workplans(ProjectId::new("project").expect("id"), None, 10)
            .expect("list")
            .entries
            .is_empty()
    );
    assert_eq!(store.snapshot().expect("snapshot"), before);
    store
        .register_workplan(request(1, "Original"))
        .expect("matching registry is allowed");
}

#[test]
fn cloned_writers_serialize_competing_revision_commands_and_preserve_winner() {
    let root = private_root();
    let store = Store::open(root.path().join("work.sqlite")).expect("store");
    store
        .register_workplan(request(1, "Original"))
        .expect("register");
    let barrier = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|index| {
            let store = store.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut candidate = request(2, "Original");
                candidate.command_id = format!("race-{index}");
                barrier.wait();
                store.register_workplan(candidate)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("join"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let current = store
        .workplan(ProjectId::new("project").expect("id"), "plan".into(), None)
        .expect("read")
        .expect("plan");
    assert_eq!(current.plan.revision, 2);
    assert_eq!(
        current.receipt,
        results.into_iter().find_map(Result::ok).expect("winner")
    );
}

#[test]
fn rebuild_rejects_registry_tampering_before_changing_cursor_or_projection() {
    let root = private_root();
    let database = root.path().join("work.sqlite");
    let store = Store::open(&database).expect("store");
    store
        .register_workplan(request(1, "Original"))
        .expect("register");
    submit(&store, task("Original")).expect("submit");
    let before = store.snapshot().expect("snapshot");
    let changes = store.subscribe_changes();
    // Explicit corruption fixture, never an application access path.
    let attacker = rusqlite::Connection::open(&database).expect("fixture connection");
    let original: String = attacker
        .query_row(
            "SELECT contract_json FROM workplan_registry_tasks WHERE task_id='task'",
            [],
            |row| row.get(0),
        )
        .expect("retained contract");
    attacker
        .execute(
            "UPDATE workplan_registry_tasks SET contract_json='{}' WHERE task_id='task'",
            [],
        )
        .expect("corrupt fixture");
    assert!(store.rebuild().is_err());
    assert_eq!(store.snapshot().expect("unchanged snapshot"), before);
    assert!(!changes.has_changed().expect("watch"));
    drop(store);
    assert!(Store::open(&database).is_err());
    attacker
        .execute(
            "UPDATE workplan_registry_tasks SET contract_json=?1 WHERE task_id='task'",
            [original],
        )
        .expect("repair fixture");
    drop(attacker);
    let reopened = Store::open(&database).expect("verified reopen");
    let rebuilt = reopened.rebuild().expect("verified rebuild");
    assert_eq!(rebuilt.projection, before.projection);
    assert_ne!(
        rebuilt.cursor.recovery_generation,
        before.cursor.recovery_generation
    );
}
