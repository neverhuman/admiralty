//! Offline domain operations and an isolated, read-only demo. No provider execution.
mod diagnostics;
mod workplans;
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, CoordinationMessage, DomainEvent, MessageId,
    MessagePayload, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract, TaskId,
    compile_task_capsule, content_digest,
};
use admiralty_server::{ServerConfig, serve};
use admiralty_store::Store;
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
};

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const SCHEMA_FILES: [&str; 3] = [
    "task-contract.schema.json",
    "coordination-message.schema.json",
    "domain-event.schema.json",
];

#[derive(Parser)]
#[command(
    name = "admiralty",
    version,
    about = "Bounded work and durable coordination; native execution remains unqualified"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run an isolated fake fleet. No accounts, real providers or live projects.
    Demo {
        #[arg(long, default_value = "127.0.0.1:18892")]
        listen: SocketAddr,
        /// Built static dashboard directory; omit for the API landing page.
        #[arg(long)]
        assets: Option<PathBuf>,
        /// Emit the simulated store snapshot and exit without binding a socket.
        #[arg(long)]
        snapshot_only: bool,
    },
    /// Report this binary's capabilities and remaining qualification limits.
    Doctor,
    /// Explain a recorded diagnostic JSON file. Never refreshes or grants execution.
    Explain {
        #[arg(long)]
        file: PathBuf,
    },
    /// Read an existing database offline, acquiring its exclusive store ownership.
    Snapshot {
        #[arg(long)]
        database: PathBuf,
    },
    /// Validate or record bounded task contracts. Does not launch tasks.
    Work {
        #[command(subcommand)]
        command: WorkCommand,
    },
    /// Preview or register immutable workplan revisions offline. Does not launch tasks.
    Workplan(workplans::WorkplanArgs),
    /// Compile a protected task capsule from an existing offline database.
    Context {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        task: String,
        #[arg(long)]
        epoch: String,
        /// UTF-8 byte estimate, not a provider tokenizer measurement.
        #[arg(long, default_value_t = 4096)]
        max_tokens: usize,
    },
    /// Offline durable mailbox operations; no terminal delivery or live transport.
    Messages {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Generate or check the current Rust JSON Schemas.
    Contracts {
        #[command(subcommand)]
        command: ContractCommand,
    },
    /// Configure model tiers and compute a route without launching a provider.
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// SQLite online backup of an exclusively owned offline database.
    Backup {
        #[arg(long)]
        database: PathBuf,
        /// New destination file; an existing destination is never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum ModelCommand {
    List {
        #[arg(long)]
        registry: PathBuf,
    },
    Replace {
        #[arg(long)]
        registry: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        expected_revision: String,
    },
    Select {
        #[arg(long)]
        registry: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        capacity: PathBuf,
    },
}

#[derive(Args)]
struct SourceArgs {
    #[arg(long, default_value = "operator")]
    producer: String,
    /// Caller-owned sequence, shared by this producer across all CLI event types.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=i64::MAX as u64))]
    sequence: u64,
}

impl SourceArgs {
    fn identity(&self) -> SourceIdentity {
        SourceIdentity {
            producer_id: self.producer.clone(),
            incarnation: "cli-v1".into(),
            sequence: self.sequence.to_string(),
        }
    }
}

#[derive(Subcommand)]
enum WorkCommand {
    Validate {
        #[arg(long)]
        file: PathBuf,
    },
    Submit {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[command(flatten)]
        source: SourceArgs,
    },
}

#[derive(Subcommand)]
enum MessageCommand {
    Queue {
        #[arg(long)]
        database: PathBuf,
        /// JSON CoordinationMessage; its scope is checked against existing tasks.
        #[arg(long)]
        file: PathBuf,
        #[command(flatten)]
        source: SourceArgs,
    },
    Inbox {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        recipient: String,
        #[arg(long)]
        include_acked: bool,
    },
    Ack {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        message: String,
        #[arg(long)]
        recipient: String,
        #[arg(long)]
        epoch: String,
        #[command(flatten)]
        source: SourceArgs,
    },
}

#[derive(Subcommand)]
enum ContractCommand {
    Export {
        #[arg(long, default_value = "schemas")]
        directory: PathBuf,
    },
    Check {
        #[arg(long, default_value = "schemas")]
        directory: PathBuf,
    },
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}

fn private_tempdir(prefix: &str) -> io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

fn read_bounded(path: &Path) -> CliResult<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC).bits() as i32,
        );
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(invalid(
            "JSON input must be a regular file of at most one MiB",
        ));
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_JSON_BYTES {
        return Err(invalid(
            "JSON input exceeds one MiB; use bounded artifact references",
        ));
    }
    Ok(data)
}

fn read_contract(path: &Path) -> CliResult<TaskContract> {
    let contract: TaskContract = serde_json::from_slice(&read_bounded(path)?)?;
    contract.validate()?;
    Ok(contract)
}

fn explain_file(path: &Path) -> CliResult<diagnostics::OperatorDiagnostic> {
    // Separate from the existing readers so this command cannot block opening
    // a FIFO/device or follow a final-component symlink. No other CLI flow changes.
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC)
                .bits() as i32,
        );
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("recorded diagnostic input must not be a symlink"));
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(invalid(
            "recorded diagnostic input must be a regular file of at most one MiB",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(diagnostics::parse_and_explain(&bytes)?)
}

fn open_existing(path: &Path) -> CliResult<Store> {
    if !path.is_file() {
        return Err(invalid(format!(
            "database does not exist: {}; submit a contract or register a workplan in a new owned database first",
            path.display()
        )));
    }
    Ok(Store::open(path)?)
}

fn print_json(value: Value) -> CliResult<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, &value)?;
    writeln!(output)?;
    Ok(())
}

fn schema_documents() -> CliResult<Vec<(&'static str, Vec<u8>)>> {
    let schemas = [
        serde_json::to_value(schemars::schema_for!(TaskContract))?,
        serde_json::to_value(schemars::schema_for!(CoordinationMessage))?,
        serde_json::to_value(schemars::schema_for!(DomainEvent))?,
    ];
    SCHEMA_FILES
        .into_iter()
        .zip(schemas)
        .map(|(name, schema)| {
            let mut bytes = serde_json::to_vec_pretty(&schema)?;
            bytes.push(b'\n');
            Ok((name, bytes))
        })
        .collect()
}

fn export_schemas(directory: &Path) -> CliResult<()> {
    let documents = schema_documents()?;
    fs::create_dir_all(directory)?;
    for (name, bytes) in documents {
        let mut staging = tempfile::NamedTempFile::new_in(directory)?;
        staging.write_all(&bytes)?;
        staging.as_file().sync_all()?;
        staging.persist(directory.join(name))?;
    }
    Ok(())
}

fn check_schemas(directory: &Path) -> CliResult<()> {
    let generated = tempfile::tempdir()?;
    export_schemas(generated.path())?;
    let mut drift = Vec::new();
    for name in SCHEMA_FILES {
        match fs::read(directory.join(name)) {
            Ok(actual) if actual == fs::read(generated.path().join(name))? => {}
            _ => drift.push(name),
        }
    }
    if !drift.is_empty() {
        return Err(invalid(format!(
            "contract schema drift or missing files in {}: {}; run contracts export --directory with the intended directory",
            directory.display(),
            drift.join(", ")
        )));
    }
    Ok(())
}

fn demo_contract(project: &str, task: &str, dependencies: Vec<TaskId>) -> CliResult<TaskContract> {
    let contract = TaskContract {
        schema_version: 1,
        project_id: ProjectId::new(project)?,
        root_id: RootId::new(format!("{project}-root"))?,
        task_id: TaskId::new(task)?,
        revision: 1,
        title: format!("Synthetic task {task}"),
        objective: "Demonstrate bounded coordination with explicit scope and uncertainty".into(),
        non_goals: vec!["No provider, publication, or real repository operations".into()],
        owned_paths: vec![format!("demo/{task}.txt")],
        forbidden_paths: vec![".github".into()],
        read_first: vec!["README.md".into()],
        dependencies,
        criteria: vec![AcceptanceCriterion {
            id: "synthetic-check".into(),
            text: "Synthetic result remains a candidate until independently verified".into(),
            check_ids: vec!["offline-check".into()],
        }],
        checks: vec![CheckSpec {
            id: "offline-check".into(),
            program: "admiralty".into(),
            args: vec!["doctor".into()],
            cwd: ".".into(),
            timeout_secs: 10,
        }],
        authority_ref: "demo:synthetic-no-live-authority".into(),
        max_attempts: 1,
        simulated: true,
    };
    contract.validate()?;
    Ok(contract)
}

/// Only records synthetic facts. No process or external effects are involved.
fn seed_demo(store: &Store) -> CliResult<()> {
    let first = demo_contract("example-harbor", "demo-contract", vec![])?;
    let second = demo_contract("example-observatory", "demo-ui", vec![])?;
    let third = demo_contract(
        "example-harbor",
        "demo-consumer",
        vec![first.task_id.clone()],
    )?;
    let owner = PrincipalId::new("demo-worker")?;
    let captain = PrincipalId::new("demo-captain")?;
    let first_attempt = AttemptId::new("demo-contract-attempt")?;
    let second_attempt = AttemptId::new("demo-ui-attempt")?;
    let assignment_id = MessageId::new("demo-assignment")?;
    let first_digest = content_digest(&first)?;
    let first_id = first.task_id.clone();
    let second_id = second.task_id.clone();
    let first_project = first.project_id.clone();
    let events = vec![
        DomainEvent::TaskSubmitted { contract: first },
        DomainEvent::TaskSubmitted { contract: second },
        DomainEvent::TaskSubmitted { contract: third },
        DomainEvent::TaskClaimed {
            task_id: first_id.clone(),
            revision: 1,
            attempt_id: first_attempt.clone(),
            owner: owner.clone(),
        },
        DomainEvent::TaskStarted {
            task_id: first_id.clone(),
            attempt_id: first_attempt,
        },
        DomainEvent::TaskClaimed {
            task_id: second_id.clone(),
            revision: 1,
            attempt_id: second_attempt.clone(),
            owner: PrincipalId::new("demo-ui-worker")?,
        },
        DomainEvent::TaskStarted {
            task_id: second_id.clone(),
            attempt_id: second_attempt.clone(),
        },
        DomainEvent::CandidateSubmitted {
            task_id: second_id,
            attempt_id: second_attempt,
            candidate_ref: "synthetic:demo-candidate-not-verified".into(),
        },
        DomainEvent::MessageQueued {
            message: CoordinationMessage {
                schema_version: 1,
                message_id: assignment_id.clone(),
                project_id: first_project.clone(),
                sender: captain,
                recipient: owner.clone(),
                causation_id: None,
                context_epoch: "demo-epoch-1".into(),
                authority_ref: "demo:synthetic-no-live-authority".into(),
                payload: MessagePayload::TaskAssigned {
                    task_id: first_id.clone(),
                    revision: 1,
                    contract_digest: first_digest,
                },
            },
        },
        DomainEvent::MessageAcknowledged {
            message_id: assignment_id.clone(),
            recipient: owner.clone(),
            context_epoch: "demo-epoch-1".into(),
        },
        DomainEvent::MessageQueued {
            message: CoordinationMessage {
                schema_version: 1,
                message_id: MessageId::new("demo-interface-question")?,
                project_id: first_project,
                sender: owner,
                recipient: PrincipalId::new("demo-interface-owner")?,
                causation_id: Some(assignment_id),
                context_epoch: "demo-epoch-1".into(),
                authority_ref: "demo:synthetic-no-live-authority".into(),
                payload: MessagePayload::Question {
                    task_id: first_id,
                    requirement: "Versioned interface before dependent work".into(),
                    question: "Which pinned interface revision should the consumer use?".into(),
                    options: vec![
                        "Wait for independently verified contract".into(),
                        "Request an explicitly provisional interface".into(),
                    ],
                },
            },
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        store.append(
            SourceIdentity {
                producer_id: "isolated-demo".into(),
                incarnation: "synthetic-v1".into(),
                sequence: (index + 1).to_string(),
            },
            event,
            Utc::now().to_rfc3339(),
        )?;
    }
    Ok(())
}

async fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::Demo {
            listen,
            assets,
            snapshot_only,
        } => {
            if !listen.ip().is_loopback() {
                return Err(invalid("demo bind must use a loopback address"));
            }
            if let Some(path) = &assets
                && (!path.is_dir() || !path.join("index.html").is_file())
            {
                return Err(invalid(
                    "assets must name a built dashboard directory containing index.html",
                ));
            }
            let directory = private_tempdir("admiralty-isolated-demo-")?;
            let store = Store::open(directory.path().join("demo.sqlite"))?;
            seed_demo(&store)?;
            eprintln!(
                "Isolated synthetic demo: no live providers, project authority or acceptance. Temporary state is removed on exit."
            );
            if snapshot_only {
                print_json(serde_json::to_value(store.snapshot()?)?)?;
            } else {
                serve(
                    store,
                    ServerConfig {
                        listen,
                        simulated: true,
                        token: None,
                        assets,
                        legacy: None,
                        models_file: None,
                    },
                )
                .await?;
            }
            // Store ownership drains before the enclosing temporary directory.
        }
        Command::Doctor => {
            print_json(json!({
                "version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS,
                "architecture": std::env::consts::ARCH,
                "capabilities": {
                    "isolated_offline_demo": true, "durable_single_owner_event_store": true,
                    "task_contract_validation": true, "offline_mailboxes": true,
                    "workplan_import_preview": true,
                    "offline_workplan_revision_registry": true,
                    "protected_capsule_compiler": true, "read_only_http_snapshot_and_replay": true,
                    "sqlite_online_backup": true, "json_schema_export_and_drift_check": true
                },
                "native_execution_qualified": false, "runtime_enabled": false,
                "limits": [
                    "No provider adapter, managed session, scheduling, fleet enrollment or publication is qualified",
                    "Offline database commands require exclusive ownership; stop its server before opening that same database",
                    "Capsule token counts are UTF-8 byte estimates; no measured savings claim",
                    "TypeScript bindings and runtime guards are generated by the repository's contracts scripts"
                ]
            }))?;
        }
        Command::Explain { file } => {
            eprintln!(
                "Recorded-only diagnostic: no live refresh, execution or acceptance authority."
            );
            let diagnostic = explain_file(&file)?;
            if diagnostic.execution_granted() || diagnostic.acceptance_granted() {
                return Err(invalid(
                    "recorded diagnostic unexpectedly granted authority",
                ));
            }
            print_json(serde_json::to_value(diagnostic)?)?;
        }
        Command::Snapshot { database } => {
            print_json(serde_json::to_value(open_existing(&database)?.snapshot()?)?)?;
        }
        Command::Workplan(args) => workplans::run(args)?,
        Command::Work { command } => match command {
            WorkCommand::Validate { file } => {
                let contract = read_contract(&file)?;
                print_json(
                    json!({"valid": true, "task_id": contract.task_id, "revision": contract.revision, "contract_digest": content_digest(&contract)?, "native_execution_qualified": false}),
                )?;
            }
            WorkCommand::Submit {
                database,
                file,
                source,
            } => {
                let contract = read_contract(&file)?;
                let store = Store::open(database)?;
                let receipt = store.append(
                    source.identity(),
                    DomainEvent::TaskSubmitted { contract },
                    Utc::now().to_rfc3339(),
                )?;
                print_json(
                    json!({"offline": true, "execution_started": false, "receipt": receipt}),
                )?;
            }
        },
        Command::Context {
            database,
            task,
            epoch,
            max_tokens,
        } => {
            let task_id = TaskId::new(task)?;
            let snapshot = open_existing(&database)?.snapshot()?;
            let view = snapshot
                .projection
                .tasks
                .get(&task_id)
                .ok_or_else(|| invalid("task does not exist in this database"))?;
            let capsule =
                compile_task_capsule(&view.contract, &epoch, max_tokens).map_err(invalid)?;
            print_json(serde_json::to_value(capsule)?)?;
        }
        Command::Messages { command } => match command {
            MessageCommand::Queue {
                database,
                file,
                source,
            } => {
                let message: CoordinationMessage = serde_json::from_slice(&read_bounded(&file)?)?;
                message.validate().map_err(invalid)?;
                let receipt = open_existing(&database)?.append(
                    source.identity(),
                    DomainEvent::MessageQueued { message },
                    Utc::now().to_rfc3339(),
                )?;
                print_json(
                    json!({"offline": true, "terminal_delivery": false, "receipt": receipt}),
                )?;
            }
            MessageCommand::Inbox {
                database,
                recipient,
                include_acked,
            } => {
                let recipient = PrincipalId::new(recipient)?;
                let snapshot = open_existing(&database)?.snapshot()?;
                let messages: Vec<_> = snapshot
                    .projection
                    .messages
                    .values()
                    .filter(|message| {
                        message.recipient == recipient
                            && (include_acked
                                || !snapshot
                                    .projection
                                    .acknowledgments
                                    .contains_key(&message.message_id))
                    })
                    .collect();
                print_json(
                    json!({"offline": true, "cursor": snapshot.cursor, "recipient": recipient, "messages": messages}),
                )?;
            }
            MessageCommand::Ack {
                database,
                message,
                recipient,
                epoch,
                source,
            } => {
                let event = DomainEvent::MessageAcknowledged {
                    message_id: MessageId::new(message)?,
                    recipient: PrincipalId::new(recipient)?,
                    context_epoch: epoch,
                };
                let receipt = open_existing(&database)?.append(
                    source.identity(),
                    event,
                    Utc::now().to_rfc3339(),
                )?;
                print_json(
                    json!({"offline": true, "acknowledgment": "application_receipt_only", "receipt": receipt}),
                )?;
            }
        },
        Command::Contracts { command } => match command {
            ContractCommand::Export { directory } => {
                export_schemas(&directory)?;
                print_json(
                    json!({"exported": SCHEMA_FILES, "directory": directory, "typescript_generated": false}),
                )?;
            }
            ContractCommand::Check { directory } => {
                check_schemas(&directory)?;
                print_json(
                    json!({"checked": SCHEMA_FILES, "directory": directory, "drift": false}),
                )?;
            }
        },
        Command::Models { command } => {
            use admiralty_runtime::models::{
                CapacitySnapshot, ModelRegistry, RegistryFile, SelectionRequest,
            };
            match command {
                ModelCommand::List { registry } => {
                    let value = RegistryFile::new(registry)?.read()?;
                    print_json(serde_json::to_value(
                        admiralty_server::models::public_models(value),
                    )?)?;
                }
                ModelCommand::Replace {
                    registry,
                    file,
                    expected_revision,
                } => {
                    let desired: ModelRegistry = serde_json::from_slice(&read_bounded(&file)?)?;
                    desired.validate()?;
                    let saved =
                        RegistryFile::new(registry)?.replace(&expected_revision, desired.models)?;
                    print_json(serde_json::to_value(
                        admiralty_server::models::public_models(saved),
                    )?)?;
                }
                ModelCommand::Select {
                    registry,
                    request,
                    capacity,
                } => {
                    let request: SelectionRequest =
                        serde_json::from_slice(&read_bounded(&request)?)?;
                    let capacity: CapacitySnapshot =
                        serde_json::from_slice(&read_bounded(&capacity)?)?;
                    let value = RegistryFile::new(registry)?
                        .read()?
                        .select(&request, &capacity)?;
                    print_json(serde_json::to_value(value)?)?;
                }
            }
        }
        Command::Backup { database, output } => {
            let snapshot = open_existing(&database)?.backup(&output)?;
            print_json(
                json!({"offline": true, "backup": output, "watermark": snapshot.cursor, "schema_version": snapshot.schema_version}),
            )?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("admiralty: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::SchedulingState;

    #[test]
    fn cli_help_and_zero_sequence_are_side_effect_free() {
        assert!(Cli::try_parse_from(["admiralty", "--help"]).is_err());
        assert!(
            Cli::try_parse_from([
                "admiralty",
                "work",
                "submit",
                "--database",
                "unused",
                "--file",
                "unused",
                "--sequence",
                "0"
            ])
            .is_err()
        );
    }

    #[test]
    fn isolated_demo_has_bounded_coordination_and_no_acceptance() -> CliResult<()> {
        let directory = private_tempdir("admiralty-demo-test-")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                directory.path().metadata()?.permissions().mode() & 0o777,
                0o700
            );
        }
        let store = Store::open(directory.path().join("demo.sqlite"))?;
        seed_demo(&store)?;
        let snapshot = store.snapshot()?;
        assert_eq!(snapshot.projection.tasks.len(), 3);
        assert!(
            snapshot
                .projection
                .tasks
                .values()
                .all(|view| view.contract.simulated)
        );
        for state in [
            SchedulingState::Running,
            SchedulingState::AwaitingVerification,
            SchedulingState::Blocked,
        ] {
            assert!(
                snapshot
                    .projection
                    .tasks
                    .values()
                    .any(|view| view.state == state)
            );
        }
        assert_eq!(snapshot.projection.messages.len(), 2);
        assert_eq!(snapshot.projection.acknowledgments.len(), 1);
        assert_eq!(snapshot.cursor.event_seq, "11");
        Ok(())
    }

    #[test]
    fn schema_check_detects_missing_and_modified_files() -> CliResult<()> {
        let directory = tempfile::tempdir()?;
        assert!(check_schemas(directory.path()).is_err());
        export_schemas(directory.path())?;
        check_schemas(directory.path())?;
        fs::write(directory.path().join(SCHEMA_FILES[0]), b"{}")?;
        assert!(check_schemas(directory.path()).is_err());
        Ok(())
    }

    #[test]
    fn invalid_task_and_oversized_inputs_fail_before_store_creation() -> CliResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("task.json");
        let mut contract = demo_contract("arbitrary-project", "bounded-task", vec![])?;
        contract.owned_paths = vec!["../foreign".into()];
        fs::write(&path, serde_json::to_vec(&contract)?)?;
        assert!(read_contract(&path).is_err());
        fs::write(&path, vec![b' '; MAX_JSON_BYTES as usize + 1])?;
        assert!(read_bounded(&path).is_err());
        assert!(!directory.path().join("store.sqlite").exists());
        Ok(())
    }

    #[test]
    fn missing_snapshot_database_is_not_created() -> CliResult<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("missing.sqlite");
        assert!(open_existing(&database).is_err());
        assert!(!database.exists());
        Ok(())
    }
    fn diagnostic_fixture() -> Value {
        json!({"schema_version":1,"status":{
            "project_id":"recorded-project","root_id":"recorded-root","task_id":"recorded-task",
            "contract_revision":1,"contract_digest":format!("sha256:{}","a".repeat(64)),"authority_ref":"grant:recorded-task",
            "attempt_id":"attempt-one","owner":"recorded-owner","producer_id":"recorded-producer","producer_incarnation":"boot-one",
            "source_sequence":"9007199254740993","source_revision":"rev-1","source_tick":"10","state":"tool_wait",
            "ownership":"known","provider_liveness":"live","evidence_presence":"unknown","evidence_provenance":"unknown",
            "native_execution_qualified":false,"candidate_head":null,"receipt_digest":null},
            "policy":{"explicit_now":"10","stale_after":"5","previous_source":null}})
    }
    #[test]
    fn explain_command_reads_only_bounded_recorded_json_and_preserves_counters() -> CliResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("recorded.json");
        let bytes = serde_json::to_vec(&diagnostic_fixture())?;
        fs::write(&path, &bytes)?;
        let cli = Cli::try_parse_from([
            "admiralty",
            "explain",
            "--file",
            path.to_str()
                .ok_or_else(|| invalid("fixture path is UTF-8"))?,
        ])?;
        assert!(matches!(cli.command, Command::Explain { .. }));
        let result = explain_file(&path)?;
        let output = serde_json::to_value(result)?;
        assert_eq!(output["recorded_only"], true);
        assert_eq!(output["read_only"], true);
        assert_eq!(output["execution_granted"], false);
        assert_eq!(output["acceptance_granted"], false);
        assert_eq!(output["source"]["sequence"], "9007199254740993");
        assert_eq!(fs::read(&path)?, bytes);
        assert_eq!(fs::read_dir(directory.path())?.count(), 1);
        fs::write(&path, vec![b' '; MAX_JSON_BYTES as usize + 1])?;
        assert!(explain_file(&path).is_err());
        assert!(explain_file(directory.path()).is_err());
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn explain_refuses_symlinks_and_special_files_before_reading() -> CliResult<()> {
        use std::os::unix::{fs::symlink, net::UnixListener};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("recorded.json");
        fs::write(&path, serde_json::to_vec(&diagnostic_fixture())?)?;
        let link = directory.path().join("alias.json");
        symlink(&path, &link)?;
        assert!(explain_file(&link).is_err());
        let socket = directory.path().join("socket");
        let _listener = UnixListener::bind(&socket)?;
        assert!(explain_file(&socket).is_err());
        Ok(())
    }
}
