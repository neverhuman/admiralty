//! Real loopback HTTP checks for request identity, readiness, Prometheus
//! metrics and the agent log digest.
use admiralty_server::{ServerConfig, app, telemetry};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

const TOKEN: &str = "telemetry-test-token-0123456789abcdef";

fn request(address: SocketAddr, path: &str, headers: &[(&str, &str)]) -> (u16, String, String) {
    let mut socket =
        TcpStream::connect_timeout(&address, Duration::from_secs(3)).expect("connect loopback");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut text = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    socket.write_all(text.as_bytes()).expect("write request");
    let mut response = Vec::new();
    socket
        .take(4 * 1024 * 1024)
        .read_to_end(&mut response)
        .expect("read response");
    let response = String::from_utf8_lossy(&response).into_owned();
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status code");
    (status, head.to_ascii_lowercase(), body.to_owned())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_ids_readiness_metrics_and_digest_work_over_loopback() {
    // Owner-only regardless of the runner's umask.
    let directory = {
        let mut builder = tempfile::Builder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        builder.tempdir().expect("tempdir")
    };
    let store = admiralty_store::Store::open(directory.path().join("state.sqlite")).expect("store");
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("listen"),
        simulated: false,
        token: Some(TOKEN.into()),
        assets: None,
        legacy: None,
        models_file: None,
    };
    let router = app(store, config)
        .expect("app")
        .layer(axum::middleware::from_fn(telemetry::observe));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let checks = tokio::task::spawn_blocking(move || {
        let bearer = format!("Bearer {TOKEN}");
        let auth = [("Authorization", bearer.as_str())];

        let (status, head, _) = request(address, "/healthz", &[]);
        assert_eq!(status, 200);
        let generated = header(&head, telemetry::REQUEST_ID).expect("generated id");
        assert!(telemetry::valid_request_id(generated), "{generated}");

        let (_, head, _) = request(address, "/healthz", &[("X-Request-Id", "caller-abc.1")]);
        assert_eq!(header(&head, telemetry::REQUEST_ID), Some("caller-abc.1"));
        let (_, head, _) = request(address, "/healthz", &[("X-Request-Id", "bad id=1")]);
        assert_ne!(header(&head, telemetry::REQUEST_ID), Some("bad id=1"));

        let (status, _, body) = request(address, "/readyz", &[]);
        assert_eq!(status, 200, "{body}");
        let ready: serde_json::Value = serde_json::from_str(&body).expect("ready json");
        assert_eq!(ready["status"], "ready");
        assert_eq!(ready["checks"]["store"], "ok");
        assert_eq!(ready["native_execution_qualified"], false);

        let (status, _, _) = request(address, "/v2/not-a-route", &auth);
        assert_eq!(status, 404);
        let (status, _, _) = request(address, "/metrics", &[]);
        assert_eq!(status, 401, "metrics are private outside the demo");
        let (status, head, metrics) = request(address, "/metrics", &auth);
        assert_eq!(status, 200);
        assert!(head.contains("content-type: text/plain; version=0.0.4"));
        for expected in [
            "admiralty_http_requests_total{route=\"/healthz\",class=\"2xx\"}",
            "admiralty_http_requests_total{route=\"/v2/{*path}\",class=\"4xx\"}",
            "admiralty_http_requests_total{route=\"/metrics\",class=\"4xx\"}",
            "admiralty_http_request_duration_seconds_bucket{route=\"/readyz\",le=\"+Inf\"}",
            "admiralty_event_streams_limit 16",
        ] {
            assert!(metrics.contains(expected), "missing {expected}:\n{metrics}");
        }

        let (status, _, _) = request(address, "/v1/logs/digest", &[]);
        assert_eq!(status, 401);
        let (status, _, _) = request(address, "/v1/logs/digest?min_level=loud", &auth);
        assert_eq!(status, 400);
        let (status, head, digest) = request(
            address,
            "/v1/logs/digest?budget_tokens=200&min_level=info",
            &auth,
        );
        assert_eq!(status, 200);
        assert!(head.contains("cache-control: no-store"));
        assert!(digest.starts_with("ADMIRALTY-LOG/1 "), "{digest}");
        assert!(digest.contains("http.denied"), "{digest}");
        assert!(
            !digest.contains("http.client_error"),
            "routine 4xx stay below the default level: {digest}"
        );
        assert!(!digest.contains(TOKEN), "digest never echoes credentials");
        let estimate = digest
            .lines()
            .last()
            .and_then(|l| l.strip_prefix("est_tokens="))
            .and_then(|l| l.split('/').next())
            .and_then(|n| n.parse::<usize>().ok())
            .expect("footer estimate");
        assert!(estimate <= 200, "{digest}");
    });
    checks.await.expect("loopback checks");
    server.abort();
    let _ = server.await;
}
