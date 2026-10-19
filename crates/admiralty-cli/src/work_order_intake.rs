//! Offline, authenticated work-order intake against an exclusively owned Store.
//! This adapter uses the same configured admission bridge for capture and record
//! writer authorities as native startup; it never starts transport or tasks.
use admiralty_core::{
    ProjectId, RootId,
    intake::{DuplicateQuery, IntakeFormat, ScopeSnapshot, SourcePayload, SourceProvenance},
};
use admiralty_server::{
    intake_configuration::load_private_intake_configuration,
    services::work_order_intake::{
        PreviewIntakeInput, WorkOrderIntakeAdmissionBridge, WorkOrderIntakeService,
    },
    startup::read_private_token,
};
use admiralty_store::{Store, StoreAuthorities, intake_capture::CaptureIntakeRequest};
use clap::{Args, Subcommand};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

#[derive(Args)]
#[command(subcommand_required = true)]
pub struct IntakeArgs {
    #[command(subcommand)]
    command: IntakeCommand,
}

#[derive(Args, Clone)]
struct AuthorityArgs {
    /// Protected intake authorization and policy configuration.
    #[arg(long, value_parser = crate::bounded_absolute_path)]
    intake_config: PathBuf,
    /// Protected opaque intake credential file; credentials are never CLI values.
    #[arg(long, value_parser = crate::bounded_absolute_path)]
    token_file: PathBuf,
}

#[derive(Subcommand)]
enum IntakeCommand {
    /// Preview one bounded request under current configured scope and policy.
    Preview {
        /// Owner-only JSON: {project_id,root_id,format,provenance,source_payload}.
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        request: PathBuf,
        #[command(flatten)]
        authority: AuthorityArgs,
    },
    /// Persist one original raw request revision; creates no task or plan.
    Capture {
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        database: PathBuf,
        /// Owner-only CaptureIntakeRequest JSON, including exact ScopeSnapshot and policy digest.
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        request: PathBuf,
        #[command(flatten)]
        authority: AuthorityArgs,
    },
    /// Atomically record an explicitly enriched request and selected bindings.
    Record {
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        database: PathBuf,
        /// Owner-only RecordIntakeRequest JSON; raw plaintext requires explicit enrichment.
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        request: PathBuf,
        #[command(flatten)]
        authority: AuthorityArgs,
    },
    /// Read one captured revision using an authenticated scope-bound request.
    ShowCapture {
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        database: PathBuf,
        /// Owner-only JSON: {scope,capture_id,revision}; revision may be null for current.
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        request: PathBuf,
        #[command(flatten)]
        authority: AuthorityArgs,
    },
    /// Read one recorded work-order revision using an authenticated scope-bound request.
    Show {
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        database: PathBuf,
        /// Owner-only JSON: {scope,work_order_id,revision}; revision may be null for current.
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        request: PathBuf,
        #[command(flatten)]
        authority: AuthorityArgs,
    },
    /// Query the service's bounded all-state duplicate index.
    Duplicates {
        /// Owner-only JSON: {project_id,root_id,query}, matching the scoped service query.
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        database: PathBuf,
        #[arg(long, value_parser = crate::bounded_absolute_path)]
        request: PathBuf,
        #[command(flatten)]
        authority: AuthorityArgs,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewFile {
    project_id: ProjectId,
    root_id: RootId,
    format: IntakeFormat,
    provenance: SourceProvenance,
    source_payload: SourcePayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureReadFile {
    scope: ScopeSnapshot,
    capture_id: String,
    revision: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkOrderReadFile {
    scope: ScopeSnapshot,
    work_order_id: String,
    revision: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DuplicateFile {
    project_id: ProjectId,
    root_id: RootId,
    query: DuplicateQuery,
}

type CliResult<T> = crate::CliResult<T>;

fn decode<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> CliResult<T> {
    Ok(serde_json::from_slice(&crate::read_private_command_input(
        path,
    )?)?)
}

struct Runtime {
    service: WorkOrderIntakeService,
    actor: admiralty_server::services::work_order_intake::AuthenticatedIntakeActor,
}

fn runtime(database: Option<&std::path::Path>, authority: &AuthorityArgs) -> CliResult<Runtime> {
    let composition = load_private_intake_configuration(&authority.intake_config)?;
    let token = read_private_token(&authority.token_file)?;
    let actor = composition.registry.authenticate(&token)?;
    let bridge = Arc::new(WorkOrderIntakeAdmissionBridge::new(
        composition.registry.clone(),
        Some(composition.policy.clone()),
    ));
    let authorities = StoreAuthorities {
        intake: Some(bridge.clone() as Arc<dyn admiralty_store::intake::IntakeAuthority>),
        intake_capture: Some(
            bridge.clone() as Arc<dyn admiralty_store::intake_capture::IntakeCaptureAuthority>
        ),
        ..StoreAuthorities::default()
    };
    // Preview performs no Store access. A private in-memory Store is unnecessary;
    // use an explicit DB only for stateful and scoped-read operations.
    let database =
        database.ok_or_else(|| crate::invalid("this intake operation requires --database"))?;
    let store = Store::open_with_startup_authorities(database, authorities)?;
    let service = WorkOrderIntakeService::new(store, bridge);
    Ok(Runtime { service, actor })
}

fn preview(authority: &AuthorityArgs, path: &std::path::Path) -> CliResult<()> {
    let input: PreviewFile = decode(path)?;
    let composition = load_private_intake_configuration(&authority.intake_config)?;
    let token = read_private_token(&authority.token_file)?;
    let actor = composition.registry.authenticate(&token)?;
    let bridge = Arc::new(WorkOrderIntakeAdmissionBridge::new(
        composition.registry.clone(),
        Some(composition.policy.clone()),
    ));
    let scratch = tempfile::tempdir()?;
    let store = Store::open_with_startup_authorities(
        scratch.path().join("preview.sqlite"),
        StoreAuthorities {
            intake: Some(bridge.clone() as Arc<dyn admiralty_store::intake::IntakeAuthority>),
            intake_capture: Some(
                bridge.clone() as Arc<dyn admiralty_store::intake_capture::IntakeCaptureAuthority>
            ),
            ..StoreAuthorities::default()
        },
    )?;
    let service = WorkOrderIntakeService::new(store, bridge);
    let result = service.preview_raw(
        &actor,
        &PreviewIntakeInput {
            project_id: input.project_id,
            root_id: input.root_id,
            format: input.format,
            provenance: input.provenance,
            source_payload: input.source_payload,
        },
    )?;
    crate::print_json(serde_json::to_value(result)?)
}

fn scope_ids(scope: &ScopeSnapshot) -> CliResult<(ProjectId, RootId)> {
    let project = scope
        .project_id
        .clone()
        .ok_or_else(|| crate::invalid("request scope lacks project_id"))?;
    let root = scope
        .root_id
        .clone()
        .ok_or_else(|| crate::invalid("request scope lacks root_id"))?;
    Ok((project, root))
}

pub fn run(args: IntakeArgs) -> CliResult<()> {
    match args.command {
        IntakeCommand::Preview { request, authority } => preview(&authority, &request),
        IntakeCommand::Capture {
            database,
            request,
            authority,
        } => {
            let value: CaptureIntakeRequest = decode(&request)?;
            let runtime = runtime(Some(&database), &authority)?;
            let result = runtime.service.capture_raw(&runtime.actor, value)?;
            crate::print_json(serde_json::to_value(result)?)
        }
        IntakeCommand::Record {
            database,
            request,
            authority,
        } => {
            let value: admiralty_core::intake::RecordIntakeRequest = decode(&request)?;
            let runtime = runtime(Some(&database), &authority)?;
            let result = runtime.service.record_enriched(&runtime.actor, value)?;
            crate::print_json(serde_json::to_value(result)?)
        }
        IntakeCommand::ShowCapture {
            database,
            request,
            authority,
        } => {
            let input: CaptureReadFile = decode(&request)?;
            let (project, root) = scope_ids(&input.scope)?;
            let runtime = runtime(Some(&database), &authority)?;
            let result = runtime
                .service
                .get_captured_intake(
                    &runtime.actor,
                    &project,
                    &root,
                    &input.capture_id,
                    input.revision,
                )?
                .ok_or_else(|| crate::invalid("captured revision was not found"))?;
            if result.request.scope != input.scope {
                return Err(crate::invalid(
                    "requested scope differs from current captured scope",
                ));
            }
            crate::print_json(serde_json::to_value(result)?)
        }
        IntakeCommand::Show {
            database,
            request,
            authority,
        } => {
            let input: WorkOrderReadFile = decode(&request)?;
            let (project, root) = scope_ids(&input.scope)?;
            let runtime = runtime(Some(&database), &authority)?;
            let result = runtime
                .service
                .get_work_order(
                    &runtime.actor,
                    &project,
                    &root,
                    &input.work_order_id,
                    input.revision,
                )?
                .ok_or_else(|| crate::invalid("work-order revision was not found"))?;
            if result.work_order.scope != input.scope {
                return Err(crate::invalid(
                    "requested scope differs from current recorded scope",
                ));
            }
            crate::print_json(serde_json::to_value(result)?)
        }
        IntakeCommand::Duplicates {
            database,
            request,
            authority,
        } => {
            let input: DuplicateFile = decode(&request)?;
            if input.project_id != input.query.project_id {
                return Err(crate::invalid(
                    "duplicate query project differs from request project",
                ));
            }
            let runtime = runtime(Some(&database), &authority)?;
            let result = runtime.service.find_duplicate_candidates(
                &runtime.actor,
                &input.project_id,
                &input.root_id,
                input.query,
            )?;
            crate::print_json(serde_json::to_value(result)?)
        }
    }
}
