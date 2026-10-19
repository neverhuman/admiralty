//! Explicitly invoked, isolated deterministic Linux integration probe. This is
//! NOT a coding-provider qualification, general issuer, production rollout, or
//! whole-packet acceptance. Requires an existing qualified delegated slice and
//! protected one-run startup authorization. Creates one new private fixture and retains evidence.
//! No network, provider calls, service configuration or existing checkout edits.
#[cfg(unix)]
mod probe {
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, PrincipalId, ProjectId, RootId,
        SchedulingState, SourceIdentity, TaskContract, TaskId, content_digest,
        nodes::NodeId,
        verification::{RequiredNegativeCheck, VerificationInputs, VerificationPolicy},
    };
    use admiralty_runtime::{
        resource_gate::{
            AdmissionIdentity, ClosedAdmission, QualifiedEnvironment, ResourceGate,
            ResourcePermitAuthority, ResourcePermitClaims, RunningAdmission, SystemdBackendConfig,
            VerifiedResourcePermit,
        },
        session::{CaptureState, InputKind, LaunchSpec, OutputStream, SessionHost},
        trusted_checks::{ExecutedChecks, execute_checks},
    };
    use admiralty_server::{
        services::native_lifecycle::{
            NativeDecisionBinding, NativeEvidenceAuthority, NativeLifecycleAuthority,
            NativeLifecyclePolicy,
        },
        startup::read_private_token,
    };
    use admiralty_store::{
        Store,
        artifacts::{ArtifactRef, ArtifactStore},
    };
    use clap::{Parser, Subcommand};
    use serde_json::{Value, json};
    use std::{
        collections::{BTreeMap, BTreeSet},
        error::Error,
        ffi::OsString,
        fs,
        io::{BufRead, Read, Write},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::{Component, Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicU8, Ordering},
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
    const MAX_BYTES: usize = 1024 * 1024;
    #[derive(Parser)]
    #[command(
        about = "Owned deterministic native lifecycle integration; not fleet/provider qualification"
    )]
    struct Cli {
        #[command(subcommand)]
        command: Mode,
    }
    #[derive(Subcommand)]
    enum Mode {
        Run {
            /// New fixture path; its existing parent must be private and owned.
            #[arg(long)]
            root: PathBuf,
            /// Existing compact, private operator authorization; required for launch.
            #[arg(long)]
            operator_authorization_file: Option<PathBuf>,
            /// Create the private fixture/request only, without admission or launch.
            #[arg(long)]
            prepare_only: bool,
            #[arg(long)]
            node: String,
            #[arg(long)]
            cpus: String,
            #[arg(long)]
            registered_cpus: String,
            #[arg(long)]
            runtime_directory: PathBuf,
            #[arg(long)]
            slice: String,
            #[arg(long, default_value_t = 256)]
            memory_mib: u64,
            #[arg(long, default_value_t = 16)]
            max_tasks: u32,
            #[arg(long, default_value_t = 90)]
            deadline_seconds: u64,
        },
        #[command(hide = true)]
        OwnedChild {
            #[arg(long)]
            nonce: String,
        },
        #[command(hide = true)]
        CheckResult {
            #[arg(long)]
            expected_value_digest: String,
        },
    }
    pub fn main() -> std::process::ExitCode {
        let cli = Cli::parse();
        if !cfg!(target_os = "linux") {
            eprintln!("Unsupported: actual Linux systemd/cgroup qualification is required");
            return std::process::ExitCode::from(2);
        }
        let result = match cli.command {
            Mode::OwnedChild { nonce } => child(&nonce),
            Mode::CheckResult {
                expected_value_digest,
            } => check_result(&expected_value_digest),
            Mode::Run {
                root,
                operator_authorization_file,
                prepare_only,
                node,
                cpus,
                registered_cpus,
                runtime_directory,
                slice,
                memory_mib,
                max_tasks,
                deadline_seconds,
            } => run(
                root,
                operator_authorization_file,
                prepare_only,
                node,
                cpus,
                registered_cpus,
                runtime_directory,
                slice,
                memory_mib,
                max_tasks,
                deadline_seconds,
            ),
        };
        match result {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Native lifecycle integration did not qualify: {e}");
                std::process::ExitCode::from(17)
            }
        }
    }
    fn result_bytes(nonce: &str) -> Vec<u8> {
        let total = (1_u64..=10_000).map(|n| n * n).sum::<u64>();
        format!("owned-native-result-v1\nnonce={nonce}\nsum_squares={total}\n").into_bytes()
    }
    fn child(nonce: &str) -> Result<()> {
        if nonce.len() != 36 || !nonce.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err("invalid fixture nonce".into());
        }
        let mut line = String::new();
        std::io::stdin().lock().take(128).read_line(&mut line)?;
        if line != format!("compute {nonce}\n") {
            // Retain the owned bounded fixture briefly after an observation
            // failure closes stdin, so an operator can inspect actual numeric
            // cgroup controls. This never computes or bypasses admission.
            thread::sleep(Duration::from_secs(45));
            return Err("missing owned launch barrier".into());
        }
        write_new(Path::new("result.txt"), &result_bytes(nonce))?;
        println!("owned deterministic computation complete");
        Ok(())
    }
    fn check_result(expected: &str) -> Result<()> {
        let bytes = read_owned_file(Path::new("result.txt"), MAX_BYTES)?;
        if content_digest(&bytes)? != expected {
            return Err("actual result differs from protected expected value".into());
        }
        println!("actual result matches protected value");
        Ok(())
    }
    fn now() -> Result<u64> {
        Ok(u64::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        )?)
    }
    fn observed_utc() -> Result<String> {
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let days = i64::try_from(seconds / 86_400)?;
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let day_of_era = z - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let mut year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_prime = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
        let month = month_prime + if month_prime < 10 { 3 } else { -9 };
        year += i64::from(month <= 2);
        let day_seconds = seconds % 86_400;
        Ok(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            day_seconds / 3600,
            day_seconds / 60 % 60,
            day_seconds % 60
        ))
    }
    fn private_directory(path: &Path) -> Result<()> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
            || fs::canonicalize(path)? != path
        {
            return Err("directory must be canonical".into());
        }
        let mut current = PathBuf::new();
        for c in path.components() {
            current.push(c);
            if fs::symlink_metadata(&current)?.file_type().is_symlink() {
                return Err("symlink directory refused".into());
            }
        }
        let m = fs::symlink_metadata(path)?;
        if !m.is_dir()
            || m.uid() != rustix::process::geteuid().as_raw()
            || m.mode() & 0o777 != 0o700
        {
            return Err("directory must be owned mode0700".into());
        }
        Ok(())
    }
    fn create_private(path: &Path) -> Result<()> {
        fs::DirBuilder::new().mode(0o700).create(path)?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        private_directory(path)
    }
    fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
    fn read_owned_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
        read_file(path, limit, true)
    }
    fn read_file(path: &Path, limit: usize, single_link: bool) -> Result<Vec<u8>> {
        let mut f = fs::OpenOptions::new()
            .read(true)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)?;
        let before = f.metadata()?;
        if !before.is_file()
            || before.uid() != rustix::process::geteuid().as_raw()
            || (single_link && before.nlink() != 1)
            || before.len() > limit as u64
        {
            return Err("owned bounded file required".into());
        }
        let mut bytes = Vec::new();
        (&mut f).take(limit as u64 + 1).read_to_end(&mut bytes)?;
        let after = f.metadata()?;
        if bytes.len() > limit
            || before.len() != bytes.len() as u64
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err("file changed while read".into());
        }
        Ok(bytes)
    }
    fn cpu_set(value: &str) -> Result<BTreeSet<u32>> {
        let mut cpus = BTreeSet::new();
        for item in value.split(',') {
            let n = item.parse::<u32>()?;
            if cpus.len() >= 256 || !cpus.insert(n) {
                return Err("CPU list must be unique and bounded".into());
            }
        }
        if cpus.is_empty() {
            return Err("explicit CPUs required".into());
        }
        Ok(cpus)
    }
    fn source(producer: &str, incarnation: &str, seq: u64) -> SourceIdentity {
        SourceIdentity {
            producer_id: producer.into(),
            incarnation: incarnation.into(),
            sequence: seq.to_string(),
        }
    }
    fn host_environment(home: &Path, tmp: &Path) -> Vec<(OsString, OsString)> {
        [
            ("PATH", "/usr/bin:/bin".to_owned()),
            ("HOME", home.to_string_lossy().into_owned()),
            ("TMPDIR", tmp.to_string_lossy().into_owned()),
            ("LANG", "C".into()),
            ("LC_ALL", "C".into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
            ("GIT_TERMINAL_PROMPT", "0".into()),
            ("GIT_NO_REPLACE_OBJECTS", "1".into()),
            ("GIT_NO_LAZY_FETCH", "1".into()),
            ("GIT_ALLOW_PROTOCOL", "file".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }
    /// All Git setup commands are fixed argv in a fresh owned repo, bounded and
    /// captured via SessionHost. No inherited Git configuration, hooks or pager.
    fn git(
        host: &SessionHost,
        root: &Path,
        checkout: &Path,
        args: &[&str],
        seq: &mut u32,
    ) -> Result<Vec<u8>> {
        *seq += 1;
        let run_id = format!("probe-git-{seq}");
        let program = fs::canonicalize("/usr/bin/git")?;
        let m = fs::metadata(&program)?;
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err("trusted system Git required".into());
        }
        let mut argv = vec![
            "--no-pager".into(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "core.fsmonitor=false".into(),
            "-c".into(),
            "core.attributesFile=/dev/null".into(),
            "-c".into(),
            "protocol.allow=never".into(),
        ];
        argv.extend(args.iter().map(|s| (*s).into()));
        host.launch_with_environment(
            LaunchSpec {
                run_id: run_id.clone(),
                program,
                args: argv,
                cwd: checkout.into(),
                spool_dir: root.join("git-spool").join(&run_id),
            },
            &host_environment(&root.join("home"), &root.join("tmp")),
        )?;
        host.close_input(&run_id)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = host.status(&run_id)?;
            if s.capture_failed || s.output_offset > MAX_BYTES as u64 || Instant::now() >= deadline
            {
                let _ = host.terminate(&run_id);
                return Err("owned Git fixture failed bounds".into());
            }
            if s.exit.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !host.wait(&run_id)?.success() {
            return Err("owned Git command failed".into());
        }
        let status = host.status(&run_id)?;
        if status.capture_state != CaptureState::Complete {
            return Err("Git capture incomplete".into());
        }
        let page = host.tail(&run_id, 0, MAX_BYTES)?;
        if page.next_offset != status.output_offset {
            return Err("Git capture truncated".into());
        }
        Ok(page
            .chunks
            .into_iter()
            .filter(|c| c.stream == OutputStream::Stdout)
            .flat_map(|c| c.bytes)
            .collect())
    }
    fn text(bytes: Vec<u8>) -> Result<String> {
        Ok(String::from_utf8(bytes)?.trim().into())
    }

    struct StartupAuthority {
        // Exact operator-approved startup configuration, outside the task fixture.
        // The fixed child receives neither its contents nor its path.
        authorization_snapshot: Vec<u8>,
        uid: u32,
        pid: u32,
        phase: Arc<AtomicU8>,
        expected: ResourcePermitClaims,
    }
    impl Drop for StartupAuthority {
        fn drop(&mut self) {
            self.authorization_snapshot.fill(0);
        }
    }
    impl ResourcePermitAuthority for StartupAuthority {
        fn is_current(&self, c: &ResourcePermitClaims) -> bool {
            self.authorization_snapshot.len() >= 32
                && rustix::process::geteuid().as_raw() == self.uid
                && std::process::id() == self.pid
                && self.phase.load(Ordering::SeqCst) <= 1
                && c == &self.expected
                && now().is_ok_and(|n| n < c.expires_at_unix_ms && n < c.lease_expires_at_unix_ms)
        }
    }
    struct EvidenceAuthority {
        startup: Arc<StartupAuthority>,
        phase: Arc<AtomicU8>,
        expected: VerificationInputs,
        worker: PrincipalId,
        verifier: PrincipalId,
        authority_ref: String,
        execution_digest: String,
        receipt_digest: String,
        closure_digest: String,
        closed_bytes: Vec<u8>,
        checks_bytes: Vec<u8>,
        custody_digest: String,
    }
    impl EvidenceAuthority {
        fn binding_matches(&self, b: &NativeDecisionBinding) -> bool {
            let i = &self.expected;
            b.project_id == i.project_id
                && b.root_id == i.root_id
                && b.task_id == i.task_id
                && b.contract_digest == i.contract_digest
                && b.attempt_id == i.attempt_id
                && b.worker == self.worker
                && b.verifier == self.verifier
                && b.worker != b.verifier
                && b.authority_ref == self.authority_ref
                && b.policy_digest == i.verification_policy_digest
                && b.protected_oracle_digest == i.protected_oracle_digest
                && b.source_artifact_digest == i.source_artifact_digest
                && b.candidate_ref == i.candidate_ref
                && b.source_commit_sha == i.source_commit_sha
                && b.source_tree_sha == i.source_tree_sha
                && b.checks_execution_digest == self.execution_digest
                && b.receipt_digest == self.receipt_digest
                && b.closure_digest == self.closure_digest
        }
        fn current(&self) -> bool {
            self.startup.authorization_snapshot.len() >= 32
                && std::process::id() == self.startup.pid
                && rustix::process::geteuid().as_raw() == self.startup.uid
                && self.phase.load(Ordering::SeqCst) == 2
                && now().is_ok_and(|n| n < self.startup.expected.lease_expires_at_unix_ms)
        }
    }
    impl NativeEvidenceAuthority for EvidenceAuthority {
        fn authenticate_evidence(
            &self,
            b: &NativeDecisionBinding,
            checks: &ExecutedChecks,
            closed: &ClosedAdmission,
        ) -> Option<String> {
            (self.current()
                && self.binding_matches(b)
                && closed.success()
                && closed.identity().identity == self.startup.expected.identity
                && closed.identity().launch_spec_digest == self.startup.expected.launch_spec_digest
                && closed.identity().worker_environment_digest
                    == self.startup.expected.worker_environment_digest
                && closed.identity().backend_config_digest
                    == self.startup.expected.backend_config_digest
                && checks.execution_digest() == self.execution_digest
                && serde_json::to_vec(checks).ok().as_ref() == Some(&self.checks_bytes)
                && serde_json::to_vec(&closed.authoritative_binding())
                    .ok()
                    .as_ref()
                    == Some(&self.closed_bytes))
            .then(|| self.custody_digest.clone())
        }
        fn remains_current(&self, b: &NativeDecisionBinding, custody: &str) -> bool {
            self.current() && self.binding_matches(b) && custody == self.custody_digest
        }
    }
    #[cfg(target_os = "linux")]
    fn prove_closed(handle: &mut RunningAdmission, host: &SessionHost) -> Result<ClosedAdmission> {
        Ok(handle.prove_closed(host)?)
    }
    #[cfg(not(target_os = "linux"))]
    fn prove_closed(_: &mut RunningAdmission, _: &SessionHost) -> Result<ClosedAdmission> {
        Err("Linux closure proof required".into())
    }
    fn stored(objects: &ArtifactStore, bytes: &[u8]) -> Result<ArtifactRef> {
        let r = objects.publish_bytes(bytes)?;
        if objects.read_verified(&r)? != bytes {
            return Err("artifact verification failed".into());
        }
        Ok(r)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        root: PathBuf,
        authorization_file: Option<PathBuf>,
        prepare_only: bool,
        node: String,
        cpus: String,
        registered_cpus: String,
        runtime_directory: PathBuf,
        slice: String,
        memory_mib: u64,
        max_tasks: u32,
        deadline_seconds: u64,
    ) -> Result<()> {
        if !(64..=512).contains(&memory_mib)
            || !(4..=32).contains(&max_tasks)
            || !(30..=120).contains(&deadline_seconds)
        {
            return Err("probe resource bounds exceeded".into());
        }
        if prepare_only && authorization_file.is_some() {
            return Err("prepare-only must not receive authorization".into());
        }
        let authorization = match authorization_file {
            Some(path) => {
                if path.starts_with(&root) {
                    return Err("operator authorization must be outside the task fixture".into());
                }
                let text = read_private_token(&path)?;
                Some(serde_json::from_str::<Value>(&text)?)
            }
            None if prepare_only => None,
            None => return Err("protected operator authorization is required".into()),
        };
        let parent = root.parent().ok_or("missing fixture parent")?;
        private_directory(parent)?;
        if prepare_only {
            create_private(&root)?;
        } else {
            private_directory(&root)?;
        }
        let result = run_owned(
            &root,
            authorization,
            prepare_only,
            node,
            &cpus,
            &registered_cpus,
            runtime_directory,
            slice,
            memory_mib,
            max_tasks,
            deadline_seconds,
        );
        if let Err(ref error) = result {
            let _ = write_new(
                &root.join("failure.json"),
                &serde_json::to_vec_pretty(
                    &json!({"schema_version":1,"status":"failed","native_execution_qualified":false,"error":error.to_string(),"next_action":"retain recovery identity; reconcile only the owned transient unit; no automatic retry"}),
                )?,
            );
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn run_owned(
        root: &Path,
        authorization: Option<Value>,
        prepare_only: bool,
        node: String,
        cpus: &str,
        registered_cpus: &str,
        runtime_directory: PathBuf,
        slice: String,
        memory_mib: u64,
        max_tasks: u32,
        deadline_seconds: u64,
    ) -> Result<()> {
        let nonce = if let Some(value) = &authorization {
            value["fixture_nonce"]
                .as_str()
                .ok_or("authorization nonce missing")?
                .to_owned()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        uuid::Uuid::parse_str(&nonce)?;
        for name in [
            "candidate",
            "objects",
            "checks",
            "git-spool",
            "worker-spool",
            "home",
            "tmp",
            "bin",
        ] {
            if prepare_only {
                create_private(&root.join(name))?;
            } else {
                private_directory(&root.join(name))?;
            }
        }
        let checkout = root.join("candidate");
        let objects = ArtifactStore::open(root.join("objects"), 128 * 1024 * 1024)?;
        let current_executable = fs::canonicalize(std::env::current_exe()?)?;
        let executable_bytes = read_file(&current_executable, 128 * 1024 * 1024, false)?;
        let executable = root.join("bin/native-lifecycle-probe");
        if prepare_only {
            write_new(&executable, &executable_bytes)?;
            fs::set_permissions(
                &executable,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            )?;
        }
        if read_owned_file(&executable, 128 * 1024 * 1024)? != executable_bytes {
            return Err("prepared executable differs from current trusted probe".into());
        }
        let exe_object = stored(&objects, &executable_bytes)?;
        let oracle = stored(&objects, &result_bytes(&nonce))?;
        let negative = stored(&objects, b"negative control: not the expected result\n")?;
        let host = SessionHost::new();
        let mut git_seq = if prepare_only { 0 } else { 1000 };
        if prepare_only {
            git(&host, root, &checkout, &["init", "-q"], &mut git_seq)?;
        }
        if fs::read_dir(&checkout)?.any(|e| e.is_ok_and(|e| e.file_name() != ".git")) {
            return Err("candidate fixture must contain only initial .git".into());
        }
        let project = ProjectId::new(format!("native-probe-{nonce}"))?;
        let root_id = RootId::new(format!("root-{nonce}"))?;
        let task_id = TaskId::new(format!("task-{nonce}"))?;
        let attempt = AttemptId::new(format!("attempt-{nonce}"))?;
        let node = NodeId::new(node)?;
        let worker = PrincipalId::new("deterministic-probe-worker")?;
        let verifier = PrincipalId::new("protected-probe-verifier")?;
        let authority_ref = format!("probe-grant:{nonce}");
        let incarnation = nonce.clone();
        let producer = "native-probe-verifier-service";
        let check = CheckSpec {
            id: "result-matches-oracle".into(),
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "check-result".into(),
                "--expected-value-digest".into(),
                content_digest(&result_bytes(&nonce))?,
            ],
            cwd: ".".into(),
            timeout_secs: 10,
        };
        let policy = VerificationPolicy {
            schema_version: 1,
            verifier: verifier.clone(),
            protected_oracle_digest: oracle.sha256.clone(),
            required_negative_checks: vec![RequiredNegativeCheck {
                check: CheckSpec {
                    id: "wrong-oracle-rejected".into(),
                    args: vec![
                        "check-result".into(),
                        "--expected-value-digest".into(),
                        content_digest(&objects.read_verified(&negative)?)?,
                    ],
                    ..check.clone()
                },
                expected_exit_code: 17,
                fixture_digest: negative.sha256.clone(),
            }],
        };
        let contract = TaskContract {
            schema_version: 1,
            project_id: project.clone(),
            root_id: root_id.clone(),
            task_id: task_id.clone(),
            revision: 1,
            title: "Owned deterministic native lifecycle integration".into(),
            objective:
                "Compute the protected deterministic result and qualify exact acceptance wiring"
                    .into(),
            non_goals: vec![
                "No provider coding, network, production rollout or fleet qualification".into(),
            ],
            owned_paths: vec!["result.txt".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "result".into(),
                text: "Actual result matches protected oracle; wrong oracle is rejected".into(),
                check_ids: vec![check.id.clone()],
            }],
            checks: vec![check],
            authority_ref: authority_ref.clone(),
            max_attempts: 1,
            simulated: false,
        };
        contract.validate()?;
        let spec = LaunchSpec {
            run_id: format!("native-probe-{nonce}"),
            program: executable.clone(),
            args: vec!["owned-child".into(), "--nonce".into(), nonce.clone()],
            cwd: checkout.clone(),
            spool_dir: root.join("worker-spool").join("owned-child"),
        };
        let environment = QualifiedEnvironment::new(BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            (
                "HOME".into(),
                root.join("home").to_string_lossy().into_owned(),
            ),
            (
                "TMPDIR".into(),
                root.join("tmp").to_string_lossy().into_owned(),
            ),
            ("LANG".into(), "C".into()),
            ("LC_ALL".into(), "C".into()),
        ]))?;
        let backend = SystemdBackendConfig {
            runtime_directory,
            delegated_slice: slice,
        };
        let requested_cpus = cpu_set(cpus)?;
        let registered_cpu_scope = cpu_set(registered_cpus)?;
        if requested_cpus.len() > 2 || !requested_cpus.is_subset(&registered_cpu_scope) {
            return Err("probe CPUs exceed explicit registered scope".into());
        }
        let expires = if let Some(value) = &authorization {
            value["claims"]["expires_at_unix_ms"]
                .as_u64()
                .ok_or("authorization expiry missing")?
        } else {
            now()?
                .checked_add(deadline_seconds * 1000)
                .ok_or("deadline overflow")?
        };
        if expires <= now()? || expires > now()?.saturating_add(deadline_seconds * 1000) {
            return Err("authorization deadline outside requested runtime bound".into());
        }
        let claims = ResourcePermitClaims {
            issuer_ref: format!("probe-issuer-{nonce}"),
            permit_id: format!("probe-permit-{nonce}"),
            reservation_ref: format!("probe-reservation-{nonce}"),
            identity: AdmissionIdentity {
                project_id: project.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt.clone(),
                node_id: node.clone(),
                lease_id: format!("probe-lease-{nonce}"),
            },
            host_incarnation: fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .into(),
            expires_at_unix_ms: expires,
            lease_expires_at_unix_ms: expires,
            registered_cpu_scope,
            cpu_set: requested_cpus,
            memory_max_bytes: memory_mib * 1024 * 1024,
            max_tasks,
            max_concurrency: 1,
            launch_spec_digest: content_digest(&spec)?,
            worker_environment_digest: environment.digest()?,
            backend_config_digest: backend.digest()?,
        };
        let claim_value = json!({"issuer_ref":claims.issuer_ref,"permit_id":claims.permit_id,"reservation_ref":claims.reservation_ref,"identity":claims.identity,"host_incarnation":claims.host_incarnation,"expires_at_unix_ms":claims.expires_at_unix_ms,"lease_expires_at_unix_ms":claims.lease_expires_at_unix_ms,"registered_cpu_scope":claims.registered_cpu_scope,"cpu_set":claims.cpu_set,"memory_max_bytes":claims.memory_max_bytes,"max_tasks":claims.max_tasks,"max_concurrency":claims.max_concurrency,"launch_spec_digest":claims.launch_spec_digest,"worker_environment_digest":claims.worker_environment_digest,"backend_config_digest":claims.backend_config_digest});
        let request = json!({"schema_version":1,"scope":"one-deterministic-native-lifecycle-probe","fixture_root":root,"fixture_nonce":nonce,"uid":rustix::process::geteuid().as_raw(),"executable_artifact":{"sha256":exe_object.sha256,"size_bytes":exe_object.size_bytes},"contract_digest":content_digest(&contract)?,"verification_policy_digest":content_digest(&policy)?,"oracle_artifact":{"sha256":oracle.sha256,"size_bytes":oracle.size_bytes},"negative_control_artifact":{"sha256":negative.sha256,"size_bytes":negative.size_bytes},"claims":claim_value});
        if prepare_only {
            write_new(
                &root.join("operator-authorization-request.json"),
                serde_json::to_string(&request)?.as_bytes(),
            )?;
            println!(
                "Prepared only; no permit or child. A trusted operator must inspect and copy the exact compact request into a separately protected startup authorization file before expiry. Fixture: {}",
                root.display()
            );
            return Ok(());
        }
        let approved = authorization.ok_or("protected operator authorization missing")?;
        if approved != request {
            return Err(
                "protected operator authorization does not match exact prepared fixture/claims"
                    .into(),
            );
        }
        let prepared: Value = serde_json::from_slice(&read_owned_file(
            &root.join("operator-authorization-request.json"),
            4096,
        )?)?;
        if prepared != request {
            return Err("prepared request differs from protected operator authorization".into());
        }
        write_new(
            &root.join("run-started.json"),
            &serde_json::to_vec(
                &json!({"authorization_digest":content_digest(&approved)?,"started_at_unix_ms":now()?}),
            )?,
        )?;
        let authorization_snapshot = serde_json::to_vec(&approved)?;
        let authorization_digest = content_digest(&approved)?;
        write_new(
            &root.join("reservation.json"),
            &serde_json::to_vec_pretty(
                &json!({"claims":claim_value,"authorization_digest":authorization_digest,"status":"held-for-isolated-probe"}),
            )?,
        )?;
        let phase = Arc::new(AtomicU8::new(0));
        let startup = Arc::new(StartupAuthority {
            authorization_snapshot,
            uid: rustix::process::geteuid().as_raw(),
            pid: std::process::id(),
            phase: phase.clone(),
            expected: claims.clone(),
        });
        let mut forged = claims.clone();
        forged.memory_max_bytes += 4096;
        if VerifiedResourcePermit::verify_current(forged, startup.clone()).is_ok() {
            return Err("modified reservation unexpectedly authorized".into());
        }
        let permit = VerifiedResourcePermit::verify_current(claims.clone(), startup.clone())?;
        let admitted = ResourceGate::configured(node, backend, environment).prepare(
            permit,
            claims.identity.clone(),
            spec.clone(),
        )?;
        write_new(
            &root.join("recovery-before-launch.json"),
            &serde_json::to_vec_pretty(&admitted.recovery_identity())?,
        )?;
        phase.store(1, Ordering::SeqCst);
        let mut running = match admitted.launch(&host, Duration::from_secs(deadline_seconds)) {
            Ok(handle) => handle,
            Err(failure) => {
                if let Some(handle) = &failure.recovery {
                    write_new(
                        &root.join("unknown-launch.json"),
                        &serde_json::to_vec_pretty(
                            &json!({"recovery":handle.recovery_identity(),"observation":handle.observation_diagnostics()}),
                        )?,
                    )?;
                }
                return Err(Box::new(failure));
            }
        };
        write_new(
            &root.join("observed-running.json"),
            &serde_json::to_vec_pretty(
                &json!({"recovery":running.recovery_identity(),"observation":running.observation_diagnostics()}),
            )?,
        )?;
        host.write_input(
            &spec.run_id,
            InputKind::Automation { expected_epoch: 0 },
            format!("compute {nonce}\n").as_bytes(),
        )?;
        host.close_input(&spec.run_id)?;
        let deadline = Instant::now() + Duration::from_secs(deadline_seconds);
        let closed = loop {
            match prove_closed(&mut running, &host) {
                Ok(closed) => break closed,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(30)),
                Err(error) => {
                    write_new(
                        &root.join("unknown-closure.json"),
                        &serde_json::to_vec_pretty(
                            &json!({"recovery":running.recovery_identity(),"observation":running.observation_diagnostics()}),
                        )?,
                    )?;
                    return Err(error);
                }
            }
        };
        if !closed.success() {
            return Err("owned computation did not succeed".into());
        }
        phase.store(2, Ordering::SeqCst);
        let result = read_owned_file(&checkout.join("result.txt"), MAX_BYTES)?;
        if result != objects.read_verified(&oracle)? {
            return Err("computed result does not match protected oracle".into());
        }
        git(
            &host,
            root,
            &checkout,
            &["add", "--", "result.txt"],
            &mut git_seq,
        )?;
        git(
            &host,
            root,
            &checkout,
            &[
                "-c",
                "user.name=Native Probe Fixture",
                "-c",
                "user.email=probe@example.invalid",
                "commit",
                "-qm",
                "Record owned deterministic result",
            ],
            &mut git_seq,
        )?;
        let commit = text(git(
            &host,
            root,
            &checkout,
            &["rev-parse", "HEAD"],
            &mut git_seq,
        )?)?;
        let tree = text(git(
            &host,
            root,
            &checkout,
            &["rev-parse", "HEAD^{tree}"],
            &mut git_seq,
        )?)?;
        let source_object = stored(
            &objects,
            &git(
                &host,
                root,
                &checkout,
                &["archive", "--format=tar", "HEAD"],
                &mut git_seq,
            )?,
        )?;
        let inputs = VerificationInputs {
            project_id: project.clone(),
            root_id: root_id.clone(),
            task_id: task_id.clone(),
            attempt_id: attempt.clone(),
            candidate_ref: source_object.sha256.clone(),
            contract_revision: 1,
            contract_digest: content_digest(&contract)?,
            verification_policy_digest: content_digest(&policy)?,
            source_commit_sha: commit.clone(),
            source_tree_sha: tree.clone(),
            source_artifact_digest: source_object.sha256.clone(),
            protected_oracle_digest: oracle.sha256.clone(),
        };
        let checks = execute_checks(
            &host,
            &inputs,
            &contract,
            &policy,
            &checkout,
            &root.join("checks"),
            "native-integration-verifier",
        )?;
        let checks_bytes = serde_json::to_vec(&checks)?;
        let closed_bytes = serde_json::to_vec(&closed.authoritative_binding())?;
        let checks_object = stored(&objects, &checks_bytes)?;
        let closed_object = stored(&objects, &closed_bytes)?;
        let receipt_object = stored(&objects, &serde_json::to_vec(checks.receipt())?)?;
        let policy_object = stored(&objects, &serde_json::to_vec(&policy)?)?;
        // Read and verify every immutable object before installing the callback.
        // The callback uses this exact bounded custody snapshot without I/O in
        // Store's writer turn. It is not a generic trust-any-receipt predicate.
        for reference in [
            &source_object,
            &oracle,
            &negative,
            &checks_object,
            &closed_object,
            &receipt_object,
            &policy_object,
            &exe_object,
        ] {
            objects.read_verified(reference)?;
        }
        let custody = stored(
            &objects,
            &serde_json::to_vec(
                &json!({"kind":"owned-native-integration-custody-v1","verifier":verifier,"worker":worker,"authority_generation":nonce,"startup_authorization_digest":authorization_digest,"source":source_object.sha256,"oracle":oracle.sha256,"negative_control":negative.sha256,"checks":checks_object.sha256,"receipt":receipt_object.sha256,"closure":closed_object.sha256,"policy":policy_object.sha256,"executable":exe_object.sha256,"commit":commit,"tree":tree,"no_external_effects":"fixed native child wrote only the owned result; fixed local Git/check argv; no network/provider operations"}),
            )?,
        )?;
        let evidence = Arc::new(EvidenceAuthority {
            startup,
            phase: phase.clone(),
            expected: inputs.clone(),
            worker: worker.clone(),
            verifier: verifier.clone(),
            authority_ref: authority_ref.clone(),
            execution_digest: checks.execution_digest().into(),
            receipt_digest: receipt_object.sha256.clone(),
            closure_digest: closed.closure_digest().into(),
            closed_bytes,
            checks_bytes,
            custody_digest: custody.sha256.clone(),
        });
        let authority = Arc::new(NativeLifecycleAuthority::new(
            NativeLifecyclePolicy {
                project_id: project,
                root_id,
                authority_ref,
                source_producer: producer.into(),
                source_incarnation: incarnation.clone(),
                verification: policy,
            },
            evidence,
            4,
        )?);
        let database = root.join("probe.sqlite");
        let store = Store::open_with_lifecycle_authority(&database, authority.clone())?;
        for (i, event) in [
            DomainEvent::TaskSubmitted { contract },
            DomainEvent::TaskClaimed {
                task_id: task_id.clone(),
                revision: 1,
                attempt_id: attempt.clone(),
                owner: worker,
            },
            DomainEvent::TaskStarted {
                task_id: task_id.clone(),
                attempt_id: attempt.clone(),
            },
            DomainEvent::CandidateSubmitted {
                task_id: task_id.clone(),
                attempt_id: attempt,
                candidate_ref: inputs.candidate_ref.clone(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            store.append(
                source("probe-controller", &incarnation, (i + 1) as u64),
                event,
                observed_utc()?,
            )?;
        }
        let task = store
            .snapshot()?
            .projection
            .tasks
            .remove(&task_id)
            .ok_or("task missing")?;
        let decision_source = source(producer, &incarnation, 1);
        let command =
            authority.issue_acceptance(&decision_source, &task, &checks, Some(&closed))?;
        let accepted = store.execute_lifecycle(
            decision_source.clone(),
            command.clone(),
            observed_utc()?,
            None,
        )?;
        let duplicate = store.execute_lifecycle(
            decision_source.clone(),
            command.clone(),
            observed_utc()?,
            None,
        )?;
        if !duplicate.duplicate || duplicate.event_seq != accepted.event_seq {
            return Err("lost-reply replay was not idempotent".into());
        }
        phase.store(3, Ordering::SeqCst);
        if store
            .execute_lifecycle(decision_source, command, observed_utc()?, None)
            .is_ok()
        {
            return Err("revoked authority unexpectedly authorized replay".into());
        }
        let projected = store.snapshot()?.projection;
        if projected.tasks[&task_id].state != SchedulingState::Accepted {
            return Err("task was not accepted".into());
        }
        drop(store);
        drop(authority);
        let reopened = Store::open(&database)?;
        if reopened.snapshot()?.projection != projected {
            return Err("accepted projection failed reopen replay".into());
        }
        drop(reopened);
        let report = json!({"schema_version":1,"status":"passed","scope":"isolated deterministic native lifecycle integration; not a coding provider or fleet rollout","native_execution_qualified":false,"root":root,"task_id":task_id,"accepted_event":accepted.event_seq,"exact_duplicate":duplicate.duplicate,"reopen_projection_matched":true,"revocation_denied":true,"modified_resource_claim_denied":true,"source_commit":commit,"source_tree":tree,"checks_object":checks_object.sha256,"closure_object":closed_object.sha256,"custody_object":custody.sha256,"recovery_identity":closed.identity()});
        write_new(
            &root.join("receipt.json"),
            &serde_json::to_vec_pretty(&report)?,
        )?;
        println!("{}", serde_json::to_string(&report)?);
        Ok(())
    }
}
#[cfg(unix)]
fn main() -> std::process::ExitCode {
    probe::main()
}
#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("Unsupported: Linux systemd/cgroup integration is required");
    std::process::ExitCode::from(2)
}
