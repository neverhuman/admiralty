//! Private startup configuration. Grants are read once from owner-only files;
//! HTTP callers cannot install authority or supply their own principal binding.
use crate::services::{
    agent_routes::{self, AgentRouteState},
    coordination::{
        CoordinationAction, CoordinationCredential, CoordinationCredentialAuthority,
        CoordinationCredentialGrant, MAX_COORDINATION_CREDENTIALS,
        MAX_COORDINATION_GRANTS_PER_CREDENTIAL,
    },
    coordination_routing::{
        AgentMessageRoute, InstalledRoute, MAX_NATIVE_ROUTES, NativeRoutingAuthority,
    },
    native_commands::{NativeCommands, OperatorLifecycleAuthority},
    work_order_intake::{
        WorkOrderIntakeAdmissionBridge, WorkOrderIntakeAuthRegistry, WorkOrderIntakePolicy,
        WorkOrderIntakeService,
    },
    work_order_routes::{ExactLoopbackOrigins, WorkOrderRouteState},
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, SourceIdentity};
use admiralty_store::{Store, StoreAuthorities, inbox::InboxScope};
use axum::Router;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::RwLock;

type Error = Box<dyn std::error::Error + Send + Sync>;
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeConfiguration {
    schema_version: u32,
    operator: Option<OperatorGrant>,
    #[serde(default)]
    agents: Vec<AgentCredential>,
    #[serde(default)]
    excursions: Vec<crate::services::improvement::excursions::ExcursionGrant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorGrant {
    principal: PrincipalId,
    projects: BTreeSet<ProjectId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentCredential {
    credential_id: String,
    token_file: PathBuf,
    actor: PrincipalId,
    grants: Vec<AgentGrant>,
    #[serde(default)]
    routes: Vec<AgentMessageRoute>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentGrant {
    scope: InboxScope,
    action: GrantAction,
    message_id: Option<MessageId>,
    source: Option<SourceIdentity>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrantAction {
    ReadPending,
    AcknowledgeApplication,
    AcknowledgePending,
}

pub struct NativeServices {
    pub store: Store,
    pub routes: Router,
}

/// Capabilities supplied by a trusted startup caller, independently of HTTP
/// identity claims and of the operator/inbox grants in the configuration file.
/// This type deliberately has no deserialization or credential fallback.
pub struct TrustedIntakeComposition {
    pub registry: Arc<WorkOrderIntakeAuthRegistry>,
    pub policy: Arc<dyn WorkOrderIntakePolicy>,
    pub origins: ExactLoopbackOrigins,
}

pub struct NativeServicesWithIntake {
    pub native: NativeServices,
    pub intake: Option<WorkOrderRouteState>,
}

/// No lifecycle execution or verifier authority is installed by this function.
/// The operator can submit work and cancel/revise unstarted work only.
pub fn open_native_services(
    database: &Path,
    config_file: &Path,
    owner_token: &str,
) -> Result<NativeServices, Error> {
    Ok(open_native_services_with_intake(database, config_file, owner_token, None)?.native)
}

/// Installs the SAME admission bridge in both raw-capture and enriched-record
/// writer policies before opening the Store, then constructs the service from
/// that Store and bridge. Existing lifecycle and inbox grants keep their scope.
/// No intake policy is inferred from the owner credential or config file.
pub fn open_native_services_with_intake(
    database: &Path,
    config_file: &Path,
    owner_token: &str,
    intake: Option<TrustedIntakeComposition>,
) -> Result<NativeServicesWithIntake, Error> {
    let bytes = read_private_file(config_file, MAX_CONFIG_BYTES)?;
    let config: NativeConfiguration =
        serde_json::from_slice(&bytes).map_err(|_| "native startup configuration is invalid")?;
    if config.schema_version != 1
        || config.agents.len() > MAX_COORDINATION_CREDENTIALS
        || config
            .operator
            .as_ref()
            .is_some_and(|grant| grant.projects.is_empty() || grant.projects.len() > 256)
        || (config.operator.is_none()
            && config.agents.is_empty()
            && intake.is_none()
            && config.excursions.is_empty())
    {
        return Err("native startup configuration exceeds admission bounds".into());
    }
    if intake
        .as_ref()
        .is_some_and(|composition| composition.registry.authenticate(owner_token).is_ok())
    {
        return Err("intake credentials must be distinct from other native credentials".into());
    }
    let mut credentials = Vec::with_capacity(config.agents.len());
    let mut message_routes = Vec::new();
    for agent in config.agents {
        if agent.grants.len() > MAX_COORDINATION_GRANTS_PER_CREDENTIAL
            || agent.routes.len() > MAX_COORDINATION_GRANTS_PER_CREDENTIAL
            || message_routes.len() + agent.routes.len() > MAX_NATIVE_ROUTES
        {
            return Err("native credential grants exceed admission bounds".into());
        }
        let token = read_private_token(&agent.token_file)?;
        // Agent read/ACK credentials must never be the owner command credential.
        if token == owner_token
            || intake
                .as_ref()
                .is_some_and(|composition| composition.registry.authenticate(&token).is_ok())
        {
            return Err("native credentials must be distinct across authority domains".into());
        }
        let grants = agent
            .grants
            .into_iter()
            .map(|grant| CoordinationCredentialGrant {
                scope: grant.scope,
                action: match grant.action {
                    GrantAction::ReadPending => CoordinationAction::ReadPending,
                    GrantAction::AcknowledgeApplication => {
                        CoordinationAction::AcknowledgeApplication
                    }
                    GrantAction::AcknowledgePending => CoordinationAction::AcknowledgePending,
                },
                message_id: grant.message_id,
                source: grant.source,
            })
            .collect();
        message_routes.extend(agent.routes.into_iter().map(|route| InstalledRoute {
            credential_id: agent.credential_id.clone(),
            route,
        }));
        credentials.push(CoordinationCredential::configured(
            agent.credential_id,
            token,
            agent.actor,
            grants,
        ));
    }
    let authority = Arc::new(RwLock::new(
        CoordinationCredentialAuthority::new(credentials)
            .map_err(|_| "native coordination credential configuration is invalid")?,
    ));
    let routing = Arc::new(NativeRoutingAuthority::new(
        authority.clone(),
        message_routes,
    )?);
    let lifecycle = config
        .operator
        .as_ref()
        .map(|operator| {
            OperatorLifecycleAuthority::new(operator.principal.clone(), operator.projects.clone())
                .map(|authority| {
                    Arc::new(authority) as Arc<dyn admiralty_store::LifecycleAuthority>
                })
        })
        .transpose()?;
    let bridge = intake.as_ref().map(|composition| {
        Arc::new(WorkOrderIntakeAdmissionBridge::new(
            composition.registry.clone(),
            Some(composition.policy.clone()),
        ))
    });
    let excursions = if config.excursions.is_empty() {
        None
    } else {
        Some(Arc::new(
            crate::services::improvement::excursions::ExcursionPolicy::new(
                owner_token,
                config.excursions,
            )?,
        ))
    };
    let store = Store::open_with_startup_authorities(
        database,
        StoreAuthorities {
            lifecycle,
            subscriptions: Some(routing.clone()),
            excursions: excursions
                .as_ref()
                .map(|p| p.clone() as Arc<dyn admiralty_store::excursions::ExcursionAuthority>),
            intake: bridge
                .as_ref()
                .map(|bridge| bridge.clone() as Arc<dyn admiralty_store::intake::IntakeAuthority>),
            intake_capture: bridge.as_ref().map(|bridge| {
                bridge.clone() as Arc<dyn admiralty_store::intake_capture::IntakeCaptureAuthority>
            }),
            ..StoreAuthorities::default()
        },
    )?;
    let intake_routes = match (intake, bridge) {
        (Some(composition), Some(bridge)) => Some(WorkOrderRouteState::new(
            Arc::new(WorkOrderIntakeService::new(store.clone(), bridge)),
            composition.registry,
            composition.origins,
        )?),
        _ => None,
    };
    let mut routes = Router::new().nest(
        "/v2/agents/inbox",
        agent_routes::router(AgentRouteState::with_routing(
            store.clone(),
            authority,
            routing,
        )),
    );
    if let Some(policy) = excursions {
        routes = routes.merge(crate::services::improvement::proposals::router(
            store.clone(),
            policy,
        ));
    }
    if let Some(operator) = config.operator {
        routes = routes.merge(
            NativeCommands::new(
                store.clone(),
                owner_token.to_owned(),
                operator.principal,
                operator.projects,
            )?
            .router(),
        );
    }
    Ok(NativeServicesWithIntake {
        native: NativeServices { store, routes },
        intake: intake_routes,
    })
}

/// Bounded, no-follow reads from an owner-only parent directory. Ancestors are
/// opened relative to retained directory descriptors, so checking a canonical
/// path cannot race a later traversal through a substituted symlink. Root/owner
/// controlled sticky temporary directories are permitted above the private root.
/// Errors contain neither secret bytes nor configuration paths. Files must have
/// one hard link. These checks detect ordinary concurrent mutation; they do not
/// create an isolation boundary against root or another process with this UID.
#[cfg(unix)]
pub(crate) fn read_private_file(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    use rustix::{
        fs::{Mode, OFlags, fstat, open, openat},
        process::geteuid,
    };
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err("private configuration path must be absolute and normalized".into());
    }
    let parent = path
        .parent()
        .ok_or("private configuration parent is missing")?;
    let name = path
        .file_name()
        .ok_or("private configuration filename is missing")?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let current_uid = geteuid().as_raw();
    let mut directory = open(Path::new("/"), flags, Mode::empty())
        .map_err(|_| "private configuration directory is unavailable")?;
    for component in parent.components() {
        let metadata =
            fstat(&directory).map_err(|_| "private configuration ancestor is unavailable")?;
        let trusted_owner = metadata.st_uid == 0 || metadata.st_uid == current_uid;
        if !trusted_owner || (metadata.st_mode & 0o022 != 0 && metadata.st_mode & 0o1000 == 0) {
            return Err("private configuration ancestor is not protected".into());
        }
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = openat(&directory, name, flags, Mode::empty())
                    .map_err(|_| "private configuration directory is unavailable")?;
            }
            _ => return Err("private configuration path must be normalized".into()),
        }
    }
    let metadata =
        fstat(&directory).map_err(|_| "private configuration directory is unavailable")?;
    if metadata.st_uid != current_uid || metadata.st_mode & 0o077 != 0 {
        return Err("private configuration directory must be owner-only".into());
    }
    let descriptor = openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "private configuration file is unavailable")?;
    let mut file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|_| "private configuration metadata is unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != current_uid
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > limit
    {
        return Err("private configuration must be a bounded owner-only regular file".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "private configuration read failed")?;
    if bytes.len() as u64 > limit {
        return Err("private configuration exceeds byte bound".into());
    }
    let after = file
        .metadata()
        .map_err(|_| "private configuration metadata is unavailable")?;
    if bytes.len() as u64 != metadata.len()
        || metadata.dev() != after.dev()
        || metadata.ino() != after.ino()
        || metadata.uid() != after.uid()
        || metadata.mode() != after.mode()
        || metadata.nlink() != after.nlink()
        || metadata.len() != after.len()
        || metadata.mtime() != after.mtime()
        || metadata.mtime_nsec() != after.mtime_nsec()
        || metadata.ctime() != after.ctime()
        || metadata.ctime_nsec() != after.ctime_nsec()
    {
        return Err("private configuration changed during read".into());
    }
    Ok(bytes)
}

#[cfg(not(unix))]
pub(crate) fn read_private_file(_: &Path, _: u64) -> Result<Vec<u8>, Error> {
    Err("private native startup is not qualified on this platform".into())
}

pub fn read_private_token(path: &Path) -> Result<String, Error> {
    let mut bytes = read_private_file(path, 4098)?;
    let result = std::str::from_utf8(&bytes)
        .ok()
        .map(str::trim)
        .filter(|value| {
            (32..=4096).contains(&value.len())
                && value.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
        })
        .map(str::to_owned)
        .ok_or_else(|| "private bearer credential is invalid".into());
    bytes.fill(0);
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn private_root() -> Result<PathBuf, Error> {
        let parent = std::env::temp_dir().canonicalize()?;
        let root = parent.join(format!("admiralty-startup-{}", uuid::Uuid::new_v4()));
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        Ok(root)
    }
    fn write_private(path: &Path, value: &[u8]) -> Result<(), Error> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(value)?;
        Ok(())
    }

    #[test]
    fn rejects_symlinks_public_modes_and_oversize_without_secret_errors() -> Result<(), Error> {
        let root = private_root()?;
        let token = root.join("token");
        write_private(&token, b"01234567890123456789012345678901")?;
        assert_eq!(read_private_token(&token)?.len(), 32);
        let link = root.join("link");
        symlink(&token, &link)?;
        assert!(read_private_token(&link).is_err());
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644))?;
        assert!(read_private_token(&token).is_err());
        write_private(&root.join("large"), &vec![b'x'; 4099])?;
        assert!(read_private_token(&root.join("large")).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn configured_startup_installs_routes_with_operator_authority() -> Result<(), Error> {
        let root = private_root()?;
        let config = root.join("native.json");
        write_private(&config, br#"{"schema_version":1,"operator":{"principal":"owner","projects":["admiralty"]},"agents":[]}"#)?;
        let services = open_native_services(
            &root.join("native.sqlite"),
            &config,
            "01234567890123456789012345678901",
        )?;
        assert!(services.store.snapshot()?.projection.tasks.is_empty());
        let router = crate::app_with_native_routes(
            services.store.clone(),
            crate::ServerConfig {
                listen: "127.0.0.1:0".parse()?,
                simulated: false,
                token: Some("01234567890123456789012345678901".into()),
                assets: None,
                legacy: None,
                models_file: None,
            },
            Some(services.routes),
        )?;
        drop(router);
        drop(services.store);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
