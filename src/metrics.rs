//! Prometheus metrics, served at `/metrics` on their own address (`--metrics-bind`, off by
//! default), so they're never exposed with the API. Requests are counted per route pattern
//! (`/api/collections/{collection}/records`), which keeps the label sets small.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::app::App;

/// Upper bounds of the request duration buckets, in seconds.
const BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

#[derive(Default)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    count: u64,
    sum: f64,
}

/// The labels of a request: its project (empty for the main installation), method and route.
type Route = (String, String, String);

#[derive(Default)]
pub struct Metrics {
    /// (route, status) -> requests
    requests: Mutex<HashMap<(Route, u16), u64>>,
    durations: Mutex<HashMap<Route, Histogram>>,
    /// Realtime events read from the outbox and delivered to this instance's clients.
    pub realtime_events: AtomicU64,
}

/// The process's metrics.
pub static METRICS: LazyLock<Metrics> = LazyLock::new(Metrics::default);
/// When the process started (set when the server starts).
pub static STARTED: LazyLock<std::time::SystemTime> = LazyLock::new(std::time::SystemTime::now);

impl Metrics {
    fn record(&self, route: Route, status: u16, seconds: f64) {
        *self
            .requests
            .lock()
            .expect("metrics lock")
            .entry((route.clone(), status))
            .or_default() += 1;
        let mut durations = self.durations.lock().expect("metrics lock");
        let h = durations.entry(route).or_default();
        for (i, bound) in BUCKETS.iter().enumerate() {
            if seconds <= *bound {
                h.buckets[i] += 1;
            }
        }
        h.count += 1;
        h.sum += seconds;
    }
}

/// Counts every request and its duration (realtime streams count when they end).
pub async fn middleware(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let project = app.project().unwrap_or_default().to_string();
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".into());
    let started = Instant::now();
    let response = next.run(request).await;
    METRICS.record(
        (project, method, route),
        response.status().as_u16(),
        started.elapsed().as_secs_f64(),
    );
    response
}

/// Escapes a label value.
fn label(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// `GET /metrics`, in Prometheus's text format.
pub async fn render(State(app): State<Arc<App>>) -> Response {
    let m = &*METRICS;
    let mut out = String::new();
    let _ = writeln!(out, "# HELP slonbase_info The running version.");
    let _ = writeln!(out, "# TYPE slonbase_info gauge");
    let _ = writeln!(
        out,
        "slonbase_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    let started = STARTED
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "# HELP process_start_time_seconds When the process started."
    );
    let _ = writeln!(out, "# TYPE process_start_time_seconds gauge");
    let _ = writeln!(out, "process_start_time_seconds {started}");

    let _ = writeln!(
        out,
        "# HELP slonbase_http_requests_total HTTP requests, by route pattern."
    );
    let _ = writeln!(out, "# TYPE slonbase_http_requests_total counter");
    let mut requests: Vec<_> = m
        .requests
        .lock()
        .expect("metrics lock")
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    requests.sort();
    for (((project, method, route), status), n) in requests {
        let _ = writeln!(
            out,
            "slonbase_http_requests_total{{project=\"{}\",method=\"{method}\",route=\"{}\",status=\"{status}\"}} {n}",
            label(&project),
            label(&route)
        );
    }

    let _ = writeln!(
        out,
        "# HELP slonbase_http_request_duration_seconds How long requests took, by route pattern."
    );
    let _ = writeln!(
        out,
        "# TYPE slonbase_http_request_duration_seconds histogram"
    );
    {
        let durations = m.durations.lock().expect("metrics lock");
        let mut keys: Vec<_> = durations.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let h = &durations[&key];
            let labels = format!(
                "project=\"{}\",method=\"{}\",route=\"{}\"",
                label(&key.0),
                key.1,
                label(&key.2)
            );
            for (bound, n) in BUCKETS.iter().zip(h.buckets) {
                let _ = writeln!(
                    out,
                    "slonbase_http_request_duration_seconds_bucket{{{labels},le=\"{bound}\"}} {n}"
                );
            }
            let _ = writeln!(
                out,
                "slonbase_http_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}",
                h.count
            );
            let _ = writeln!(
                out,
                "slonbase_http_request_duration_seconds_sum{{{labels}}} {}",
                h.sum
            );
            let _ = writeln!(
                out,
                "slonbase_http_request_duration_seconds_count{{{labels}}} {}",
                h.count
            );
        }
    }

    let _ = writeln!(
        out,
        "# HELP slonbase_realtime_events_total Realtime events delivered to this instance's clients."
    );
    let _ = writeln!(out, "# TYPE slonbase_realtime_events_total counter");
    let _ = writeln!(
        out,
        "slonbase_realtime_events_total {}",
        m.realtime_events.load(Ordering::Relaxed)
    );

    // the rest needs the database (not in setup mode)
    if let Ok(core) = app.core() {
        let gauge = |out: &mut String, name: &str, help: &str, value: String| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} gauge");
            let _ = writeln!(out, "{name} {value}");
        };
        gauge(
            &mut out,
            "slonbase_realtime_clients",
            "Realtime clients connected to this instance.",
            core.realtime.client_count().to_string(),
        );
        gauge(
            &mut out,
            "slonbase_instances",
            "Instances sharing the database (this one included).",
            (core.realtime.other_instances() + 1).to_string(),
        );
        let status = core.pool.status();
        let _ = writeln!(
            out,
            "# HELP slonbase_db_connections Pooled database connections."
        );
        let _ = writeln!(out, "# TYPE slonbase_db_connections gauge");
        let idle = status.available;
        let _ = writeln!(out, "slonbase_db_connections{{state=\"idle\"}} {idle}");
        let _ = writeln!(
            out,
            "slonbase_db_connections{{state=\"in_use\"}} {}",
            status.size.saturating_sub(idle)
        );
        let _ = writeln!(
            out,
            "slonbase_db_connections{{state=\"max\"}} {}",
            status.max_size
        );
        gauge(
            &mut out,
            "slonbase_db_waiting",
            "Requests waiting for a pooled database connection.",
            status.waiting.to_string(),
        );
    }

    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        out,
    )
        .into_response()
}

/// The metrics server's router.
pub fn router(app: Arc<App>) -> axum::Router {
    axum::Router::new()
        .route("/metrics", axum::routing::get(render))
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histograms_are_cumulative() {
        let m = Metrics::default();
        let route = || (String::new(), "GET".to_string(), "/api/health".to_string());
        m.record(route(), 200, 0.02);
        m.record(route(), 200, 3.0);
        let durations = m.durations.lock().unwrap();
        let h = &durations[&route()];
        assert_eq!(h.buckets[0], 0, "<= 5ms");
        assert_eq!(h.buckets[2], 1, "<= 25ms");
        assert_eq!(h.buckets[BUCKETS.len() - 1], 2, "<= 10s");
        assert_eq!(h.count, 2);
        assert_eq!(m.requests.lock().unwrap()[&(route(), 200)], 2);
    }
}
