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
const MAX_STATUS_BYTES: u64 = 8 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;

fn read_bounded(path: &PathBuf, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|_| "legacy source is unavailable")?;
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

pub async fn snapshot(
    config: LegacyConfig,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let result = tokio::task::spawn_blocking(move || {
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
        Ok(value)
    })
    .await;
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
