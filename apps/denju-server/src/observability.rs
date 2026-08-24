use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    body::Body,
    extract::State,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use denju_registry::{Registry, RegistryOperationalMetrics};
use serde::Serialize;

const LATENCY_BUCKETS_MS: [u64; 10] = [1, 5, 10, 25, 50, 100, 200, 500, 1_000, 2_000];

pub(crate) struct HttpMetrics {
    started: Instant,
    requests_total: AtomicU64,
    responses_5xx_total: AtomicU64,
    latency_micros_total: AtomicU64,
    latency_micros_max: AtomicU64,
    latency_buckets: [AtomicU64; LATENCY_BUCKETS_MS.len()],
    sse_connections_total: AtomicU64,
    active_sse_connections: AtomicU64,
    sse_overflows_total: AtomicU64,
}

#[derive(Debug, Serialize)]
pub(crate) struct HttpMetricsSnapshot {
    pub uptime_ms: u64,
    pub requests_total: u64,
    pub responses_5xx_total: u64,
    pub latency_micros_total: u64,
    pub latency_micros_max: u64,
    pub latency_buckets: Vec<LatencyBucket>,
    pub sse_connections_total: u64,
    pub active_sse_connections: u64,
    pub sse_overflows_total: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct LatencyBucket {
    pub le_ms: u64,
    pub count: u64,
}

#[derive(Debug, Serialize)]
struct OperationalMetricsResponse {
    http: HttpMetricsSnapshot,
    registry: RegistryOperationalMetrics,
}

pub(crate) async fn observe_request(
    State(metrics): State<Arc<HttpMetrics>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed();
    let status = response.status();
    metrics.record_request(status.as_u16(), elapsed);
    let latency_micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
    if status.is_server_error() {
        tracing::warn!(
            target: "denju_server::http",
            method = %method,
            path = %path,
            status = status.as_u16(),
            latency_micros,
            "http_request_failed"
        );
    } else {
        tracing::debug!(
            target: "denju_server::http",
            method = %method,
            path = %path,
            status = status.as_u16(),
            latency_micros,
            "http_request"
        );
    }
    response
}

pub(crate) async fn health_metrics(
    State(registry): State<Arc<Registry>>,
    Extension(metrics): Extension<Arc<HttpMetrics>>,
) -> Response {
    match registry.operational_metrics().await {
        Ok(registry) => Json(OperationalMetricsResponse {
            http: metrics.snapshot(),
            registry,
        })
        .into_response(),
        Err(_) => {
            tracing::warn!(target: "denju_server::health", "operational_metrics_unavailable");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"status": "unavailable"})),
            )
                .into_response()
        }
    }
}

impl HttpMetrics {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            requests_total: AtomicU64::new(0),
            responses_5xx_total: AtomicU64::new(0),
            latency_micros_total: AtomicU64::new(0),
            latency_micros_max: AtomicU64::new(0),
            latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            sse_connections_total: AtomicU64::new(0),
            active_sse_connections: AtomicU64::new(0),
            sse_overflows_total: AtomicU64::new(0),
        }
    }

    pub(crate) fn record_request(&self, status: u16, elapsed: Duration) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if status >= 500 {
            self.responses_5xx_total.fetch_add(1, Ordering::Relaxed);
        }
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.latency_micros_total
            .fetch_add(micros, Ordering::Relaxed);
        self.latency_micros_max.fetch_max(micros, Ordering::Relaxed);
        for (index, upper_bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if elapsed <= Duration::from_millis(*upper_bound) {
                self.latency_buckets[index].fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn start_sse(self: &Arc<Self>) -> SseConnectionGuard {
        self.sse_connections_total.fetch_add(1, Ordering::Relaxed);
        self.active_sse_connections.fetch_add(1, Ordering::Relaxed);
        SseConnectionGuard {
            metrics: self.clone(),
        }
    }

    pub(crate) fn record_sse_overflow(&self) {
        self.sse_overflows_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> HttpMetricsSnapshot {
        HttpMetricsSnapshot {
            uptime_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            requests_total: self.requests_total.load(Ordering::Relaxed),
            responses_5xx_total: self.responses_5xx_total.load(Ordering::Relaxed),
            latency_micros_total: self.latency_micros_total.load(Ordering::Relaxed),
            latency_micros_max: self.latency_micros_max.load(Ordering::Relaxed),
            latency_buckets: LATENCY_BUCKETS_MS
                .iter()
                .enumerate()
                .map(|(index, le_ms)| LatencyBucket {
                    le_ms: *le_ms,
                    count: self.latency_buckets[index].load(Ordering::Relaxed),
                })
                .collect(),
            sse_connections_total: self.sse_connections_total.load(Ordering::Relaxed),
            active_sse_connections: self.active_sse_connections.load(Ordering::Relaxed),
            sse_overflows_total: self.sse_overflows_total.load(Ordering::Relaxed),
        }
    }
}

pub(crate) struct SseConnectionGuard {
    metrics: Arc<HttpMetrics>,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        self.metrics
            .active_sse_connections
            .fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_bounded_and_sse_gauge_is_scoped() {
        let metrics = Arc::new(HttpMetrics::new());
        metrics.record_request(200, Duration::from_millis(12));
        metrics.record_request(503, Duration::from_millis(250));
        metrics.record_sse_overflow();
        {
            let _connection = metrics.start_sse();
            let snapshot = metrics.snapshot();
            assert_eq!(snapshot.requests_total, 2);
            assert_eq!(snapshot.responses_5xx_total, 1);
            assert_eq!(snapshot.active_sse_connections, 1);
            assert_eq!(snapshot.sse_overflows_total, 1);
            assert_eq!(snapshot.latency_buckets.len(), LATENCY_BUCKETS_MS.len());
        }
        assert_eq!(metrics.snapshot().active_sse_connections, 0);
    }
}
