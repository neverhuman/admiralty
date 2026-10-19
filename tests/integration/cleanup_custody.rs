//! Disposable-root custody and registered local scratch executor qualification.

use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceError, WorkspaceSpec};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use admiralty_testkit::clock::{ClockError, FakeClock};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn private_fixture() -> tempfile::TempDir {
    // Resolve platform aliases such as macOS /var before creating the fixture:
    // the workspace registry deliberately rejects symlinked path components.
    let base = fs::canonicalize(std::env::temp_dir()).expect("canonical temporary base");
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-cleanup-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir_in(base)
        .expect("create owner-only disposable fixture")
}

fn private_directory(path: &Path) {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .expect("create private fixture directory");
}

fn binding() -> AssetBinding {
    AssetBinding {
        project_id: ProjectId::new("project-1").expect("valid fixture project"),
        root_id: RootId::new("root-1").expect("valid fixture root"),
        task_id: TaskId::new("task-1").expect("valid fixture task"),
        asset_id: "asset-1".into(),
        owner_id: PrincipalId::new("owner-1").expect("valid fixture owner"),
        sha256: format!("sha256:{}", "a".repeat(64)),
        generation: "1".into(),
    }
}

fn observation(binding: &AssetBinding, registered: bool, at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: (at - 10).to_string(),
        clock_id: "cleanup-fixture-clock".into(),
        observed_at: at.to_string(),
        decision_at: (at + 1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:cleanup-custody".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn workspace_registry_excludes_forbidden_foreign_and_symlinked_paths() {
    let fixture = private_fixture();
    let root = fs::canonicalize(fixture.path()).expect("canonical workspace fixture root");
    let source = root.join("src");
    let ignored = source.join("ignored");
    let foreign = root.join("foreign");
    private_directory(&ignored);
    private_directory(&foreign);
    fs::write(source.join("keep.txt"), b"owned bytes").expect("write owned fixture bytes");
    fs::write(ignored.join("keep.txt"), b"ignored bytes").expect("write ignored fixture bytes");
    fs::write(foreign.join("keep.txt"), b"foreign bytes").expect("write foreign fixture bytes");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&foreign, source.join("linked")).expect("create symlink fixture");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-1").expect("valid fixture node"),
        root_id: RootId::new("root-1").expect("valid fixture root"),
        project_id: ProjectId::new("project-1").expect("valid fixture project"),
        workspace: root,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/ignored")],
    };
    let validated = spec.validate().expect("validate disposable workspace");

    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(&PathBuf::from("src/keep.txt")));
    assert!(!validated.owns(&PathBuf::from("src/ignored/keep.txt")));
    assert!(!validated.owns(&PathBuf::from("foreign/keep.txt")));
    #[cfg(unix)]
    assert!(!validated.owns(&PathBuf::from("src/linked/keep.txt")));

    assert_eq!(
        fs::read(source.join("keep.txt")).expect("read owned fixture"),
        b"owned bytes"
    );
    assert_eq!(
        fs::read(ignored.join("keep.txt")).expect("read ignored fixture"),
        b"ignored bytes"
    );
    assert_eq!(
        fs::read(foreign.join("keep.txt")).expect("read foreign fixture"),
        b"foreign bytes"
    );
}

#[cfg(unix)]
#[test]
fn workspace_registry_rejects_symlink_root_without_changing_contents() {
    let fixture = private_fixture();
    let real_workspace = fixture.path().join("real-workspace");
    fs::create_dir(&real_workspace).expect("create real workspace fixture");
    let sentinel = real_workspace.join("keep.txt");
    fs::write(&sentinel, b"must remain unchanged").expect("write sentinel fixture");
    let alias = fixture.path().join("workspace-alias");
    std::os::unix::fs::symlink(&real_workspace, &alias).expect("create workspace symlink");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-symlink").expect("valid fixture node"),
        root_id: RootId::new("root-symlink").expect("valid fixture root"),
        project_id: ProjectId::new("project-symlink").expect("valid fixture project"),
        workspace: alias,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: Vec::new(),
    };

    assert!(matches!(
        spec.validate(),
        Err(WorkspaceError::WorkspaceContainsSymlink)
    ));
    assert_eq!(
        fs::read(sentinel).expect("read untouched sentinel fixture"),
        b"must remain unchanged"
    );
}

#[test]
fn durable_active_pin_blocks_retirement_claim_until_release() {
    let fixture = private_fixture();
    let path = fixture.path().join("custody.sqlite");
    let mut connection = Connection::open(path).expect("open fixture database");
    custody::initialize(&connection).expect("initialize custody schema");
    let binding = binding();

    custody::register_asset(
        &mut connection,
        RegisterAssetRequest {
            command_id: "register-1".into(),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation(&binding, false, 10),
        },
    )
    .expect("register fixture asset");
    let mut over_age_observation = observation(&binding, true, 11);
    over_age_observation.max_age = (custody::MAX_OBSERVATION_AGE + 1).to_string();
    assert!(matches!(
        custody::acquire_pin(
            &mut connection,
            AcquirePinRequest {
                command_id: "acquire-over-age-observation".into(),
                binding: binding.clone(),
                pin_id: "pin-over-age".into(),
                expected_revision: "1".into(),
                observation: over_age_observation,
            }
        ),
        Err(CustodyError::UnsafeObservation)
    ));

    assert!(matches!(
        custody::acquire_pin(
            &mut connection,
            AcquirePinRequest {
                command_id: "acquire-oversized-pin".into(),
                binding: binding.clone(),
                pin_id: "p".repeat(custody::MAX_ID_BYTES + 1),
                expected_revision: "1".into(),
                observation: observation(&binding, true, 11),
            }
        ),
        Err(CustodyError::Invalid("identifier bounds"))
    ));

    let mut logical_clock = FakeClock::new(11);
    assert_eq!(
        logical_clock.advance_to_ms(10),
        Err(ClockError::Backwards {
            now_ms: 11,
            target_ms: 10,
        })
    );
    assert_eq!(logical_clock.now_ms(), 11);

    let acquire_request = AcquirePinRequest {
        command_id: "acquire-1".into(),
        binding: binding.clone(),
        pin_id: "pin-1".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, logical_clock.now_ms()),
    };
    let acquired =
        custody::acquire_pin(&mut connection, acquire_request.clone()).expect("record active pin");
    let replayed = custody::acquire_pin(&mut connection, acquire_request)
        .expect("exact replay returns recorded receipt");
    assert_eq!(replayed, acquired);

    assert!(matches!(
        custody::release_pin(
            &mut connection,
            ReleasePinRequest {
                command_id: "release-missing".into(),
                binding: binding.clone(),
                pin_id: "missing-pin".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            }
        ),
        Err(CustodyError::InvalidPin)
    ));

    assert!(matches!(
        custody::release_pin(
            &mut connection,
            ReleasePinRequest {
                command_id: "release-missing".into(),
                binding: binding.clone(),
                pin_id: "missing-pin".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            }
        ),
        Err(CustodyError::InvalidPin)
    ));

    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "claim-stale-revision".into(),
                binding: binding.clone(),
                claim_id: "stale-claim".into(),
                expected_revision: "1".into(),
                observation: observation(&binding, true, 11),
            }
        ),
        Err(CustodyError::StaleRevision)
    ));

    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "claim-blocked".into(),
                binding: binding.clone(),
                claim_id: "claim-1".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            }
        ),
        Err(CustodyError::ActivePins)
    ));

    let mut foreign_binding = binding.clone();
    foreign_binding.root_id = RootId::new("foreign-root").expect("valid foreign root");
    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "claim-foreign".into(),
                binding: foreign_binding.clone(),
                claim_id: "foreign-claim".into(),
                expected_revision: "2".into(),
                observation: observation(&foreign_binding, true, 12),
            }
        ),
        Err(CustodyError::ForeignAsset)
    ));

    custody::release_pin(
        &mut connection,
        ReleasePinRequest {
            command_id: "release-1".into(),
            binding: binding.clone(),
            pin_id: "pin-1".into(),
            expected_revision: "2".into(),
            observation: observation(&binding, true, 12),
        },
    )
    .expect("release active pin");
    custody::claim_retirement(
        &mut connection,
        ClaimRetirementRequest {
            command_id: "claim-after-release".into(),
            binding: binding.clone(),
            claim_id: "claim-2".into(),
            expected_revision: "3".into(),
            observation: observation(&binding, true, 13),
        },
    )
    .expect("claim retirement after final pin release");
}

#[cfg(target_os = "linux")]
mod executor {
    use super::*;
    use admiralty_core::retirement::{AssetId, RetirementScope};
    use admiralty_runtime::workspace::cleanup::{CleanupExecutor, Preservation, Stage};
    use std::{
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };

    fn facts() -> Preservation {
        Preservation {
            dirty: Some(false),
            ignored_private: Some(false),
            unpushed: Some(false),
            unmerged: Some(false),
            process_held: Some(false),
            foreign: Some(false),
            backup_candidate_held: Some(false),
            unique_data: Some(false),
        }
    }
    fn scope() -> RetirementScope {
        let b = binding();
        RetirementScope {
            project: b.project_id,
            root: b.root_id,
            task: b.task_id,
            owner: b.owner_id,
            asset_id: AssetId::new(b.asset_id).expect("asset"),
        }
    }
    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        ledger: PathBuf,
        connection: Connection,
        binding: AssetBinding,
        executor: CleanupExecutor,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_processes(&[])
        }
        fn with_processes(process_ids: &[u32]) -> Self {
            let temp = private_fixture();
            let root = temp.path().join("scratch");
            let ledger = temp.path().join("evidence");
            private_directory(&root.join("asset"));
            private_directory(&ledger);
            fs::write(root.join("asset/data"), b"disposable bytes").expect("write scratch");
            fs::set_permissions(root.join("asset/data"), fs::Permissions::from_mode(0o600))
                .expect("private scratch");
            let executor = CleanupExecutor::open(&root, &ledger).expect("open executor");
            let registered = executor
                .register(scope(), "asset", process_ids)
                .expect("register explicit scratch");
            let mut b = binding();
            b.sha256 = registered.registration_digest;
            let mut connection =
                Connection::open(temp.path().join("custody.sqlite")).expect("database");
            custody::initialize(&connection).expect("schema");
            custody::register_asset(
                &mut connection,
                RegisterAssetRequest {
                    command_id: "register".into(),
                    binding: b.clone(),
                    expected_revision: "0".into(),
                    observation: observation(&b, false, 10),
                },
            )
            .expect("register custody");
            Self {
                _temp: temp,
                root,
                ledger,
                connection,
                binding: b,
                executor,
            }
        }
        fn request(&self) -> ClaimRetirementRequest {
            ClaimRetirementRequest {
                command_id: "retire".into(),
                binding: self.binding.clone(),
                claim_id: "claim".into(),
                expected_revision: "1".into(),
                observation: observation(&self.binding, true, 11),
            }
        }
        fn quarantine_path(root: &Path) -> PathBuf {
            fs::read_dir(root)
                .expect("scratch entries")
                .map(|e| e.expect("entry").path())
                .find(|p| {
                    p.file_name()
                        .expect("name")
                        .to_string_lossy()
                        .starts_with(".retired-")
                })
                .expect("quarantined directory")
        }
    }

    #[test]
    fn registered_scratch_runs_preview_claim_quarantine_delete_and_keeps_receipt() {
        let mut f = Fixture::new();
        assert_eq!(
            f.executor.preview(&scope()).expect("preview").stage,
            Stage::Registered
        );
        let request = f.request();
        let mut fence =
            custody::retirement_fence(&mut f.connection, request).expect("durable fence");
        f.executor
            .claim(&scope(), &facts(), &mut fence)
            .expect("claim");
        f.executor
            .quarantine(&scope(), &facts(), &mut fence)
            .expect("quarantine");
        let done = f
            .executor
            .delete(&scope(), &facts(), &mut fence)
            .expect("delete");
        assert_eq!(done.stage, Stage::Deleted);
        assert_eq!(done.disposed_bytes, "16");
        assert!(done.retained_evidence);
        assert_eq!(fs::read_dir(&f.root).expect("root").count(), 0);
        assert!(fs::read_dir(&f.ledger).expect("ledger").count() >= 2);
        assert_eq!(
            f.executor
                .delete(&scope(), &facts(), &mut fence)
                .expect("idempotent delete")
                .stage,
            Stage::Deleted
        );
    }

    #[test]
    fn active_pin_race_blocks_fence_and_claim_prevents_new_pins() {
        let mut f = Fixture::new();
        custody::acquire_pin(
            &mut f.connection,
            AcquirePinRequest {
                command_id: "pin".into(),
                binding: f.binding.clone(),
                pin_id: "consumer".into(),
                expected_revision: "1".into(),
                observation: observation(&f.binding, true, 11),
            },
        )
        .expect("pin wins race");
        let mut request = f.request();
        request.expected_revision = "2".into();
        request.observation = observation(&f.binding, true, 12);
        assert!(custody::retirement_fence(&mut f.connection, request).is_err());
        assert!(f.root.join("asset/data").exists());

        let mut f = Fixture::new();
        let request = f.request();
        let mut fence =
            custody::retirement_fence(&mut f.connection, request).expect("claim wins race");
        f.executor
            .claim(&scope(), &facts(), &mut fence)
            .expect("claim executor");
        assert!(matches!(
            custody::acquire_pin(
                &mut f.connection,
                AcquirePinRequest {
                    command_id: "late-pin".into(),
                    binding: f.binding.clone(),
                    pin_id: "late".into(),
                    expected_revision: "2".into(),
                    observation: observation(&f.binding, true, 12),
                }
            ),
            Err(CustodyError::RetirementClaimed)
        ));
    }

    #[test]
    fn all_preservation_holds_and_unknown_facts_keep_registered_bytes() {
        let mut f = Fixture::new();
        let request = f.request();
        let mut fence = custody::retirement_fence(&mut f.connection, request).expect("fence");
        let mut cases = vec![Preservation::default()];
        for index in 0..8 {
            let mut v = facts();
            let field = match index {
                0 => &mut v.dirty,
                1 => &mut v.ignored_private,
                2 => &mut v.unpushed,
                3 => &mut v.unmerged,
                4 => &mut v.process_held,
                5 => &mut v.foreign,
                6 => &mut v.backup_candidate_held,
                _ => &mut v.unique_data,
            };
            *field = Some(true);
            cases.push(v);
        }
        for held in cases {
            assert!(f.executor.claim(&scope(), &held, &mut fence).is_err());
            assert_eq!(
                fs::read(f.root.join("asset/data")).expect("preserved"),
                b"disposable bytes"
            );
        }
    }

    #[test]
    fn changed_dirty_ignored_symlink_hardlink_and_nested_git_data_are_preserved() {
        for change in ["dirty", "ignored", "symlink", "hardlink", "git"] {
            let mut f = Fixture::new();
            let request = f.request();
            let mut fence = custody::retirement_fence(&mut f.connection, request).expect("fence");
            f.executor
                .claim(&scope(), &facts(), &mut fence)
                .expect("claim");
            let asset = f.root.join("asset");
            match change {
                "dirty" => fs::write(asset.join("data"), b"unique changed bytes").expect("dirty"),
                "ignored" => fs::write(asset.join(".secret"), b"private bytes").expect("ignored"),
                "symlink" => {
                    std::os::unix::fs::symlink(&f.ledger, asset.join("link")).expect("symlink")
                }
                "hardlink" => {
                    fs::hard_link(asset.join("data"), f.root.join("external")).expect("hardlink")
                }
                _ => fs::create_dir(asset.join(".git")).expect("Git metadata"),
            }
            assert!(
                f.executor
                    .quarantine(&scope(), &facts(), &mut fence)
                    .is_err(),
                "{change}"
            );
            assert!(asset.join("data").exists(), "{change}");
        }
    }

    #[test]
    fn root_and_asset_identity_swaps_refuse_before_quarantine() {
        for swap_root in [false, true] {
            let mut f = Fixture::new();
            let request = f.request();
            let mut fence = custody::retirement_fence(&mut f.connection, request).expect("fence");
            f.executor
                .claim(&scope(), &facts(), &mut fence)
                .expect("claim");
            let original = if swap_root {
                f.root.clone()
            } else {
                f.root.join("asset")
            };
            let kept = original.with_extension("kept");
            fs::rename(&original, &kept).expect("swap out original identity");
            private_directory(&original);
            fs::write(original.join("foreign"), b"foreign bytes").expect("foreign replacement");
            assert!(
                f.executor
                    .quarantine(&scope(), &facts(), &mut fence)
                    .is_err()
            );
            assert!(
                kept.join(if swap_root { "asset/data" } else { "data" })
                    .exists()
            );
            assert_eq!(
                fs::read(original.join("foreign")).expect("foreign untouched"),
                b"foreign bytes"
            );
        }
    }

    #[test]
    fn active_real_registered_process_blocks_even_with_false_process_assertion() {
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("owned child");
        let mut f = Fixture::with_processes(&[child.id()]);
        let request = f.request();
        let mut fence = custody::retirement_fence(&mut f.connection, request).expect("fence");
        let result = f.executor.claim(&scope(), &facts(), &mut fence);
        child.kill().expect("stop owned fixture child");
        child.wait().expect("reap owned fixture child");
        assert!(result.is_err());
        assert!(f.root.join("asset/data").exists());
        f.executor
            .claim(&scope(), &facts(), &mut fence)
            .expect("after process exit");
    }

    #[test]
    fn foreign_scope_and_registration_digest_cannot_borrow_custody() {
        let mut f = Fixture::new();
        let mut foreign = scope();
        foreign.root = RootId::new("foreign-root").expect("root");
        assert!(f.executor.preview(&foreign).is_err());
        let mut request = f.request();
        request.binding.sha256 = format!("sha256:{}", "b".repeat(64));
        request.observation.binding = request.binding.clone();
        assert!(custody::retirement_fence(&mut f.connection, request).is_err());
        assert!(f.root.join("asset/data").exists());
    }

    #[test]
    fn reopen_recovers_quarantine_and_partial_delete_without_dropping_evidence() {
        for stage in ["quarantining", "deleting"] {
            let mut f = Fixture::new();
            let request = f.request();
            let mut fence =
                custody::retirement_fence(&mut f.connection, request.clone()).expect("fence");
            f.executor
                .claim(&scope(), &facts(), &mut fence)
                .expect("claim");
            f.executor
                .quarantine(&scope(), &facts(), &mut fence)
                .expect("quarantine");
            let path = fs::read_dir(&f.ledger)
                .expect("ledger")
                .map(|e| e.expect("entry").path())
                .find(|p| p.extension().is_some_and(|e| e == "json"))
                .expect("journal");
            let mut journal: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).expect("journal bytes"))
                    .expect("journal JSON");
            journal["stage"] = stage.into();
            fs::write(
                &path,
                serde_json::to_vec(&journal).expect("encode crash point"),
            )
            .expect("inject durable crash point");
            if stage == "deleting" {
                fs::remove_file(Fixture::quarantine_path(&f.root).join("data"))
                    .expect("simulate completed unlink");
            }
            drop(f.executor);
            let executor = CleanupExecutor::open(&f.root, &f.ledger).expect("reopen writer");
            let mut fence = custody::retirement_fence(&mut f.connection, request)
                .expect("replay durable fence");
            if stage == "quarantining" {
                executor
                    .quarantine(&scope(), &facts(), &mut fence)
                    .expect("recover rename");
            }
            assert_eq!(
                executor
                    .delete(&scope(), &facts(), &mut fence)
                    .expect("recover delete")
                    .stage,
                Stage::Deleted
            );
            assert!(path.exists());
        }
    }

    #[test]
    fn unregistered_discovery_and_second_writer_have_no_authority() {
        let f = Fixture::new();
        assert!(CleanupExecutor::open(&f.root, &f.ledger).is_err());
        let mut unknown = scope();
        unknown.asset_id = AssetId::new("unknown").expect("asset");
        assert!(f.executor.preview(&unknown).is_err());
        assert!(f.root.join("asset/data").exists());
    }

    #[test]
    fn custody_corruption_after_quarantine_blocks_every_delete_effect() {
        let mut f = Fixture::new();
        let request = f.request();
        let mut fence = custody::retirement_fence(&mut f.connection, request).expect("fence");
        f.executor
            .claim(&scope(), &facts(), &mut fence)
            .expect("claim");
        f.executor
            .quarantine(&scope(), &facts(), &mut fence)
            .expect("quarantine");
        let other =
            Connection::open(f._temp.path().join("custody.sqlite")).expect("second connection");
        other.execute("UPDATE custody_assets SET retirement_claim_id='foreign-claim' WHERE asset_id='asset-1'", []).expect("inject mismatched custody projection");
        assert!(f.executor.delete(&scope(), &facts(), &mut fence).is_err());
        assert_eq!(
            fs::read(Fixture::quarantine_path(&f.root).join("data")).expect("retained bytes"),
            b"disposable bytes"
        );
    }
}
