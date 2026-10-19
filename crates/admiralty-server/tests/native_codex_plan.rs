//! Exercise the registered production module and actual sibling exports.
//! These fixtures qualify composition/custody only, never a provider turn or
//! Linux execution.
pub use admiralty_server::services::{controller, native_codex_plan, native_executor};

#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{controller, native_codex_plan, native_executor};
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, SchedulingState,
        SourceIdentity, TaskContract, TaskId, TaskView, content_digest, nodes::NodeId,
    };
    use admiralty_runtime::{
        models::{ModelSpec, Provider},
        providers::{
            codex::{
                CodexAdmission, CodexCliObservation, CodexEffort, CodexExecutionOptions,
                CodexNdjsonStream, CodexSandbox, SUPPORTED_CLI_VERSION,
            },
            supervision::SupervisionScope,
        },
        reservations::{
            AccountId, CurrencyUnit, MinorUnits, Reservation, ReservationEvent, ReservationLedger,
            ReservationScope,
        },
        resource_gate::{
            AdmissionIdentity, QualifiedEnvironment, ResourcePermitAuthority, ResourcePermitClaims,
            SystemdBackendConfig,
        },
        session::{CaptureState, LaunchSpec, OutputStream, SessionError, SessionHost},
        workspace::{
            preparation::{PreparedWorkspace, WorkspacePreparation, prepare_workspace},
            registry::NodeRef,
        },
    };
    use admiralty_server::model_selection::{InferenceSlotClaim, model_config_digest};
    use admiralty_store::{
        BudgetAndSlotAdmission,
        admission_reservations::{self, ReservationCommand},
        inference_slots::{self, SlotClaim, SlotCommand, SlotEvent, SlotLedger},
        outbox,
    };
    use native_codex_plan::{
        BoundNativeCodexPlan, CodexPlanError, CodexPlanInput, CodexResourceBinding,
        PreparedNativeCodexPlan, prepare_codex_plan,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        io::Write,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct Fixture {
        workspace: Option<PreparedWorkspace>,
        task: TaskView,
        admission: CodexAdmission,
        model: ModelSpec,
        options: CodexExecutionOptions,
        cli: CodexCliObservation,
        source: SourceIdentity,
        context: String,
        policy: String,
        capsule: String,
        root: PrivateDirectory,
    }
    struct PrivateDirectory(PathBuf);
    impl Drop for PrivateDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn mkdir(path: &Path) {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn write(path: &Path, bytes: &[u8]) {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        f.set_permissions(fs::Permissions::from_mode(0o600))
            .unwrap();
        f.write_all(bytes).unwrap();
    }
    fn command(program: &Path, args: &[&str], cwd: &Path, root: &Path, id: &str) -> String {
        let host = SessionHost::new();
        host.launch_with_environment(
            LaunchSpec {
                run_id: id.into(),
                program: program.into(),
                args: args.iter().map(|s| (*s).into()).collect(),
                cwd: cwd.into(),
                spool_dir: root.join(format!("capture-{id}")),
            },
            &[
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), root.join("home").into_os_string()),
                ("TMPDIR".into(), root.join("tmp").into_os_string()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
                ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ],
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        retry_busy(|| host.close_input(id), end);
        loop {
            let status = retry_busy(|| host.status(id), end);
            if status.capture_failed || status.output_offset > 64 * 1024 || Instant::now() > end {
                let _ = host.terminate(id);
                panic!("owned fixture command exceeded bounds");
            }
            if status.exit.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(retry_busy(|| host.wait(id), end).success());
        let status = retry_busy(|| host.status(id), end);
        let tail = retry_busy(|| host.tail(id, 0, 64 * 1024), end);
        assert_eq!(status.capture_state, CaptureState::Complete);
        assert_eq!(tail.next_offset, status.output_offset);
        String::from_utf8(
            tail.chunks
                .into_iter()
                .filter(|c| c.stream == OutputStream::Stdout)
                .flat_map(|c| c.bytes)
                .collect(),
        )
        .unwrap()
        .trim()
        .into()
    }
    fn retry_busy<T>(mut operation: impl FnMut() -> Result<T, SessionError>, end: Instant) -> T {
        loop {
            match operation() {
                Ok(value) => return value,
                Err(SessionError::Busy) if Instant::now() < end => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("owned fixture session operation failed: {error}"),
            }
        }
    }
    impl Fixture {
        fn new() -> Self {
            let root = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("native-codex-plan-{}", uuid::Uuid::new_v4()));
            mkdir(&root);
            let root = PrivateDirectory(root);
            for name in ["base", "managed", "spool", "home", "tmp"] {
                mkdir(&root.0.join(name));
            }
            mkdir(&root.0.join("base/src"));
            write(
                &root.0.join("base/src/lib.rs"),
                b"pub fn value()->u8 { 1 }\n",
            );
            let git = fs::canonicalize("/usr/bin/git").unwrap();
            let run_git = |args: &[&str], id: &str| {
                let mut fixed = vec![
                    "--no-pager",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "core.fsmonitor=false",
                ];
                fixed.extend_from_slice(args);
                // Git-created metadata must remain private even when the owned
                // test process inherits umask 002. Preserve every argv byte;
                // change only this Git child's mask, never the process-global mask.
                let mut wrapped = vec![
                    "-c",
                    "umask 077; exec \"$@\"",
                    "fixture-git",
                    git.to_str().unwrap(),
                ];
                wrapped.extend_from_slice(&fixed);
                command(
                    Path::new("/bin/sh"),
                    &wrapped,
                    &root.0.join("base"),
                    &root.0,
                    id,
                )
            };
            run_git(&["init", "--quiet", "--object-format=sha1"], "git-init");
            run_git(&["add", "--", "src/lib.rs"], "git-add");
            run_git(
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-qm",
                    "Fixture base",
                ],
                "git-commit",
            );
            let commit = run_git(&["rev-parse", "HEAD"], "git-head");
            let contract = TaskContract {
                schema_version: 1,
                project_id: ProjectId::new("fixture-project").unwrap(),
                root_id: RootId::new("fixture-root").unwrap(),
                task_id: TaskId::new("fixture-task").unwrap(),
                revision: 1,
                title: "Bound fixture".into(),
                objective: "Edit owned source".into(),
                non_goals: vec!["No provider turn".into()],
                owned_paths: vec!["src/lib.rs".into()],
                forbidden_paths: vec!["secrets".into()],
                read_first: vec![],
                dependencies: vec![],
                criteria: vec![AcceptanceCriterion {
                    id: "check".into(),
                    text: "Exact check".into(),
                    check_ids: vec!["check".into()],
                }],
                checks: vec![CheckSpec {
                    id: "check".into(),
                    program: "fixture-check".into(),
                    args: vec![],
                    cwd: ".".into(),
                    timeout_secs: 1,
                }],
                authority_ref: "fixture-authority".into(),
                max_attempts: 2,
                simulated: false,
            };
            let attempt = AttemptId::new("fixture-attempt").unwrap();
            let workspace = prepare_workspace(WorkspacePreparation {
                source: root.0.join("base"),
                managed_parent: root.0.join("managed"),
                destination_name: "work".into(),
                pinned_commit: commit,
                node_id: NodeRef::new("fixture-node").unwrap(),
                root_id: contract.root_id.clone(),
                project_id: contract.project_id.clone(),
                task_id: contract.task_id.clone(),
                attempt_id: attempt.clone(),
                owned_paths: vec!["src/lib.rs".into()],
                forbidden_paths: vec!["secrets".into()],
            })
            .unwrap();
            let program = root.0.join("fixture-codex");
            write(&program,format!("#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' '{SUPPORTED_CLI_VERSION}';;\n--help) printf '%s\\n' '--no-daemon --ask-for-approval';;\nexec) test \"$2\" = '--help' || exit 77; printf '%s\\n' '--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd';;\n*) exit 77;;\nesac\n").as_bytes());
            fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
            let version = command(&program, &["--version"], &root.0, &root.0, "cli-version");
            let exec_help = command(
                &program,
                &["exec", "--help"],
                &root.0,
                &root.0,
                "cli-exec-help",
            );
            let root_help = command(&program, &["--help"], &root.0, &root.0, "cli-root-help");
            let cli = CodexCliObservation::from_help(
                "fixture-node",
                program,
                &version,
                &exec_help,
                &root_help,
                Some(10),
            )
            .unwrap();
            let model = ModelSpec {
                id: "registry-fixture-codex".into(),
                label: "Fixture Codex".into(),
                provider: Provider::CodexCli,
                provider_model: "native-fixture-model".into(),
                tier: 0,
                enabled: true,
                node_id: Some("fixture-node".into()),
                endpoint: None,
                credential_file_ref: None,
                max_concurrency: 1,
                capabilities: BTreeSet::from(["code".into()]),
                task_max_attempts: 2,
                escalate_to: None,
            };
            let admission = CodexAdmission {
                scope: SupervisionScope {
                    project_id: contract.project_id.clone(),
                    root_id: contract.root_id.clone(),
                    task_id: contract.task_id.clone(),
                    attempt_id: attempt,
                    owner: PrincipalId::new("fixture-worker").unwrap(),
                    incarnation: "fixture-incarnation".into(),
                    authority_ref: contract.authority_ref.clone(),
                    contract_digest: content_digest(&contract).unwrap(),
                    revision: 1,
                },
                attempt_ordinal: 1,
                registry_revision: "42".into(),
                selected_model_digest: content_digest(&model).unwrap(),
                cli_observation_digest: cli.fingerprint().into(),
                node_id: "fixture-node".into(),
                working_dir: workspace.workspace().workspace.clone(),
                spool_root: root.0.join("spool"),
                lease_ref: "fixture-lease".into(),
                resource_permit_ref: "fixture-permit".into(),
                custody_ref: "fixture-custody".into(),
                auth_context_ref: "fixture-auth-context".into(),
                admitted_tick: 10,
                expires_tick: 20,
            };
            let capsule = "Literal capsule $(touch forbidden); `touch forbidden`".to_owned();
            Self {
                workspace: Some(workspace),
                task: TaskView {
                    contract,
                    state: SchedulingState::Ready,
                    attempt_id: None,
                    owner: None,
                    candidate_ref: None,
                    lifecycle: None,
                },
                admission,
                model,
                options: CodexExecutionOptions {
                    effort: CodexEffort::Medium,
                    sandbox: CodexSandbox::WorkspaceWrite,
                },
                cli,
                source: SourceIdentity {
                    producer_id: "fixture-coordinator".into(),
                    incarnation: "fixture-incarnation".into(),
                    sequence: "1".into(),
                },
                context: content_digest(&capsule).unwrap(),
                policy: content_digest(&"fixture-policy").unwrap(),
                capsule,
                root,
            }
        }
        fn input(&self) -> CodexPlanInput<'_> {
            CodexPlanInput {
                task: &self.task,
                admission: &self.admission,
                model: &self.model,
                options: self.options,
                cli: &self.cli,
                task_capsule: &self.capsule,
                context_digest: &self.context,
                policy_digest: &self.policy,
                source: &self.source,
                observed_utc: "2026-10-18T23:00:00Z",
                not_before: 1,
                now_tick: Some(10),
            }
        }
        fn prepare(&mut self) -> PreparedNativeCodexPlan {
            let workspace = self.workspace.take().unwrap();
            prepare_codex_plan(workspace, self.input()).unwrap()
        }
        fn resources(&self, prepared: &PreparedNativeCodexPlan) -> CodexResourceBinding {
            let environment = QualifiedEnvironment::new(BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                (
                    "HOME".into(),
                    self.root.0.join("home").to_string_lossy().into_owned(),
                ),
                (
                    "TMPDIR".into(),
                    self.root.0.join("tmp").to_string_lossy().into_owned(),
                ),
            ]))
            .unwrap();
            let backend = SystemdBackendConfig {
                runtime_directory: self.root.0.join("runtime"),
                delegated_slice: "fixture.slice".into(),
            };
            let t = prepared.prepared_provider().tuple();
            let expires = u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
            )
            .unwrap()
                + 60_000;
            let claims = ResourcePermitClaims {
                issuer_ref: "fixture-issuer".into(),
                permit_id: t.resource_permit_ref.clone(),
                reservation_ref: "fixture-kernel-reservation".into(),
                identity: AdmissionIdentity {
                    project_id: t.scope.project_id.clone(),
                    task_id: t.scope.task_id.clone(),
                    attempt_id: t.scope.attempt_id.clone(),
                    node_id: NodeId::new(&t.node_id).unwrap(),
                    lease_id: t.lease_ref.clone(),
                },
                host_incarnation: "fixture-host-boot".into(),
                expires_at_unix_ms: expires,
                lease_expires_at_unix_ms: expires,
                registered_cpu_scope: BTreeSet::from([0, 1]),
                cpu_set: BTreeSet::from([0]),
                memory_max_bytes: 256 * 1024 * 1024,
                max_tasks: 16,
                max_concurrency: 1,
                launch_spec_digest: content_digest(prepared.prepared_provider().launch_spec())
                    .unwrap(),
                worker_environment_digest: environment.digest().unwrap(),
                backend_config_digest: backend.digest().unwrap(),
            };
            let reservation = Reservation {
                id: claims.reservation_ref.clone(),
                scope: ReservationScope {
                    project_id: t.scope.project_id.clone(),
                    root_id: t.scope.root_id.clone(),
                    task_id: t.scope.task_id.clone(),
                    attempt_id: t.scope.attempt_id.clone(),
                    owner: t.scope.owner.clone(),
                    incarnation: claims.host_incarnation.clone(),
                    contract_revision: 1,
                    contract_digest: t.scope.contract_digest.clone(),
                    authority_ref: t.scope.authority_ref.clone(),
                    resource_claims_digest: native_executor::native_resource_claims_digest(&claims)
                        .unwrap(),
                    account_id: AccountId::new("fixture-account").unwrap(),
                    unit: CurrencyUnit::new("USD", 2).unwrap(),
                },
                correlation_id: "fixture-correlation".into(),
                revision: 1,
                original: MinorUnits::new(1),
                settled: MinorUnits::new(0),
                stop_requested: false,
                outstanding: true,
            };
            let authority = Arc::new(FixturePermit {
                expected: claims.clone(),
                current: AtomicBool::new(true),
            });
            CodexResourceBinding {
                claims,
                reservation,
                authority,
                environment,
                backend,
                now_tick: Some(10),
            }
        }
    }
    // Test-only exact adapters are component fixtures, not authenticated production
    // selection/permit issuers. No execution path invokes them in this harness.
    struct FixturePermit {
        expected: ResourcePermitClaims,
        current: AtomicBool,
    }
    impl ResourcePermitAuthority for FixturePermit {
        fn is_current(&self, c: &ResourcePermitClaims) -> bool {
            self.current.load(Ordering::SeqCst) && c == &self.expected
        }
    }
    struct FixtureAdmission {
        task: TaskView,
        request: controller::LaunchRequest,
        current: bool,
    }
    impl controller::CurrentAdmission for FixtureAdmission {
        fn authorize_current(
            &self,
            task: &TaskView,
            r: &controller::LaunchRequest,
        ) -> Result<(), String> {
            if self.current && task == &self.task && r == &self.request {
                Ok(())
            } else {
                Err("fixture denied".into())
            }
        }
    }
    fn current(f: &Fixture, p: &PreparedNativeCodexPlan) -> FixtureAdmission {
        FixtureAdmission {
            task: f.task.clone(),
            request: p.launch_request().clone(),
            current: true,
        }
    }

    fn clone_slot_inputs(inputs: &BudgetAndSlotAdmission) -> BudgetAndSlotAdmission {
        BudgetAndSlotAdmission {
            budget_command: inputs.budget_command.clone(),
            budget_proof: Arc::clone(&inputs.budget_proof),
            slot_command: inputs.slot_command.clone(),
            slot_proof: Arc::clone(&inputs.slot_proof),
        }
    }

    struct SlotExpected {
        request: controller::LaunchRequest,
        spec: LaunchSpec,
        reservation: Reservation,
        tuple_digest: String,
        original_head: String,
        original_workspace: PathBuf,
    }

    fn slot_fixture() -> Fixture {
        let mut fixture = Fixture::new();
        // Explicitly choose the existing Store profile before preparing the
        // original request. The Codex producer incarnation remains independent.
        fixture.source.incarnation = "fixture-host-boot".into();
        fixture
    }

    fn slot_inputs(
        fixture: &mut Fixture,
    ) -> (
        BoundNativeCodexPlan,
        BudgetAndSlotAdmission,
        InferenceSlotClaim,
        SlotExpected,
    ) {
        let prepared = fixture.prepare();
        let request = prepared.launch_request().clone();
        let provider = prepared.prepared_provider();
        let tuple = provider.tuple();
        let inference = InferenceSlotClaim {
            project_id: tuple.scope.project_id.clone(),
            root_id: tuple.scope.root_id.clone(),
            task_id: tuple.scope.task_id.clone(),
            attempt_id: tuple.scope.attempt_id.clone(),
            contract_revision: tuple.scope.revision,
            contract_digest: tuple.scope.contract_digest.clone(),
            model_id: fixture.model.id.clone(),
            provider: fixture.model.provider.clone(),
            provider_model: fixture.model.provider_model.clone(),
            tier: fixture.model.tier,
            node_id: fixture.model.node_id.clone(),
            slots_reserved: 1,
            registry_revision: tuple.registry_revision.clone(),
            // Data only: no observed capacity or authorization is created here.
            snapshot_revision: "fixture-unattested-snapshot".into(),
            model_config_digest: model_config_digest(&fixture.model).unwrap(),
        };
        let resources = fixture.resources(&prepared);
        let expected = SlotExpected {
            request: request.clone(),
            spec: provider.launch_spec().clone(),
            reservation: resources.reservation.clone(),
            tuple_digest: provider.tuple_digest().into(),
            original_head: prepared.prepared_workspace().head_oid().into(),
            original_workspace: prepared.prepared_workspace().workspace().workspace.clone(),
        };
        let intent = controller::planned_launch_intent(&fixture.task, &request).unwrap();
        let intent_digest = outbox::intent_digest(&intent).unwrap();
        let budget_command = ReservationCommand {
            schema_version: 1,
            command_id: "fixture-slot-budget".into(),
            source: request.source.clone(),
            expected_ledger_revision: 0,
            expected_ledger_digest:
                admission_reservations::ledger_digest(&ReservationLedger::new()).unwrap(),
            launch_intent_digest: Some(intent_digest.clone()),
            event: ReservationEvent::Reserve {
                event_id: "fixture-slot-budget".into(),
                reservation_id: resources.reservation.id.clone(),
                scope: resources.reservation.scope.clone(),
                correlation_id: resources.reservation.correlation_id.clone(),
                amount: resources.reservation.original.clone(),
            },
        };
        let slot_command = SlotCommand {
            schema_version: 1,
            command_id: "fixture-slot-command".into(),
            source: request.source.clone(),
            expected_ledger_revision: 0,
            expected_ledger_digest: inference_slots::ledger_digest(&SlotLedger::default()).unwrap(),
            event: SlotEvent::Hold {
                hold_id: "fixture-slot-hold".into(),
                claim: SlotClaim {
                    scope: resources.reservation.scope.clone(),
                    pool_id: "fixture-managed-pool".into(),
                    generation: "fixture-generation".into(),
                    model_id: inference.model_id.clone(),
                    model_config_digest: inference.model_config_digest.clone(),
                    inference_claims_digest: content_digest(&inference).unwrap(),
                    kernel_claims_digest: native_executor::native_resource_claims_digest(
                        &resources.claims,
                    )
                    .unwrap(),
                    budget_reservation_id: resources.reservation.id.clone(),
                    budget_command_digest: admission_reservations::validate_command(
                        &budget_command,
                    )
                    .unwrap(),
                    launch_intent_digest: intent_digest,
                    slots: inference.slots_reserved,
                },
            },
        };
        let inputs = BudgetAndSlotAdmission {
            budget_command,
            budget_proof: Arc::new("fixture-private-budget-proof".to_owned()),
            slot_command,
            slot_proof: Arc::new("fixture-private-slot-proof".to_owned()),
        };
        let admission = current(fixture, &prepared);
        let bound = prepared.bind_resources(resources, &admission).unwrap();
        (bound, inputs, inference, expected)
    }

    fn slot_claim_mut(inputs: &mut BudgetAndSlotAdmission) -> &mut SlotClaim {
        match &mut inputs.slot_command.event {
            SlotEvent::Hold { claim, .. } => claim,
            _ => panic!("owned fixture requires a slot hold"),
        }
    }

    fn sync_inference_digest(inputs: &mut BudgetAndSlotAdmission, inference: &InferenceSlotClaim) {
        let claim = slot_claim_mut(inputs);
        claim.model_id = inference.model_id.clone();
        claim.model_config_digest = inference.model_config_digest.clone();
        claim.slots = inference.slots_reserved;
        claim.inference_claims_digest = content_digest(inference).unwrap();
    }

    fn sync_budget_digest(inputs: &mut BudgetAndSlotAdmission) {
        let digest = admission_reservations::validate_command(&inputs.budget_command).unwrap();
        slot_claim_mut(inputs).budget_command_digest = digest;
    }

    #[test]
    fn slot_factory_binds_distinct_model_ids_digest_domains_and_global_node_without_a_turn() {
        for global_model in [false, true] {
            let mut fixture = slot_fixture();
            if global_model {
                fixture.model.node_id = None;
                fixture.admission.selected_model_digest = content_digest(&fixture.model).unwrap();
            }
            let (bound, inputs, inference, expected) = slot_inputs(&mut fixture);
            assert_ne!(fixture.model.id, fixture.model.provider_model);
            assert_ne!(
                content_digest(&fixture.model).unwrap(),
                inference.model_config_digest
            );
            let budget_proof = Arc::clone(&inputs.budget_proof);
            let slot_proof = Arc::clone(&inputs.slot_proof);
            let slot_bound = bound
                .bind_inference_slot(inputs, inference.clone())
                .unwrap();
            let (plan, request, provider, _, _, retained) = slot_bound.into_parts();
            assert!(retained.matches_plan(&request, &plan));
            assert_eq!(request, expected.request);
            assert_eq!(request.model, fixture.model.id);
            assert_eq!(plan.model_declared, fixture.model.id);
            assert_eq!(provider.tuple().registry_model_id, fixture.model.id);
            assert_eq!(
                provider.tuple().requested_model,
                fixture.model.provider_model
            );
            assert_eq!(
                provider.tuple().selected_model_digest,
                content_digest(&fixture.model).unwrap()
            );
            assert_eq!(provider.tuple_digest(), expected.tuple_digest);
            assert_ne!(
                provider.tuple().scope.incarnation,
                plan.permit_claims.host_incarnation
            );
            assert_eq!(
                request.source.incarnation,
                plan.permit_claims.host_incarnation
            );
            assert_eq!(inference.node_id, fixture.model.node_id);
            assert_eq!(provider.tuple().node_id, "fixture-node");
            assert_eq!(plan.reservation, expected.reservation);
            assert_eq!(plan.prepared_workspace.head_oid(), expected.original_head);
            assert_eq!(
                plan.prepared_workspace.workspace().workspace,
                expected.original_workspace
            );
            plan.prepared_workspace.validate_current_custody().unwrap();
            assert!(Arc::ptr_eq(&retained.inputs().budget_proof, &budget_proof));
            assert!(Arc::ptr_eq(&retained.inputs().slot_proof, &slot_proof));
            let summary = CodexNdjsonStream::from_prepared(&provider)
                .unwrap()
                .finish(None, false);
            assert_eq!(summary.reported_model, None);
            assert!(!summary.native_execution_qualified());
            assert!(!fixture.root.0.join("forbidden").exists());
        }
    }

    #[test]
    fn slot_factory_rejection_matrix_returns_original_preparation_proofs_and_claim_for_correction()
    {
        let mut fixture = slot_fixture();
        let (mut bound, original, inference, expected) = slot_inputs(&mut fixture);
        for case in 0..38 {
            let mut inputs = clone_slot_inputs(&original);
            let mut claimed = inference.clone();
            match case {
                0 => claimed.model_id = "different-registry-id".into(),
                1 => claimed.provider = Provider::OpenaiCompatible,
                2 => claimed.provider_model = "different-provider-model".into(),
                3 => claimed.tier = 1,
                4 => claimed.node_id = None,
                5 => claimed.registry_revision = "43".into(),
                6 => claimed.model_config_digest = content_digest(&fixture.model).unwrap(),
                7 => claimed.project_id = ProjectId::new("foreign-project").unwrap(),
                8 => claimed.root_id = RootId::new("foreign-root").unwrap(),
                9 => claimed.task_id = TaskId::new("foreign-task").unwrap(),
                10 => claimed.attempt_id = AttemptId::new("foreign-attempt").unwrap(),
                11 => claimed.contract_revision += 1,
                12 => claimed.contract_digest = content_digest(&"foreign-contract").unwrap(),
                13 => claimed.slots_reserved = 0,
                14 => claimed.slots_reserved = 2,
                15 => claimed.snapshot_revision.clear(),
                16 => claimed.snapshot_revision = "x".repeat(129),
                17 => {
                    inputs.budget_command.source.producer_id = "foreign-producer".into();
                    inputs.slot_command.source = inputs.budget_command.source.clone();
                    sync_budget_digest(&mut inputs);
                }
                18 => {
                    inputs.budget_command.source.incarnation = "foreign-host-boot".into();
                    inputs.slot_command.source = inputs.budget_command.source.clone();
                    if let ReservationEvent::Reserve { scope, .. } =
                        &mut inputs.budget_command.event
                    {
                        scope.incarnation = "foreign-host-boot".into();
                        slot_claim_mut(&mut inputs).scope.incarnation = "foreign-host-boot".into();
                    }
                    sync_budget_digest(&mut inputs);
                }
                19 => {
                    if let ReservationEvent::Reserve { reservation_id, .. } =
                        &mut inputs.budget_command.event
                    {
                        *reservation_id = "foreign-reservation".into();
                    }
                    slot_claim_mut(&mut inputs).budget_reservation_id =
                        "foreign-reservation".into();
                    sync_budget_digest(&mut inputs);
                }
                20 => {
                    if let ReservationEvent::Reserve { amount, .. } =
                        &mut inputs.budget_command.event
                    {
                        *amount = MinorUnits::new(2);
                    }
                    sync_budget_digest(&mut inputs);
                }
                21 => {
                    if let ReservationEvent::Reserve { correlation_id, .. } =
                        &mut inputs.budget_command.event
                    {
                        *correlation_id = "foreign-correlation".into();
                    }
                    sync_budget_digest(&mut inputs);
                }
                22 => {
                    if let ReservationEvent::Reserve { scope, .. } =
                        &mut inputs.budget_command.event
                    {
                        scope.root_id = RootId::new("foreign-kernel-root").unwrap();
                    }
                    slot_claim_mut(&mut inputs).scope.root_id =
                        RootId::new("foreign-kernel-root").unwrap();
                    sync_budget_digest(&mut inputs);
                }
                23 => {
                    let wrong = content_digest(&"different-kernel-claims").unwrap();
                    if let ReservationEvent::Reserve { scope, .. } =
                        &mut inputs.budget_command.event
                    {
                        scope.resource_claims_digest = wrong.clone();
                    }
                    let claim = slot_claim_mut(&mut inputs);
                    claim.scope.resource_claims_digest = wrong.clone();
                    claim.kernel_claims_digest = wrong;
                    sync_budget_digest(&mut inputs);
                }
                24 => {
                    let wrong = content_digest(&"different-launch-intent").unwrap();
                    inputs.budget_command.launch_intent_digest = Some(wrong.clone());
                    slot_claim_mut(&mut inputs).launch_intent_digest = wrong;
                    sync_budget_digest(&mut inputs);
                }
                25 => {
                    inputs.budget_command.expected_ledger_revision = u64::MAX;
                    sync_budget_digest(&mut inputs);
                }
                26 => inputs.slot_command.expected_ledger_revision = u64::MAX,
                27 => {
                    slot_claim_mut(&mut inputs).budget_command_digest =
                        content_digest(&"different-budget-command").unwrap()
                }
                28 => {
                    slot_claim_mut(&mut inputs).inference_claims_digest =
                        expected.reservation.scope.resource_claims_digest.clone()
                }
                29 => slot_claim_mut(&mut inputs).model_id = "foreign-slot-model".into(),
                30 => {
                    slot_claim_mut(&mut inputs).model_config_digest =
                        content_digest(&fixture.model).unwrap()
                }
                31 => slot_claim_mut(&mut inputs).slots = 2,
                32 => slot_claim_mut(&mut inputs).pool_id.clear(),
                33 => slot_claim_mut(&mut inputs).generation.clear(),
                34 => {
                    slot_claim_mut(&mut inputs).kernel_claims_digest =
                        content_digest(&"foreign-kernel").unwrap()
                }
                35 => {
                    slot_claim_mut(&mut inputs).scope.owner =
                        PrincipalId::new("foreign-owner").unwrap()
                }
                36 => inputs.slot_command.source.sequence = "2".into(),
                37 => inputs.budget_command.source.sequence = "2".into(),
                _ => unreachable!(),
            }
            if case <= 16 {
                // Preserve matching sidecar hashes so the actual prepared-model
                // mapping, rather than only a stale digest, must reject the claim.
                sync_inference_digest(&mut inputs, &claimed);
            }
            let command_before = inputs.budget_command.clone();
            let slot_before = inputs.slot_command.clone();
            let budget_proof = Arc::clone(&inputs.budget_proof);
            let slot_proof = Arc::clone(&inputs.slot_proof);
            let failure = bound
                .bind_inference_slot(inputs, claimed.clone())
                .err()
                .unwrap_or_else(|| panic!("factory accepted invalid binding case {case}"));
            let debug = format!("{failure:?}");
            assert!(!debug.contains("fixture-private-budget-proof"));
            assert!(!debug.contains("fixture-private-slot-proof"));
            assert!(!debug.contains(fixture.root.0.to_str().unwrap()));
            let (error, returned, inputs, returned_claim) = failure.into_parts();
            assert_eq!(error, CodexPlanError::ResourceBinding, "case {case}");
            assert_eq!(inputs.budget_command, command_before, "case {case}");
            assert_eq!(inputs.slot_command, slot_before, "case {case}");
            assert!(
                Arc::ptr_eq(&inputs.budget_proof, &budget_proof),
                "case {case}"
            );
            assert!(Arc::ptr_eq(&inputs.slot_proof, &slot_proof), "case {case}");
            assert_eq!(returned_claim, claimed, "case {case}");
            bound = returned;
        }
        // Explicit correction uses the same returned original preparation; no
        // reconstructed workspace, silent profile rewrite, or automatic retry.
        let (plan, request, provider, _, _, retained) = bound
            .bind_inference_slot(original, inference)
            .unwrap()
            .into_parts();
        assert!(retained.matches_plan(&request, &plan));
        assert_eq!(plan.prepared_workspace.head_oid(), expected.original_head);
        assert_eq!(
            plan.prepared_workspace.workspace().workspace,
            expected.original_workspace
        );
        plan.prepared_workspace.validate_current_custody().unwrap();
        assert_eq!(provider.tuple_digest(), expected.tuple_digest);
        assert_eq!(plan.reservation, expected.reservation);
        assert!(!fixture.root.0.join("forbidden").exists());
    }

    #[test]
    fn slot_factory_does_not_equate_codex_and_kernel_incarnation_namespaces() {
        // The ordinary fixture deliberately has different request-source and
        // kernel incarnations. Existing Store profile cannot bind that request;
        // rejection must preserve it, rather than silently rewrite either value.
        let mut fixture = Fixture::new();
        let (bound, inputs, inference, expected) = slot_inputs(&mut fixture);
        let source_before = inputs.budget_command.source.clone();
        assert_ne!(
            source_before.incarnation,
            expected.reservation.scope.incarnation
        );
        let proof_before = Arc::clone(&inputs.slot_proof);
        let (_, bound, returned, claim) = bound
            .bind_inference_slot(inputs, inference.clone())
            .err()
            .unwrap()
            .into_parts();
        assert_eq!(returned.budget_command.source, source_before);
        assert_eq!(returned.slot_command.source, source_before);
        assert_eq!(claim, inference);
        assert!(Arc::ptr_eq(&returned.slot_proof, &proof_before));
        let (plan, request, provider, _, _) = bound.into_parts();
        assert_eq!(request.source, source_before);
        assert_eq!(plan.reservation, expected.reservation);
        assert_ne!(
            provider.tuple().scope.incarnation,
            plan.permit_claims.host_incarnation
        );
        plan.prepared_workspace.validate_current_custody().unwrap();
    }

    #[test]
    fn private_slot_binding_rejects_public_request_and_plan_mutation_after_extraction() {
        let mut fixture = slot_fixture();
        let (bound, inputs, inference, expected) = slot_inputs(&mut fixture);
        let (mut plan, request, _, _, _, retained) = bound
            .bind_inference_slot(inputs, inference)
            .unwrap()
            .into_parts();
        assert!(retained.matches_plan(&request, &plan));
        for case in 0..12 {
            let mut changed = request.clone();
            match case {
                0 => changed.source.producer_id = "foreign-producer".into(),
                1 => changed.source.incarnation = "foreign-incarnation".into(),
                2 => changed.source.sequence = "2".into(),
                3 => changed.model = fixture.model.provider_model.clone(),
                4 => changed.provider = "openai_compatible".into(),
                5 => changed.context_digest = content_digest(&"foreign-context").unwrap(),
                6 => changed.policy_digest = content_digest(&"foreign-policy").unwrap(),
                7 => changed.run_id = "foreign-run".into(),
                8 => changed.owner = PrincipalId::new("foreign-owner").unwrap(),
                9 => changed.not_before += 1,
                10 => changed.observed_utc = "2026-10-18T23:00:01Z".into(),
                11 => {
                    changed.expected_contract_digest = content_digest(&"foreign-contract").unwrap()
                }
                _ => unreachable!(),
            }
            assert!(
                !retained.matches_plan(&changed, &plan),
                "request case {case}"
            );
            assert!(retained.matches_plan(&request, &plan));
        }
        let original_claims = plan.permit_claims.clone();
        let original_workspace = plan.workspace.clone();
        let original_authority = Arc::clone(&plan.permit_authority);
        for case in 0..26 {
            match case {
                0 => plan.model_declared = fixture.model.provider_model.clone(),
                1 => plan.provider_declared = "openai_compatible".into(),
                2 => plan.context_digest = content_digest(&"foreign-context").unwrap(),
                3 => plan.policy_digest = content_digest(&"foreign-policy").unwrap(),
                4 => plan.run_id = "foreign-run".into(),
                5 => plan.contract_digest = content_digest(&"foreign-contract").unwrap(),
                6 => plan.task_id = TaskId::new("foreign-task").unwrap(),
                7 => plan.attempt_id = AttemptId::new("foreign-attempt").unwrap(),
                8 => plan.launch_spec.args.push("foreign-argument".into()),
                9 => plan.launch_spec.program = PathBuf::from("/bin/false"),
                10 => plan.launch_spec.cwd = fixture.root.0.clone(),
                11 => plan.launch_spec.spool_dir = fixture.root.0.join("foreign-spool"),
                12 => plan.launch_spec.run_id = "foreign-spec-run".into(),
                13 => plan.permit_claims.memory_max_bytes += 1,
                14 => plan.permit_claims.max_tasks += 1,
                15 => plan.permit_claims.expires_at_unix_ms -= 1,
                16 => {
                    plan.permit_claims.cpu_set.insert(1);
                }
                17 => plan.permit_claims.host_incarnation = "foreign-boot".into(),
                18 => plan.reservation.revision += 1,
                19 => plan.reservation.original = MinorUnits::new(2),
                20 => plan.reservation.stop_requested = true,
                21 => plan.reservation.scope.root_id = RootId::new("foreign-root").unwrap(),
                22 => plan.workspace.node_id = NodeRef::new("foreign-node").unwrap(),
                23 => plan.workspace.owned_paths.push("foreign.rs".into()),
                24 => plan.workspace.workspace = fixture.root.0.clone(),
                25 => {
                    plan.permit_authority = Arc::new(FixturePermit {
                        expected: original_claims.clone(),
                        current: AtomicBool::new(true),
                    })
                }
                _ => unreachable!(),
            }
            assert!(!retained.matches_plan(&request, &plan), "plan case {case}");
            plan.task_id = request.task_id.clone();
            plan.attempt_id = request.attempt_id.clone();
            plan.contract_digest = request.expected_contract_digest.clone();
            plan.context_digest = request.context_digest.clone();
            plan.policy_digest = request.policy_digest.clone();
            plan.provider_declared = request.provider.clone();
            plan.model_declared = request.model.clone();
            plan.run_id = request.run_id.clone();
            plan.launch_spec = expected.spec.clone();
            plan.permit_claims = original_claims.clone();
            plan.reservation = expected.reservation.clone();
            plan.workspace = original_workspace.clone();
            plan.permit_authority = Arc::clone(&original_authority);
            assert!(
                retained.matches_plan(&request, &plan),
                "restored plan case {case}"
            );
        }
        plan.prepared_workspace.validate_current_custody().unwrap();
        assert!(!fixture.root.0.join("forbidden").exists());
    }

    #[test]
    fn actual_preparation_and_adapter_argv_bind_without_observed_identity_or_spawn() {
        let mut f = Fixture::new();
        let p = f.prepare();
        let provider = p.prepared_provider();
        let spec = provider.launch_spec();
        assert_eq!(provider.tuple().requested_model, "native-fixture-model");
        assert_eq!(provider.tuple().requested_effort, CodexEffort::Medium);
        assert_eq!(
            provider.tuple().argv_digest,
            content_digest(&spec.args).unwrap()
        );
        assert_eq!(spec.cwd, p.prepared_workspace().workspace().workspace);
        assert!(spec.args.contains(&"--strict-config".to_owned()));
        let prompt: serde_json::Value = serde_json::from_str(spec.args.last().unwrap()).unwrap();
        assert_eq!(prompt["task_capsule"], f.capsule);
        assert!(!f.root.0.join("forbidden").exists());
        let resources = f.resources(&p);
        let admission = current(&f, &p);
        let (plan, request, provider, env, backend) = p
            .bind_resources(resources, &admission)
            .unwrap()
            .into_parts();
        plan.prepared_workspace.validate_current_custody().unwrap();
        assert_eq!(request.model, "registry-fixture-codex");
        assert_eq!(request.provider, "codex_cli");
        assert_ne!(
            plan.permit_claims.host_incarnation,
            provider.tuple().scope.incarnation
        );
        assert_eq!(
            content_digest(&plan.launch_spec).unwrap(),
            content_digest(provider.launch_spec()).unwrap()
        );
        assert_eq!(
            plan.permit_claims.worker_environment_digest,
            env.digest().unwrap()
        );
        assert_eq!(
            plan.permit_claims.backend_config_digest,
            backend.digest().unwrap()
        );
        let parser = CodexNdjsonStream::from_prepared(&provider).unwrap();
        let summary = parser.finish(None, false);
        assert_eq!(summary.reported_model, None);
        assert_eq!(summary.reported_effort, None);
        assert!(!summary.native_execution_qualified());
    }
    #[test]
    fn preparation_mismatch_returns_original_workspace_and_runtime_guards_stay_strict() {
        let mut f = Fixture::new();
        let workspace = f.workspace.take().unwrap();
        let original_head = workspace.head_oid().to_owned();
        f.admission.selected_model_digest = content_digest(&"wrong-model").unwrap();
        let (error, workspace) = prepare_codex_plan(workspace, f.input())
            .err()
            .unwrap()
            .into_parts();
        assert_eq!(error, CodexPlanError::ProviderPreparation);
        assert_eq!(workspace.head_oid(), original_head);
        workspace.validate_current_custody().unwrap();
        f.admission.selected_model_digest = content_digest(&f.model).unwrap();
        f.context = content_digest(&"wrong-capsule").unwrap();
        let (error, workspace) = prepare_codex_plan(workspace, f.input())
            .err()
            .unwrap()
            .into_parts();
        assert_eq!(error, CodexPlanError::InvalidInput);
        workspace.validate_current_custody().unwrap();
        f.context = content_digest(&f.capsule).unwrap();
        let p = prepare_codex_plan(workspace, f.input()).unwrap();
        assert_eq!(p.prepared_workspace().head_oid(), original_head);
    }
    #[test]
    fn resource_and_authority_rejections_return_every_original_input_without_release() {
        let mut f = Fixture::new();
        let mut p = f.prepare();
        let tuple = p.prepared_provider().tuple_digest().to_owned();
        for case in 0..8 {
            let mut resources = f.resources(&p);
            let mut admission = current(&f, &p);
            match case {
                0 => resources.claims.launch_spec_digest = content_digest(&"wrong argv").unwrap(),
                1 => resources.claims.identity.lease_id = "foreign-lease".into(),
                2 => resources.backend.delegated_slice = "foreign.slice".into(),
                3 => resources.reservation.scope.root_id = RootId::new("foreign-root").unwrap(),
                4 => resources.claims.lease_expires_at_unix_ms = 0,
                5 => resources.now_tick = None,
                6 => admission.current = false,
                _ => {
                    resources.authority = Arc::new(FixturePermit {
                        expected: resources.claims.clone(),
                        current: AtomicBool::new(false),
                    })
                }
            }
            let authority = Arc::clone(&resources.authority);
            let original_reservation = resources.reservation.clone();
            let (error, returned, resources) = p
                .bind_resources(resources, &admission)
                .err()
                .unwrap()
                .into_parts();
            assert_eq!(
                error,
                match case {
                    4 | 5 => CodexPlanError::Clock,
                    6 => CodexPlanError::AdmissionDenied,
                    7 => CodexPlanError::PermitDenied,
                    _ => CodexPlanError::ResourceBinding,
                }
            );
            assert!(Arc::ptr_eq(&authority, &resources.authority));
            assert_eq!(resources.reservation, original_reservation);
            assert!(resources.reservation.outstanding);
            assert_eq!(returned.prepared_provider().tuple_digest(), tuple);
            returned
                .prepared_workspace()
                .validate_current_custody()
                .unwrap();
            p = returned;
        }
        let resources = f.resources(&p);
        let admission = current(&f, &p);
        assert!(p.bind_resources(resources, &admission).is_ok());
    }
    #[test]
    fn moved_original_workspace_is_returned_on_custody_error_not_reopened() {
        let mut f = Fixture::new();
        let p = f.prepare();
        let resources = f.resources(&p);
        let admission = current(&f, &p);
        let original = p.prepared_workspace().workspace().workspace.clone();
        let moved = f.root.0.join("managed/moved");
        fs::rename(&original, &moved).unwrap();
        let (error, p, _) = p
            .bind_resources(resources, &admission)
            .err()
            .unwrap()
            .into_parts();
        assert_eq!(error, CodexPlanError::WorkspaceCustody);
        assert!(p.prepared_workspace().validate_current_custody().is_err());
        fs::rename(moved, &original).unwrap();
        p.prepared_workspace().validate_current_custody().unwrap();
    }
    #[test]
    fn bind_clock_cannot_move_backwards_from_actual_provider_preparation() {
        let mut f = Fixture::new();
        let workspace = f.workspace.take().unwrap();
        let mut input = f.input();
        input.now_tick = Some(15);
        let p = prepare_codex_plan(workspace, input).unwrap();
        let mut resources = f.resources(&p);
        resources.now_tick = Some(14);
        let admission = current(&f, &p);
        let (error, p, mut resources) = p
            .bind_resources(resources, &admission)
            .err()
            .unwrap()
            .into_parts();
        assert_eq!(error, CodexPlanError::Clock);
        p.prepared_workspace().validate_current_custody().unwrap();
        resources.now_tick = Some(15);
        assert!(p.bind_resources(resources, &admission).is_ok());
    }
}
