//! Compile the actual unregistered production module against actual sibling
//! exports. Root integration owns services/mod.rs registration. These fixtures
//! qualify composition/custody only, never a provider turn or Linux execution.
pub use admiralty_server::services::{controller, native_executor};
#[path = "../src/services/native_codex_plan.rs"]
mod native_codex_plan;

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
        reservations::{AccountId, CurrencyUnit, MinorUnits, Reservation, ReservationScope},
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
    use native_codex_plan::{
        CodexPlanError, CodexPlanInput, CodexResourceBinding, PreparedNativeCodexPlan,
        prepare_codex_plan,
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
                command(&git, &fixed, &root.0.join("base"), &root.0, id)
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
