//! HTTP access telemetry: request identity, per-route latency and status
//! metrics, coalesced error events, periodic one-line summaries and a
//! dependency-free Prometheus text exposition.
//!
//! Successful requests are never logged one by one at the default level.
//! They are counted and summarized once per interval (`http.summary`), so a
//! busy dashboard costs one line a minute instead of thousands. Failures are
//! logged individually, but identical failures coalesce (see
//! `admiralty_telemetry`), keeping logs short enough for agents to read.
use admiralty_telemetry::Level;
use axum::{
    extract::{MatchedPath, Request},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// Request correlation header, accepted from clients when well formed and
/// always returned on the response.
pub const REQUEST_ID: &str = "x-request-id";
/// Upper bounds, in milliseconds, of the latency histogram buckets.
pub const BUCKETS_MS: [u64; 12] = [1, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000];
/// Distinct route labels retained; later routes aggregate under `other`.
pub const MAX_ROUTES: usize = 128;
/// A successful non-streaming response slower than this is logged once.
pub const SLOW_REQUEST: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RouteStats {
    /// Responses by status class: 1xx, 2xx, 3xx, 4xx, 5xx.
    pub by_class: [u64; 5],
    /// Non-cumulative counts per bucket; the last slot is +Inf.
    pub buckets: [u64; BUCKETS_MS.len() + 1],
    pub sum_micros: u64,
    pub max_micros: u64,
}

impl RouteStats {
    fn record(&mut self, status: StatusCode, elapsed: Duration) {
        let class = usize::from(status.as_u16() / 100).clamp(1, 5) - 1;
        self.by_class[class] += 1;
        let millis = elapsed.as_secs_f64() * 1000.0;
        let bucket = BUCKETS_MS
            .iter()
            .position(|bound| millis <= *bound as f64)
            .unwrap_or(BUCKETS_MS.len());
        self.buckets[bucket] += 1;
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.sum_micros = self.sum_micros.saturating_add(micros);
        self.max_micros = self.max_micros.max(micros);
    }

    pub fn count(&self) -> u64 {
        self.by_class.iter().sum()
    }

    /// Upper bound (ms) of the bucket containing quantile `q`; `None` when
    /// empty, `u64::MAX` when it falls in the +Inf bucket.
    pub fn quantile_le_ms(&self, q: f64) -> Option<u64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let target = ((total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (index, count) in self.buckets.iter().enumerate() {
            seen += count;
            if seen >= target {
                return Some(BUCKETS_MS.get(index).copied().unwrap_or(u64::MAX));
            }
        }
        Some(u64::MAX)
    }
}

#[derive(Default)]
struct Inner {
    routes: BTreeMap<String, RouteStats>,
    window: RouteStats,
    window_routes: BTreeMap<String, u64>,
}

pub struct HttpMetrics {
    started: Instant,
    inner: Mutex<Inner>,
}

/// One interval's request totals, emitted as `http.summary`.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSummary {
    pub requests: u64,
    pub client_errors: u64,
    pub server_errors: u64,
    pub p50_le_ms: Option<u64>,
    pub p95_le_ms: Option<u64>,
    pub max_ms: u64,
    pub top_route: Option<(String, u64)>,
}

impl Default for HttpMetrics {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            inner: Mutex::new(Inner::default()),
        }
    }
}

impl HttpMetrics {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn record(&self, route: &str, status: StatusCode, elapsed: Duration) {
        let mut inner = self.lock();
        let label = if inner.routes.contains_key(route) || inner.routes.len() < MAX_ROUTES {
            route.to_owned()
        } else {
            "other".to_owned()
        };
        inner
            .routes
            .entry(label.clone())
            .or_default()
            .record(status, elapsed);
        inner.window.record(status, elapsed);
        *inner.window_routes.entry(label).or_default() += 1;
    }

    pub fn routes(&self) -> BTreeMap<String, RouteStats> {
        self.lock().routes.clone()
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Take and reset the current summary window.
    pub fn take_window(&self) -> WindowSummary {
        let mut inner = self.lock();
        let window = std::mem::take(&mut inner.window);
        let routes = std::mem::take(&mut inner.window_routes);
        let top_route = routes
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)));
        WindowSummary {
            requests: window.count(),
            client_errors: window.by_class[3],
            server_errors: window.by_class[4],
            p50_le_ms: window.quantile_le_ms(0.5),
            p95_le_ms: window.quantile_le_ms(0.95),
            max_ms: window.max_micros / 1000,
            top_route,
        }
    }
}

/// Process-wide HTTP metrics shared by the middleware and `/metrics`.
pub fn metrics() -> &'static HttpMetrics {
    static METRICS: OnceLock<HttpMetrics> = OnceLock::new();
    METRICS.get_or_init(HttpMetrics::default)
}

/// Accept caller ids made of 1..=64 URL-safe characters; anything else is
/// replaced so a header cannot inject log fields or unbounded text.
pub fn valid_request_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn new_request_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_owned()
}

/// Axum middleware recording identity, metrics and failure events.
pub async fn observe(mut request: Request, next: Next) -> Response {
    let started = Instant::now();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let method = request.method().as_str().to_owned();
    let id = request
        .headers()
        .get(REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(new_request_id);
    let header = HeaderValue::from_str(&id).ok();
    if let Some(header) = &header {
        request.headers_mut().insert(REQUEST_ID, header.clone());
    }
    let mut response = next.run(request).await;
    let elapsed = started.elapsed();
    if let Some(header) = header {
        response.headers_mut().insert(REQUEST_ID, header);
    }
    let status = response.status();
    metrics().record(&route, status, elapsed);
    let streaming = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    // Default (info) output keeps what an operator or agent acts on: server
    // faults, throttling, slow responses and authentication denials. Routine
    // 404/422s and successes are counted in metrics and summaries instead.
    let (level, code) = match status.as_u16() {
        500..=599 => (Level::Error, "http.server_error"),
        429 => (Level::Warn, "http.throttled"),
        401 | 403 => (Level::Info, "http.denied"),
        400..=499 => (Level::Debug, "http.client_error"),
        _ if !streaming && elapsed >= SLOW_REQUEST => (Level::Warn, "http.slow"),
        _ => (Level::Debug, "http.access"),
    };
    admiralty_telemetry::event(level, "srv.http", code)
        .field("method", method)
        .field("route", route)
        .field("st", status.as_u16())
        .volatile("ms", elapsed.as_millis() as u64)
        .volatile("rid", id)
        .emit();
    response
}

/// Emit one `http.summary` line for the elapsed window (if it saw traffic)
/// and close any expired log bursts.
pub fn emit_summary(window: Duration) {
    let summary = metrics().take_window();
    if summary.requests > 0 {
        let mut event = admiralty_telemetry::event(Level::Info, "srv.http", "http.summary")
            .volatile("n", summary.requests)
            .volatile("e4", summary.client_errors)
            .volatile("e5", summary.server_errors)
            .volatile("max_ms", summary.max_ms)
            .volatile("win_s", window.as_secs());
        if let Some(p50) = summary.p50_le_ms {
            event = event.volatile("p50_le_ms", p50);
        }
        if let Some(p95) = summary.p95_le_ms {
            event = event.volatile("p95_le_ms", p95);
        }
        if let Some((route, count)) = summary.top_route {
            event = event.volatile("top", route).volatile("top_n", count);
        }
        event.emit();
    }
    admiralty_telemetry::global().flush_expired();
}

/// Extra gauges supplied by the server state at scrape time.
pub struct Gauges {
    pub event_streams_active: usize,
    pub event_streams_limit: usize,
    pub read_permits_available: usize,
    pub simulated: bool,
}

fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Prometheus text exposition format 0.0.4.
pub fn render_prometheus(gauges: &Gauges) -> String {
    let metrics = metrics();
    let mut out = String::with_capacity(4096);
    let _ = writeln!(
        out,
        "# HELP admiralty_build_info Build and mode metadata.\n# TYPE admiralty_build_info gauge\nadmiralty_build_info{{component=\"admiraltyd\",version=\"{}\",simulated=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION"),
        gauges.simulated
    );
    let mut push = |name: &str, kind: &str, help: &str, value: String| {
        let _ = writeln!(
            out,
            "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}"
        );
    };
    push(
        "admiralty_uptime_seconds",
        "gauge",
        "Seconds since the HTTP metrics registry started.",
        format!("{:.3}", metrics.uptime().as_secs_f64()),
    );
    push(
        "admiralty_event_streams_active",
        "gauge",
        "Open SSE event streams.",
        gauges.event_streams_active.to_string(),
    );
    push(
        "admiralty_event_streams_limit",
        "gauge",
        "Maximum concurrent SSE event streams.",
        gauges.event_streams_limit.to_string(),
    );
    push(
        "admiralty_read_permits_available",
        "gauge",
        "Unused snapshot read slots.",
        gauges.read_permits_available.to_string(),
    );
    let log = admiralty_telemetry::global().counters();
    let _ = writeln!(
        out,
        "# HELP admiralty_log_events_total Log events by disposition (suppressed = coalesced repeats).\n# TYPE admiralty_log_events_total counter\nadmiralty_log_events_total{{disposition=\"emitted\"}} {}\nadmiralty_log_events_total{{disposition=\"suppressed\"}} {}",
        log.emitted, log.suppressed
    );
    let routes = metrics.routes();
    let _ = writeln!(
        out,
        "# HELP admiralty_http_requests_total HTTP responses by route template and status class.\n# TYPE admiralty_http_requests_total counter"
    );
    for (route, stats) in &routes {
        for (index, count) in stats.by_class.iter().enumerate() {
            if *count > 0 {
                let _ = writeln!(
                    out,
                    "admiralty_http_requests_total{{route=\"{}\",class=\"{}xx\"}} {count}",
                    label(route),
                    index + 1
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "# HELP admiralty_http_request_duration_seconds Time to response headers by route template.\n# TYPE admiralty_http_request_duration_seconds histogram"
    );
    for (route, stats) in &routes {
        let route = label(route);
        let mut cumulative = 0;
        for (index, count) in stats.buckets.iter().enumerate() {
            cumulative += count;
            let le = BUCKETS_MS
                .get(index)
                .map_or("+Inf".to_owned(), |ms| format!("{}", *ms as f64 / 1000.0));
            let _ = writeln!(
                out,
                "admiralty_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"{le}\"}} {cumulative}"
            );
        }
        let _ = writeln!(
            out,
            "admiralty_http_request_duration_seconds_sum{{route=\"{route}\"}} {:.6}\nadmiralty_http_request_duration_seconds_count{{route=\"{route}\"}} {}",
            stats.sum_micros as f64 / 1_000_000.0,
            stats.count()
        );
    }
    out
}

/// Compare secrets without an early exit on the first differing byte.
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_stats_bucket_quantiles_and_classes() {
        let mut stats = RouteStats::default();
        for ms in [0, 3, 7, 40, 40, 90, 600, 20_000] {
            stats.record(StatusCode::OK, Duration::from_millis(ms));
        }
        stats.record(StatusCode::NOT_FOUND, Duration::from_millis(2));
        stats.record(StatusCode::BAD_GATEWAY, Duration::from_millis(2));
        assert_eq!(stats.count(), 10);
        assert_eq!(stats.by_class, [0, 8, 0, 1, 1]);
        assert_eq!(stats.quantile_le_ms(0.5), Some(10));
        assert_eq!(stats.quantile_le_ms(0.95), Some(u64::MAX));
        assert_eq!(stats.max_micros, 20_000_000);
        assert_eq!(RouteStats::default().quantile_le_ms(0.5), None);
    }

    #[test]
    fn registry_bounds_route_labels_and_windows_reset() {
        let metrics = HttpMetrics::default();
        for n in 0..(MAX_ROUTES + 5) {
            metrics.record(&format!("/r{n}"), StatusCode::OK, Duration::from_millis(1));
        }
        metrics.record(
            "/r0",
            StatusCode::INTERNAL_SERVER_ERROR,
            Duration::from_millis(3),
        );
        let routes = metrics.routes();
        assert_eq!(routes.len(), MAX_ROUTES + 1, "bounded plus other");
        assert_eq!(routes["other"].count(), 5);
        let window = metrics.take_window();
        assert_eq!(window.requests, MAX_ROUTES as u64 + 6);
        assert_eq!(window.server_errors, 1);
        assert_eq!(window.top_route, Some(("other".into(), 5)));
        assert_eq!(metrics.take_window().requests, 0, "window resets");
        assert_eq!(metrics.routes()["/r0"].count(), 2, "totals do not reset");
    }

    #[test]
    fn request_ids_are_bounded_tokens() {
        assert!(valid_request_id("abc-123_x.y"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id(&"a".repeat(65)));
        assert!(!valid_request_id("a b"));
        assert!(!valid_request_id("a\nb=c"));
        let generated = new_request_id();
        assert_eq!(generated.len(), 16);
        assert!(valid_request_id(&generated));
    }

    #[test]
    fn prometheus_text_is_well_formed_and_escapes_labels() {
        metrics().record("/t/{\"q\"}", StatusCode::OK, Duration::from_millis(4));
        let text = render_prometheus(&Gauges {
            event_streams_active: 2,
            event_streams_limit: 16,
            read_permits_available: 7,
            simulated: true,
        });
        assert!(text.contains("admiralty_event_streams_active 2\n"));
        assert!(text.contains("admiralty_read_permits_available 7\n"));
        assert!(text.contains("simulated=\"true\""));
        assert!(
            text.contains("admiralty_http_requests_total{route=\"/t/{\\\"q\\\"}\",class=\"2xx\"}")
        );
        assert!(text.contains("le=\"0.005\"}"));
        assert!(text.contains("le=\"+Inf\"}"));
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let value = line.rsplit(' ').next().expect("value");
            assert!(value.parse::<f64>().is_ok(), "{line}");
        }
    }

    #[test]
    fn constant_time_comparison_matches_equality() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
        assert!(!constant_time_eq(b"secret-token", b"secret-tokeN"));
        assert!(!constant_time_eq(b"short", b"longer"));
        assert!(constant_time_eq(b"", b""));
    }
}
