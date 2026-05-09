//! Axum HTTP handlers for the Drone Service.
//!
//! All state access goes through `DroneService` – handlers are stateless.
//! Endpoints use path parameters instead of request bodies on GET requests,
//! which is more idiomatic REST than the original Java `@RequestBody` approach.
use crate::service::DroneService;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use std::sync::Arc;

pub type AppState = Arc<DroneService>;

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
    State(svc): State<AppState>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    // Bind the result of in_flight() to a variable so it isn't a temporary
    let in_flight_container = svc.in_flight();
    let map = in_flight_container.lock().unwrap();
    match map.get(&order_id) {
        Some(entry) => (StatusCode::OK, entry.to_string()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// GET /drone/{droneId}/events
// Returns the full persisted event history for a drone (event sourcing log).
pub async fn drone_events(
    State(svc): State<AppState>,
    Path(drone_id): Path<String>,
) -> impl IntoResponse {
    match svc.store.get_events_for_drone(&drone_id).await {
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

// GET /order/{orderId}/events
// Returns all events associated with a specific order (across all drones).
pub async fn order_events(
    State(svc): State<AppState>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    match svc.store.get_events_for_order(&order_id).await {
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
    State(svc): State<AppState>,
    Path(drone_id): Path<String>,
) -> impl IntoResponse {
    match svc.store.rebuild_drone(&drone_id).await {
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
