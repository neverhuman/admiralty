//! Explicit read-only bridge to an existing dashboard's already collected facts.
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug)]
pub struct LegacyConfig {
    pub status_file: PathBuf,
    pub index_file: PathBuf,
    /// Explicitly preserve the existing loopback dashboard's public read scope.
    /// This flag never bypasses native task or model mutation authentication.
    pub public_read: bool,
}
/// Same bound as the native observer's atomic `status.json` writer. A smaller
/// reader bound made every legacy route fail once a valid producer snapshot
/// passed 8 MiB.
pub(crate) const MAX_STATUS_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) fn read_bounded(path: &PathBuf, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Refuse non-regular producers without blocking on a FIFO open first.
        // Regular-file reads and existing source symlinks retain their behavior.
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC).bits() as i32,
        );
    }
    let mut file = options
        .open(path)
        .map_err(|_| "legacy source is unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "legacy source metadata unavailable")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("legacy source exceeds its bounded regular file limit".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "legacy source read failed")?;
    if bytes.len() as u64 > limit {
        return Err("legacy source exceeds its byte limit".into());
    }
    Ok(bytes)
}

fn read_snapshot(config: &LegacyConfig) -> Result<(serde_json::Value, Vec<u8>), String> {
    let bytes = read_bounded(&config.status_file, MAX_STATUS_BYTES)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "legacy source is not valid JSON")?;
    if !value.is_object()
        || !value["projects"].is_object()
        || !value["order"].is_array()
        || !value["generated_utc"].is_string()
    {
        return Err("legacy snapshot is missing required source fields".to_string());
    }
    Ok((value, bytes))
}

pub async fn snapshot(
    config: LegacyConfig,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let result =
        tokio::task::spawn_blocking(move || read_snapshot(&config).map(|(value, _)| value)).await;
    match result {
        Ok(Ok(value)) => Ok(Json(value)),
        _ => Err((
            StatusCode::BAD_GATEWAY,
            Json(
                serde_json::json!({"error":"Legacy snapshot unavailable, malformed or outside bounds; source facts were not fabricated."}),
            ),
        )),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SnapshotView {
    Full,
    Detail,
    Initial,
    Brief,
}

fn detail_projection(mut value: serde_json::Value, initial: bool) -> serde_json::Value {
    let root = value.as_object_mut().expect("validated snapshot object");
    // Git Flow retains its independent full source; the detail UI never uses it.
    root.remove("git");
    root.insert(
        "admiralty_delivery".into(),
        if initial { "initial" } else { "complete" }.into(),
    );
    if !initial {
        return value;
    }
    root.remove("history");
    root.remove("remote_screens");
    if let Some(projects) = root.get_mut("projects").and_then(|v| v.as_object_mut()) {
        for project in projects.values_mut().filter_map(|v| v.as_object_mut()) {
            project.retain(|key, _| {
                matches!(
                    key.as_str(),
                    "name"
                        | "ledger_source"
                        | "ledger_updated_utc"
                        | "observed_utc"
                        | "total"
                        | "stages"
                        | "counts"
                        | "stage_labels"
                        | "done"
                        | "ip"
                        | "in_progress"
                        | "in_pr"
                        | "blocked"
                        | "ready"
                        | "stalled"
                        | "flags"
                        | "blockers"
                        | "needs_jepson"
                        | "blocked_detail"
                        | "recent"
                        | "captain"
                        | "workers"
                        | "pr"
                )
            });
        }
    }
    if let Some(packets) = root
        .get_mut("rollout")
        .and_then(|v| v.get_mut("packets"))
        .and_then(|v| v.as_array_mut())
    {
        for packet in packets.iter_mut().filter_map(|v| v.as_object_mut()) {
            packet
                .retain(|key, _| matches!(key.as_str(), "id" | "title" | "status" | "provenance"));
            if let Some(provenance) = packet.get_mut("provenance").and_then(|v| v.as_object_mut()) {
                provenance.retain(|key, _| key == "warnings");
            }
        }
    }
    value
}

/// File identity of the bounded status source. The observer publishes with an
/// atomic rename, so a new snapshot is always a new inode or modification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64, i64, i64),
}

impl SourceIdentity {
    pub(crate) fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            inode: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        }
    }
}

/// Identity of the configured source without reading its contents.
pub(crate) fn source_identity(path: &std::path::Path) -> Option<SourceIdentity> {
    fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| SourceIdentity::of(&m))
}

/// One encoded projection of one source revision.
#[derive(Debug)]
pub(crate) struct EncodedSnapshot {
    identity: SourceIdentity,
    pub(crate) etag: String,
    pub(crate) body: axum::body::Bytes,
    pub(crate) gzip: bool,
    pub(crate) cache_hit: bool,
}

type CacheKey = (PathBuf, u8, bool);

/// Projection cache shared by every viewer. Each key keeps only the latest
/// source revision, so memory is bounded by (views × encodings) per source.
/// Every request still opens and stats the source; only unchanged files skip
/// the parse, projection, serialization and compression work.
fn cache()
-> &'static std::sync::Mutex<std::collections::HashMap<CacheKey, std::sync::Arc<EncodedSnapshot>>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<CacheKey, std::sync::Arc<EncodedSnapshot>>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn view_key(view: SnapshotView) -> u8 {
    match view {
        SnapshotView::Full => 0,
        SnapshotView::Detail => 1,
        SnapshotView::Initial => 2,
        SnapshotView::Brief => 3,
    }
}

fn encode_snapshot(
    config: &LegacyConfig,
    gzip: bool,
    view: SnapshotView,
) -> Result<std::sync::Arc<EncodedSnapshot>, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC).bits() as i32,
        );
    }
    let mut file = options
        .open(&config.status_file)
        .map_err(|_| "legacy source is unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "legacy source metadata unavailable")?;
    if !metadata.is_file() || metadata.len() > MAX_STATUS_BYTES {
        return Err("legacy source exceeds its bounded regular file limit".into());
    }
    let identity = SourceIdentity::of(&metadata);
    let key = (config.status_file.clone(), view_key(view), gzip);
    let cacheable = !matches!(view, SnapshotView::Brief);
    if cacheable
        && let Ok(cache) = cache().lock()
        && let Some(hit) = cache.get(&key).filter(|hit| hit.identity == identity)
    {
        return Ok(std::sync::Arc::new(EncodedSnapshot {
            identity: hit.identity.clone(),
            etag: hit.etag.clone(),
            body: hit.body.clone(),
            gzip: hit.gzip,
            cache_hit: true,
        }));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_STATUS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "legacy source read failed")?;
    if bytes.len() as u64 > MAX_STATUS_BYTES {
        return Err("legacy source exceeds its byte limit".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "legacy source is not valid JSON")?;
    if !value.is_object()
        || !value["projects"].is_object()
        || !value["order"].is_array()
        || !value["generated_utc"].is_string()
    {
        return Err("legacy snapshot is missing required source fields".to_string());
    }
    let bytes = match view {
        SnapshotView::Brief => crate::fleet_brief::compile_value(
            &value,
            &bytes,
            chrono::Utc::now(),
            crate::fleet_brief::MAX_BRIEF_BYTES,
        )
        .map_err(str::to_owned)?,
        SnapshotView::Full => {
            drop(value);
            bytes
        }
        SnapshotView::Detail | SnapshotView::Initial => {
            drop(bytes);
            serde_json::to_vec(&detail_projection(
                value,
                matches!(view, SnapshotView::Initial),
            ))
            .map_err(|_| "legacy projection encoding failed".to_owned())?
        }
    };
    let digest = Sha256::digest(&bytes);
    let hex = digest
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    // Preserve the source bytes and validation bounds; compression changes
    // transport only, never trims history or manufactures fresher data.
    let (body, is_gzip) = crate::http_bytes::encode("application/json", bytes, gzip)
        .map_err(|_| "legacy response encoding failed".to_owned())?;
    // Strong tags must differ per content-coding of the same projection.
    let etag = format!("\"{hex}{}\"", if is_gzip { "-gz" } else { "" });
    let encoded = std::sync::Arc::new(EncodedSnapshot {
        identity,
        etag,
        body: axum::body::Bytes::from(body),
        gzip: is_gzip,
        cache_hit: false,
    });
    if cacheable && let Ok(mut cache) = cache().lock() {
        cache.insert(key, encoded.clone());
    }
    Ok(encoded)
}

pub(crate) async fn snapshot_encoded(
    config: LegacyConfig,
    gzip: bool,
    view: SnapshotView,
) -> Result<std::sync::Arc<EncodedSnapshot>, (StatusCode, Json<serde_json::Value>)> {
    let result = tokio::task::spawn_blocking(move || encode_snapshot(&config, gzip, view)).await;
    match result {
        Ok(Ok(encoded)) => Ok(encoded),
        _ => Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error":"Legacy snapshot unavailable, malformed or outside bounds; source facts were not fabricated."
            })),
        )),
    }
}

/// Parsed snapshot for server-side summaries such as the agent brief.
pub(crate) async fn snapshot_value(
    config: LegacyConfig,
) -> Result<serde_json::Value, (StatusCode, Json<serde_json::Value>)> {
    match tokio::task::spawn_blocking(move || read_snapshot(&config).map(|(value, _)| value)).await
    {
        Ok(Ok(value)) => Ok(value),
        _ => Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error":"Legacy snapshot unavailable, malformed or outside bounds; source facts were not fabricated."
            })),
        )),
    }
}

pub async fn index(config: LegacyConfig) -> Response {
    match tokio::task::spawn_blocking(move || read_bounded(&config.index_file, MAX_INDEX_BYTES))
        .await
    {
        Ok(Ok(bytes)) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        _ => (
            StatusCode::BAD_GATEWAY,
            "Legacy detailed view is unavailable",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn fifo_source_is_refused_without_waiting_for_a_writer() {
        let root = tempfile::tempdir().expect("fixture");
        let fifo = root.path().join("producer");
        assert!(
            std::process::Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        assert!(read_bounded(&fifo, 1024).is_err());
        let config = LegacyConfig {
            status_file: fifo,
            index_file: root.path().join("index.html"),
            public_read: false,
        };
        assert!(encode_snapshot(&config, false, SnapshotView::Brief).is_err());
    }

    #[test]
    fn projected_source_decimals_round_trip_without_changing_float_bits() {
        // These source literals lost one ULP under serde_json's fast parser.
        // Compare with Rust's correctly rounded parser, not a second JSON parse.
        for literal in [
            "10.311165210389579",
            "97.30287417226819",
            "125.57848358154297",
            "1.2368568906011719",
            "96.42857142857143",
            "-0.0",
            "1e-308",
        ] {
            let raw = format!(
                r#"{{"generated_utc":"2026-10-19T22:26:07Z","projects":{{}},"global_resources":{{"observed":{literal}}}}}"#
            );
            let source: serde_json::Value = serde_json::from_str(&raw).expect("valid source");
            let expected = literal.parse::<f64>().expect("source float").to_bits();
            for initial in [true, false] {
                let projected = detail_projection(source.clone(), initial);
                assert_eq!(
                    projected["global_resources"]["observed"]
                        .as_f64()
                        .expect("numeric observation")
                        .to_bits(),
                    expected,
                    "source literal {literal}, initial={initial}"
                );
                let encoded = serde_json::to_vec(&projected).expect("encoded projection");
                let decoded: serde_json::Value =
                    serde_json::from_slice(&encoded).expect("decoded projection");
                assert_eq!(
                    decoded["global_resources"]["observed"]
                        .as_f64()
                        .expect("decoded observation")
                        .to_bits(),
                    expected,
                    "serialized literal {literal}, initial={initial}"
                );
            }
        }
    }

    #[test]
    fn initial_projection_keeps_current_facts_warnings_unknowns_and_source_age() {
        let source = json!({"generated_utc":"2026-10-19T12:00:00Z","order":["fleet"],
            "projects":{"fleet":{"name":"Fleet","total":null,"stages":{"verified":4,"done":1},
                "flags":["source warning"],"captain":{"model":null},"workers":[],"coord_log":{"tail":["full evidence"]}}},
            "rollout":{"counts":{"verified":4},"packets":[{"id":"P02","status":"verified","contract":"full contract",
                "provenance":{"warnings":["not accepted"],"source_blob":"full source"}}]},
            "history":{"rows":[{"observed":"history fact"}]},"git":{"fleet":{"commits":["git fact"]}},
            "collector":{"implementation":"rust","model_turns":0},"token_efficiency":{"quota":{"used_percent":null}}});
        let initial = detail_projection(source.clone(), true);
        assert_eq!(initial["admiralty_delivery"], "initial");
        assert_eq!(initial["generated_utc"], source["generated_utc"]);
        assert_eq!(
            initial["projects"]["fleet"]["stages"],
            source["projects"]["fleet"]["stages"]
        );
        assert_eq!(
            initial["projects"]["fleet"]["total"],
            serde_json::Value::Null
        );
        assert_eq!(
            initial["projects"]["fleet"]["flags"],
            source["projects"]["fleet"]["flags"]
        );
        assert_eq!(
            initial["rollout"]["packets"][0]["provenance"]["warnings"],
            json!(["not accepted"])
        );
        assert!(initial.get("history").is_none() && initial.get("git").is_none());
        assert!(initial["projects"]["fleet"].get("coord_log").is_none());
        assert!(initial["rollout"]["packets"][0].get("contract").is_none());
        let detail = detail_projection(source.clone(), false);
        assert_eq!(detail["history"], source["history"]);
        assert_eq!(detail["projects"], source["projects"]);
        assert_eq!(detail["rollout"], source["rollout"]);
        assert_eq!(detail["generated_utc"], source["generated_utc"]);
        assert!(detail.get("git").is_none());
        assert!(
            source.get("git").is_some(),
            "original full source remains independent"
        );
    }
}
