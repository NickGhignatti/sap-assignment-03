use anyhow::Result;
use chrono::Utc;
use common::{OrderMessage, SagaEvent};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use std::time::Duration;
use tracing::info;
use uuid::Uuid;

// These replace the old DRONE_QUEUE and SAGA_EVENTS_EXCHANGE constants.
pub const SAGA_EVENTS_TOPIC: &str = "saga-events";
pub const DRONE_REQUESTS_TOPIC: &str = "drone-requests";

// FutureProducer is Arc-backed → Clone is cheap, same as Channel was.
#[derive(Clone)]
pub struct DeliveryService {
    producer: FutureProducer,
}

impl DeliveryService {
    pub fn new(producer: FutureProducer) -> Self {
        Self { producer }
    }

    /// Schedule a delivery for the given order:
    ///
    /// 1. Assign a `delivery_id`.
    /// 2. Forward the `OrderMessage` to the Drone Service via `drone-requests`.
    /// 3. Publish `DeliveryScheduled` on `saga-events` so the orchestrator
    ///    can advance to Step 3 (drone assignment).
    pub async fn schedule(&self, order: OrderMessage) -> Result<()> {
        let delivery_id = Uuid::new_v4().to_string();

        info!(
            order_id = order.order_id,
            delivery_id, "Delivery scheduled – forwarding to Drone Service"
        );

        // Step 1: forward the full order to the Drone Service.
        // Key = order_id ensures all messages for the same order go to the
        // same partition on drone-requests, preserving ordering.
        self.publish_to_topic(DRONE_REQUESTS_TOPIC, &order.order_id.clone(), &order)
            .await?;

        // Step 2: notify the SAGA orchestrator that delivery scheduling succeeded.
        let event = SagaEvent::DeliveryScheduled {
            saga_id: String::new(), // orchestrator correlates by order_id
            order_id: order.order_id.clone(),
            delivery_id: delivery_id.clone(),
            timestamp: Utc::now(),
        };
        // Same key → same partition as the drone-requests message above.
        // The orchestrator will process these in order.
        self.publish_to_topic(SAGA_EVENTS_TOPIC, &order.order_id, &event)
            .await?;

        info!(
            order_id = order.order_id,
            delivery_id, "DeliveryScheduled event published"
        );

        Ok(())
    }

    // ── Private Kafka helper ──────────────────────────────────────────────────

    async fn publish_to_topic<T: serde::Serialize>(
        &self,
        topic: &str,
        key: &str,
        msg: &T,
    ) -> Result<()> {
        let payload = serde_json::to_vec(msg)?;
        self.producer
            .send(
                FutureRecord::to(topic).key(key).payload(payload.as_slice()),
                Timeout::After(Duration::from_secs(5)),
            )
            .await
            .map_err(|(e, _)| anyhow::anyhow!("Kafka produce error on '{topic}': {e}"))?;
        Ok(())
    }
}
