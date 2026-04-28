//! Axum HTTP handlers for the Order service.
use crate::service::{CreateOrderRequest, OrderResponse, OrderService, SagaStatusResponse};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use std::sync::Arc;

pub type AppState = Arc<OrderService>;

// POST /
pub async fn create_order(
    State(svc): State<AppState>,
    Json(req): Json<CreateOrderRequest>,
) -> impl IntoResponse {
    let delivery_time = req
        .requested_delivery_time
        .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(2));

    match svc
        .create_order(
            req.customer_id,
            req.from_address,
            req.to_address,
            req.package_weight,
            delivery_time,
            req.max_delivery_time_minutes,
        )
        .await
    {
        Ok(resp) => (StatusCode::CREATED, Json(resp)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// GET /{orderId}/saga-status
pub async fn saga_status(
    State(svc): State<AppState>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    match svc.get_order_status(&order_id).await {
        Ok(Some(saga)) => {
            let resp = SagaStatusResponse {
                saga_id: saga.saga_id,
                order_id: saga.order_id,
                status: format!("{:?}", saga.status),
                current_step: format!("{:?}", saga.current_step),
                completed_steps: saga
                    .completed_steps
                    .iter()
                    .map(|s| format!("{:?}", s))
                    .collect(),
                failure_reason: saga.failure_reason,
                start_time: saga.start_time,
                end_time: saga.end_time,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
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
    "Order Service is running"
}
