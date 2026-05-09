//! AMQP consumer: listens on `order_queue` for new orders from the orchestrator.
//!
//! Replaces the Java `@RabbitListener` on `OrderMessageConsumer`.
//! Ack/nack is handled explicitly for full control over requeue behaviour:
//!   - Invalid message (empty order_id)  → nack, no requeue  (discard silently)
//!   - Transient service error           → nack, requeue     (retry later)
//!   - Success                           → ack
use crate::service::DeliveryService;
use common::OrderMessage;
use lapin::{
    Channel,
    options::{BasicAckOptions, BasicConsumeOptions, BasicNackOptions, QueueDeclareOptions},
    types::FieldTable,
};
use tracing::{error, info, warn};

const ORDER_QUEUE: &str = "order_queue";

/// Declare the queue, start a background consumer task and return immediately.
/// The task runs until the channel is closed or the process exits.
pub async fn start(channel: Channel, svc: DeliveryService) -> anyhow::Result<()> {
    use futures::StreamExt;

    // Idempotent declaration – safe to call on every startup.
    channel
        .queue_declare(
            ORDER_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    let mut consumer = channel
        .basic_consume(
            ORDER_QUEUE,
            // Unique consumer tag – avoids conflicts when multiple instances run.
            &format!("delivery-service-{}", uuid::Uuid::new_v4()),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;

    info!("Delivery order consumer started");

    tokio::spawn(async move {
        while let Some(delivery) = consumer.next().await {
            match delivery {
                Ok(delivery) => {
                    match serde_json::from_slice::<OrderMessage>(&delivery.data) {
                        Ok(order) => {
                            // Guard: every valid order must have a non-empty order_id.
                            if order.order_id.is_empty() {
                                warn!("Received OrderMessage with empty order_id – discarding");
                                let _ = delivery
                                    .nack(BasicNackOptions {
                                        requeue: false,
                                        ..Default::default()
                                    })
                                    .await;
                                continue;
                            }

                            info!(order_id = order.order_id, "Order received by Delivery Service");

                            if let Err(e) = svc.schedule(order).await {
                                // Transient error (e.g. broker unavailable) – requeue for retry.
                                error!("Failed to schedule delivery: {e}");
                                let _ = delivery
                                    .nack(BasicNackOptions {
                                        requeue: true,
                                        ..Default::default()
                                    })
                                    .await;
                                continue;
                            }
                        }

                        Err(e) => {
                            // Malformed message – nack without requeue to avoid poison-pill loops.
                            error!("Failed to deserialise OrderMessage: {e}");
                            let _ = delivery
                                .nack(BasicNackOptions {
                                    requeue: false,
                                    ..Default::default()
                                })
                                .await;
                            continue;
                        }
                    }

                    let _ = delivery.ack(BasicAckOptions::default()).await;
                }

                Err(e) => error!("AMQP delivery error: {e}"),
            }
        }

        error!("Delivery order consumer stopped – channel closed");
    });

    Ok(())
}
