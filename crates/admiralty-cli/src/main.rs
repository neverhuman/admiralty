//! Offline domain operations and an isolated, read-only demo. No provider execution.
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
    fs::{self, File},
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
        #[arg(long, default_value = "127.0.0.1:18891")]
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
    /// Generate or check the current Rust JSON Schemas. No TypeScript generator yet.
    Contracts {
        #[command(subcommand)]
        command: ContractCommand,
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

fn read_bounded(path: &Path) -> CliResult<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut data)?;
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

fn open_existing(path: &Path) -> CliResult<Store> {
    if !path.is_file() {
        return Err(invalid(format!(
            "database does not exist: {}; submit a contract to a new owned database first",
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
                authority_ref: "demo:synthetic".into(),
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
                authority_ref: "demo:synthetic".into(),
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
            let directory = tempfile::Builder::new()
                .prefix("admiralty-isolated-demo-")
                .tempdir()?;
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
                    "protected_capsule_compiler": true, "read_only_http_snapshot_and_replay": true,
                    "sqlite_online_backup": true, "json_schema_export_and_drift_check": true
                },
                "native_execution_qualified": false, "runtime_enabled": false,
                "limits": [
                    "No provider adapter, managed session, scheduling, fleet enrollment or publication is qualified",
                    "Offline database commands require exclusive ownership; stop its server before opening that same database",
                    "Capsule token counts are UTF-8 byte estimates; no measured savings claim",
                    "JSON Schemas are generated from Rust; TypeScript binding generation is not implemented"
                ]
            }))?;
        }
        Command::Snapshot { database } => {
            print_json(serde_json::to_value(open_existing(&database)?.snapshot()?)?)?;
        }
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
        let directory = tempfile::tempdir()?;
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
}
