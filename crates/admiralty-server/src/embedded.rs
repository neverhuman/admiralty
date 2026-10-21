//! Dashboard assets compiled into the binary (see `build.rs`).
//!
//! A release built with `ADMIRALTY_EMBED_DASHBOARD=<abs dist>` carries its own
//! console, so one file is the whole deployment and a binary rollback always
//! restores the matching UI. An explicit `--assets` directory still wins: the
//! UI-only overlay procedure in `deploy/UI_ONLY.md` keeps working unchanged.
//!
//! Embedded responses are cheaper than directory reads: no filesystem access,
//! gzip variants precompressed at build time, and strong `ETag`s derived from
//! the content SHA-256 so browsers revalidate `index.html` with a 304.
use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

pub(crate) struct EmbeddedAsset {
    pub(crate) path: &'static str,
    pub(crate) mime: &'static str,
    pub(crate) sha256: &'static str,
    pub(crate) body: &'static [u8],
    pub(crate) gzip: Option<&'static [u8]>,
}

include!(concat!(env!("OUT_DIR"), "/embedded_dashboard.rs"));

/// Summary printed at startup and reported by `admiralty doctor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedDashboard {
    pub files: usize,
    pub bytes: u64,
    pub tree_sha256: String,
}

/// `Some` when this binary was built with an embedded dashboard.
pub fn embedded_dashboard() -> Option<EmbeddedDashboard> {
    EMBEDDED_TREE_SHA256.map(|tree| EmbeddedDashboard {
        files: EMBEDDED_ASSETS.len(),
        bytes: EMBEDDED_ASSETS.iter().map(|a| a.body.len() as u64).sum(),
        tree_sha256: tree.to_owned(),
    })
}

fn lookup(path: &str) -> Option<&'static EmbeddedAsset> {
    EMBEDDED_ASSETS
        .binary_search_by(|asset| asset.path.cmp(path))
        .ok()
        .map(|index| &EMBEDDED_ASSETS[index])
}

/// Vite names content-hashed files `name-XXXXXXXX.ext` (eight URL-safe chars).
pub(crate) fn is_hashed_asset(requested: &str) -> bool {
    let Some((stem, extension)) = requested.rsplit_once('.') else {
        return false;
    };
    let stem = stem.rsplit('/').next().unwrap_or(stem);
    let bytes = stem.as_bytes();
    requested.starts_with("assets/")
        && matches!(extension, "js" | "css" | "webp" | "woff2")
        && bytes.len() > 9
        && bytes[bytes.len() - 9] == b'-'
        && bytes[bytes.len() - 8..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

fn etag_matches(headers: &HeaderMap, etags: &[&str]) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|candidate| candidate.trim().trim_start_matches("W/"))
        .any(|candidate| candidate == "*" || etags.contains(&candidate))
}

/// Serve an embedded file, SPA routes (no dot) falling back to `index.html`.
/// Returns `None` when nothing is embedded or the path is unknown.
pub(crate) fn respond(requested: &str, headers: &HeaderMap, gzip_ok: bool) -> Option<Response> {
    let asset = lookup(requested).or_else(|| {
        (!requested.contains('.'))
            .then(|| lookup("index.html"))
            .flatten()
    })?;
    let identity_tag = format!("\"{}\"", &asset.sha256[..32]);
    let gzip_tag = format!("\"{}-gz\"", &asset.sha256[..32]);
    let use_gzip = gzip_ok && asset.gzip.is_some();
    let etag = if use_gzip { &gzip_tag } else { &identity_tag };
    let cache = if is_hashed_asset(requested) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = if etag_matches(headers, &[&identity_tag, &gzip_tag]) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let body = match (use_gzip, asset.gzip) {
            (true, Some(compressed)) => compressed,
            _ => asset.body,
        };
        let mut response = (
            [
                (header::CONTENT_TYPE, asset.mime),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            body,
        )
            .into_response();
        if use_gzip {
            response
                .headers_mut()
                .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        }
        response
    };
    let headers = response.headers_mut();
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_asset_rule_matches_vite_names_only() {
        assert!(is_hashed_asset("assets/index-AbC123_-.js"));
        assert!(is_hashed_asset("assets/admiralty-mark-abc_12--.webp"));
        assert!(is_hashed_asset(
            "assets/atkinson-hyperlegible-next-latin-wght-normal-BcXVPD7q.woff2"
        ));
        assert!(!is_hashed_asset("assets/index.js"));
        assert!(!is_hashed_asset("index-12345678.js"));
        assert!(!is_hashed_asset("assets/index-12345678.png"));
        assert!(!is_hashed_asset("assets/x-1234567.css"));
    }

    #[test]
    fn if_none_match_accepts_lists_weak_tags_and_wildcards() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"other\", W/\"abc\""),
        );
        assert!(etag_matches(&headers, &["\"abc\""]));
        assert!(!etag_matches(&headers, &["\"zzz\""]));
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(etag_matches(&headers, &["\"zzz\""]));
    }

    #[test]
    fn default_builds_embed_nothing_and_never_answer() {
        // Tests run without ADMIRALTY_EMBED_DASHBOARD, so behavior is unchanged.
        if EMBEDDED_ASSETS.is_empty() {
            assert!(embedded_dashboard().is_none());
            assert!(respond("index.html", &HeaderMap::new(), true).is_none());
        } else {
            let summary = embedded_dashboard().expect("non-empty table has a digest");
            assert_eq!(summary.files, EMBEDDED_ASSETS.len());
            assert!(lookup("index.html").is_some());
            let sorted = EMBEDDED_ASSETS.windows(2).all(|w| w[0].path < w[1].path);
            assert!(sorted, "binary search requires sorted paths");
        }
    }
}
