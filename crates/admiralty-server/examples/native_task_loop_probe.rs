//! Explicit, one-run Linux proof of NativeTaskLoop -> retained workspace/closure
//! -> changed source -> independent positive/negative checks -> Store acceptance.
//! The fixed helper makes no provider/network calls. The separate `codex` mode
//! prepares one explicit operator-qualified provider attempt and never grants
//! general admission, startup activation, production rollout or packet acceptance.
//! Preparation is create-only; run requires the exact fresh private authorization
//! outside the fixture. Any failed/unknown run is retained and MUST NOT be retried.
//! Source-candidate errors retain original handles until process shutdown;
//! journals/files remain, but this probe makes no restart recovery claim.
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
        models::{ModelSpec, RegistryFile},
        providers::{
            codex::{
                self, CodexAdmission, CodexCliObservation, CodexEffort, CodexExecutionOptions,
                CodexNdjsonStream, CodexSandbox, CodexTermination, PreparedCodexLaunch,
            },
            supervision::SupervisionScope,
        },
        reservations::{AccountId, CurrencyUnit, MinorUnits, Reservation, ReservationScope},
        resource_gate::{
            AdmissionIdentity, ClosedAdmission, QualifiedEnvironment, ResourceGate,
            ResourcePermitAuthority, ResourcePermitClaims, SystemdBackendConfig,
            VerifiedResourcePermit,
        },
        session::{CaptureState, LaunchSpec, OutputStream, SessionError, SessionHost},
        trusted_checks::{ExecutedChecks, execute_checks},
        workspace::{
            preparation::{WorkspacePreparation, prepare_workspace},
            registry::{NodeRef, WorkspaceSpec},
            source_candidate::{try_materialize_source_candidate, try_prove_closed_workspace},
        },
    };
    use admiralty_server::services::{
        controller::{
            CurrentAdmission, DispatchAdmission, DispatchIntent, LaunchRequest, WallClock,
        },
        native_codex_plan::{
            CodexBindingFailure, CodexPlanInput, CodexPreparationFailure, CodexResourceBinding,
            PreparedNativeCodexPlan, prepare_codex_plan,
        },
        native_executor::{CompletedCapture, NativeExecutionPlan, native_resource_claims_digest},
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
        /// Separate, fresh one-run Codex qualification; never general admission.
        Codex(Box<CodexArgs>),
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
    #[derive(Args)]
    struct CodexArgs {
        #[command(flatten)]
        common: RunArgs,
        #[arg(long)]
        codex_program: PathBuf,
        #[arg(long)]
        model_registry: PathBuf,
        #[arg(long)]
        model_id: String,
        #[arg(long, value_parser=["minimal","low","medium","high","xhigh"])]
        effort: String,
        /// Existing externally qualified private HOME; no credential files read/copied.
        #[arg(long)]
        provider_home: PathBuf,
        /// Opaque registered auth-context:ID, never a credential value/file.
        #[arg(long)]
        auth_context_ref: String,
    }
    struct CodexProfile {
        program: PathBuf,
        registry: PathBuf,
        model_id: String,
        effort: CodexEffort,
        home: PathBuf,
        auth_context_ref: String,
    }
    // Keep actual opaque custody inside the propagated error until process
    // shutdown. Errors never reconstruct a workspace/provider from JSON/path.
    enum RetainedProviderFailure {
        Preparation(CodexPreparationFailure),
        Binding(CodexBindingFailure),
        Tuple(Box<PreparedNativeCodexPlan>),
    }
    impl std::fmt::Debug for RetainedProviderFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Preparation(error) => f
                    .debug_tuple("retained provider preparation")
                    .field(error)
                    .finish(),
                Self::Binding(error) => f
                    .debug_tuple("retained provider resource binding")
                    .field(error)
                    .finish(),
                Self::Tuple(plan) => {
                    let _ = plan.prepared_provider().tuple_digest();
                    f.write_str("retained provider tuple differs from authorized preview")
                }
            }
        }
    }
    impl std::fmt::Display for RetainedProviderFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Debug::fmt(self, f)
        }
    }
    impl Error for RetainedProviderFailure {}
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
            Mode::Codex(args) => {
                let effort = match args.effort.as_str() {
                    "minimal" => CodexEffort::Minimal,
                    "low" => CodexEffort::Low,
                    "medium" => CodexEffort::Medium,
                    "high" => CodexEffort::High,
                    "xhigh" => CodexEffort::Xhigh,
                    _ => unreachable!("clap bounds effort"),
                };
                run_with_provider(
                    args.common,
                    Some(CodexProfile {
                        program: args.codex_program,
                        registry: args.model_registry,
                        model_id: args.model_id,
                        effort,
                        home: args.provider_home,
                        auth_context_ref: args.auth_context_ref,
                    }),
                )
            }
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
            let status = session_observation(|| host.status(id), end)?;
            if status.capture_state == CaptureState::Failed
                || status.output_offset > MAX_BYTES as u64
                || Instant::now() >= end
            {
                let _ = host.terminate(id);
                return Err("owned check command exceeded bounds; inspect retained spool".into());
            }
            if status.exit.is_some() && status.capture_state == CaptureState::Complete {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !host.wait(id)?.success() {
            return Err("owned check command failed".into());
        }
        let status = session_observation(|| host.status(id), end)?;
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
    // Status is explicitly nonblocking; a reader holding the capture lock may
    // return Busy. Retry that observation only, under the original aggregate
    // command deadline. Never relaunch or retry an ambiguous input operation.
    fn session_observation<T>(
        mut observe: impl FnMut() -> std::result::Result<T, SessionError>,
        end: Instant,
    ) -> Result<T> {
        loop {
            match observe() {
                Ok(value) => return Ok(value),
                Err(SessionError::Busy) if Instant::now() < end => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        }
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
    fn auth_context_reference(value: &str) -> bool {
        value.strip_prefix("auth-context:").is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    }
    struct CodexFixture {
        model: ModelSpec,
        cli: CodexCliObservation,
        admission: CodexAdmission,
        options: CodexExecutionOptions,
        capsule: String,
        preview: PreparedCodexLaunch,
        snapshot: Value,
    }
    struct CodexFixtureContext<'a> {
        args: &'a RunArgs,
        contract: &'a TaskContract,
        nonce: &'a str,
        workspace: &'a Path,
        expires: u64,
        objects: &'a ArtifactStore,
        host: &'a SessionHost,
    }
    fn prepare_codex_fixture(
        profile: &CodexProfile,
        ctx: CodexFixtureContext<'_>,
    ) -> Result<CodexFixture> {
        let root = &ctx.args.root;
        let directory = root.join("provider-observation");
        if ctx.args.prepare_only {
            create_private(&directory)?;
        } else {
            private_directory(&directory)?;
        }
        let program_bytes = read_file(&profile.program, 128 * 1024 * 1024, false)?;
        let program = stored(ctx.objects, &program_bytes)?;
        let registry = RegistryFile::new(profile.registry.clone())?.read()?;
        let model = registry
            .models
            .iter()
            .find(|model| model.id == profile.model_id)
            .cloned()
            .ok_or("explicit Codex model missing from qualified registry")?;
        let registry_object = stored(ctx.objects, &serde_json::to_vec(&registry)?)?;
        let observations = [
            ("version", vec!["--no-daemon".into(), "--version".into()]),
            (
                "exec-help",
                vec!["--no-daemon".into(), "exec".into(), "--help".into()],
            ),
            ("root-help", vec!["--no-daemon".into(), "--help".into()]),
        ];
        let mut observed = Vec::new();
        let mut observation_objects = Vec::new();
        for (id, args) in observations {
            let path = directory.join(format!("{id}.txt"));
            let bytes = if ctx.args.prepare_only {
                let bytes = fixed_command(
                    ctx.host,
                    &format!("observe-{id}"),
                    &profile.program,
                    &args,
                    root,
                    &directory,
                    Duration::from_secs(5),
                )?;
                if bytes.is_empty() || bytes.len() > 64 * 1024 {
                    return Err("CLI observation exceeds bounds".into());
                }
                write_new(&path, &bytes)?;
                bytes
            } else {
                read_owned_file(&path, 64 * 1024)?
            };
            observation_objects.push(artifact_value(&stored(ctx.objects, &bytes)?));
            observed.push(String::from_utf8(bytes)?);
        }
        let tick_path = directory.join("observed-tick.json");
        let observed_tick = if ctx.args.prepare_only {
            let tick = now()? / 1000;
            write_new(&tick_path, &serde_json::to_vec(&tick)?)?;
            tick
        } else {
            serde_json::from_slice::<u64>(&read_owned_file(&tick_path, 64)?)?
        };
        let current_tick = now()? / 1000;
        if current_tick < observed_tick || current_tick - observed_tick > 300 {
            return Err("retained actual CLI observation is stale or clock moved backwards; new fixture required".into());
        }
        let cli = CodexCliObservation::from_help(
            &ctx.args.node,
            profile.program.clone(),
            observed[0].trim(),
            &observed[1],
            &observed[2],
            Some(observed_tick),
        )?;
        let mut capsule =
            admiralty_core::context::compile_task_capsule(ctx.contract, ctx.nonce, 4096)
                .map_err(|_| "essential toy task capsule exceeded bound")?
                .text;
        capsule.push_str(&format!("\nONE-RUN TOY TASK: Edit only src/main.rs. Compute the sum of n*n for n=1..10000 in Rust; print exactly {:?} including its trailing newline. Preserve this nonce. Do not edit any other file, Git metadata, verifier, journal, auth context or credentials. Do not publish, launch other agents, change services or contact other hosts. Provider transport is externally authorized; no network/credential isolation is claimed. Leave the edit for independent source verification.\n",String::from_utf8(result_bytes(ctx.nonce))?));
        if capsule.len() > 16 * 1024 {
            return Err("toy provider capsule exceeded bound".into());
        }
        let admission = CodexAdmission {
            scope: SupervisionScope {
                project_id: ctx.contract.project_id.clone(),
                root_id: ctx.contract.root_id.clone(),
                task_id: ctx.contract.task_id.clone(),
                attempt_id: AttemptId::new(format!("attempt-{}", ctx.nonce))?,
                owner: PrincipalId::new("codex-owned-worker")?,
                incarnation: ctx.nonce.into(),
                authority_ref: ctx.contract.authority_ref.clone(),
                contract_digest: content_digest(ctx.contract)?,
                revision: ctx.contract.revision,
            },
            attempt_ordinal: 1,
            registry_revision: registry.revision,
            selected_model_digest: content_digest(&model)?,
            cli_observation_digest: cli.fingerprint().into(),
            node_id: ctx.args.node.clone(),
            working_dir: ctx.workspace.into(),
            spool_root: root.join("worker-spool"),
            lease_ref: format!("lease-{}", ctx.nonce),
            resource_permit_ref: format!("permit-{}", ctx.nonce),
            custody_ref: format!("one-run-workspace:{}", ctx.nonce),
            auth_context_ref: profile.auth_context_ref.clone(),
            admitted_tick: observed_tick,
            expires_tick: ctx.expires / 1000,
        };
        let options = CodexExecutionOptions {
            effort: profile.effort,
            sandbox: CodexSandbox::WorkspaceWrite,
        };
        // Pure preview for the operator request: no workspace capability or
        // admission is manufactured during prepare-only. Run creates the actual
        // preparation and uses prepare_codex_plan before resource binding.
        let preview = codex::prepare_launch(
            ctx.contract,
            admission.clone(),
            &model,
            options,
            &cli,
            &capsule,
            Some(current_tick),
        )?;
        let snapshot = json!({"kind":"one-run-codex-provider-qualification-v1","program":profile.program,
            "program_artifact":artifact_value(&program),"cli_observation":cli,"help_objects":observation_objects,
            "registry_path":profile.registry,"registry_artifact":artifact_value(&registry_object),"model":model,
            "selected_model_digest":admission.selected_model_digest,"capsule":capsule,"provider_tuple":preview.tuple(),
            "provider_tuple_digest":preview.tuple_digest(),"provider_home":profile.home,"auth_context_ref":profile.auth_context_ref,
            "auth_delivery":"existing externally qualified HOME-default .codex context; no CODEX_HOME forwarding, credential read/copy or fallback",
            "capacity_authority":"fresh external operator one-run authorization only","durable_capacity_qualified":false,
            "billing_settlement":false,"provider_network_expected":true,"network_or_credential_sandbox_qualified":false,
            "native_execution_qualified":false,"coding_provider_qualified":false});
        Ok(CodexFixture {
            model,
            cli,
            admission,
            options,
            capsule,
            preview,
            snapshot,
        })
    }
    fn codex_capture_summary(
        provider: &PreparedCodexLaunch,
        capture: &CompletedCapture,
        run_id: &str,
    ) -> Result<Value> {
        if capture.run_id() != run_id || provider.launch_spec().run_id != run_id {
            return Err("typed provider capture run differs from actual completion".into());
        }
        let mut parser = CodexNdjsonStream::from_prepared(provider)?;
        for chunk in capture.chunks() {
            parser.push_chunk(chunk)?;
        }
        let usage: Vec<_> = parser
            .observations()
            .iter()
            .filter_map(|observation| observation.usage.clone())
            .collect();
        let total = |counter: fn(&codex::CodexTurnUsage) -> Option<&str>| -> Option<String> {
            if usage.is_empty() {
                return None;
            }
            usage
                .iter()
                .try_fold(0_u128, |sum, item| {
                    sum.checked_add(counter(item)?.parse::<u128>().ok()?)
                })
                .map(|n| n.to_string())
        };
        let input = total(|item| item.input_tokens.as_deref());
        let output = total(|item| item.output_tokens.as_deref());
        let cached = total(|item| item.cached_input_tokens.as_deref());
        let cache_write = total(|item| item.cache_write_input_tokens.as_deref());
        let summary = parser.finish_from_status(capture.final_status());
        let identity_complete = summary.capture_complete && !summary.parser_failed;
        let sufficient = identity_complete
            && summary.unknown_events == "0"
            && summary.termination == CodexTermination::ProviderTurnCompleted
            && summary.reported_model.as_deref() == Some(provider.tuple().requested_model.as_str())
            && summary.reported_effort.as_deref()
                == Some(provider.tuple().requested_effort.as_str())
            && input.is_some()
            && output.is_some();
        Ok(
            json!({"kind":"actual-owned-codex-capture-v1","summary":summary,
            "provider_observed":if identity_complete{Some("codex_cli")}else{None},
            "model_observed":if identity_complete{summary.reported_model.clone()}else{None},
            "effort_observed":if identity_complete{summary.reported_effort.clone()}else{None},
            "usage":{"input_tokens":input,"output_tokens":output,"cached_input_tokens":cached,"cache_write_input_tokens":cache_write,
                "semantics":"sum of actual provider per-turn reports; missing counters remain unknown; not billing settlement"},
            "provider_evidence_complete":sufficient,"native_execution_qualified":false,"durable_capacity_qualified":false,
            "billing_settlement":false,"source_or_acceptance_grant":false}),
        )
    }
    fn run(args: RunArgs) -> Result<()> {
        run_with_provider(args, None)
    }
    fn run_with_provider(args: RunArgs, provider: Option<CodexProfile>) -> Result<()> {
        validate_args(&args)?;
        if let Some(profile) = &provider {
            validate_program(&profile.program)?;
            private_directory(&profile.home)?;
            if profile.home.starts_with(&args.root)
                || args.root.starts_with(&profile.home)
                || !auth_context_reference(&profile.auth_context_ref)
            {
                return Err("separate externally qualified private provider HOME and opaque auth-context:ID required".into());
            }
        }
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
        let result = run_owned(&args, authorization, provider.as_ref());
        if result.is_err() {
            let _ = write_new(
                &args.root.join("failure.json"),
                &serde_json::to_vec_pretty(&json!({
                    "status":"failed-or-unknown", "native_execution_qualified":false,
                    "provider_observed":Value::Null,"automatic_retry":false,
                    "next_action":"preserve fixture/journals and reconcile only owned unit; use new fixture and authorization for any new run",
                    "source_candidate_error_limit":"original handles retained inside propagated source-candidate errors until process shutdown; no restart recovery claimed",
                }))?,
            );
        }
        result
    }
    fn run_owned(
        args: &RunArgs,
        authorization: Option<Value>,
        provider_profile: Option<&CodexProfile>,
    ) -> Result<()> {
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
        let worker = PrincipalId::new(if provider_profile.is_some() {
            "codex-owned-worker"
        } else {
            "deterministic-owned-worker"
        })?;
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
            non_goals: vec![if provider_profile.is_some() {
                "Only the explicitly authorized provider attempt; no production repositories, other agents/hosts, service changes or general fleet qualification".into()
            } else {
                "No coding provider, network, general issuer, production/fleet qualification".into()
            }],
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
        let workspace_path = root.join("workspaces/owned-run");
        let codex_fixture = provider_profile
            .map(|profile| {
                prepare_codex_fixture(
                    profile,
                    CodexFixtureContext {
                        args,
                        contract: &contract,
                        nonce: &nonce,
                        workspace: &workspace_path,
                        expires,
                        objects: &objects,
                        host: &host,
                    },
                )
            })
            .transpose()?;
        let context_digest = if let Some(fixture) = &codex_fixture {
            content_digest(&fixture.capsule)?
        } else {
            content_digest(&("one-run-toy-context-v1", &nonce, &base_commit, &base_tree))?
        };
        let spec = if let Some(fixture) = &codex_fixture {
            fixture.preview.launch_spec().clone()
        } else {
            LaunchSpec {
                run_id: format!("loop-probe-{nonce}"),
                program: executable.clone(),
                args: vec!["owned-child".into(), "--nonce".into(), nonce.clone()],
                cwd: workspace_path.clone(),
                spool_dir: root.join("worker-spool/owned-child"),
            }
        };
        let environment_values = BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            (
                "HOME".into(),
                provider_profile
                    .map(|profile| profile.home.clone())
                    .unwrap_or_else(|| root.join("home"))
                    .to_string_lossy()
                    .into_owned(),
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
        let provider_declared = if codex_fixture.is_some() {
            "codex_cli"
        } else {
            "owned-deterministic-helper"
        };
        let model_declared = codex_fixture
            .as_ref()
            .map(|fixture| fixture.model.id.as_str())
            .unwrap_or("no-model");
        let request = json!({"schema_version":1,"scope":if codex_fixture.is_some(){"one-actual-codex-provider-qualification"}else{"one-actual-native-task-loop-proof"},
            "fixture_root":root,"fixture_nonce":nonce,"uid":rustix::process::geteuid().as_raw(),
            "admiralty_build_source_sha":SOURCE_SHA,"admiralty_executable":artifact_value(&exe_object),
            "toy_base_commit":base_commit,"toy_base_tree":base_tree,"toy_base_source_digest":content_digest(&toy_source(&nonce,false)?)?,
            "workspace":workspace_path,"owned_paths":["src/main.rs"],"forbidden_paths":[],
            "compiler_path":args.rustc,"compiler_artifact":artifact_value(&compiler),"contract":contract,"verification_policy":policy,
            "context_digest":context_digest,"launch_spec":spec,"environment":environment_values,
            "claims":claim_value,"reservation":reservation,"executor_deadline_seconds":args.deadline_seconds,
            "dispatch_lease_seconds":args.deadline_seconds+1,"provider_declared":provider_declared,
            "model_declared":model_declared,"provider_observed":Value::Null,"coding_provider_qualified":false,
            "provider_qualification":codex_fixture.as_ref().map(|fixture|&fixture.snapshot)});
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
            provider: provider_declared.into(),
            model: model_declared.into(),
            observed_utc: observed_utc()?,
            not_before: now()? / 1000,
        };
        let admission = ExactAdmission {
            startup: startup.clone(),
            request: launch.clone(),
            contract: contract.clone(),
        };
        let run_id = spec.run_id.clone();
        let mut provider_preparation = None;
        let plan = if let Some(fixture) = &codex_fixture {
            let task = store
                .snapshot()?
                .projection
                .tasks
                .remove(&task_id)
                .ok_or("toy task missing before provider preparation")?;
            let provider = prepare_codex_plan(
                prepared,
                CodexPlanInput {
                    task: &task,
                    admission: &fixture.admission,
                    model: &fixture.model,
                    options: fixture.options,
                    cli: &fixture.cli,
                    task_capsule: &fixture.capsule,
                    context_digest: &launch.context_digest,
                    policy_digest: &launch.policy_digest,
                    source: &launch.source,
                    observed_utc: &launch.observed_utc,
                    not_before: launch.not_before,
                    now_tick: Some(now()? / 1000),
                },
            )
            .map_err(|error| {
                Box::new(RetainedProviderFailure::Preparation(error))
                    as Box<dyn Error + Send + Sync>
            })?;
            if provider.prepared_provider().tuple_digest() != fixture.preview.tuple_digest()
                || provider.launch_request() != &launch
            {
                return Err(Box::new(RetainedProviderFailure::Tuple(Box::new(provider))));
            }
            let bound = provider
                .bind_resources(
                    CodexResourceBinding {
                        claims,
                        reservation,
                        authority: startup.clone(),
                        environment: environment.clone(),
                        backend: backend.clone(),
                        now_tick: Some(now()? / 1000),
                    },
                    &admission,
                )
                .map_err(|error| {
                    Box::new(RetainedProviderFailure::Binding(error))
                        as Box<dyn Error + Send + Sync>
                })?;
            let (plan, _request, provider, _environment, _backend) = bound.into_parts();
            provider_preparation = Some(provider);
            plan
        } else {
            NativeExecutionPlan {
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
            }
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
        let (runtime_receipt, original_preparation, closed, transcript, capture) =
            completion.into_completion_with_capture();
        if !closed.success() {
            return Err("actual child did not close successfully".into());
        }
        phase.store(2, Ordering::SeqCst);
        let provider_evidence=provider_preparation.as_ref().map(|provider|match capture.as_ref() {
            Some(capture)=>codex_capture_summary(provider,capture,&runtime_receipt.run_id).unwrap_or_else(|_|json!({
                "status":"actual-capture-protocol-unqualified","provider_evidence_complete":false,
                "provider_observed":Value::Null,"model_observed":Value::Null,"usage":Value::Null,
                "detail":"typed parser rejected actual owned capture; retain private owner journal; no identity/usage substitution"})),
            None=>json!({"status":"typed-capture-unavailable","provider_evidence_complete":false,
                "provider_observed":Value::Null,"model_observed":Value::Null,"usage":Value::Null}),
        });
        if let Some(evidence) = &provider_evidence {
            write_new(
                &root.join("provider-observation.json"),
                &serde_json::to_vec_pretty(evidence)?,
            )?;
        }
        // Do not duplicate/publish raw provider output into artifact receipts.
        // Original private session/journal custody remains with the owner.
        let transcript_object = if provider_evidence.is_some() {
            stored(
                &objects,
                &serde_json::to_vec(&json!({"kind":"private-provider-capture-reference-v1",
                "output_digest":content_digest(&transcript)?,"output_bytes":transcript.len(),
                "runtime_receipt_ref":runtime_receipt.receipt_ref,"raw_output_retained_by_session_owner":true,"not_source":true}))?,
            )?
        } else {
            stored(&objects, &transcript)?
        };
        // Propagated errors own the actual original workspace/closure and any
        // partial-destination reconciliation evidence until process shutdown.
        let execution = try_prove_closed_workspace(original_preparation, closed)?;
        let candidate = try_materialize_source_candidate(
            execution,
            &root.join("candidates"),
            "verified-source",
        )?;
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
            "effect_scope":if codex_fixture.is_some(){"one explicitly authorized Codex attempt; provider network expected; auth context externally qualified; no network/credential isolation claim; budget/capacity settlement remains unqualified"}
                else{"fixed child changes only owned toy Rust source; fixed local Git/compiler/check argv; no network/provider"},
            "provider_evidence":provider_evidence,"durable_capacity_qualified":false,"billing_settlement":false}))?,
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
        let scoped_provider_qualified = provider_evidence
            .as_ref()
            .is_some_and(|evidence| evidence["provider_evidence_complete"] == true);
        let report = json!({"schema_version":1,"status":"passed","scope":if codex_fixture.is_some(){"one-run operator-authorized Codex source/check/acceptance proof; provider evidence eligibility reported separately"}else{"isolated actual NativeTaskLoop deterministic source/check/acceptance proof"},
            "native_execution_qualified":false,"coding_provider_qualified":scoped_provider_qualified,
            "qualification_scope":"this fresh one-run fixture only; no general provider/fleet/runtime admission",
            "provider_observed":provider_evidence.as_ref().map(|evidence|&evidence["provider_observed"]),
            "model_observed":provider_evidence.as_ref().map(|evidence|&evidence["model_observed"]),
            "provider_evidence":provider_evidence,"durable_capacity_qualified":false,"billing_settlement":false,
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
        fn only_busy_observations_retry_inside_the_existing_deadline() {
            let mut reads = 0;
            let value = session_observation(
                || {
                    reads += 1;
                    if reads < 3 {
                        Err(SessionError::Busy)
                    } else {
                        Ok(7)
                    }
                },
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap();
            assert_eq!((value, reads), (7, 3));
            let mut unknown_reads = 0;
            let unknown: Result<()> = session_observation(
                || {
                    unknown_reads += 1;
                    Err(SessionError::InputOutcomeUnknown)
                },
                Instant::now() + Duration::from_secs(1),
            );
            assert!(unknown.is_err());
            assert_eq!(unknown_reads, 1);
            let mut expired_reads = 0;
            let expired: Result<()> = session_observation(
                || {
                    expired_reads += 1;
                    Err(SessionError::Busy)
                },
                Instant::now(),
            );
            assert!(expired.is_err());
            assert_eq!(expired_reads, 1);
        }
        #[test]
        fn command_help_and_required_arguments_are_effect_free() {
            use clap::CommandFactory;
            let mut command = Cli::command();
            let mut help = Vec::new();
            command.write_long_help(&mut help).unwrap();
            assert!(String::from_utf8(help).unwrap().contains("NativeTaskLoop"));
            assert!(Cli::try_parse_from(["probe", "run", "--root", "/never-created"]).is_err());
            assert!(Cli::try_parse_from(["probe", "codex", "--root", "/never-created"]).is_err());
        }
        struct ProviderFixture {
            _custody: PrivateFixture,
            args: RunArgs,
            profile: CodexProfile,
            contract: TaskContract,
            workspace: PathBuf,
            expires: u64,
        }
        impl ProviderFixture {
            fn new() -> Self {
                use admiralty_runtime::models::{ModelRegistry, Provider};
                let custody = private_fixture();
                let root = custody.path().join("fixture");
                create_private(&root).unwrap();
                for name in ["home", "tmp", "worker-spool", "objects"] {
                    create_private(&root.join(name)).unwrap();
                }
                let home = custody.path().join("external-provider-home");
                create_private(&home).unwrap();
                let program = custody.path().join("metadata-only-codex");
                // This executable accepts metadata queries only. An exec task,
                // stdin prompt or any unrecognized argv fails before effects.
                write_new(&program, format!("#!/bin/sh\ntest \"$1\" = '--no-daemon' || exit 77\nshift\ncase \"$1\" in\n--version) test \"$#\" = 1 || exit 77; printf '%s\\n' '{}';;\n--help) test \"$#\" = 1 || exit 77; printf '%s\\n' '--no-daemon --ask-for-approval';;\nexec) test \"$#\" = 2 && test \"$2\" = '--help' || exit 77; printf '%s\\n' '--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd';;\n*) exit 77;;\nesac\n",codex::SUPPORTED_CLI_VERSION).as_bytes()).unwrap();
                fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
                let registry = custody.path().join("models.json");
                write_new(
                    &registry,
                    &serde_json::to_vec(&ModelRegistry {
                        schema_version: 1,
                        revision: "1".into(),
                        models: vec![ModelSpec {
                            id: "explicit-fixture-model".into(),
                            label: "Metadata fixture".into(),
                            provider: Provider::CodexCli,
                            provider_model: "actual-native-fixture-model".into(),
                            tier: 0,
                            enabled: true,
                            node_id: Some("fixture-node".into()),
                            endpoint: None,
                            credential_file_ref: None,
                            max_concurrency: 1,
                            capabilities: BTreeSet::from(["code".into()]),
                            task_max_attempts: 1,
                            escalate_to: None,
                        }],
                    })
                    .unwrap(),
                )
                .unwrap();
                let contract = TaskContract {
                    schema_version: 1,
                    project_id: ProjectId::new("fixture-project").unwrap(),
                    root_id: RootId::new("fixture-root").unwrap(),
                    task_id: TaskId::new("fixture-task").unwrap(),
                    revision: 1,
                    title: "Metadata preparation only".into(),
                    objective: "Edit the isolated toy source".into(),
                    non_goals: vec!["No provider turn in this test".into()],
                    owned_paths: vec!["src/main.rs".into()],
                    forbidden_paths: vec![],
                    read_first: vec![],
                    dependencies: vec![],
                    criteria: vec![AcceptanceCriterion {
                        id: "toy".into(),
                        text: "Independent oracle".into(),
                        check_ids: vec!["toy".into()],
                    }],
                    checks: vec![CheckSpec {
                        id: "toy".into(),
                        program: "fixture-check".into(),
                        args: vec![],
                        cwd: ".".into(),
                        timeout_secs: 1,
                    }],
                    authority_ref: "fixture-authority".into(),
                    max_attempts: 1,
                    simulated: false,
                };
                let workspace = root.join("workspaces/owned-run");
                Self {
                    _custody: custody,
                    args: RunArgs {
                        root,
                        operator_authorization_file: None,
                        prepare_only: true,
                        node: "fixture-node".into(),
                        cpus: "0".into(),
                        registered_cpus: "0".into(),
                        runtime_directory: PathBuf::from("/never-used-runtime"),
                        slice: "fixture.slice".into(),
                        rustc: PathBuf::from("/never-used-rustc"),
                        memory_mib: 256,
                        max_tasks: 16,
                        deadline_seconds: 90,
                    },
                    profile: CodexProfile {
                        program,
                        registry,
                        model_id: "explicit-fixture-model".into(),
                        effort: CodexEffort::Medium,
                        home,
                        auth_context_ref: "auth-context:fixture".into(),
                    },
                    contract,
                    workspace,
                    expires: now().unwrap() + 600_000,
                }
            }
            fn prepare(&self) -> Result<CodexFixture> {
                let objects =
                    ArtifactStore::open(self.args.root.join("objects"), 128 * 1024 * 1024)?;
                prepare_codex_fixture(
                    &self.profile,
                    CodexFixtureContext {
                        args: &self.args,
                        contract: &self.contract,
                        nonce: "00000000-0000-4000-8000-000000000001",
                        workspace: &self.workspace,
                        expires: self.expires,
                        objects: &objects,
                        host: &SessionHost::new(),
                    },
                )
            }
        }
        #[test]
        fn codex_preparation_observes_metadata_and_binds_exact_tuple_without_provider_turn() {
            let mut fixture = ProviderFixture::new();
            let prepared = fixture.prepare().unwrap();
            assert_eq!(
                prepared.preview.tuple().registry_model_id,
                "explicit-fixture-model"
            );
            assert_eq!(
                prepared.preview.tuple().requested_model,
                "actual-native-fixture-model"
            );
            assert_eq!(
                prepared.preview.tuple().requested_effort,
                CodexEffort::Medium
            );
            assert_eq!(prepared.preview.launch_spec().cwd, fixture.workspace);
            assert!(!fixture.workspace.exists());
            for flag in [
                "native_execution_qualified",
                "coding_provider_qualified",
                "durable_capacity_qualified",
                "billing_settlement",
                "network_or_credential_sandbox_qualified",
            ] {
                assert_eq!(prepared.snapshot[flag], false, "{flag}");
            }
            assert_eq!(
                prepared.snapshot["provider_home"],
                fixture.profile.home.to_str().unwrap()
            );
            let private_tick: u64 = serde_json::from_slice(
                &read_owned_file(
                    &fixture
                        .args
                        .root
                        .join("provider-observation/observed-tick.json"),
                    64,
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(prepared.admission.admitted_tick, private_tick);
            fixture.args.prepare_only = false;
            let replay = fixture.prepare().unwrap();
            // Reuse the exact external authorization expiry without relabeling
            // the observed CLI tick or rerunning any metadata process.
            assert_eq!(prepared.snapshot, replay.snapshot);
            assert_eq!(
                prepared.preview.tuple_digest(),
                replay.preview.tuple_digest()
            );
            assert_eq!(prepared.cli.fingerprint(), replay.cli.fingerprint());
            assert_eq!(
                prepared.snapshot["program_artifact"],
                replay.snapshot["program_artifact"]
            );
            assert_eq!(
                prepared.snapshot["registry_artifact"],
                replay.snapshot["registry_artifact"]
            );
            assert_eq!(prepared.snapshot["capsule"], replay.snapshot["capsule"]);
            assert_eq!(prepared.admission.auth_context_ref, "auth-context:fixture");
        }
        #[test]
        fn codex_revalidation_detects_binary_registry_and_observation_drift() {
            let mut fixture = ProviderFixture::new();
            let original = fixture.prepare().unwrap();
            fixture.args.prepare_only = false;
            let mut bytes = read_owned_file(&fixture.profile.program, 4096).unwrap();
            bytes.extend_from_slice(b"# changed executable bytes\n");
            fs::write(&fixture.profile.program, bytes).unwrap();
            let changed = fixture.prepare().unwrap();
            assert_ne!(
                original.snapshot["program_artifact"],
                changed.snapshot["program_artifact"]
            );
            let mut registry: Value =
                serde_json::from_slice(&read_owned_file(&fixture.profile.registry, 4096).unwrap())
                    .unwrap();
            registry["models"][0]["provider_model"] = json!("different-native-model");
            fs::write(
                &fixture.profile.registry,
                serde_json::to_vec(&registry).unwrap(),
            )
            .unwrap();
            let changed_model = fixture.prepare().unwrap();
            assert_ne!(
                original.snapshot["registry_artifact"],
                changed_model.snapshot["registry_artifact"]
            );
            assert_ne!(
                original.preview.tuple_digest(),
                changed_model.preview.tuple_digest()
            );
            assert_eq!(
                changed_model.preview.tuple().requested_model,
                "different-native-model"
            );
            let tick = fixture
                .args
                .root
                .join("provider-observation/observed-tick.json");
            fs::write(
                &tick,
                serde_json::to_vec(&(now().unwrap() / 1000 + 60)).unwrap(),
            )
            .unwrap();
            assert!(fixture.prepare().is_err());
            fs::write(
                &tick,
                serde_json::to_vec(&(now().unwrap() / 1000 - 301)).unwrap(),
            )
            .unwrap();
            assert!(fixture.prepare().is_err());
        }
        #[test]
        fn codex_profile_has_no_implicit_model_or_credential_value_reference() {
            assert!(auth_context_reference("auth-context:registered-profile_1"));
            for invalid in [
                "",
                "auth-context:",
                "sk-private-token",
                "/home/user/.codex/auth.json",
                "auth-context:../secret",
                "auth-context:one two",
            ] {
                assert!(!auth_context_reference(invalid));
            }
            let fixture = ProviderFixture::new();
            let mut profile = fixture.profile;
            profile.model_id = "unregistered".into();
            let objects =
                ArtifactStore::open(fixture.args.root.join("objects"), 128 * 1024 * 1024).unwrap();
            assert!(
                prepare_codex_fixture(
                    &profile,
                    CodexFixtureContext {
                        args: &fixture.args,
                        contract: &fixture.contract,
                        nonce: "00000000-0000-4000-8000-000000000001",
                        workspace: &fixture.workspace,
                        expires: now().unwrap() + 600_000,
                        objects: &objects,
                        host: &SessionHost::new(),
                    }
                )
                .is_err()
            );
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
