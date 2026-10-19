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

// Call inside the existing blocking read task, under the existing byte limits.
pub(crate) fn response(mime: &'static str, bytes: Vec<u8>, gzip: bool) -> io::Result<Response> {
    let compressible =
        mime.starts_with("text/") || matches!(mime, "application/json" | "image/svg+xml");
    let encoded = if gzip && compressible && bytes.len() >= 1024 {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&bytes)?;
        let compressed = encoder.finish()?;
        (compressed.len() < bytes.len()).then_some(compressed)
    } else {
        None
    };
    let is_gzip = encoded.is_some();
    let mut response = (
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::VARY, "Accept-Encoding"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        encoded.unwrap_or(bytes),
    )
        .into_response();
    if is_gzip {
        response.headers_mut().insert(
            header::CONTENT_ENCODING,
            "gzip".parse().expect("static encoding"),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

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
