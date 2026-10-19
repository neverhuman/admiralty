//! Explicit, one-run Linux proof of NativeTaskLoop -> retained workspace/closure
//! -> changed source -> independent positive/negative checks -> Store acceptance.
//! This deterministic helper is not a coding provider, general issuer, startup
//! activation, production rollout, or full packet acceptance. No network calls.
//! Preparation is create-only; run requires the exact fresh private authorization
//! outside the fixture. Any failed/unknown run is retained and MUST NOT be retried.
//! The current consuming source-candidate API can lose in-memory handles on an
//! error; journals/files remain, but this probe makes no restart recovery claim.
#[cfg(unix)]
mod probe {
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, LifecycleCommand, PrincipalId,
        ProjectId, RootId, SchedulingState, SourceIdentity, TaskContract, TaskId, TaskView,
        content_digest,
        nodes::NodeId,
        verification::{RequiredNegativeCheck, VerificationInputs, VerificationPolicy},
    };
    use admiralty_runtime::{
        reservations::{AccountId, CurrencyUnit, MinorUnits, Reservation, ReservationScope},
        resource_gate::{
            AdmissionIdentity, ClosedAdmission, QualifiedEnvironment, ResourceGate,
            ResourcePermitAuthority, ResourcePermitClaims, SystemdBackendConfig,
            VerifiedResourcePermit,
        },
        session::{CaptureState, LaunchSpec, OutputStream, SessionHost},
        trusted_checks::{ExecutedChecks, execute_checks},
        workspace::{
            preparation::{WorkspacePreparation, prepare_workspace},
            registry::{NodeRef, WorkspaceSpec},
            source_candidate::{materialize_source_candidate, prove_closed_workspace},
        },
    };
    use admiralty_server::services::{
        controller::{
            CurrentAdmission, DispatchAdmission, DispatchIntent, LaunchRequest, WallClock,
        },
        native_executor::{NativeExecutionPlan, native_resource_claims_digest},
        native_lifecycle::{
            NativeDecisionBinding, NativeEvidenceAuthority, NativeLifecycleAuthority,
            NativeLifecyclePolicy,
        },
        native_loop::StartedScope,
        native_task_loop::{
            AdmissionStatus, CompletionStatus, EffectAckState, NativeTaskLoop,
            NativeTaskLoopConfig, NativeTaskRuntimeConfig, TaskDispatchStatus,
        },
    };
    use admiralty_store::{
        LifecycleAuthority, LifecycleEffect, Store,
        artifacts::{ArtifactRef, ArtifactStore},
    };
    use clap::{Args, Parser, Subcommand};
    use serde_json::{Value, json};
    use std::{
        collections::{BTreeMap, BTreeSet},
        error::Error,
        ffi::OsString,
        fs,
        io::{Read, Write},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        path::{Component, Path, PathBuf},
        sync::{
            Arc, OnceLock,
            atomic::{AtomicU8, Ordering},
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
    const MAX_BYTES: usize = 1024 * 1024;
    const AUTH_BYTES: usize = 64 * 1024;
    const ORACLE_MISMATCH_EXIT: u8 = 23;
    const SOURCE_SHA: Option<&str> = option_env!("ADMIRALTY_PROBE_SOURCE_SHA");

    // Only a successful command with complete bounded output can construct
    // this outcome. Compiler/setup/capture failures are infrastructure errors;
    // they must never satisfy the required wrong-oracle negative check.
    #[derive(Debug)]
    struct OracleMismatch;
    impl std::fmt::Display for OracleMismatch {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("compiled source differs from protected oracle")
        }
    }
    impl Error for OracleMismatch {}
    fn failure_exit(error: &(dyn Error + Send + Sync + 'static)) -> u8 {
        if error.is::<OracleMismatch>() {
            ORACLE_MISMATCH_EXIT
        } else {
            17
        }
    }
    fn verify_compiled_output(actual: Result<Vec<u8>>, expected: &str) -> Result<()> {
        let actual = actual?;
        if content_digest(&actual)? != expected {
            return Err(OracleMismatch.into());
        }
        Ok(())
    }

    #[derive(Parser)]
    #[command(about = "One-run actual Linux NativeTaskLoop proof; no provider/fleet qualification")]
    struct Cli {
        #[command(subcommand)]
        command: Mode,
    }
    #[derive(Subcommand)]
    enum Mode {
        Run(RunArgs),
        #[command(hide = true)]
        OwnedChild {
            #[arg(long)]
            nonce: String,
        },
        #[command(hide = true)]
        CheckSource {
            #[arg(long)]
            rustc: PathBuf,
            #[arg(long)]
            rustc_digest: String,
            #[arg(long)]
            output: PathBuf,
            #[arg(long)]
            expected_value_digest: String,
        },
    }
    #[derive(Args)]
    struct RunArgs {
        /// New canonical fixture path beneath an existing owned mode-0700 parent.
        #[arg(long)]
        root: PathBuf,
        /// Exact request copied outside fixture into an owned mode-0400 file.
        #[arg(long)]
        operator_authorization_file: Option<PathBuf>,
        /// Creates toy base/request only; no resource permit, workspace or worker.
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
        /// Canonical actual compiler binary, not a rustup shim or shell wrapper.
        #[arg(long)]
        rustc: PathBuf,
        #[arg(long, default_value_t = 256)]
        memory_mib: u64,
        #[arg(long, default_value_t = 16)]
        max_tasks: u32,
        #[arg(long, default_value_t = 90)]
        deadline_seconds: u64,
    }
    pub fn main() -> std::process::ExitCode {
        let cli = Cli::parse();
        if !cfg!(target_os = "linux") {
            eprintln!(
                "Unsupported: effectful modes require actual Linux systemd/cgroup qualification"
            );
            return std::process::ExitCode::from(2);
        }
        let result = match cli.command {
            Mode::Run(args) => run(args),
            Mode::OwnedChild { nonce } => child(&nonce),
            Mode::CheckSource {
                rustc,
                rustc_digest,
                output,
                expected_value_digest,
            } => check_source(&rustc, &rustc_digest, &output, &expected_value_digest),
        };
        match result {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Native task-loop proof did not qualify: {error}");
                std::process::ExitCode::from(failure_exit(error.as_ref()))
            }
        }
    }
    fn valid_nonce(nonce: &str) -> bool {
        uuid::Uuid::parse_str(nonce).is_ok_and(|parsed| parsed.to_string() == nonce)
    }
    fn result_bytes(nonce: &str) -> Vec<u8> {
        format!(
            "owned-task-loop-v1 {nonce} {}\n",
            (1_u64..=10_000).map(|n| n * n).sum::<u64>()
        )
        .into_bytes()
    }
    fn toy_source(nonce: &str, fixed: bool) -> Result<Vec<u8>> {
        if !valid_nonce(nonce) {
            return Err("invalid fixture nonce".into());
        }
        let expression = if fixed {
            "(1_u64..=10_000).map(|n| n*n).sum::<u64>()"
        } else {
            "0_u64"
        };
        Ok(format!(
            "fn main() {{ println!(\"owned-task-loop-v1 {nonce} {{}}\", {expression}); }}\n"
        )
        .into_bytes())
    }
    fn child(nonce: &str) -> Result<()> {
        let before = toy_source(nonce, false)?;
        let after = toy_source(nonce, true)?;
        if read_owned_file(Path::new("src/main.rs"), 4096)? != before {
            return Err("owned toy base differs".into());
        }
        // NativeExecutor closes stdin. This fixed, noninteractive child remains
        // alive for the actual resource observation before making its sole edit.
        thread::sleep(Duration::from_secs(4));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open("src/main.rs")?;
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.nlink() != 1
            || meta.mode() & 0o777 != 0o600
        {
            return Err("owned toy source metadata differs".into());
        }
        file.set_len(0)?;
        file.write_all(&after)?;
        file.sync_all()?;
        println!("owned toy source changed");
        Ok(())
    }
    fn check_source(rustc: &Path, digest: &str, output: &Path, expected: &str) -> Result<()> {
        if !sha256(digest) || !sha256(expected) {
            return Err("invalid protected check digest".into());
        }
        validate_program(rustc)?;
        if content_digest(&read_file(rustc, 128 * 1024 * 1024, false)?)? != digest {
            return Err("compiler bytes differ from protected compiler".into());
        }
        private_directory(output.parent().ok_or("missing check parent")?)?;
        create_private(output)?;
        // Retain the exact create-only output directory while the compiler
        // runs; a pathname replacement cannot authorize chmod on another tree.
        let output_directory = open_private_output_directory(output)?;
        let source = read_owned_file(Path::new("src/main.rs"), 4096)?;
        if source.is_empty() {
            return Err("source is empty".into());
        }
        let cwd = fs::canonicalize(".")?;
        let host = SessionHost::new();
        let binary = output.join("toy");
        fixed_command(
            &host,
            "compile",
            rustc,
            &[
                "--edition=2024".into(),
                "-Copt-level=0".into(),
                "src/main.rs".into(),
                "-o".into(),
                binary.to_string_lossy().into_owned(),
            ],
            &cwd,
            output,
            Duration::from_secs(20),
        )?;
        normalize_compiler_output(output, &output_directory)?;
        validate_program(&binary)?;
        verify_compiled_output(
            fixed_command(
                &host,
                "execute",
                &binary,
                &[],
                &cwd,
                output,
                Duration::from_secs(2),
            ),
            expected,
        )?;
        println!("compiled changed source matches protected oracle");
        Ok(())
    }
    fn open_private_output_directory(path: &Path) -> Result<fs::File> {
        private_directory(path)?;
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(
                (rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC)
                    .bits() as i32,
            )
            .open(path)?;
        validate_output_directory(path, &directory)?;
        Ok(directory)
    }
    fn validate_output_directory(path: &Path, directory: &fs::File) -> Result<()> {
        let actual = directory.metadata()?;
        let entry = fs::symlink_metadata(path)?;
        if !actual.is_dir()
            || actual.uid() != rustix::process::geteuid().as_raw()
            || actual.mode() & 0o7777 != 0o700
            || !entry.is_dir()
            || entry.uid() != actual.uid()
            || entry.mode() & 0o7777 != 0o700
            || entry.dev() != actual.dev()
            || entry.ino() != actual.ino()
        {
            return Err("compiler output directory custody changed".into());
        }
        Ok(())
    }
    fn normalize_compiler_output(output: &Path, directory: &fs::File) -> Result<()> {
        validate_output_directory(output, directory)?;
        // No process-global umask mutation. Only the exact new compiler output
        // below our retained private directory may become private/executable.
        let fd = rustix::fs::openat(
            directory,
            "toy",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?;
        let file = fs::File::from(fd);
        let before = file.metadata()?;
        let entry = fs::symlink_metadata(output.join("toy"))?;
        if !before.is_file()
            || before.uid() != rustix::process::geteuid().as_raw()
            || before.nlink() != 1
            || before.mode() & 0o7000 != 0
            || before.mode() & 0o111 == 0
            || before.len() == 0
            || before.len() > 128 * 1024 * 1024
            || !entry.is_file()
            || entry.dev() != before.dev()
            || entry.ino() != before.ino()
        {
            return Err(
                "compiler output must be an owned executable single-link regular file".into(),
            );
        }
        file.set_permissions(fs::Permissions::from_mode(0o700))?;
        file.sync_all()?;
        let after = file.metadata()?;
        let entry = fs::symlink_metadata(output.join("toy"))?;
        if after.dev() != before.dev()
            || after.ino() != before.ino()
            || after.uid() != before.uid()
            || after.nlink() != 1
            || after.len() != before.len()
            || after.mode() & 0o7777 != 0o700
            || !entry.is_file()
            || entry.dev() != after.dev()
            || entry.ino() != after.ino()
        {
            return Err("compiler output changed during private-mode normalization".into());
        }
        validate_output_directory(output, directory)?;
        Ok(())
    }
    fn fixed_command(
        host: &SessionHost,
        id: &str,
        program: &Path,
        args: &[String],
        cwd: &Path,
        parent: &Path,
        deadline: Duration,
    ) -> Result<Vec<u8>> {
        let spec = LaunchSpec {
            run_id: id.into(),
            program: program.into(),
            args: args.to_vec(),
            cwd: cwd.into(),
            spool_dir: parent.join(format!("{id}-spool")),
        };
        host.launch_with_environment(spec, &host_environment(parent, parent))?;
        host.close_input(id)?;
        let end = Instant::now() + deadline;
        loop {
            let status = host.status(id)?;
            if status.capture_state == CaptureState::Failed
                || status.output_offset > MAX_BYTES as u64
                || Instant::now() >= end
            {
                let _ = host.terminate(id);
                return Err("owned check command exceeded bounds; inspect retained spool".into());
            }
            if status.exit.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !host.wait(id)?.success() {
            return Err("owned check command failed".into());
        }
        let status = host.status(id)?;
        let tail = host.tail(id, 0, MAX_BYTES)?;
        if status.capture_state != CaptureState::Complete
            || tail.next_offset != status.output_offset
        {
            return Err("owned check capture incomplete".into());
        }
        Ok(tail
            .chunks
            .into_iter()
            .filter(|c| c.stream == OutputStream::Stdout)
            .flat_map(|c| c.bytes)
            .collect())
    }
    fn sha256(s: &str) -> bool {
        s.strip_prefix("sha256:").is_some_and(|h| {
            h.len() == 64
                && h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    }
    fn validate_program(path: &Path) -> Result<()> {
        if !path.is_absolute() || fs::canonicalize(path)? != path {
            return Err("program must be canonical".into());
        }
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_file()
            || meta.mode() & 0o022 != 0
            || meta.mode() & 0o111 == 0
            || !(meta.uid() == 0 || meta.uid() == rustix::process::geteuid().as_raw())
        {
            return Err("program metadata is unsafe".into());
        }
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
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
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
            || !(before.uid() == rustix::process::geteuid().as_raw()
                || (!single_link && before.uid() == 0))
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
            || before.dev() != after.dev()
            || before.len() != after.len()
            || before.uid() != after.uid()
            || before.mode() != after.mode()
            || before.nlink() != after.nlink()
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
                && serde_json::to_vec(closed).ok().as_ref() == Some(&self.closed_bytes))
            .then(|| self.custody_digest.clone())
        }
        fn remains_current(&self, b: &NativeDecisionBinding, custody: &str) -> bool {
            self.current() && self.binding_matches(b) && custody == self.custody_digest
        }
    }

    // Private one-shot composition, installed only after actual immutable proof
    // custody is checked. No network input, permissive default or replace method.
    #[derive(Default)]
    struct OneRunAuthority(OnceLock<Arc<NativeLifecycleAuthority>>);
    impl LifecycleAuthority for OneRunAuthority {
        fn authorize(
            &self,
            source: &SourceIdentity,
            command: &LifecycleCommand,
            task: &TaskView,
            effect: Option<&LifecycleEffect<'_>>,
        ) -> bool {
            self.0
                .get()
                .is_some_and(|authority| authority.authorize(source, command, task, effect))
        }
    }
    struct ExactAdmission {
        startup: Arc<StartupAuthority>,
        request: LaunchRequest,
        contract: TaskContract,
    }
    impl ExactAdmission {
        fn current(&self, task: &TaskView) -> bool {
            self.startup.is_current(&self.startup.expected) && task.contract == self.contract
        }
    }
    impl CurrentAdmission for ExactAdmission {
        fn authorize_current(
            &self,
            task: &TaskView,
            request: &LaunchRequest,
        ) -> std::result::Result<(), String> {
            if self.current(task) && request == &self.request {
                Ok(())
            } else {
                Err("one-run authorization differs or expired".into())
            }
        }
    }
    impl DispatchAdmission for ExactAdmission {
        fn authorize_dispatch(
            &self,
            task: &TaskView,
            intent: &DispatchIntent,
        ) -> std::result::Result<(), String> {
            let r = &self.request;
            if self.current(task)
                && intent.schema_version == 1
                && intent.task_id == r.task_id.as_str()
                && intent.attempt_id == r.attempt_id.as_str()
                && intent.contract_revision == self.contract.revision
                && intent.contract_digest == r.expected_contract_digest
                && intent.owner == r.owner.as_str()
                && intent.run_id == r.run_id
                && intent.context_digest == r.context_digest
                && intent.policy_digest == r.policy_digest
                && intent.provider == r.provider
                && intent.model == r.model
            {
                Ok(())
            } else {
                Err("one-run dispatch identity differs or expired".into())
            }
        }
    }
    fn artifact_value(reference: &ArtifactRef) -> Value {
        json!({"sha256":reference.sha256,"size_bytes":reference.size_bytes})
    }
    fn stored(objects: &ArtifactStore, bytes: &[u8]) -> Result<ArtifactRef> {
        let reference = objects.publish_bytes(bytes)?;
        if objects.read_verified(&reference)? != bytes {
            return Err("immutable artifact differs".into());
        }
        Ok(reference)
    }
    fn validate_args(args: &RunArgs) -> Result<()> {
        if !(64..=512).contains(&args.memory_mib)
            || !(4..=32).contains(&args.max_tasks)
            || !(30..=120).contains(&args.deadline_seconds)
        {
            return Err("probe resource bounds exceeded".into());
        }
        NodeId::new(&args.node)?;
        let requested = cpu_set(&args.cpus)?;
        if requested.len() > 2 || !requested.is_subset(&cpu_set(&args.registered_cpus)?) {
            return Err("probe CPUs exceed explicitly registered scope".into());
        }
        if args.prepare_only == args.operator_authorization_file.is_some() {
            return Err(
                "choose prepare-only without authorization, or run with authorization".into(),
            );
        }
        let sha = SOURCE_SHA.ok_or(
            "build with ADMIRALTY_PROBE_SOURCE_SHA set to exact checked Admiralty source SHA",
        )?;
        if sha.len() != 40
            || !sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("invalid build-time Admiralty source SHA".into());
        }
        validate_program(&args.rustc)?;
        Ok(())
    }
    fn read_authorization(path: &Path, fixture: &Path) -> Result<Value> {
        if path.starts_with(fixture) || !path.is_absolute() {
            return Err("authorization must be outside the task fixture".into());
        }
        private_directory(path.parent().ok_or("authorization has no parent")?)?;
        let meta = fs::symlink_metadata(path)?;
        if meta.mode() & 0o777 != 0o400 {
            return Err("authorization must be mode0400".into());
        }
        let bytes = read_owned_file(path, AUTH_BYTES)?;
        let value: Value = serde_json::from_slice(&bytes)?;
        if serde_json::to_vec(&value)? != bytes {
            return Err("authorization must be the exact compact prepared bytes".into());
        }
        Ok(value)
    }
    fn run(args: RunArgs) -> Result<()> {
        validate_args(&args)?;
        private_directory(args.root.parent().ok_or("fixture parent missing")?)?;
        let authorization = args
            .operator_authorization_file
            .as_ref()
            .map(|p| read_authorization(p, &args.root))
            .transpose()?;
        if args.prepare_only {
            create_private(&args.root)?;
        } else {
            private_directory(&args.root)?;
        }
        // The create-only fence is BEFORE any run-time Git command/workspace,
        // not just before launch. Never reuse authorization after an error.
        if !args.prepare_only {
            write_new(
                &args.root.join("run-started.json"),
                &serde_json::to_vec(&json!({
                    "authorization_digest":content_digest(&authorization)?, "started_at_unix_ms":now()?,
                    "retry_allowed":false,
                }))?,
            )?;
        }
        let result = run_owned(&args, authorization);
        if result.is_err() {
            let _ = write_new(
                &args.root.join("failure.json"),
                &serde_json::to_vec_pretty(&json!({
                    "status":"failed-or-unknown", "native_execution_qualified":false,
                    "provider_observed":Value::Null,"automatic_retry":false,
                    "next_action":"preserve fixture/journals and reconcile only owned unit; use new fixture and authorization for any new run",
                    "source_candidate_error_limit":"consuming API can drop process-local handles on source-candidate error; no restart recovery claimed",
                }))?,
            );
        }
        result
    }
    fn run_owned(args: &RunArgs, authorization: Option<Value>) -> Result<()> {
        let root = &args.root;
        let nonce = authorization
            .as_ref()
            .and_then(|v| v["fixture_nonce"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if !valid_nonce(&nonce) {
            return Err("invalid fixture nonce".into());
        }
        for name in [
            "base",
            "workspaces",
            "candidates",
            "objects",
            "checks",
            "git-spool",
            "worker-spool",
            "journal",
            "home",
            "tmp",
            "bin",
        ] {
            if args.prepare_only {
                create_private(&root.join(name))?;
            } else {
                private_directory(&root.join(name))?;
            }
        }
        let objects = ArtifactStore::open(root.join("objects"), 128 * 1024 * 1024)?;
        let current_executable = fs::canonicalize(std::env::current_exe()?)?;
        validate_program(&current_executable)?;
        let exe_bytes = read_file(&current_executable, 128 * 1024 * 1024, false)?;
        let executable = root.join("bin/native-task-loop-probe");
        if args.prepare_only {
            write_new(&executable, &exe_bytes)?;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        }
        if read_owned_file(&executable, 128 * 1024 * 1024)? != exe_bytes {
            return Err("prepared executable differs from current checked probe".into());
        }
        let exe_object = stored(&objects, &exe_bytes)?;
        let compiler = stored(&objects, &read_file(&args.rustc, 128 * 1024 * 1024, false)?)?;
        let compiler_digest = content_digest(&objects.read_verified(&compiler)?)?;
        let oracle = stored(&objects, &result_bytes(&nonce))?;
        let negative = stored(&objects, b"wrong protected oracle\n")?;
        let host = SessionHost::new();
        let mut git_seq = if args.prepare_only { 0 } else { 1000 };
        let base = root.join("base");
        if args.prepare_only {
            create_private(&base.join("src"))?;
            write_new(&base.join("src/main.rs"), &toy_source(&nonce, false)?)?;
            git(
                &host,
                root,
                &base,
                &["init", "--quiet", "--object-format=sha1"],
                &mut git_seq,
            )?;
            git(
                &host,
                root,
                &base,
                &["add", "--", "src/main.rs"],
                &mut git_seq,
            )?;
            git(
                &host,
                root,
                &base,
                &[
                    "-c",
                    "user.name=Native Probe Fixture",
                    "-c",
                    "user.email=probe@example.invalid",
                    "commit",
                    "-qm",
                    "Pinned toy base",
                ],
                &mut git_seq,
            )?;
        }
        if !git(
            &host,
            root,
            &base,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            &mut git_seq,
        )?
        .is_empty()
            || read_owned_file(&base.join("src/main.rs"), 4096)? != toy_source(&nonce, false)?
        {
            return Err("pinned toy base changed".into());
        }
        let base_commit = text(git(
            &host,
            root,
            &base,
            &["rev-parse", "HEAD"],
            &mut git_seq,
        )?)?;
        let base_tree = text(git(
            &host,
            root,
            &base,
            &["rev-parse", "HEAD^{tree}"],
            &mut git_seq,
        )?)?;
        let project = ProjectId::new(format!("loop-probe-{nonce}"))?;
        let root_id = RootId::new(format!("root-{nonce}"))?;
        let task_id = TaskId::new(format!("task-{nonce}"))?;
        let attempt = AttemptId::new(format!("attempt-{nonce}"))?;
        let node = NodeId::new(&args.node)?;
        let worker = PrincipalId::new("deterministic-owned-worker")?;
        let verifier = PrincipalId::new("independent-compiler-verifier")?;
        let authority_ref = format!("one-run-loop:{nonce}");
        let incarnation = nonce.clone();
        let producer = "one-run-loop-verifier";
        let check_args = |output: &str, expected: String| {
            vec![
                "check-source".into(),
                "--rustc".into(),
                args.rustc.to_string_lossy().into_owned(),
                "--rustc-digest".into(),
                compiler_digest.clone(),
                "--output".into(),
                root.join("checks")
                    .join(output)
                    .to_string_lossy()
                    .into_owned(),
                "--expected-value-digest".into(),
                expected,
            ]
        };
        let check = CheckSpec {
            id: "compiled-source-oracle".into(),
            program: executable.to_string_lossy().into_owned(),
            args: check_args(
                "positive",
                content_digest(&objects.read_verified(&oracle)?)?,
            ),
            cwd: ".".into(),
            timeout_secs: 30,
        };
        let policy = VerificationPolicy {
            schema_version: 1,
            verifier: verifier.clone(),
            protected_oracle_digest: oracle.sha256.clone(),
            required_negative_checks: vec![RequiredNegativeCheck {
                check: CheckSpec {
                    id: "wrong-oracle-rejected".into(),
                    args: check_args(
                        "negative",
                        content_digest(&objects.read_verified(&negative)?)?,
                    ),
                    ..check.clone()
                },
                expected_exit_code: i32::from(ORACLE_MISMATCH_EXIT),
                fixture_digest: negative.sha256.clone(),
            }],
        };
        let contract = TaskContract {
            schema_version: 1,
            project_id: project.clone(),
            root_id: root_id.clone(),
            task_id: task_id.clone(),
            revision: 1,
            title: "Isolated actual NativeTaskLoop proof".into(),
            objective: "Change toy Rust source; verify compiled source against protected oracle"
                .into(),
            non_goals: vec![
                "No coding provider, network, general issuer, production/fleet qualification"
                    .into(),
            ],
            owned_paths: vec!["src/main.rs".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "source".into(),
                text: "Actual changed source compiles and matches oracle; wrong oracle fails"
                    .into(),
                check_ids: vec![check.id.clone()],
            }],
            checks: vec![check],
            authority_ref: authority_ref.clone(),
            max_attempts: 1,
            simulated: false,
        };
        contract.validate()?;
        let contract_digest = content_digest(&contract)?;
        let policy_digest = content_digest(&policy)?;
        let context_digest =
            content_digest(&("one-run-toy-context-v1", &nonce, &base_commit, &base_tree))?;
        let workspace_path = root.join("workspaces/owned-run");
        let spec = LaunchSpec {
            run_id: format!("loop-probe-{nonce}"),
            program: executable.clone(),
            args: vec!["owned-child".into(), "--nonce".into(), nonce.clone()],
            cwd: workspace_path.clone(),
            spool_dir: root.join("worker-spool/owned-child"),
        };
        let environment_values = BTreeMap::from([
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
        ]);
        let environment = QualifiedEnvironment::new(environment_values.clone())?;
        let backend = SystemdBackendConfig {
            runtime_directory: args.runtime_directory.clone(),
            delegated_slice: args.slice.clone(),
        };
        // Ten-minute approval window, not a worker deadline extension. The real
        // executor/dispatch lease retains the separately bounded runtime deadline.
        let expires = authorization
            .as_ref()
            .and_then(|v| v["claims"]["expires_at_unix_ms"].as_u64())
            .unwrap_or(now()?.checked_add(600_000).ok_or("expiry overflow")?);
        if expires <= now()?.saturating_add((args.deadline_seconds + 60) * 1000)
            || expires > now()?.saturating_add(600_000)
        {
            return Err(
                "authorization expiry cannot cover bounded run/checks or is too distant".into(),
            );
        }
        let claims = ResourcePermitClaims {
            issuer_ref: format!("one-run-issuer-{nonce}"),
            permit_id: format!("permit-{nonce}"),
            reservation_ref: format!("reservation-{nonce}"),
            identity: AdmissionIdentity {
                project_id: project.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt.clone(),
                node_id: node.clone(),
                lease_id: format!("lease-{nonce}"),
            },
            host_incarnation: fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .into(),
            expires_at_unix_ms: expires,
            lease_expires_at_unix_ms: expires,
            registered_cpu_scope: cpu_set(&args.registered_cpus)?,
            cpu_set: cpu_set(&args.cpus)?,
            memory_max_bytes: args.memory_mib * 1024 * 1024,
            max_tasks: args.max_tasks,
            max_concurrency: 1,
            launch_spec_digest: content_digest(&spec)?,
            worker_environment_digest: environment.digest()?,
            backend_config_digest: backend.digest()?,
        };
        let claim_value = json!({"issuer_ref":claims.issuer_ref,"permit_id":claims.permit_id,
            "reservation_ref":claims.reservation_ref,"identity":claims.identity,"host_incarnation":claims.host_incarnation,
            "expires_at_unix_ms":claims.expires_at_unix_ms,"lease_expires_at_unix_ms":claims.lease_expires_at_unix_ms,
            "registered_cpu_scope":claims.registered_cpu_scope,"cpu_set":claims.cpu_set,
            "memory_max_bytes":claims.memory_max_bytes,"max_tasks":claims.max_tasks,"max_concurrency":claims.max_concurrency,
            "launch_spec_digest":claims.launch_spec_digest,"worker_environment_digest":claims.worker_environment_digest,
            "backend_config_digest":claims.backend_config_digest});
        let reservation = Reservation {
            id: claims.reservation_ref.clone(),
            correlation_id: format!("toy-{nonce}"),
            revision: 1,
            scope: ReservationScope {
                project_id: project.clone(),
                root_id: root_id.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt.clone(),
                owner: worker.clone(),
                incarnation: claims.host_incarnation.clone(),
                contract_revision: 1,
                contract_digest: contract_digest.clone(),
                authority_ref: authority_ref.clone(),
                resource_claims_digest: native_resource_claims_digest(&claims)?,
                account_id: AccountId::new("isolated-toy-reservation")?,
                unit: CurrencyUnit::new("PROBE", 0)?,
            },
            original: MinorUnits::new(1),
            settled: MinorUnits::new(0),
            stop_requested: false,
            outstanding: true,
        };
        let request = json!({"schema_version":1,"scope":"one-actual-native-task-loop-proof",
            "fixture_root":root,"fixture_nonce":nonce,"uid":rustix::process::geteuid().as_raw(),
            "admiralty_build_source_sha":SOURCE_SHA,"admiralty_executable":artifact_value(&exe_object),
            "toy_base_commit":base_commit,"toy_base_tree":base_tree,"toy_base_source_digest":content_digest(&toy_source(&nonce,false)?)?,
            "workspace":workspace_path,"owned_paths":["src/main.rs"],"forbidden_paths":[],
            "compiler_path":args.rustc,"compiler_artifact":artifact_value(&compiler),"contract":contract,"verification_policy":policy,
            "context_digest":context_digest,"launch_spec":spec,"environment":environment_values,
            "claims":claim_value,"reservation":reservation,"executor_deadline_seconds":args.deadline_seconds,
            "dispatch_lease_seconds":args.deadline_seconds+1,"provider_declared":"owned-deterministic-helper",
            "model_declared":"no-model","provider_observed":Value::Null,"coding_provider_qualified":false});
        let encoded = serde_json::to_vec(&request)?;
        if encoded.len() > AUTH_BYTES {
            return Err("prepared authorization exceeds bound".into());
        }
        if args.prepare_only {
            write_new(&root.join("operator-authorization-request.json"), &encoded)?;
            println!(
                "Prepared only; no admitted worker. Inspect exact request, copy it outside fixture mode0400 beneath private mode0700 parent, then run once before expiry. {}",
                root.display()
            );
            return Ok(());
        }
        let approved = authorization.ok_or("operator authorization missing")?;
        if approved != request
            || serde_json::from_slice::<Value>(&read_owned_file(
                &root.join("operator-authorization-request.json"),
                AUTH_BYTES,
            )?)? != request
        {
            return Err("authorization or prepared fixture differs; retain failed fixture, never reuse authorization".into());
        }
        let phase = Arc::new(AtomicU8::new(0));
        let startup = Arc::new(StartupAuthority {
            authorization_snapshot: encoded,
            uid: rustix::process::geteuid().as_raw(),
            pid: std::process::id(),
            phase: phase.clone(),
            expected: claims.clone(),
        });
        let mut forged = claims.clone();
        forged.memory_max_bytes += 4096;
        if VerifiedResourcePermit::verify_current(forged, startup.clone()).is_ok() {
            return Err("modified resource claims authorized".into());
        }
        // This is the original, opaque preparation capability for THIS process.
        let prepared = prepare_workspace(WorkspacePreparation {
            source: base,
            managed_parent: root.join("workspaces"),
            destination_name: "owned-run".into(),
            pinned_commit: base_commit.clone(),
            node_id: NodeRef::new(&args.node)?,
            root_id: root_id.clone(),
            project_id: project.clone(),
            task_id: task_id.clone(),
            attempt_id: attempt.clone(),
            owned_paths: vec!["src/main.rs".into()],
            forbidden_paths: vec![],
        })?;
        if prepared.head_oid() != base_commit || prepared.tree_oid() != base_tree {
            return Err("actual preparation differs from approved base".into());
        }
        fs::set_permissions(
            workspace_path.join("src"),
            fs::Permissions::from_mode(0o700),
        )?;
        fs::set_permissions(
            workspace_path.join("src/main.rs"),
            fs::Permissions::from_mode(0o600),
        )?;
        let workspace = WorkspaceSpec {
            node_id: NodeRef::new(&args.node)?,
            root_id: root_id.clone(),
            project_id: project.clone(),
            workspace: workspace_path,
            owned_paths: vec!["src/main.rs".into()],
            forbidden_paths: vec![],
        };
        let database = root.join("probe.sqlite");
        let one_authority = Arc::new(OneRunAuthority::default());
        let store = Store::open_with_lifecycle_authority(&database, one_authority.clone())?;
        store.append(
            source("one-run-submit", &incarnation, 1),
            DomainEvent::TaskSubmitted {
                contract: contract.clone(),
            },
            observed_utc()?,
        )?;
        let launch = LaunchRequest {
            source: source("one-run-admit", &incarnation, 1),
            task_id: task_id.clone(),
            expected_contract_digest: contract_digest.clone(),
            attempt_id: attempt.clone(),
            owner: worker.clone(),
            run_id: spec.run_id.clone(),
            context_digest: context_digest.clone(),
            policy_digest: policy_digest.clone(),
            provider: "owned-deterministic-helper".into(),
            model: "no-model".into(),
            observed_utc: observed_utc()?,
            not_before: now()? / 1000,
        };
        let admission = ExactAdmission {
            startup: startup.clone(),
            request: launch.clone(),
            contract: contract.clone(),
        };
        let run_id = spec.run_id.clone();
        let plan = NativeExecutionPlan {
            task_id: task_id.clone(),
            attempt_id: attempt.clone(),
            contract_digest: contract_digest.clone(),
            context_digest,
            policy_digest: policy_digest.clone(),
            provider_declared: launch.provider.clone(),
            model_declared: launch.model.clone(),
            run_id: run_id.clone(),
            launch_spec: spec,
            prepared_workspace: prepared,
            workspace,
            reservation,
            permit_claims: claims,
            permit_authority: startup.clone(),
        };
        let mut task_loop = NativeTaskLoop::new(
            store.clone(),
            ResourceGate::configured(node.clone(), backend, environment),
            NativeTaskLoopConfig {
                scope: StartedScope {
                    project_id: project.clone(),
                    root_id: root_id.clone(),
                    node_id: node,
                    source_producer: "one-run-observed-start".into(),
                    source_incarnation: incarnation.clone(),
                },
                task_owner: worker.clone(),
                dispatch_owner: format!("dispatcher-{nonce}"),
                dispatch_lease_secs: args.deadline_seconds + 1,
                max_retained_runs: 1,
                max_buffered_completions: 1,
            },
            NativeTaskRuntimeConfig {
                journal_root: root.join("journal"),
                deadline: Duration::from_secs(args.deadline_seconds),
                capture_limit: MAX_BYTES,
            },
        )?;
        let intent = match task_loop.admit(launch, plan, &admission) {
            Ok(AdmissionStatus::Queued(intent)) => intent,
            Ok(AdmissionStatus::Held(_)) => {
                return Err("admission unknown; loop retains plan, no retry".into());
            }
            Err(rejected) => {
                // No caller drops the original plan while pretending a retry is
                // safe. The process exits and preserves files/journal evidence.
                let _retained_original = rejected.plan;
                return Err(Box::new(rejected.reason));
            }
        };
        if task_loop.retry_admission(&run_id, &admission)?
            != AdmissionStatus::Queued(intent.clone())
        {
            return Err("exact retained admission reply differs".into());
        }
        phase.store(1, Ordering::SeqCst);
        let dispatch = task_loop.dispatch(&run_id, &WallClock, &admission)?;
        write_new(
            &root.join("dispatch.json"),
            &serde_json::to_vec_pretty(&json!({"outbox_key":intent.outbox_key,
            "status":format!("{dispatch:?}"),"actual_started_state":store.snapshot()?.projection.tasks[&task_id].state}))?,
        )?;
        if !matches!(dispatch, TaskDispatchStatus::EffectRecorded(_)) {
            return Err(
                "dispatch not durably acknowledged; retain owned unit/loop journals, no retry"
                    .into(),
            );
        }
        if !matches!(
            task_loop.dispatch(&run_id, &WallClock, &admission)?,
            TaskDispatchStatus::Held {
                runtime_attempted: false,
                ..
            }
        ) {
            return Err("duplicate dispatch could attempt another child".into());
        }
        let completion = match task_loop.reconcile_completion(&run_id)? {
            CompletionStatus::Buffered {
                effect_ack: EffectAckState::Recorded,
                ..
            } => task_loop
                .take_completion(&run_id)
                .ok_or("acknowledged completion unavailable")?,
            _ => {
                return Err(
                    "completion unknown; owned evidence retained, no automatic retry".into(),
                );
            }
        };
        let events = store.events_after(0, 32)?;
        let claimed = events.iter().filter(|e| matches!(&e.event, DomainEvent::TaskClaimed { task_id:id, attempt_id:a, .. } if id==&task_id && a==&attempt)).count();
        let started = events.iter().filter(|e| matches!(&e.event, DomainEvent::TaskStarted { task_id:id, attempt_id:a } if id==&task_id && a==&attempt)).count();
        if claimed != 1
            || started != 1
            || store.snapshot()?.projection.tasks[&task_id].state != SchedulingState::Running
        {
            return Err(
                "real loop did not produce exactly one durable claim and observed start".into(),
            );
        }
        let (runtime_receipt, original_preparation, closed, transcript) =
            completion.into_completion_parts();
        if !closed.success() {
            return Err("actual child did not close successfully".into());
        }
        phase.store(2, Ordering::SeqCst);
        let transcript_object = stored(&objects, &transcript)?;
        // Consuming source APIs have no error-return ownership envelope yet.
        // On error this run is failed, never re-created/retried from JSON/path.
        let execution = prove_closed_workspace(original_preparation, closed)?;
        let candidate =
            materialize_source_candidate(execution, &root.join("candidates"), "verified-source")?;
        candidate.revalidate_custody()?;
        if candidate.base_commit_sha() != base_commit
            || candidate.source_commit_sha() == base_commit
        {
            return Err("candidate is not actual changed source from approved base".into());
        }
        let inputs =
            candidate.verification_inputs(1, &contract_digest, &policy_digest, &oracle.sha256);
        let checks = execute_checks(
            &host,
            &inputs,
            &contract,
            &policy,
            candidate.checkout_path(),
            &root.join("checks"),
            "loop-probe-checks",
        )?;
        candidate.revalidate_custody()?;
        let closed = candidate
            .execution()
            .closed_admission()
            .ok_or("original closed admission missing")?;
        let decision_evidence = candidate.decision_evidence(&checks, verifier.clone())?;
        let checks_bytes = serde_json::to_vec(&checks)?;
        let closed_bytes = serde_json::to_vec(closed)?;
        let checks_object = stored(&objects, &checks_bytes)?;
        let closed_object = stored(&objects, &closed_bytes)?;
        let receipt_object = stored(&objects, &serde_json::to_vec(checks.receipt())?)?;
        let policy_object = stored(&objects, &serde_json::to_vec(&policy)?)?;
        let candidate_object = stored(
            &objects,
            &serde_json::to_vec(&json!({"decision_evidence":decision_evidence,
            "candidate_ref":candidate.candidate_ref(),"source_artifact_digest":candidate.source_artifact_digest(),
            "source_commit":candidate.source_commit_sha(),"source_tree":candidate.source_tree_sha(),"custody":candidate.custody_digest()}))?,
        )?;
        for reference in [
            &oracle,
            &negative,
            &exe_object,
            &compiler,
            &checks_object,
            &closed_object,
            &receipt_object,
            &policy_object,
            &candidate_object,
            &transcript_object,
        ] {
            objects.read_verified(reference)?;
        }
        let custody = stored(
            &objects,
            &serde_json::to_vec(&json!({"kind":"actual-native-task-loop-custody-v1",
            "authorization":content_digest(&approved)?,"candidate":artifact_value(&candidate_object),"candidate_custody":candidate.custody_digest(),
            "checks":artifact_value(&checks_object),"receipt":artifact_value(&receipt_object),"closure":artifact_value(&closed_object),"policy":artifact_value(&policy_object),
            "oracle":artifact_value(&oracle),"negative":artifact_value(&negative),"executable":artifact_value(&exe_object),"compiler":artifact_value(&compiler),
            "transcript_not_source":artifact_value(&transcript_object),"effect_ack":"recorded","runtime_receipt":format!("{runtime_receipt:?}"),
            "admiralty_build_source_sha":SOURCE_SHA,"toy_base_commit":base_commit,
            "no_external_effects":"fixed child changes only owned toy Rust source; fixed local Git/compiler/check argv; no network/provider"}))?,
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
            1,
        )?);
        one_authority
            .0
            .set(authority.clone())
            .map_err(|_| "one-run lifecycle authority already installed")?;
        store.append(
            source("one-run-candidate", &incarnation, 1),
            candidate.candidate_submitted_event(),
            observed_utc()?,
        )?;
        let task = store
            .snapshot()?
            .projection
            .tasks
            .remove(&task_id)
            .ok_or("task missing")?;
        let decision_source = source(producer, &incarnation, 1);
        let command = authority.issue_acceptance(&decision_source, &task, &checks, Some(closed))?;
        candidate.revalidate_custody()?;
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
            return Err("acceptance exact replay differs".into());
        }
        phase.store(3, Ordering::SeqCst);
        if store
            .execute_lifecycle(decision_source, command, observed_utc()?, None)
            .is_ok()
        {
            return Err("revoked acceptance replay authorized".into());
        }
        let projection = store.snapshot()?.projection;
        if projection.tasks[&task_id].state != SchedulingState::Accepted {
            return Err("task not accepted".into());
        }
        // Drop every Store clone before reopening; the loop is not an active
        // worker after the one-shot acknowledged completion has been handed off.
        drop(task_loop);
        drop(store);
        drop(authority);
        drop(one_authority);
        let reopened = Store::open(&database)?;
        if reopened.snapshot()?.projection != projection {
            return Err("Store reopen projection differs".into());
        }
        let report = json!({"schema_version":1,"status":"passed","scope":"isolated actual NativeTaskLoop deterministic source/check/acceptance proof",
            "native_execution_qualified":false,"coding_provider_qualified":false,"provider_observed":Value::Null,"model_observed":Value::Null,
            "admiralty_build_source_sha":SOURCE_SHA,"source_sha_attestation":"build-time value must be backed by operator exact-head executable receipt",
            "root":root,"task_id":task_id,"real_claim_events":claimed,"actual_observed_start_events":started,
            "effect_ack":"recorded","accepted_event":accepted.event_seq,"exact_duplicate":duplicate.duplicate,
            "retained_admission_reply_matched":true,"duplicate_dispatch_no_runtime_attempt":true,
            "revocation_denied":true,"reopen_projection_matched":true,"modified_resource_claim_denied":true,
            "toy_base_commit":base_commit,"toy_candidate_commit":candidate.source_commit_sha(),"toy_candidate_tree":candidate.source_tree_sha(),
            "candidate_ref":candidate.candidate_ref(),"candidate_source_digest":candidate.source_artifact_digest(),
            "checks_object":artifact_value(&checks_object),"closure_object":artifact_value(&closed_object),"custody_object":artifact_value(&custody),"recovery_identity":closed.identity()});
        write_new(
            &root.join("receipt.json"),
            &serde_json::to_vec_pretty(&report)?,
        )?;
        println!("{}", serde_json::to_string(&report)?);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        struct PrivateFixture(PathBuf);
        impl PrivateFixture {
            fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for PrivateFixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        fn private_fixture() -> PrivateFixture {
            let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
            let path = parent.join(format!("loop-probe-test-{}", uuid::Uuid::new_v4()));
            create_private(&path).unwrap();
            PrivateFixture(path)
        }
        #[test]
        fn helper_source_is_bounded_scoped_and_not_the_oracle() {
            let nonce = "00000000-0000-4000-8000-000000000001";
            let base = toy_source(nonce, false).unwrap();
            let changed = toy_source(nonce, true).unwrap();
            assert_ne!(base, changed);
            assert!(base.len() < 256 && changed.len() < 256);
            assert!(String::from_utf8(changed).unwrap().contains("map(|n| n*n)"));
            assert_eq!(
                result_bytes(nonce),
                format!("owned-task-loop-v1 {nonce} 333383335000\n").as_bytes()
            );
            assert!(toy_source("../outside", true).is_err());
        }
        #[test]
        fn cpu_and_digest_bounds_reject_ambiguous_inputs() {
            assert!(cpu_set("1,1").is_err());
            assert!(cpu_set("").is_err());
            assert!(cpu_set("0-3").is_err());
            assert_eq!(cpu_set("1,3").unwrap(), BTreeSet::from([1, 3]));
            assert!(sha256(&format!("sha256:{}", "a".repeat(64))));
            assert!(!sha256(&format!("sha256:{}", "A".repeat(64))));
            assert!(!sha256(&"a".repeat(64)));
        }
        #[test]
        fn command_help_and_required_arguments_are_effect_free() {
            use clap::CommandFactory;
            let mut command = Cli::command();
            let mut help = Vec::new();
            command.write_long_help(&mut help).unwrap();
            assert!(String::from_utf8(help).unwrap().contains("NativeTaskLoop"));
            assert!(Cli::try_parse_from(["probe", "run", "--root", "/never-created"]).is_err());
        }
        #[test]
        fn authorization_requires_exact_private_outside_single_link_bytes() {
            let dir = private_fixture();
            let fixture = dir.path().join("fixture");
            create_private(&fixture).unwrap();
            let approval = dir.path().join("approval.json");
            write_new(&approval, br#"{"scope":"fixture"}"#).unwrap();
            assert!(read_authorization(&approval, &fixture).is_err());
            fs::set_permissions(&approval, fs::Permissions::from_mode(0o400)).unwrap();
            assert_eq!(
                read_authorization(&approval, &fixture).unwrap()["scope"],
                "fixture"
            );
            assert!(read_authorization(&approval, dir.path()).is_err());
            let alias = dir.path().join("alias.json");
            std::os::unix::fs::symlink(&approval, &alias).unwrap();
            assert!(read_authorization(&alias, &fixture).is_err());
            fs::remove_file(&alias).unwrap();
            fs::hard_link(&approval, &alias).unwrap();
            assert!(read_authorization(&approval, &fixture).is_err());
            fs::remove_file(&alias).unwrap();
            fs::set_permissions(&approval, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&approval, b"{\"scope\": \"fixture\"}\n").unwrap();
            fs::set_permissions(&approval, fs::Permissions::from_mode(0o400)).unwrap();
            assert!(read_authorization(&approval, &fixture).is_err());
        }
        #[test]
        fn one_run_fence_is_create_only_and_private_reads_are_bounded() {
            let dir = private_fixture();
            let fence = dir.path().join("run-started.json");
            write_new(&fence, b"first attempt").unwrap();
            assert!(write_new(&fence, b"second attempt").is_err());
            assert_eq!(fs::metadata(&fence).unwrap().mode() & 0o777, 0o600);
            assert_eq!(read_owned_file(&fence, 32).unwrap(), b"first attempt");
            assert!(read_owned_file(&fence, 4).is_err());
            let alias = dir.path().join("outside-fence");
            std::os::unix::fs::symlink(&fence, &alias).unwrap();
            assert!(write_new(&alias, b"overwrite").is_err());
            assert_eq!(read_owned_file(&fence, 32).unwrap(), b"first attempt");
        }
        #[test]
        fn setup_failure_is_not_a_completed_oracle_mismatch() {
            let dir = private_fixture();
            let digest = content_digest(&b"protected value".to_vec()).unwrap();
            let output = dir.path().join("not-created");
            let error = check_source(&dir.path().join("missing-rustc"), &digest, &output, &digest)
                .unwrap_err();
            assert_eq!(failure_exit(error.as_ref()), 17);
            assert!(!error.is::<OracleMismatch>());
            assert!(!output.exists());
        }
        #[test]
        fn command_and_capture_failures_cannot_satisfy_wrong_oracle_negative() {
            let dir = private_fixture();
            let host = SessionHost::new();
            let wrong = content_digest(&b"wrong oracle".to_vec()).unwrap();
            // Actual bounded read-only OS children exercise the very same
            // command/capture path as the compiler, without compiling source,
            // launching a resource-gated worker, or qualifying a Linux runtime.
            let compiler_failure = fixed_command(
                &host,
                "compiler-failure",
                Path::new("/usr/bin/false"),
                &[],
                dir.path(),
                dir.path(),
                Duration::from_secs(2),
            );
            assert!(compiler_failure.is_err());
            let error = verify_compiled_output(compiler_failure, &wrong).unwrap_err();
            assert_eq!(failure_exit(error.as_ref()), 17);
            assert!(!error.is::<OracleMismatch>());
            let capture_failure = fixed_command(
                &host,
                "capture-failure",
                Path::new("/usr/bin/head"),
                &["-c".into(), (MAX_BYTES + 1).to_string(), "/dev/zero".into()],
                dir.path(),
                dir.path(),
                Duration::from_secs(2),
            );
            assert!(capture_failure.is_err());
            let error = verify_compiled_output(capture_failure, &wrong).unwrap_err();
            assert_eq!(failure_exit(error.as_ref()), 17);
            assert!(!error.is::<OracleMismatch>());
            let actual = fixed_command(
                &host,
                "oracle-output",
                Path::new("/usr/bin/printf"),
                &["actual value".into()],
                dir.path(),
                dir.path(),
                Duration::from_secs(2),
            )
            .unwrap();
            let expected = content_digest(&actual).unwrap();
            assert!(verify_compiled_output(Ok(actual.clone()), &expected).is_ok());
            let mismatch = verify_compiled_output(Ok(actual), &wrong).unwrap_err();
            assert!(mismatch.is::<OracleMismatch>());
            assert_eq!(failure_exit(mismatch.as_ref()), ORACLE_MISMATCH_EXIT);
            assert_ne!(ORACLE_MISMATCH_EXIT, 17);
        }
        #[test]
        fn compiler_output_775_is_normalized_without_weakening_program_validation() {
            let dir = private_fixture();
            let output = dir.path().join("compiler-output");
            create_private(&output).unwrap();
            let custody = open_private_output_directory(&output).unwrap();
            let binary = output.join("toy");
            write_new(&binary, b"owned compiler output").unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o775)).unwrap();
            assert!(validate_program(&binary).is_err());
            normalize_compiler_output(&output, &custody).unwrap();
            assert_eq!(fs::metadata(&binary).unwrap().mode() & 0o7777, 0o700);
            assert_eq!(
                read_owned_file(&binary, 128).unwrap(),
                b"owned compiler output"
            );
            validate_program(&binary).unwrap();
        }
        #[test]
        fn compiler_output_normalization_refuses_links_and_replaced_directory() {
            let dir = private_fixture();
            let output = dir.path().join("compiler-output");
            create_private(&output).unwrap();
            let custody = open_private_output_directory(&output).unwrap();
            let outside = dir.path().join("outside");
            write_new(&outside, b"must not chmod").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o775)).unwrap();
            let binary = output.join("toy");
            std::os::unix::fs::symlink(&outside, &binary).unwrap();
            assert!(normalize_compiler_output(&output, &custody).is_err());
            assert_eq!(fs::metadata(&outside).unwrap().mode() & 0o7777, 0o775);
            fs::remove_file(&binary).unwrap();
            fs::hard_link(&outside, &binary).unwrap();
            assert!(normalize_compiler_output(&output, &custody).is_err());
            assert_eq!(fs::metadata(&outside).unwrap().mode() & 0o7777, 0o775);
            fs::remove_file(&binary).unwrap();
            let moved = dir.path().join("original-output");
            fs::rename(&output, &moved).unwrap();
            create_private(&output).unwrap();
            write_new(&binary, b"replacement output").unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o775)).unwrap();
            assert!(normalize_compiler_output(&output, &custody).is_err());
            assert_eq!(fs::metadata(&binary).unwrap().mode() & 0o7777, 0o775);
        }
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
