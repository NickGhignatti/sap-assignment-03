//! Delivery business logic.
//!
//! Responsibilities:
//!   1. Assign a unique delivery ID to the incoming order.
//!   2. Forward the order to the Drone Service via `drone_queue`.
//!   3. Notify the SAGA orchestrator by publishing a `DeliveryScheduled` event.
//!
//! `lapin::Channel` is Arc-backed, so cloning `DeliveryService` is cheap.
use anyhow::Result;
use chrono::Utc;
use common::{OrderMessage, SagaEvent};
use lapin::{BasicProperties, Channel, options::BasicPublishOptions};
use tracing::info;
use uuid::Uuid;

const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";
const DRONE_QUEUE: &str = "drone_queue";

#[derive(Clone)]
pub struct DeliveryService {
    channel: Channel,
}

impl DeliveryService {
    pub fn new(channel: Channel) -> Self {
        Self { channel }
    }

    /// Schedule a delivery for the given order:
    ///
    /// 1. Assign a `delivery_id`.
    /// 2. Forward the `OrderMessage` to the Drone Service via `drone_queue`.
    /// 3. Publish `DeliveryScheduled` on the SAGA exchange so the orchestrator
    ///    can advance to Step 3 (drone assignment).
    ///
    /// The `saga_id` is intentionally left empty: the orchestrator correlates
    /// events by `order_id`, so it does not need the saga_id here.
    pub async fn schedule(&self, order: OrderMessage) -> Result<()> {
        let delivery_id = Uuid::new_v4().to_string();

        info!(
            order_id = order.order_id,
            delivery_id, "Delivery scheduled – forwarding to Drone Service"
        );

        // Step 1: forward the full order to the Drone Service.
        // The drone service will use it to create and dispatch the drone.
        self.publish_to_queue(DRONE_QUEUE, &order).await?;

        // Step 2: notify the SAGA orchestrator that delivery scheduling succeeded.
        let event = SagaEvent::DeliveryScheduled {
            saga_id: String::new(),
            order_id: order.order_id.clone(),
            delivery_id: delivery_id.clone(),
            timestamp: Utc::now(),
        };
        self.publish_event("saga.delivery_scheduled", &event).await?;

        info!(
            order_id = order.order_id,
            delivery_id, "DeliveryScheduled event published"
        );

        Ok(())
    }

    // ── Private AMQP helpers ──────────────────────────────────────────────────

    /// Publish a `SagaEvent` on the topic exchange.
    async fn publish_event(&self, routing_key: &str, event: &SagaEvent) -> Result<()> {
        let payload = serde_json::to_vec(event)?;
        self.channel
            .basic_publish(
                SAGA_EVENTS_EXCHANGE,
                routing_key,
                BasicPublishOptions::default(),
                &payload,
                BasicProperties::default().with_content_type("application/json".into()),
            )
            .await?
            .await?;
        Ok(())
    }

    /// Publish any serialisable message directly to a named queue
    /// (via the default exchange, routing key = queue name).
    async fn publish_to_queue<T: serde::Serialize>(&self, queue: &str, msg: &T) -> Result<()> {
        let payload = serde_json::to_vec(msg)?;
        self.channel
            .basic_publish(
                "",
                queue,
                BasicPublishOptions::default(),
                &payload,
                BasicProperties::default().with_content_type("application/json".into()),
            )
            .await?
            .await?;
        Ok(())
    }
}
