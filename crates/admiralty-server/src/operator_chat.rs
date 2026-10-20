//! Owner-authenticated instructions delivered to protected, explicit role mailboxes.
//! Delivery means durable inbox publication, never terminal input or execution.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::get,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;
type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, Json<Value>)>;
const MAX_FILES: usize = 2048;
const MAX_TEXT: usize = 8192;
const MAX_RECORD: u64 = 24 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub schema_version: u32,
    pub targets: Vec<Target>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub role: String,
    pub label: String,
    pub projects: Vec<String>,
    pub directory: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub id: String,
    pub project_id: String,
    pub target_id: String,
    pub kind: String,
    pub text: String,
    pub state: String,
    pub created_utc: String,
    pub detail: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub id: String,
    pub project_id: String,
    pub target_id: String,
    pub text: String,
    pub created_utc: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatRequest {
    id: String,
    project_id: String,
    target_id: String,
    kind: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    project_id: String,
}
#[derive(Clone)]
struct Service {
    token: String,
    config: Option<Configuration>,
    health: Arc<RwLock<Value>>,
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|u| u.to_string() == value)
}
fn text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_TEXT
        && !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Existing directories only. Configuration is never accepted over HTTP.
fn private_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("mailbox path must be absolute and normalized".into());
    }
    for ancestor in path.ancestors() {
        let m = fs::symlink_metadata(ancestor)?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err("mailbox directory must not traverse symlinks".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let uid = rustix::process::geteuid().as_raw();
            if !(m.uid() == uid || m.uid() == 0)
                || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
            {
                return Err("mailbox ancestor is not protected".into());
            }
            if ancestor == path && (m.uid() != uid || m.mode() & 0o077 != 0) {
                return Err("mailbox directory must be owner-only".into());
            }
        }
    }
    Ok(())
}
pub fn load(path: &Path) -> Result<Configuration> {
    let bytes = crate::startup::read_private_file(path, 64 * 1024)?;
    let config: Configuration =
        serde_json::from_slice(&bytes).map_err(|_| "invalid operator chat configuration")?;
    if config.schema_version != 1 || config.targets.is_empty() || config.targets.len() > 32 {
        return Err("operator chat configuration exceeds bounds".into());
    }
    let mut targets = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut routes = BTreeSet::new();
    for target in &config.targets {
        if !id(&target.id)
            || !targets.insert(&target.id)
            || !matches!(target.role.as_str(), "admiral" | "captain")
            || !text(&target.label)
            || target.label.len() > 200
            || target.label.chars().any(char::is_control)
            || target.projects.is_empty()
            || target.projects.len() > 64
            || !directories.insert(&target.directory)
        {
            return Err("invalid or duplicate chat target".into());
        }
        for project in &target.projects {
            if !id(project) || !routes.insert((project, &target.role)) {
                return Err("ambiguous chat project/role route".into());
            }
        }
        private_directory(&target.directory)?;
    }
    Ok(config)
}
fn lock(directory: &Path) -> Result<File> {
    private_directory(directory)?;
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags, open};
        let fd = open(
            directory.join(".chat.lock"),
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let file = File::from(fd);
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        if !m.is_file()
            || m.uid() != rustix::process::geteuid().as_raw()
            || m.mode() & 0o077 != 0
            || m.nlink() != 1
        {
            return Err("mailbox lock is not private".into());
        }
        file.try_lock_exclusive()?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        Err("operator mailboxes require qualified Unix permissions".into())
    }
}
fn files(directory: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for (i, entry) in fs::read_dir(directory)?.enumerate() {
        if i >= MAX_FILES {
            return Err(
                "mailbox inventory exceeds 2048 entries; archive acknowledged records".into(),
            );
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(key) = name
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(".json"))
            && uuid(key)
        {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&crate::startup::read_private_file(
        path, MAX_RECORD,
    )?)?)
}
fn publish(directory: &Path, path: &Path, value: &impl Serialize) -> Result<()> {
    publish_with_sync(directory, path, value, |directory| {
        File::open(directory)?.sync_all()?;
        Ok(())
    })
}
fn publish_with_sync(
    directory: &Path,
    path: &Path,
    value: &impl Serialize,
    sync: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err("mailbox record exceeds bounds".into());
    }
    // Called under the shared directory lock. A temporary is never consumed as a message.
    let temporary = directory.join(format!(".chat-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync(directory)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn send(config: &Configuration, request: ChatRequest, automatic: bool) -> Result<Message> {
    if !uuid(&request.id)
        || !id(&request.project_id)
        || !text(&request.text)
        || !(matches!(request.kind.as_str(), "message" | "command")
            || (automatic && request.kind == "git_health"))
    {
        return Err("invalid bounded chat instruction".into());
    }
    let target = config
        .targets
        .iter()
        .find(|t| t.id == request.target_id && t.projects.contains(&request.project_id))
        .ok_or("target is not configured for this project")?;
    let _lock = lock(&target.directory)?;
    let path = target.directory.join(format!("msg-{}.json", request.id));
    if path.try_exists()? {
        let existing: Message = read(&path)?;
        if existing.id == request.id
            && existing.project_id == request.project_id
            && existing.target_id == request.target_id
            && existing.kind == request.kind
            && existing.text == request.text
        {
            File::open(&target.directory)?.sync_all()?;
            return Ok(existing);
        }
        return Err("message id already belongs to different content".into());
    }
    if files(&target.directory, "msg-")?.len() >= 512 {
        return Err(
            "mailbox message capacity reached; preserve and archive acknowledged records".into(),
        );
    }
    let message = Message {
        id: request.id,
        project_id: request.project_id,
        target_id: request.target_id,
        kind: request.kind,
        text: request.text,
        state: "delivered".into(),
        created_utc: now(),
        detail: Some(
            "Delivered to target inbox; acknowledgment and execution are separate.".into(),
        ),
    };
    publish(&target.directory, &path, &message)?;
    Ok(message)
}
/// Agent-side inbox read. Records remain until an explicit operator archives them.
pub fn inbox(directory: &Path) -> Result<Vec<Message>> {
    let _lock = lock(directory)?;
    let mut messages = files(directory, "msg-")?
        .iter()
        .map(|p| read(p))
        .collect::<Result<Vec<Message>>>()?;
    messages.sort_by(|a, b| a.created_utc.cmp(&b.created_utc).then(a.id.cmp(&b.id)));
    Ok(messages)
}
/// A reply acknowledges a message without claiming its command was executed.
pub fn reply(directory: &Path, message_id: &str, content: &str) -> Result<Reply> {
    if !uuid(message_id) || !text(content) {
        return Err("reply needs a canonical message UUID and bounded text".into());
    }
    let _lock = lock(directory)?;
    let message: Message = read(&directory.join(format!("msg-{message_id}.json")))?;
    if message.id != message_id || !id(&message.project_id) || !id(&message.target_id) {
        return Err("message identity does not match inbox record".into());
    }
    let path = directory.join(format!("reply-{message_id}.json"));
    if path.try_exists()? {
        let existing: Reply = read(&path)?;
        if existing.text == content
            && existing.id == message_id
            && existing.project_id == message.project_id
            && existing.target_id == message.target_id
        {
            File::open(directory)?.sync_all()?;
            return Ok(existing);
        }
        return Err("reply is immutable; message already has a different reply".into());
    }
    files(directory, "reply-")?;
    let response = Reply {
        id: message.id,
        project_id: message.project_id,
        target_id: message.target_id,
        text: content.into(),
        created_utc: now(),
    };
    publish(directory, &path, &response)?;
    Ok(response)
}
fn failure(code: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({"error":message})))
}
fn auth(
    service: &Service,
    headers: &HeaderMap,
) -> std::result::Result<(), (StatusCode, Json<Value>)> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1
        || headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            != Some(service.token.as_str())
    {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "owner session access required",
        ));
    }
    Ok(())
}
async fn history(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Query(scope): Query<Scope>,
) -> ApiResult<Value> {
    auth(&service, &headers)?;
    if !id(&scope.project_id) {
        return Err(failure(StatusCode::BAD_REQUEST, "invalid project scope"));
    }
    tokio::task::spawn_blocking(move || {
        let mut targets = Vec::new(); let mut messages = Vec::new(); let mut responses = Vec::new();
        if let Some(config) = &service.config {
            for target in config.targets.iter().filter(|t| t.projects.contains(&scope.project_id)) {
                targets.push(json!({"id":target.id,"role":target.role,"label":target.label}));
                let _lock = lock(&target.directory)?;
                for path in files(&target.directory, "msg-")? { let record: Message = read(&path)?; if record.project_id == scope.project_id && record.target_id == target.id { messages.push(record); } }
                for path in files(&target.directory, "reply-")? { let record: Reply = read(&path)?; if record.project_id == scope.project_id && record.target_id == target.id { responses.push(record); } }
            }
        }
        messages.sort_by(|a,b| a.created_utc.cmp(&b.created_utc).then(a.id.cmp(&b.id)));
        responses.sort_by(|a,b| a.created_utc.cmp(&b.created_utc).then(a.id.cmp(&b.id)));
        let history_truncated = messages.len() > 100 || responses.len() > 100;
        messages.drain(..messages.len().saturating_sub(100));
        responses.drain(..responses.len().saturating_sub(100));
        let enabled = !targets.is_empty();
        Ok::<_, Error>(json!({"schema_version":1,"enabled":enabled,"simulated":false,"targets":targets,"messages":messages,"responses":responses,"history_truncated":history_truncated,"error":if enabled {Value::Null} else {json!("No mailbox route configured for this project. Start admiraltyd with --operator-chat-config and enroll the captain/admiral inbox.")}}))
    }).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE,"chat worker unavailable"))?
        .map(Json).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE,"chat history unavailable or exceeds bounds; records retained"))
}
async fn submit(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> ApiResult<Message> {
    auth(&service, &headers)?;
    let config = service.config.clone().ok_or_else(|| {
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "operator chat is not configured",
        )
    })?;
    tokio::task::spawn_blocking(move || send(&config, request, false))
        .await
        .map_err(|_| {
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "chat delivery outcome unknown; retry the same message id",
            )
        })?
        .map(Json)
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("different content") {
                failure(
                    StatusCode::CONFLICT,
                    "message id already belongs to different content",
                )
            } else if message.contains("invalid bounded") || message.contains("not configured") {
                failure(
                    StatusCode::BAD_REQUEST,
                    "invalid instruction or target/project route",
                )
            } else {
                failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "delivery outcome unknown; retry the same message id; records retained",
                )
            }
        })
}
async fn health(State(service): State<Arc<Service>>, headers: HeaderMap) -> ApiResult<Value> {
    auth(&service, &headers)?;
    let report = service
        .health
        .read()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "health cache unavailable"))?
        .clone();
    Ok(Json(report))
}
fn deterministic_id(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 15) | 0x50;
    bytes[8] = (bytes[8] & 63) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthDeliveryState {
    fingerprint: String,
    episode: u64,
    notified: bool,
}

fn notify_health(
    config: &Configuration,
    target: &Target,
    project: &str,
    report: &admiralty_core::git_health::GitHealthReport,
) -> Result<()> {
    let delivery_key = deterministic_id(&[project, &target.id]);
    let (episode, notified) = {
        let _lock = lock(&target.directory)?;
        let path = target.directory.join(format!("health-{delivery_key}.json"));
        let previous: Option<HealthDeliveryState> = if path.try_exists()? {
            Some(read(&path)?)
        } else {
            None
        };
        match previous {
            Some(prior) if prior.fingerprint == report.fingerprint => {
                // Repair the durability boundary after a prior uncertain rename.
                File::open(&target.directory)?.sync_all()?;
                (prior.episode, prior.notified)
            }
            prior => {
                let episode = prior
                    .map_or(Some(1), |p| p.episode.checked_add(1))
                    .ok_or("notification episode exhausted")?;
                publish(
                    &target.directory,
                    &path,
                    &HealthDeliveryState {
                        fingerprint: report.fingerprint.clone(),
                        episode,
                        notified: false,
                    },
                )?;
                (episode, false)
            }
        }
    };
    if !report.actionable || notified {
        return Ok(());
    }
    let content = format!(
        "Git health requires attention for {project}. Integration ref: {}. Findings: {}. Inspect Git Flow evidence before integrating; this alert grants no Git write authority.",
        report
            .integration_ref
            .as_deref()
            .unwrap_or("not configured"),
        report
            .issues
            .iter()
            .map(|i| format!(
                "{} [ref={}, checkout={}]: {}",
                i.code,
                i.ref_name.as_deref().unwrap_or("repository"),
                i.checkout_id.as_deref().unwrap_or("none"),
                i.detail
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    send(
        config,
        ChatRequest {
            id: deterministic_id(&[
                project,
                &target.id,
                &report.fingerprint,
                &episode.to_string(),
            ]),
            project_id: project.into(),
            target_id: target.id.clone(),
            kind: "git_health".into(),
            text: content.chars().take(2000).collect(),
        },
        true,
    )?;
    let _lock = lock(&target.directory)?;
    let path = target.directory.join(format!("health-{delivery_key}.json"));
    let mut state: HealthDeliveryState = read(&path)?;
    if state.episode == episode && state.fingerprint == report.fingerprint {
        state.notified = true;
        publish(&target.directory, &path, &state)?;
    }
    Ok(())
}

fn refresh(service: &Service, status_file: &Path) -> Result<()> {
    let raw = crate::legacy::read_bounded(&status_file.to_path_buf(), 8 * 1024 * 1024)?;
    let source: Value = serde_json::from_slice(&raw)?;
    let reports: BTreeMap<String, admiralty_core::git_health::GitHealthReport> =
        serde_json::from_value(
            source
                .get("git_health")
                .cloned()
                .unwrap_or_else(|| json!({})),
        )?;
    if reports.len() > 64 {
        return Err("health report inventory exceeds bounds".into());
    }
    let producer_fresh = source["generated_utc"]
        .as_str()
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .is_some_and(|stamp| {
            let age = (chrono::Utc::now() - stamp.with_timezone(&chrono::Utc)).num_seconds();
            (0..=90).contains(&age)
        });
    let mut notification_failed = false;
    if producer_fresh && let Some(config) = &service.config {
        for (project, report) in &reports {
            for target in config
                .targets
                .iter()
                .filter(|t| t.role == "captain" && t.projects.contains(project))
            {
                // Isolate destinations: one locked/full inbox must not starve others.
                if notify_health(config, target, project, report).is_err() {
                    notification_failed = true;
                }
            }
        }
    }
    let error = if !producer_fresh {
        json!(
            "Git health producer is stale or has no valid generation time; reports are retained evidence"
        )
    } else if notification_failed {
        json!("Some captain notifications could not be delivered; retry pending, reports retained")
    } else if reports.is_empty() {
        json!("Git health policy/reports not published by producer")
    } else {
        Value::Null
    };
    *service
        .health
        .write()
        .map_err(|_| "health cache unavailable")? = json!({"schema_version":1,"reports":reports,"producer_generated_utc":source["generated_utc"],"stale":!producer_fresh,"notification_delivery_failed":notification_failed,"error":error});
    Ok(())
}
/// One cache/notification loop per server, independent of browser presence.
pub fn router(
    token: String,
    config: Option<Configuration>,
    status_file: Option<PathBuf>,
) -> Router {
    let service = Arc::new(Service {
        token,
        config,
        health: Arc::new(RwLock::new(
            json!({"schema_version":1,"reports":{},"error":"Git health source is not configured"}),
        )),
    });
    if let Some(path) = status_file {
        let weak = Arc::downgrade(&service);
        tokio::spawn(async move {
            while let Some(service) = weak.upgrade() {
                let path = path.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if refresh(&service,&path).is_err() && let Ok(mut cache) = service.health.write() { cache["error"] = json!("Git health source or captain notification unavailable; retained reports may be stale"); cache["stale"] = json!(true); }
                }).await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }
    Router::new()
        .route("/v2/operator-chat", get(history).post(submit))
        .route("/v2/git/health", get(health))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
        .layer(DefaultBodyLimit::max(24 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            service.clone(),
            |State(service): State<Arc<Service>>,
             request: axum::extract::Request,
             next: axum::middleware::Next| async move {
                use axum::response::IntoResponse;
                let mut response = match auth(&service, request.headers()) {
                    Ok(()) => next.run(request).await,
                    Err(error) => error.into_response(),
                };
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
        .with_state(service)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Configuration) {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().expect("temp");
        fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).expect("mode");
        let path = d.path().canonicalize().expect("canonical");
        let c = Configuration {
            schema_version: 1,
            targets: vec![Target {
                id: "captain-dope".into(),
                role: "captain".into(),
                label: "DOPE Captain".into(),
                projects: vec!["dope".into()],
                directory: path,
            }],
        };
        (d, c)
    }
    fn request() -> ChatRequest {
        ChatRequest {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: "dope".into(),
            target_id: "captain-dope".into(),
            kind: "command".into(),
            text: "Inspect split branches; preserve dirty work".into(),
        }
    }
    #[test]
    fn exact_retry_scope_and_reply_survive_reopen() {
        let (_d, c) = fixture();
        let r = request();
        let first = send(&c, r.clone(), false).expect("deliver");
        assert_eq!(send(&c, r.clone(), false).expect("retry"), first);
        let mut conflict = r.clone();
        conflict.text = "different".into();
        assert!(send(&c, conflict, false).is_err());
        let mut foreign = r.clone();
        foreign.project_id = "other".into();
        assert!(send(&c, foreign, false).is_err());
        let directory = &c.targets[0].directory;
        assert_eq!(inbox(directory).expect("inbox").len(), 1);
        let response = reply(directory, &r.id, "Received; inspecting").expect("reply");
        assert_eq!(
            reply(directory, &r.id, "Received; inspecting").expect("retry"),
            response
        );
        assert!(reply(directory, &r.id, "replaced").is_err());
        assert_eq!(inbox(directory).expect("reopen")[0], first);
    }
    #[test]
    fn invalid_ids_controls_and_symlinks_fail_closed() {
        let (d, c) = fixture();
        let mut r = request();
        r.id = "../escape".into();
        assert!(send(&c, r, false).is_err());
        let mut r = request();
        r.text = "bad\rterminal".into();
        assert!(send(&c, r, false).is_err());
        let mut r = request();
        r.kind = "git_health".into();
        assert!(send(&c, r, false).is_err());
        let r = request();
        let outside = d.path().join("outside");
        fs::write(&outside, b"retained").expect("outside");
        std::os::unix::fs::symlink(
            &outside,
            c.targets[0].directory.join(format!("msg-{}.json", r.id)),
        )
        .expect("symlink");
        assert!(send(&c, r, false).is_err());
        assert_eq!(fs::read(outside).expect("retained"), b"retained");
    }
    #[test]
    fn notification_identity_is_stable_and_target_scoped() {
        assert_eq!(
            deterministic_id(&["p", "captain", "sha"]),
            deterministic_id(&["p", "captain", "sha"])
        );
        assert_ne!(
            deterministic_id(&["p", "captain", "sha"]),
            deterministic_id(&["p", "admiral", "sha"])
        );
    }
    #[test]
    fn fresh_failed_observation_alerts_once_and_stale_producer_does_not_alert() {
        let (d, c) = fixture();
        let directory = c.targets[0].directory.clone();
        let source_path = d.path().join("status.json");
        let repository = admiralty_core::views::GitRepository {
            instance_id: "repo".into(),
            node: "node".into(),
            observed_utc: "2020-01-01T00:00:00Z".into(),
            error: Some("scan failed".into()),
            shallow: false,
            truncated: false,
            commits: vec![],
            refs: vec![],
            checkouts: vec![],
        };
        let report = admiralty_core::git_health::evaluate(&repository, Some("refs/heads/main"), 20);
        let service = Service {
            token: "owner".into(),
            config: Some(c),
            health: Arc::new(RwLock::new(json!({}))),
        };
        fs::write(
            &source_path,
            serde_json::to_vec(
                &json!({"generated_utc":"2020-01-01T00:00:00Z","git_health":{"dope":report}}),
            )
            .expect("json"),
        )
        .expect("source");
        refresh(&service, &source_path).expect("stale refresh");
        assert!(inbox(&directory).expect("inbox").is_empty());
        assert_eq!(service.health.read().expect("cache")["stale"], true);
        assert!(service.health.read().expect("cache")["error"].is_string());
        fs::write(
            &source_path,
            serde_json::to_vec(&json!({"generated_utc":now(),"git_health":{"dope":report}}))
                .expect("json"),
        )
        .expect("source");
        refresh(&service, &source_path).expect("fresh scan failure");
        refresh(&service, &source_path).expect("retry");
        assert_eq!(inbox(&directory).expect("dedup").len(), 1);
        assert_eq!(inbox(&directory).expect("kind")[0].kind, "git_health");
        let mut recovered = report.clone();
        recovered.fingerprint = "recovered".into();
        recovered.actionable = false;
        recovered.state = admiralty_core::git_health::GitHealthState::Healthy;
        recovered.issues.clear();
        fs::write(
            &source_path,
            serde_json::to_vec(&json!({"generated_utc":now(),"git_health":{"dope":recovered}}))
                .expect("json"),
        )
        .expect("recovered source");
        refresh(&service, &source_path).expect("recovery");
        fs::write(
            &source_path,
            serde_json::to_vec(&json!({"generated_utc":now(),"git_health":{"dope":report}}))
                .expect("json"),
        )
        .expect("recurring source");
        refresh(&service, &source_path).expect("recurrence");
        assert_eq!(inbox(&directory).expect("new episode").len(), 2);
        refresh(&service, &source_path).expect("restart stable");
        assert_eq!(inbox(&directory).expect("dedup episode").len(), 2);
        let mut rotated = service.config.clone().expect("configured");
        rotated.targets[0].id = "new-captain-incarnation".into();
        notify_health(&rotated, &rotated.targets[0], "dope", &report)
            .expect("new target current alert");
        assert_eq!(inbox(&directory).expect("target rotation").len(), 3);
    }
    #[test]
    fn failed_destination_does_not_starve_other_captains() {
        let (d, mut c) = fixture();
        let good = c.targets[0].directory.clone();
        let mut bad = c.targets[0].clone();
        bad.id = "missing".into();
        bad.projects = vec!["aaa".into()];
        bad.directory = d.path().join("missing");
        c.targets.insert(0, bad);
        let repo = admiralty_core::views::GitRepository {
            instance_id: "repo".into(),
            node: "n".into(),
            observed_utc: now(),
            error: Some("failed".into()),
            shallow: false,
            truncated: false,
            commits: vec![],
            refs: vec![],
            checkouts: vec![],
        };
        let report = admiralty_core::git_health::evaluate(&repo, Some("refs/heads/main"), 20);
        let source = d.path().join("status.json");
        fs::write(
            &source,
            json!({"generated_utc":now(),"git_health":{"aaa":report,"dope":report}}).to_string(),
        )
        .expect("source");
        let service = Service {
            token: "t".into(),
            config: Some(c),
            health: Arc::new(RwLock::new(json!({}))),
        };
        refresh(&service, &source).expect("independent refresh");
        assert_eq!(inbox(&good).expect("good inbox").len(), 1);
        assert_eq!(
            service.health.read().expect("cache")["notification_delivery_failed"],
            true
        );
    }
    #[test]
    fn retry_repairs_uncertain_publish_and_rejects_copied_reply_identity() {
        let (_d, c) = fixture();
        let r = request();
        let message = Message {
            id: r.id.clone(),
            project_id: r.project_id.clone(),
            target_id: r.target_id.clone(),
            kind: r.kind.clone(),
            text: r.text.clone(),
            state: "delivered".into(),
            created_utc: now(),
            detail: None,
        };
        let dir = &c.targets[0].directory;
        let path = dir.join(format!("msg-{}.json", r.id));
        assert!(
            publish_with_sync(dir, &path, &message, |_| Err(
                "injected directory sync failure after rename".into()
            ))
            .is_err()
        );
        assert_eq!(
            send(&c, r.clone(), false).expect("repair durability"),
            message
        );
        let response = reply(dir, &r.id, "received").expect("reply");
        let mut copied = response.clone();
        copied.project_id = "other".into();
        let _lock = lock(dir).expect("lock");
        publish(dir, &dir.join(format!("reply-{}.json", r.id)), &copied).expect("fixture copy");
        drop(_lock);
        assert!(reply(dir, &r.id, "received").is_err());
    }
    async fn http(
        address: std::net::SocketAddr,
        method: &str,
        path: &str,
        token: &str,
        body: &str,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut out = String::new();
        stream.read_to_string(&mut out).await.expect("read");
        out
    }
    #[tokio::test]
    async fn actual_http_auth_delivery_retry_and_reply_are_scoped() {
        let (_d, c) = fixture();
        let directory = c.targets[0].directory.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let app = router("owner-secret".into(), Some(c), None);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        assert!(
            http(
                address,
                "GET",
                "/v2/operator-chat?project_id=dope",
                "wrong",
                ""
            )
            .await
            .starts_with("HTTP/1.1 401")
        );
        let denied = http(
            address,
            "POST",
            "/v2/operator-chat",
            "wrong",
            "malformed JSON",
        )
        .await;
        assert!(denied.starts_with("HTTP/1.1 401"), "{denied}");
        assert!(denied.contains("cache-control: no-store"));
        assert!(
            inbox(&directory)
                .expect("unauthorized has no effects")
                .is_empty()
        );
        let r = request();
        let body=json!({"id":r.id,"project_id":r.project_id,"target_id":r.target_id,"kind":r.kind,"text":r.text}).to_string();
        let first = http(address, "POST", "/v2/operator-chat", "owner-secret", &body).await;
        assert!(first.starts_with("HTTP/1.1 200"), "{first}");
        assert!(first.contains("cache-control: no-store"));
        assert_eq!(inbox(&directory).expect("inbox").len(), 1);
        assert!(
            http(address, "POST", "/v2/operator-chat", "owner-secret", &body)
                .await
                .starts_with("HTTP/1.1 200")
        );
        reply(&directory, &r.id, "Received; preserving work").expect("agent reply");
        let history = http(
            address,
            "GET",
            "/v2/operator-chat?project_id=dope",
            "owner-secret",
            "",
        )
        .await;
        assert!(history.contains("Received; preserving work"));
        let foreign = http(
            address,
            "GET",
            "/v2/operator-chat?project_id=other",
            "owner-secret",
            "",
        )
        .await;
        assert!(!foreign.contains("preserving"));
        assert!(foreign.contains("\"enabled\":false"));
        let conflict = body.replace("Inspect split branches; preserve dirty work", "other text");
        assert!(
            http(
                address,
                "POST",
                "/v2/operator-chat",
                "owner-secret",
                &conflict
            )
            .await
            .starts_with("HTTP/1.1 409")
        );
        server.abort();
    }
}
