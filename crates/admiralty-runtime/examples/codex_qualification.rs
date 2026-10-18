//! Explicit operator-controlled transport conformance, never domain admission.
//! Build/test/default invocation performs no provider inference.
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract,
    TaskId, content_digest, limits::bounded_json_size,
};
use admiralty_runtime::{
    models::{ModelSpec, Provider},
    providers::{
        codex::{
            CodexAdmission, CodexCliObservation, CodexEffort, CodexExecutionOptions,
            CodexNdjsonStream, CodexSandbox, CodexStreamSummary, CodexTermination,
        },
        supervision::SupervisionScope,
    },
    session::{CaptureState, LaunchSpec, OutputStream, SessionError, SessionHost, SessionStatus},
    workspace::registry::{NodeRef, WorkspaceSpec},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const AUTHORIZATION: &str = "controlled_exact_fixture_transport_only";
const MAX_CONFIG: usize = 16 * 1024;
const MAX_FILE: usize = 64 * 1024;
const MAX_TREE_BYTES: usize = 1024 * 1024;
const MAX_TREE_FILES: usize = 128;
const MAX_PROGRAM_BYTES: u64 = 512 * 1024 * 1024;
const EXPECTED: &str =
    include_str!("../../../tests/qualification/codex/P08Q_expected_saturating_increment.rs");
const INITIAL: &str = "pub fn saturating_increment(_value: u32) -> u32 {\n    todo!(\"replace with the canonical fixture\")\n}\n";
const MANIFEST: &str =
    "[package]\nname = \"codex_transport_fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n";
const HELP: &str = "Usage: codex_qualification --config /absolute/private/config.json [--run]\nWithout --run: read-only validation/plan; no children, inference or scratch writes.\n--run authorizes one controlled exact-fixture transport run, never native domain admission.\nLinux live runs require an explicit, already owned inherited CPU affinity.\n";
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn invalid(message: &'static str) -> Box<dyn Error> {
    io::Error::new(io::ErrorKind::InvalidInput, message).into()
}
fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn clean_text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    registered_node_id: String,
    run_id: String,
    scratch_parent: PathBuf,
    codex_program: PathBuf,
    git_program: PathBuf,
    env_program: PathBuf,
    registry_model_id: String,
    provider_model: String,
    effort: CodexEffort,
    expected_cpu_affinity: String,
    cli_probe_timeout_secs: u64,
    provider_timeout_secs: u64,
    overall_timeout_secs: u64,
    max_output_bytes: usize,
    operator_transport_authorization: String,
}
impl Config {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !safe_id(&self.registered_node_id)
            || !safe_id(&self.run_id)
            || !safe_id(&self.registry_model_id)
            || !clean_text(&self.provider_model, 256)
            || self.operator_transport_authorization != AUTHORIZATION
        {
            return Err(invalid(
                "unsupported config or explicit transport-only authorization missing",
            ));
        }
        if !(1..=30).contains(&self.cli_probe_timeout_secs)
            || !(1..=300).contains(&self.provider_timeout_secs)
            || self.overall_timeout_secs > 600
            || self.overall_timeout_secs
                < self.provider_timeout_secs + 4 * self.cli_probe_timeout_secs + 5
            || !(4096..=262144).contains(&self.max_output_bytes)
        {
            return Err(invalid(
                "explicit bounded probe/provider/overall deadlines and output bound required",
            ));
        }
        if self.expected_cpu_affinity.is_empty()
            || self.expected_cpu_affinity.len() > 128
            || !self
                .expected_cpu_affinity
                .bytes()
                .all(|b| b.is_ascii_digit() || b",-".contains(&b))
        {
            return Err(invalid("explicit exact inherited CPU affinity required"));
        }
        private_directory(&self.scratch_parent)?;
        if std::env::var_os("HOME")
            .and_then(|p| fs::canonicalize(p).ok())
            .as_deref()
            == Some(self.scratch_parent.as_path())
        {
            return Err(invalid(
                "use a dedicated scratch parent, not the home directory",
            ));
        }
        // Fresh artifacts must not be placed inside an existing source checkout.
        if self
            .scratch_parent
            .ancestors()
            .any(|p| p.join(".git").exists())
        {
            return Err(invalid(
                "scratch parent must be outside existing Git checkouts",
            ));
        }
        if self
            .scratch_parent
            .join(&self.run_id)
            .symlink_metadata()
            .is_ok()
        {
            return Err(invalid(
                "run directory already exists; reconcile and choose a reviewed new identity",
            ));
        }
        for program in [&self.codex_program, &self.git_program, &self.env_program] {
            program_identity(program)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct Identity {
    dev: String,
    ino: String,
    uid: u32,
    mode: u32,
    len: String,
    mtime: String,
    mtime_nsec: i64,
}
#[cfg(unix)]
fn identity(path: &Path) -> Result<Identity> {
    metadata_identity(&fs::symlink_metadata(path)?)
}
#[cfg(unix)]
fn metadata_identity(m: &fs::Metadata) -> Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    Ok(Identity {
        dev: m.dev().to_string(),
        ino: m.ino().to_string(),
        uid: m.uid(),
        mode: m.mode(),
        len: m.len().to_string(),
        mtime: m.mtime().to_string(),
        mtime_nsec: m.mtime_nsec(),
    })
}
#[cfg(not(unix))]
fn metadata_identity(_: &fs::Metadata) -> Result<Identity> {
    Err(invalid("Unix executable identity required"))
}
#[cfg(not(unix))]
fn identity(_: &Path) -> Result<Identity> {
    Err(invalid(
        "controlled qualification currently requires Unix custody",
    ))
}
fn canonical_components(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.as_os_str().len() > 2048
        || path.components().count() > 64
        || !path.to_str().is_some_and(|s| {
            clean_text(s, 2048)
                && !s.contains('\\')
                && s.strip_prefix('/')
                    .is_some_and(|s| s.split('/').all(|p| !p.is_empty() && p != "." && p != ".."))
        })
    {
        return Err(invalid(
            "bounded lexically normalized absolute path required",
        ));
    }
    if fs::canonicalize(path)? != path {
        return Err(invalid(
            "use exact canonical paths, not aliases or symlinks",
        ));
    }
    let mut part = PathBuf::new();
    for c in path.components() {
        if !matches!(c, Component::RootDir | Component::Normal(_)) {
            return Err(invalid("path traversal refused"));
        }
        part.push(c.as_os_str());
        let metadata = fs::symlink_metadata(&part)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid("symlink component refused"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let owner =
                metadata.uid() == 0 || metadata.uid() == rustix::process::geteuid().as_raw();
            let sticky = metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
            if !owner || metadata.mode() & 0o022 != 0 && !sticky {
                return Err(invalid("path ancestor ownership or permissions are unsafe"));
            }
        }
    }
    Ok(())
}
#[cfg(unix)]
fn private_directory(path: &Path) -> Result<()> {
    canonical_components(path)?;
    let m = fs::symlink_metadata(path)?;
    let id = identity(path)?;
    if !m.is_dir() || id.uid != rustix::process::geteuid().as_raw() || id.mode & 0o777 != 0o700 {
        return Err(invalid(
            "existing effective-user-owned 0700 scratch parent required",
        ));
    }
    Ok(())
}
#[cfg(not(unix))]
fn private_directory(_: &Path) -> Result<()> {
    Err(invalid("Unix private directory required"))
}
fn program_identity(path: &Path) -> Result<Identity> {
    canonical_components(path)?;
    let m = fs::symlink_metadata(path)?;
    let id = identity(path)?;
    #[cfg(unix)]
    let owner_ok = id.uid == 0 || id.uid == rustix::process::geteuid().as_raw();
    #[cfg(not(unix))]
    let owner_ok = false;
    if !m.is_file()
        || !owner_ok
        || id.mode & 0o022 != 0
        || id.mode & 0o111 == 0
        || m.len() > MAX_PROGRAM_BYTES
    {
        return Err(invalid(
            "registered executable must be bounded, regular and non-writable by other users",
        ));
    }
    Ok(id)
}
/// Raw executable SHA-256, not canonical-JSON content_digest. This detects
/// ordinary replacement/drift, not a race-free hostile exec/loader substitution.
fn program_sha256(path: &Path, expected: &Identity) -> Result<String> {
    if program_identity(path)? != *expected {
        return Err(invalid("executable identity drift before hash"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let mut file = options.open(path)?;
    if metadata_identity(&file.metadata()?)? != *expected {
        return Err(invalid("opened executable identity differs"));
    }
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 65536];
    {
        let mut reader = (&mut file).take(MAX_PROGRAM_BYTES + 1);
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| invalid("executable hash size overflow"))?;
            if total > MAX_PROGRAM_BYTES {
                return Err(invalid("executable hash exceeds size bound"));
            }
            hasher.update(&buffer[..count]);
        }
    }
    if total.to_string() != expected.len
        || metadata_identity(&file.metadata()?)? != *expected
        || program_identity(path)? != *expected
    {
        return Err(invalid("executable changed during hash"));
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}
fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max as u64 {
        return Err(invalid("bounded regular file without symlink required"));
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err(invalid("hard-linked state file refused"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32);
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        let opened = file.metadata()?;
        if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
            return Err(invalid("file identity changed during open"));
        }
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(invalid("file exceeded read bound"));
    }
    Ok(bytes)
}
fn config(path: &Path) -> Result<Config> {
    canonical_components(path)?;
    let id = identity(path)?;
    #[cfg(unix)]
    if id.uid != rustix::process::geteuid().as_raw() || id.mode & 0o777 != 0o600 {
        return Err(invalid("config must be effective-user-owned 0600"));
    }
    let config: Config = serde_json::from_slice(&read_bounded(path, MAX_CONFIG)?)?;
    config.validate()?;
    Ok(config)
}
fn new_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    Ok(())
}
fn new_file(path: &Path, bytes: &[u8], readonly: bool) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(if readonly { 0o400 } else { 0o600 });
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct TreeEntry {
    kind: String,
    mode: u32,
    digest: Option<String>,
}
fn tree(path: &Path) -> Result<BTreeMap<String, TreeEntry>> {
    canonical_components(path)?;
    fn visit(
        base: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, TreeEntry>,
        bytes: &mut usize,
        depth: usize,
    ) -> Result<()> {
        if depth > 8 {
            return Err(invalid("scratch tree depth exceeded"));
        }
        let mut entries: Vec<_> = fs::read_dir(dir)?
            .take(MAX_TREE_FILES + 1)
            .collect::<io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if out.len() >= MAX_TREE_FILES {
                return Err(invalid("scratch tree entry limit exceeded"));
            }
            let path = entry.path();
            let m = fs::symlink_metadata(&path)?;
            let name = path
                .strip_prefix(base)?
                .to_str()
                .filter(|s| clean_text(s, 2048))
                .ok_or_else(|| invalid("invalid scratch path"))?
                .to_owned();
            let mode = identity(&path)?.mode & 0o777;
            if m.is_dir() {
                out.insert(
                    name,
                    TreeEntry {
                        kind: "directory".into(),
                        mode,
                        digest: None,
                    },
                );
                visit(base, &path, out, bytes, depth + 1)?;
            } else if m.is_file() {
                let data = read_bounded(&path, MAX_FILE)?;
                *bytes = bytes
                    .checked_add(data.len())
                    .ok_or_else(|| invalid("tree size overflow"))?;
                if *bytes > MAX_TREE_BYTES {
                    return Err(invalid("scratch tree byte limit exceeded"));
                }
                out.insert(
                    name,
                    TreeEntry {
                        kind: "file".into(),
                        mode,
                        digest: Some(content_digest(&data)?),
                    },
                );
            } else {
                return Err(invalid("symlink or special scratch artifact refused"));
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(path, path, &mut out, &mut 0, 0)?;
    Ok(out)
}
fn unchanged_except_source(
    before: &BTreeMap<String, TreeEntry>,
    after: &BTreeMap<String, TreeEntry>,
) -> bool {
    before
        .iter()
        .filter(|(p, _)| p.as_str() != "src/lib.rs")
        .eq(after.iter().filter(|(p, _)| p.as_str() != "src/lib.rs"))
}
fn protected_verify(
    work: &Path,
    oracle: &Path,
    baseline: &BTreeMap<String, TreeEntry>,
    oracle_baseline: &BTreeMap<String, TreeEntry>,
) -> Result<String> {
    if tree(oracle)? != *oracle_baseline {
        return Err(invalid("protected oracle changed"));
    }
    let after = tree(work)?;
    if !unchanged_except_source(baseline, &after) {
        return Err(invalid("artifact outside the one source path changed"));
    }
    let actual = read_bounded(&work.join("src/lib.rs"), MAX_FILE)?;
    if actual != EXPECTED.as_bytes()
        || read_bounded(&oracle.join("expected.rs"), MAX_FILE)? != EXPECTED.as_bytes()
    {
        return Err(invalid(
            "candidate differs from the protected safe exact-source fixture",
        ));
    }
    // No candidate code is compiled or executed. Equality binds it to this
    // reviewed fixture; these cases check the trusted fixture's known behavior.
    for (input, expected) in [
        (0, 1),
        (7, 8),
        (u32::MAX - 1, u32::MAX),
        (u32::MAX, u32::MAX),
    ] {
        if reference::saturating_increment(input) != expected {
            return Err(invalid("protected fixture semantics invalid"));
        }
    }
    Ok(content_digest(&actual)?)
}
mod reference {
    include!("../../../tests/qualification/codex/P08Q_expected_saturating_increment.rs");
}

#[derive(Debug, Serialize)]
struct ChildReceipt {
    run_id: String,
    exit_code: Option<i32>,
    exit_success: bool,
    raw_bytes: String,
    final_output_offset: String,
}
struct ChildOutput {
    receipt: ChildReceipt,
    stdout: Vec<u8>,
}
fn status(host: &SessionHost, run: &str) -> Result<Option<SessionStatus>> {
    match host.status(run) {
        Ok(status) => Ok(Some(status)),
        Err(SessionError::Busy) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
#[derive(Debug, Serialize)]
struct CleanupReceipt {
    run_id: String,
    outcome: String,
}
fn stop_owned(host: &SessionHost, run: &str) -> CleanupReceipt {
    let outcome = if host.terminate(run).is_ok() {
        "owned_direct_child_reaped_capture_drained;escaped_descendants_unqualified"
    } else {
        "cleanup_unknown_reconciliation_required"
    };
    CleanupReceipt {
        run_id: run.into(),
        outcome: outcome.into(),
    }
}
fn run_child(
    host: &SessionHost,
    spec: LaunchSpec,
    timeout: Duration,
    overall: Instant,
    max: usize,
    cleanups: &mut Vec<CleanupReceipt>,
) -> Result<ChildOutput> {
    let run = spec.run_id.clone();
    if Instant::now() >= overall {
        return Err(invalid("overall deadline expired before helper launch"));
    }
    let deadline = (Instant::now() + timeout).min(overall);
    if let Err(error) = host.launch(spec) {
        cleanups.push(CleanupReceipt {
            run_id: run,
            outcome: "launch_error_reconciliation_required".into(),
        });
        return Err(error.into());
    }
    let result = (|| {
        host.close_input_if_epoch(&run, 0)?;
        let mut offset = 0;
        let mut stdout = Vec::new();
        loop {
            if Instant::now() >= deadline || Instant::now() >= overall {
                return Err(invalid("owned child deadline exceeded"));
            }
            let tail = host.tail(&run, offset, 8192)?;
            for chunk in tail.chunks {
                if chunk.offset != offset {
                    return Err(invalid("child journal gap"));
                }
                offset = offset
                    .checked_add(chunk.bytes.len() as u64)
                    .ok_or_else(|| invalid("child byte counter overflow"))?;
                if offset > max as u64 {
                    return Err(invalid("child output bound exceeded"));
                }
                if chunk.stream == OutputStream::Stdout {
                    stdout.extend_from_slice(&chunk.bytes);
                }
            }
            if let Some(state) = status(host, &run)? {
                if state.capture_failed {
                    return Err(invalid("owned child capture failed"));
                }
                if let Some((code, success)) = state.exit
                    && state.capture_state == CaptureState::Complete
                    && state.final_output_offset == Some(offset)
                {
                    return Ok(ChildOutput {
                        receipt: ChildReceipt {
                            run_id: run.clone(),
                            exit_code: code,
                            exit_success: success,
                            raw_bytes: offset.to_string(),
                            final_output_offset: offset.to_string(),
                        },
                        stdout,
                    });
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        cleanups.push(stop_owned(host, &run));
    }
    result
}
#[derive(Debug, Serialize)]
struct Receipt {
    schema_version: u32,
    kind: String,
    node_registration_basis: String,
    registered_node_id: String,
    run_id: String,
    config_digest: String,
    authority_basis: String,
    program_identity: Identity,
    program_sha256: String,
    program_sha256_after: Option<String>,
    program_unchanged_after: Option<bool>,
    cpu_affinity: String,
    launch_tuple_digest: Option<String>,
    launch_tuple: Option<serde_json::Value>,
    requested_model: String,
    requested_effort: CodexEffort,
    stream: Option<CodexStreamSummary>,
    probes: Vec<ChildReceipt>,
    cleanups: Vec<CleanupReceipt>,
    oracle_result: String,
    artifact_digest: Option<String>,
    failure: Option<String>,
    native_execution_qualified: bool,
    domain_admission_granted: bool,
    task_acceptance_granted: bool,
}
fn affinity() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        let text = String::from_utf8(
            read_bounded(Path::new("/proc/self/status"), MAX_FILE).or_else(|_| {
                // procfs reports size zero; the read remains bounded, but nlink is not
                // a durable-state custody check for this explicit kernel observation.
                let mut bytes = Vec::new();
                fs::File::open("/proc/self/status")?
                    .take(MAX_FILE as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > MAX_FILE {
                    return Err(invalid("proc status bound"));
                }
                Ok(bytes)
            })?,
        )?;
        text.lines()
            .find_map(|line| line.strip_prefix("Cpus_allowed_list:").map(str::trim))
            .map(str::to_owned)
            .ok_or_else(|| invalid("CPU affinity observation unavailable"))
    }
    #[cfg(not(target_os = "linux"))]
    Err(invalid(
        "live qualification currently requires Linux recorded CPU affinity",
    ))
}
fn helper_spec(
    program: PathBuf,
    args: Vec<String>,
    work: &Path,
    spool: &Path,
    run: &str,
) -> LaunchSpec {
    LaunchSpec {
        run_id: run.into(),
        program,
        args,
        cwd: work.to_owned(),
        spool_dir: spool.join(run),
    }
}
fn run(config: &Config) -> Result<PathBuf> {
    config.validate()?;
    let actual_affinity = affinity()?;
    if actual_affinity != config.expected_cpu_affinity {
        return Err(invalid(
            "inherited CPU affinity differs from the explicit owned permit; no launch",
        ));
    }
    let program = program_identity(&config.codex_program)?;
    let program_hash = program_sha256(&config.codex_program, &program)?;
    let git = program_identity(&config.git_program)?;
    let env = program_identity(&config.env_program)?;
    let root = config.scratch_parent.join(&config.run_id);
    new_directory(&root)?;
    let work = root.join("work");
    let spool = root.join("spool");
    let oracle = root.join("protected-oracle");
    for path in [&work, &spool, &oracle] {
        new_directory(path)?;
    }
    new_directory(&work.join("src"))?;
    new_file(&work.join("Cargo.toml"), MANIFEST.as_bytes(), false)?;
    new_file(&work.join("src/lib.rs"), INITIAL.as_bytes(), false)?;
    new_file(&oracle.join("expected.rs"), EXPECTED.as_bytes(), true)?;
    new_file(
        &oracle.join("policy.txt"),
        b"P08Q protected safe exact-source fixture; no open-ended equivalence or acceptance\n",
        true,
    )?;
    let oracle_baseline = tree(&oracle)?;
    let mut receipt = Receipt {
        schema_version: 1,
        kind: "controlled_exact_fixture_transport_conformance".into(),
        node_registration_basis: "operator_supplied_registration_assertion; not_enrollment_proof"
            .into(),
        registered_node_id: config.registered_node_id.clone(),
        run_id: config.run_id.clone(),
        config_digest: content_digest(config)?,
        authority_basis:
            "explicit --run and literal operator_transport_authorization; no domain admission"
                .into(),
        program_identity: program.clone(),
        program_sha256: program_hash.clone(),
        program_sha256_after: None,
        program_unchanged_after: None,
        cpu_affinity: actual_affinity,
        launch_tuple_digest: None,
        launch_tuple: None,
        requested_model: config.provider_model.clone(),
        requested_effort: config.effort,
        stream: None,
        probes: vec![],
        cleanups: vec![],
        oracle_result: "not_run".into(),
        artifact_digest: None,
        failure: None,
        native_execution_qualified: false,
        domain_admission_granted: false,
        task_acceptance_granted: false,
    };
    let intent = serde_json::to_vec_pretty(
        &serde_json::json!({"config":config,"program_sha256":program_hash,"expected_fixture_digest":content_digest(&EXPECTED)?,"authority_basis":receipt.authority_basis,"native_execution_qualified":false,"domain_admission_granted":false,"task_acceptance_granted":false}),
    )?;
    let intent_path = root.join("intent.json");
    new_file(&intent_path, &intent, false)?;
    let started = Instant::now();
    let overall = started + Duration::from_secs(config.overall_timeout_secs);
    let host = SessionHost::new();
    let mut stage = "git_init";
    let result = (|| -> Result<()> {
        let git_args = vec![
            "-i".into(),
            "LC_ALL=C".into(),
            "GIT_CONFIG_NOSYSTEM=1".into(),
            "GIT_CONFIG_GLOBAL=/dev/null".into(),
            config
                .git_program
                .to_str()
                .ok_or_else(|| invalid("UTF-8 git path required"))?
                .into(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "core.fsmonitor=false".into(),
            "init".into(),
            "--quiet".into(),
            "--template=".into(),
            "--initial-branch=qualification".into(),
            ".".into(),
        ];
        if program_identity(&config.git_program)? != git
            || program_identity(&config.env_program)? != env
        {
            return Err(invalid("helper executable identity drift"));
        }
        let init = run_child(
            &host,
            helper_spec(
                config.env_program.clone(),
                git_args,
                &work,
                &spool,
                "git-init",
            ),
            Duration::from_secs(config.cli_probe_timeout_secs),
            overall,
            config.max_output_bytes,
            &mut receipt.cleanups,
        )?;
        let init_success = init.receipt.exit_success;
        receipt.probes.push(init.receipt);
        if !init_success {
            return Err(invalid("isolated Git initialization failed"));
        }
        let baseline = tree(&work)?;
        let observed = WorkspaceSpec {
            node_id: NodeRef::new(&config.registered_node_id)?,
            root_id: RootId::new(format!("root-{}", config.run_id))?,
            project_id: ProjectId::new(format!("project-{}", config.run_id))?,
            workspace: work.clone(),
            owned_paths: vec!["src/lib.rs".into()],
            forbidden_paths: vec!["Cargo.toml".into(), ".git".into()],
        }
        .validate()?;
        if !observed.owns(Path::new("src/lib.rs")) {
            return Err(invalid(
                "workspace custody observation does not own fixture source",
            ));
        }
        let mut help = Vec::new();
        stage = "read_only_cli_observations";
        for (run, args) in [
            ("cli-version", vec!["--version".into()]),
            ("cli-exec-help", vec!["exec".into(), "--help".into()]),
            ("cli-root-help", vec!["--help".into()]),
        ] {
            if program_identity(&config.codex_program)? != program {
                return Err(invalid("Codex executable identity drift"));
            }
            let output = run_child(
                &host,
                helper_spec(config.codex_program.clone(), args, &work, &spool, run),
                Duration::from_secs(config.cli_probe_timeout_secs),
                overall,
                65536,
                &mut receipt.cleanups,
            )?;
            let success = output.receipt.exit_success;
            receipt.probes.push(output.receipt);
            if !success {
                return Err(invalid("read-only CLI observation failed"));
            }
            help.push(String::from_utf8(output.stdout)?);
        }
        let tick = started.elapsed().as_secs();
        let cli = CodexCliObservation::from_help(
            &config.registered_node_id,
            config.codex_program.clone(),
            &help[0],
            &help[1],
            &help[2],
            Some(tick),
        )?;
        let contract=TaskContract{schema_version:1,project_id:observed.project_id.clone(),root_id:observed.root_id.clone(),task_id:TaskId::new(format!("task-{}",config.run_id))?,revision:1,title:"Controlled exact-source transport fixture".into(),objective:"Replace only src/lib.rs with the supplied canonical saturating-increment Rust source; do not run commands or modify any other path.".into(),non_goals:vec!["No open-ended coding-quality, domain admission, acceptance, network, dependencies or publication".into()],owned_paths:vec!["src/lib.rs".into()],forbidden_paths:vec!["Cargo.toml".into(),".git".into()],read_first:vec!["src/lib.rs".into()],dependencies:vec![],criteria:vec![AcceptanceCriterion{id:"exact_fixture".into(),text:"Protected independent verifier matches the safe exact-source fixture and unchanged forbidden tree".into(),check_ids:vec!["builtin_fixture_verifier".into()]}],checks:vec![CheckSpec{id:"builtin_fixture_verifier".into(),program:"operator-owned-builtin-exact-fixture-verifier".into(),args:vec![],cwd:".".into(),timeout_secs:5}],authority_ref:format!("legacy-operator-transport:{}",config.run_id),max_attempts:1,simulated:false};
        let model = ModelSpec {
            id: config.registry_model_id.clone(),
            label: "Explicit controlled transport model".into(),
            provider: Provider::CodexCli,
            provider_model: config.provider_model.clone(),
            tier: 0,
            enabled: true,
            node_id: Some(config.registered_node_id.clone()),
            endpoint: None,
            credential_file_ref: None,
            max_concurrency: 1,
            capabilities: BTreeSet::from(["code".into()]),
            task_max_attempts: 1,
            escalate_to: None,
        };
        let scope = SupervisionScope {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            attempt_id: AttemptId::new(format!("attempt-{}", config.run_id))?,
            owner: PrincipalId::new("controlled-legacy-operator")?,
            incarnation: config.run_id.clone(),
            authority_ref: contract.authority_ref.clone(),
            contract_digest: content_digest(&contract)?,
            revision: 1,
        };
        let expires_tick = tick
            .checked_add(config.provider_timeout_secs)
            .ok_or_else(|| invalid("deadline overflow"))?;
        let admission = CodexAdmission {
            scope,
            attempt_ordinal: 1,
            registry_revision: "0".into(),
            selected_model_digest: content_digest(&model)?,
            cli_observation_digest: cli.fingerprint().into(),
            node_id: config.registered_node_id.clone(),
            working_dir: work.clone(),
            spool_root: spool.clone(),
            lease_ref: format!("legacy-operator-only:{}", config.run_id),
            resource_permit_ref: format!("operator-recorded-cpu:{}", config.expected_cpu_affinity),
            custody_ref: format!("registered-only:{}", contract.root_id),
            auth_context_ref: "operator-established-ambient-codex-auth:not-read-or-attested".into(),
            admitted_tick: tick,
            expires_tick,
        };
        let capsule = format!(
            "One tiny controlled transport fixture. Replace only src/lib.rs. Its bytes must exactly equal the following canonical source, including final newline. No comments, tests, extra files, Git changes, commands, network calls or delegation. Do not inspect or change the parent/spool/protected oracle. Report completion as a claim only.\n\n```rust\n{EXPECTED}```\n"
        );
        stage = "prepare_exact_launch";
        let prepared = admiralty_runtime::providers::codex::prepare_launch(
            &contract,
            admission,
            &model,
            CodexExecutionOptions {
                effort: config.effort,
                sandbox: CodexSandbox::WorkspaceWrite,
            },
            &cli,
            &capsule,
            Some(started.elapsed().as_secs()),
        )?;
        receipt.launch_tuple_digest = Some(prepared.tuple_digest().into());
        receipt.launch_tuple = Some(serde_json::to_value(prepared.tuple())?);
        // These are references to a manual fixture decision, never inferred grants.
        if tree(&work)? != baseline
            || tree(&oracle)? != oracle_baseline
            || program_identity(&config.codex_program)? != program
            || program_sha256(&config.codex_program, &program)? != program_hash
            || affinity()? != config.expected_cpu_affinity
        {
            return Err(invalid("pre-launch custody, executable or affinity drift"));
        }
        let mut parser = CodexNdjsonStream::from_prepared(&prepared)?;
        let spec = prepared.launch_spec().clone();
        let run = spec.run_id.clone();
        stage = "provider_launch_and_capture";
        if started.elapsed().as_secs() >= expires_tick || Instant::now() >= overall {
            return Err(invalid("provider admission deadline expired before launch"));
        }
        if let Err(error) = host.launch(spec) {
            receipt.cleanups.push(CleanupReceipt {
                run_id: run.clone(),
                outcome: "launch_error_reconciliation_required".into(),
            });
            return Err(error.into());
        }
        let provider = (|| -> Result<SessionStatus> {
            let eof = host.close_input_if_epoch(&run, 0)?;
            if eof.input_epoch != 1 {
                return Err(invalid("unexpected initial EOF epoch"));
            }
            let deadline = (started + Duration::from_secs(expires_tick)).min(overall);
            let mut offset = 0;
            loop {
                if Instant::now() >= deadline || Instant::now() >= overall {
                    return Err(invalid(
                        "provider deadline exceeded; owned cleanup required",
                    ));
                }
                let tail = host.tail(&run, offset, 8192)?;
                for chunk in tail.chunks {
                    if chunk.offset != offset {
                        return Err(invalid("provider journal gap"));
                    }
                    offset = offset
                        .checked_add(chunk.bytes.len() as u64)
                        .ok_or_else(|| invalid("provider byte overflow"))?;
                    if offset > config.max_output_bytes as u64 {
                        return Err(invalid("provider output bound exceeded"));
                    }
                    parser.push_chunk(&chunk)?;
                }
                if let Some(state) = status(&host, &run)? {
                    if state.capture_failed {
                        return Err(invalid("provider capture failed"));
                    }
                    if state.input_epoch != 1 || !state.input_closed || state.input_outcome_unknown
                    {
                        return Err(invalid("provider input ownership changed"));
                    }
                    if state.exit.is_some()
                        && state.capture_state == CaptureState::Complete
                        && state.final_output_offset == Some(offset)
                    {
                        return Ok(state);
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
        })();
        match provider {
            Ok(state) => receipt.stream = Some(parser.finish_from_status(&state)),
            Err(error) => {
                receipt.cleanups.push(stop_owned(&host, &run));
                receipt.program_sha256_after = program_sha256(&config.codex_program, &program).ok();
                receipt.program_unchanged_after =
                    Some(receipt.program_sha256_after.as_ref() == Some(&program_hash));
                receipt.stream = Some(match host.status(&run) {
                    Ok(state) => parser.finish_from_status(&state),
                    Err(_) => parser.finish_at_watermark(None, CaptureState::Failed, None),
                });
                return Err(error);
            }
        }
        receipt.program_sha256_after = program_sha256(&config.codex_program, &program).ok();
        receipt.program_unchanged_after =
            Some(receipt.program_sha256_after.as_ref() == Some(&program_hash));
        if receipt.program_unchanged_after != Some(true) {
            return Err(invalid(
                "Codex executable byte digest changed or unavailable",
            ));
        }
        if receipt.stream.as_ref().is_none_or(|s| {
            s.termination != CodexTermination::ProviderTurnCompleted || !s.capture_complete
        }) {
            return Err(invalid(
                "provider termination/capture is not a complete coherent report",
            ));
        }
        if program_identity(&config.codex_program)? != program {
            return Err(invalid("Codex executable identity changed"));
        }
        stage = "independent_safe_exact_fixture_verifier";
        if read_bounded(&intent_path, MAX_CONFIG * 2)? != intent {
            return Err(invalid("retained launch intent changed"));
        }
        receipt.artifact_digest = Some(protected_verify(
            &work,
            &oracle,
            &baseline,
            &oracle_baseline,
        )?);
        receipt.oracle_result = "passed_safe_exact_source_transport_conformance_only".into();
        Ok(())
    })();
    if result.is_err() {
        receipt.oracle_result = "failed_or_unavailable".into();
        receipt.failure = Some(format!(
            "stage={stage}; controlled transport run failed; inspect private bounded journals and reconcile owned artifacts; no retry or acceptance"
        ));
    }
    bounded_json_size(&receipt, 128 * 1024).map_err(|_| invalid("receipt bound exceeded"))?;
    let receipt_path = root.join("receipt.json");
    new_file(&receipt_path, &serde_json::to_vec_pretty(&receipt)?, false)?;
    // Never delete artifacts, adopt a prior run, replay input or change a gate.
    if result.is_err() {
        return Err(invalid(
            "transport conformance failed; receipt retained in the configured fresh run directory",
        ));
    }
    Ok(receipt_path)
}

fn arguments(args: Vec<std::ffi::OsString>) -> Result<Option<(PathBuf, bool)>> {
    if args.is_empty() || args == [std::ffi::OsString::from("--help")] {
        return Ok(None);
    }
    if !(args.len() == 2 || args.len() == 3)
        || args[0] != "--config"
        || args.len() == 3 && args[2] != "--run"
    {
        return Err(invalid(
            "expected --config PATH and optional explicit --run",
        ));
    }
    Ok(Some((PathBuf::from(&args[1]), args.len() == 3)))
}
fn main() {
    let result = (|| -> Result<()> {
        let Some((path, execute)) = arguments(std::env::args_os().skip(1).take(5).collect())?
        else {
            print!("{HELP}");
            return Ok(());
        };
        let config = config(&path)?;
        if !execute {
            println!(
                "{}",
                serde_json::to_string(
                    &serde_json::json!({"mode":"read_only_plan","run_id":config.run_id,"node_registration_basis":"operator_supplied","requested_model":config.provider_model,"effort":config.effort,"requires_explicit_run":true,"native_execution_qualified":false,"domain_admission_granted":false,"task_acceptance_granted":false})
                )?
            );
            return Ok(());
        }
        let receipt = run(&config)?;
        println!("receipt: {}", receipt.display());
        Ok(())
    })();
    if result.is_err() {
        eprintln!(
            "codex_qualification: refused or failed; inspect the private config and retained scratch receipt; no native qualification or acceptance granted"
        );
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
        let mut builder = tempfile::Builder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        builder.tempdir()
    }

    #[test]
    fn no_run_is_the_default_and_unknown_arguments_fail() {
        assert!(arguments(vec![]).expect("args").is_none());
        assert_eq!(
            arguments(vec!["--config".into(), "/fixture/config".into()]).expect("args"),
            Some(("/fixture/config".into(), false))
        );
        assert!(arguments(vec!["--run".into()]).is_err());
        assert!(
            arguments(vec![
                "--config".into(),
                "/fixture/config".into(),
                "--run".into(),
                "extra".into()
            ])
            .is_err()
        );
    }
    #[test]
    fn safe_oracle_rejects_source_injection_forbidden_changes_and_oracle_mutation() -> Result<()> {
        let temp = private_tempdir()?;
        let base = temp.path().canonicalize()?;
        let work = base.join("work");
        let oracle = base.join("oracle");
        new_directory(&work)?;
        new_directory(&work.join("src"))?;
        new_directory(&oracle)?;
        new_file(&work.join("Cargo.toml"), MANIFEST.as_bytes(), false)?;
        new_file(&work.join("src/lib.rs"), INITIAL.as_bytes(), false)?;
        new_file(&oracle.join("expected.rs"), EXPECTED.as_bytes(), false)?;
        let before = tree(&work)?;
        let protected = tree(&oracle)?;
        fs::write(work.join("src/lib.rs"), EXPECTED)?;
        assert!(protected_verify(&work, &oracle, &before, &protected).is_ok());
        fs::write(
            work.join("src/lib.rs"),
            format!("{EXPECTED}fn injected() {{ std::process::exit(0); }}"),
        )?;
        assert!(protected_verify(&work, &oracle, &before, &protected).is_err());
        fs::write(work.join("src/lib.rs"), EXPECTED)?;
        fs::write(work.join("Cargo.toml"), "changed")?;
        assert!(protected_verify(&work, &oracle, &before, &protected).is_err());
        fs::write(work.join("Cargo.toml"), MANIFEST)?;
        fs::write(oracle.join("expected.rs"), "changed")?;
        assert!(protected_verify(&work, &oracle, &before, &protected).is_err());
        Ok(())
    }
    #[test]
    fn closed_config_rejects_unknown_and_duplicate_fields() {
        let bytes = include_bytes!("../../../tests/qualification/codex/P08Q_config_template.json");
        let mut value: serde_json::Value = serde_json::from_slice(bytes).expect("fixture");
        value["api_key"] = serde_json::json!("not accepted");
        assert!(serde_json::from_value::<Config>(value).is_err());
        assert!(
            serde_json::from_str::<Config>("{\"schema_version\":1,\"schema_version\":1}").is_err()
        );
    }
    #[cfg(unix)]
    #[test]
    fn executable_raw_sha256_detects_same_identity_and_timestamp_content_mutation() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let temp = private_tempdir()?;
        let path = temp.path().canonicalize()?.join("fake-program");
        new_file(&path, b"abc", false)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        let before = program_identity(&path)?;
        let modified = fs::metadata(&path)?.modified()?;
        let hash = program_sha256(&path, &before)?;
        assert_eq!(
            hash,
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::write(&path, b"abd")?;
        let file = OpenOptions::new().write(true).open(&path)?;
        file.set_times(fs::FileTimes::new().set_modified(modified))?;
        assert_eq!(program_identity(&path)?, before);
        assert_ne!(program_sha256(&path, &before)?, hash);
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn owned_fake_child_is_bounded_and_has_complete_eof_without_provider_inference() -> Result<()> {
        let temp = private_tempdir()?;
        let base = temp.path().canonicalize()?;
        let spool = base.join("spool");
        new_directory(&spool)?;
        let host = SessionHost::new();
        let mut cleanups = Vec::new();
        let output = run_child(
            &host,
            helper_spec(
                "/bin/sh".into(),
                vec!["-c".into(), "printf probe".into()],
                &base,
                &spool,
                "probe",
            ),
            Duration::from_secs(1),
            Instant::now() + Duration::from_secs(2),
            4096,
            &mut cleanups,
        )?;
        assert_eq!(output.stdout, b"probe");
        assert_eq!(output.receipt.final_output_offset, "5");
        assert!(
            run_child(
                &host,
                helper_spec(
                    "/bin/sh".into(),
                    vec!["-c".into(), "printf ready; sleep 5".into()],
                    &base,
                    &spool,
                    "timeout"
                ),
                Duration::from_millis(20),
                Instant::now() + Duration::from_secs(2),
                4096,
                &mut cleanups
            )
            .is_err()
        );
        assert!(
            run_child(
                &host,
                helper_spec(
                    "/bin/sh".into(),
                    vec!["-c".into(), "printf too-much".into()],
                    &base,
                    &spool,
                    "bounds"
                ),
                Duration::from_secs(1),
                Instant::now() + Duration::from_secs(2),
                2,
                &mut cleanups
            )
            .is_err()
        );
        assert_eq!(cleanups.len(), 2);
        assert!(cleanups.iter().all(|c| {
            c.outcome
                .starts_with("owned_direct_child_reaped_capture_drained")
        }));
        Ok(())
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn full_linux_harness_uses_only_fake_provider_and_retains_false_qualification() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let temp = private_tempdir()?;
        let parent = temp.path().canonicalize()?;
        let program = parent.join("fake-codex");
        new_file(
            &program,
            include_bytes!("../../../tests/qualification/codex/P08Q_fake_codex.sh"),
            false,
        )?;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700))?;
        let config = Config {
            schema_version: 1,
            registered_node_id: "fake-node".into(),
            run_id: "p08q-fake-test".into(),
            scratch_parent: parent,
            codex_program: program,
            git_program: fs::canonicalize("/usr/bin/git")?,
            env_program: fs::canonicalize("/usr/bin/env")?,
            registry_model_id: "fake-registry-model".into(),
            provider_model: "fixture-model".into(),
            effort: CodexEffort::Medium,
            expected_cpu_affinity: affinity()?,
            cli_probe_timeout_secs: 1,
            provider_timeout_secs: 3,
            overall_timeout_secs: 12,
            max_output_bytes: 4096,
            operator_transport_authorization: AUTHORIZATION.into(),
        };
        let receipt = run(&config)?;
        let value: serde_json::Value =
            serde_json::from_slice(&read_bounded(&receipt, MAX_CONFIG * 4)?)?;
        assert_eq!(value["native_execution_qualified"], false);
        assert_eq!(value["domain_admission_granted"], false);
        assert_eq!(value["task_acceptance_granted"], false);
        assert_eq!(value["requested_model"], "fixture-model");
        assert_eq!(value["stream"]["reported_model"], serde_json::Value::Null);
        assert_eq!(
            value["oracle_result"],
            "passed_safe_exact_source_transport_conformance_only"
        );
        assert!(run(&config).is_err()); // No adoption/replay of an existing run.
        Ok(())
    }
}
