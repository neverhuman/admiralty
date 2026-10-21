//! Lossless, negotiated compression for bounded read-only dashboard responses.
use axum::{
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use flate2::{Compression, write::GzEncoder};
use std::io::{self, Write};

pub(crate) fn timed(
    mut response: Response,
    metric: &str,
    elapsed: std::time::Duration,
) -> Response {
    response.headers_mut().insert(
        "server-timing",
        format!("{metric};dur={:.3}", elapsed.as_secs_f64() * 1000.0)
            .parse()
            .expect("internal timing metric"),
    );
    response
}

pub(crate) fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut explicit = None;
    let mut wildcard = false;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for entry in value.split(',') {
            let mut parts = entry.split(';');
            let coding = parts.next().unwrap_or("").trim();
            let mut quality = 1.0_f32;
            for part in parts {
                let Some((key, value)) = part.trim().split_once('=') else {
                    quality = 0.0;
                    break;
                };
                if !key.trim().eq_ignore_ascii_case("q") {
                    quality = 0.0;
                    break;
                }
                quality = value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|q| q.is_finite() && (0.0..=1.0).contains(q))
                    .unwrap_or(0.0);
            }
            if coding.eq_ignore_ascii_case("gzip") {
                // An explicit refusal wins over a wildcard or duplicate entry.
                explicit = Some(explicit.unwrap_or(true) && quality > 0.0);
            } else if coding == "*" {
                wildcard |= quality > 0.0;
            }
        }
    }
    explicit.unwrap_or(wildcard)
}

/// Negotiated transport encoding of one body. `gzip` is true only when the
/// compressed form is actually smaller; identity clients always get the source.
pub(crate) fn encode(mime: &str, bytes: Vec<u8>, gzip: bool) -> io::Result<(Vec<u8>, bool)> {
    let compressible =
        mime.starts_with("text/") || matches!(mime, "application/json" | "image/svg+xml");
    if gzip && compressible && bytes.len() >= 1024 {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&bytes)?;
        let compressed = encoder.finish()?;
        if compressed.len() < bytes.len() {
            return Ok((compressed, true));
        }
    }
    Ok((bytes, false))
}

/// Build the response for an already encoded body. Headers are identical to
/// [`response`]; a cached body therefore cannot change the wire contract.
pub(crate) fn encoded_response(
    mime: &'static str,
    body: impl Into<axum::body::Body>,
    is_gzip: bool,
) -> Response {
    let mut response = (
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::VARY, "Accept-Encoding"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.into(),
    )
        .into_response();
    if is_gzip {
        response.headers_mut().insert(
            header::CONTENT_ENCODING,
            "gzip".parse().expect("static encoding"),
        );
    }
    response
}

// Call inside the existing blocking read task, under the existing byte limits.
pub(crate) fn response(mime: &'static str, bytes: Vec<u8>, gzip: bool) -> io::Result<Response> {
    let (body, is_gzip) = encode(mime, bytes, gzip)?;
    Ok(encoded_response(mime, body, is_gzip))
}

/// Compare secrets without an early exit on the first differing byte, so
/// response timing does not reveal how much of a bearer token matched.
pub(crate) fn constant_time_eq(expected: &[u8], candidate: &[u8]) -> bool {
    let mut difference = expected.len() ^ candidate.len();
    for index in 0..expected.len().max(candidate.len()) {
        difference |= usize::from(
            expected.get(index).copied().unwrap_or(0) ^ candidate.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

/// True when any entity tag in `If-None-Match` equals `etag` (or is `*`).
pub(crate) fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|tag| tag.trim().trim_start_matches("W/"))
        .any(|tag| tag == "*" || tag == etag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn constant_time_comparison_is_exact() {
        assert!(constant_time_eq(b"token-value", b"token-value"));
        assert!(!constant_time_eq(b"token-value", b"token-valuX"));
        assert!(!constant_time_eq(b"token-value", b"token-value-longer"));
        assert!(!constant_time_eq(b"token-value", b""));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn entity_tags_match_lists_weak_prefix_and_wildcard_only() {
        let tag = "\"abc-gz\"";
        for (header, expected) in [
            ("\"abc-gz\"", true),
            ("\"x\", \"abc-gz\"", true),
            ("W/\"abc-gz\"", true),
            ("*", true),
            ("\"abc\"", false),
            ("abc-gz", false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, header.parse().expect("header"));
            assert_eq!(not_modified(&headers, tag), expected, "{header}");
        }
        assert!(!not_modified(&HeaderMap::new(), tag));
    }

    #[test]
    fn negotiation_honors_explicit_refusal_and_invalid_quality() {
        for (value, expected) in [
            ("gzip, br", true),
            ("GZip;q=0.5", true),
            ("gzip;q=0, *;q=1", false),
            ("gzip;q=0.000", false),
            ("br", false),
            ("*;q=0.5", true),
            ("gzip;q=NaN", false),
            ("gzip;q=2", false),
            ("gzip;q=0, gzip;q=1", false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, value.parse().expect("header"));
            assert_eq!(accepts_gzip(&headers), expected, "{value}");
        }
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[tokio::test]
    async fn compression_is_lossless_and_identity_clients_remain_compatible() {
        let bytes = "Unicode fleet snapshot 🧭 第二段\n"
            .repeat(10_000)
            .into_bytes();
        let compressed = response("application/json", bytes.clone(), true).expect("response");
        assert_eq!(compressed.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(compressed.headers()[header::VARY], "Accept-Encoding");
        assert_eq!(compressed.headers()[header::CACHE_CONTROL], "no-store");
        let wire = axum::body::to_bytes(compressed.into_body(), bytes.len())
            .await
            .expect("bounded body");
        assert!(wire.len() < bytes.len() / 10);
        let mut restored = Vec::new();
        flate2::read::GzDecoder::new(&wire[..])
            .read_to_end(&mut restored)
            .expect("decode gzip");
        assert_eq!(restored, bytes);
        let identity = response("application/json", bytes.clone(), false).expect("response");
        assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(
            axum::body::to_bytes(identity.into_body(), bytes.len())
                .await
                .expect("identity body")
                .as_ref(),
            bytes
        );
    }
}
