//! Thin service layer: generates an order ID and delegates to the orchestrator.
use crate::orchestrator::SagaOrchestrator;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct OrderResponse {
    pub order_id: String,
    pub saga_id: String,
    pub status: &'static str,
}

pub struct OrderService {
    orchestrator: SagaOrchestrator,
}

impl OrderService {
    pub fn new(orchestrator: SagaOrchestrator) -> Self {
        Self { orchestrator }
    }

    pub async fn create_order(
        &self,
        customer_id: String,
        from_address: String,
        to_address: String,
        package_weight: f64,
        requested_delivery_time: DateTime<Utc>,
        max_delivery_time_minutes: i32,
    ) -> Result<OrderResponse> {
        let order_id = Uuid::new_v4().to_string();

        let saga_id = self
            .orchestrator
            .start_order_saga(
                order_id.clone(),
                customer_id,
                from_address,
                to_address,
                package_weight,
                requested_delivery_time,
                max_delivery_time_minutes,
            )
            .await?;

        Ok(OrderResponse {
            order_id,
            saga_id,
            status: "SAGA_STARTED",
        })
    }

    pub async fn get_order_status(&self, order_id: &str) -> Result<Option<crate::saga::SagaState>> {
        self.orchestrator.get_saga_by_order_id(order_id).await
    }
}

// ── HTTP request / response DTOs ─────────────────────────────────────────────

/// POST / body
#[derive(Debug, Deserialize)]
pub struct CreateOrderRequest {
    pub customer_id: String,
    pub from_address: String,
    pub to_address: String,
    pub package_weight: f64,
    /// ISO-8601 UTC; defaults to now+2h if omitted
    pub requested_delivery_time: Option<DateTime<Utc>>,
    pub max_delivery_time_minutes: i32,
}

/// GET /{orderId}/saga-status response
#[derive(Debug, Serialize)]
pub struct SagaStatusResponse {
    pub saga_id: String,
    pub order_id: String,
    pub status: String,
    pub current_step: String,
    pub completed_steps: Vec<String>,
    pub failure_reason: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
}
