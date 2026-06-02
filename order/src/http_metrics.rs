//! HTTP-level metrics for the availability SLI.
//!
//! A single labelled counter `http_requests_total{status="..."}` is incremented
//! by an Axum middleware on every response, regardless of route. The availability
//! SLI is then computed in PromQL as the ratio of non-5xx responses to total.

use std::sync::Arc;

use axum::{extract::Request, extract::State, middleware::Next, response::Response};
use prometheus::{IntCounterVec, Opts, Registry};

pub struct HttpMetrics {
    /// Total HTTP responses, labelled by status code (e.g. "200", "422", "500").
    pub requests_total: IntCounterVec,
}

impl HttpMetrics {
    pub fn new(registry: &Registry) -> Arc<Self> {
        let requests_total = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP responses by status code"),
            &["status"], // one label dimension: the status code
        )
        .unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        Arc::new(Self { requests_total })
    }
}

/// Middleware: runs the handler, then records the response status code.
///
/// Registered with `from_fn_with_state`, so it receives `Arc<HttpMetrics>` as
/// state in addition to the request and the `Next` handle.
pub async fn track_metrics(
    State(metrics): State<Arc<HttpMetrics>>,
    req: Request,
    next: Next,
) -> Response {
    // Let the actual handler run and produce the response.
    let response = next.run(req).await;

    // Record its status code on the way out.
    let status = response.status().as_u16().to_string();
    metrics
        .requests_total
        .with_label_values(&[status.as_str()])
        .inc();

    response
}
