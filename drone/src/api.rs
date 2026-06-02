//! Axum HTTP handlers for the Drone Service.
//!
//! All state access goes through `DroneEventStore` – handlers are stateless.
//! Endpoints use path parameters instead of request bodies on GET requests,
//! which is more idiomatic REST than the original Java `@RequestBody` approach.
use crate::store::DroneEventStore;
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use prometheus::{Encoder, Registry, TextEncoder};
use serde::Serialize;
use std::sync::Arc;

pub type AppState = Arc<DroneEventStore>;

// GET /metrics — Prometheus exposition endpoint.
pub async fn metrics(State(registry): State<Registry>) -> impl IntoResponse {
    let metric_families = registry.gather();
    let mut buffer = Vec::new();
    TextEncoder::new()
        .encode(&metric_families, &mut buffer)
        .unwrap();
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        buffer,
    )
}

/// Summary of a single event returned by the history endpoints.
#[derive(Debug, Serialize)]
pub struct EventSummary {
    pub event_type: &'static str,
    pub timestamp: String,
    pub version: u64,
}

// GET /order/{orderId}/status
// Returns the current in-flight status of the drone assigned to the given order.
pub async fn get_order_status(
    State(store): State<AppState>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    match store.get_events_for_order(&order_id).await {
        Ok(events) if !events.is_empty() => {
            let latest = events.last().unwrap();
            (
                StatusCode::OK,
                format!(
                    "Latest event: {} at {}",
                    latest.event_type(),
                    latest.timestamp().to_rfc3339()
                ),
            )
                .into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// GET /drone/{droneId}/events
// Returns the full persisted event history for a drone (event sourcing log).
pub async fn drone_events(
    State(store): State<AppState>,
    Path(drone_id): Path<String>,
) -> impl IntoResponse {
    match store.get_events_for_drone(&drone_id).await {
        Ok(events) if !events.is_empty() => {
            let latest = events.last().unwrap();
            (
                StatusCode::OK,
                format!(
                    "Latest event: {} at {}",
                    latest.event_type(),
                    latest.timestamp().to_rfc3339()
                ),
            )
                .into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// GET /order/{orderId}/events
// Returns all events associated with a specific order (across all drones).
pub async fn order_events(
    State(store): State<AppState>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    match store.get_events_for_order(&order_id).await {
        Ok(events) if !events.is_empty() => {
            let summaries: Vec<EventSummary> = events
                .iter()
                .map(|e| EventSummary {
                    event_type: e.event_type(),
                    timestamp: e.timestamp().to_rfc3339(),
                    version: e.version(),
                })
                .collect();
            (StatusCode::OK, Json(summaries)).into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// GET /drone/{droneId}/rebuild
// Reconstructs the drone state from its event log (demonstrates Event Sourcing).
// The result should always match the in-flight map entry for an active drone.
pub async fn rebuild_drone(
    State(store): State<AppState>,
    Path(drone_id): Path<String>,
) -> impl IntoResponse {
    match store.rebuild_drone(&drone_id).await {
        Ok(Some(entry)) => (StatusCode::OK, entry.to_string()).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// GET /health
pub async fn health() -> &'static str {
    "Drone Service is running"
}
