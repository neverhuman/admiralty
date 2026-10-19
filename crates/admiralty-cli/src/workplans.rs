//! Offline workplan UX. Database access always acquires the Store writer lock.
use crate::{CliResult, invalid, open_existing, print_json, read_bounded};
use admiralty_core::{
    ProjectId, TaskId, content_digest,
    workplans::{
        Workplan, preview_workplan,
        source::{PlanSource, preview_source},
    },
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct WorkplanArgs {
    /// Compatibility shorthand for `workplan preview --file ...`.
    #[arg(long, required = true)]
    file: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<WorkplanCommand>,
}

#[derive(Subcommand)]
enum WorkplanCommand {
    /// Preview typed JSON/TOML or a guided Markdown task table without recording it.
    Preview {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = SourceFormat::Json)]
        format: SourceFormat,
    },
    /// Record one immutable revision. Creates an exclusively owned offline database if absent.
    Register {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        file: PathBuf,
        /// Compare-and-swap current revision; 0 creates a new plan.
        #[arg(long, default_value = "0")]
        expected_revision: String,
        /// Stable retry identity. Omit to derive it from the exact scope, expectation and input.
        #[arg(long)]
        command_id: Option<String>,
    },
    /// Read a registered plan; omit revision for the current one.
    Show {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        plan: String,
        #[arg(long)]
        revision: Option<u32>,
    },
    /// List a bounded page of current plans within a project.
    List {
        #[arg(long)]
        database: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceFormat {
    Json,
    Toml,
    Markdown,
}

impl From<SourceFormat> for PlanSource {
    fn from(value: SourceFormat) -> Self {
        match value {
            SourceFormat::Json => Self::Json,
            SourceFormat::Toml => Self::Toml,
            SourceFormat::Markdown => Self::Markdown,
        }
    }
}

fn preview(file: &Path) -> CliResult<()> {
    let preview = preview_workplan(&read_bounded(file)?)?;
    print_json(json!({
        "preview_only": true, "execution_started": false,
        "plan_id": preview.plan_id, "revision": preview.revision,
        "project_id": preview.project_id, "source_digest": preview.source_digest,
        "task_order": preview.task_order, "task_digests": preview.task_digests,
    }))
}

pub fn run(args: WorkplanArgs) -> CliResult<()> {
    match args.command {
        None => preview(
            args.file
                .as_deref()
                .ok_or_else(|| invalid("--file is required"))?,
        ),
        Some(WorkplanCommand::Preview {
            file,
            format: SourceFormat::Json,
        }) => preview(&file),
        Some(WorkplanCommand::Preview { file, format }) => {
            let result = preview_source(&read_bounded(&file)?, format.into())?;
            print_json(json!({"preview_only": true, "execution_started": false, "source": result}))
        }
        Some(WorkplanCommand::Register {
            database,
            file,
            expected_revision,
            command_id,
        }) => {
            let source = read_bounded(&file)?;
            let preview = preview_workplan(&source)?;
            let plan: Workplan = serde_json::from_slice(&source)?;
            if expected_revision.len() > 10 {
                return Err(invalid("expected revision must be a canonical u32"));
            }
            let expected: u32 = expected_revision.parse()?;
            if expected.to_string() != expected_revision
                || expected.checked_add(1) != Some(plan.revision)
            {
                return Err(invalid(
                    "workplan revision must advance the canonical expected revision by one",
                ));
            }
            if let Some(id) = &command_id {
                if id.len() > 128 {
                    return Err(invalid("workplan command ID exceeds 128 bytes"));
                }
                TaskId::new(id.as_str())?;
            }
            let canonical_digest = content_digest(&plan)?;
            // The full raw and canonical identities both participate: whitespace
            // changes are distinct requests, while exact retries reuse one receipt.
            let command_id = match command_id {
                Some(id) => id,
                None => format!(
                    "workplan-{}",
                    content_digest(&(
                        &preview.project_id,
                        &preview.plan_id,
                        &expected_revision,
                        &preview.source_digest,
                        &canonical_digest,
                    ))?
                    .strip_prefix("sha256:")
                    .ok_or_else(|| invalid("unexpected workplan digest format"))?
                ),
            };
            let request = RegisterWorkplanRequest {
                command_id,
                project_id: preview.project_id,
                plan_id: preview.plan_id,
                expected_current_revision: expected_revision,
                source,
                source_digest: preview.source_digest,
                canonical_digest,
            };
            let receipt = Store::open(database)?.register_workplan(request)?;
            print_json(json!({"offline": true, "execution_started": false, "receipt": receipt}))
        }
        Some(WorkplanCommand::Show {
            database,
            project,
            plan,
            revision,
        }) => {
            let registered = open_existing(&database)?
                .workplan(ProjectId::new(project)?, plan, revision)?
                .ok_or_else(|| invalid("registered workplan revision was not found"))?;
            print_json(serde_json::to_value(registered)?)
        }
        Some(WorkplanCommand::List {
            database,
            project,
            after,
            limit,
        }) => {
            let page =
                open_existing(&database)?.workplans(ProjectId::new(project)?, after, limit)?;
            print_json(serde_json::to_value(page)?)
        }
    }
}
