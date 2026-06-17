//! Kafka consumer: subscribes to order-requests and forwards orders to
//! DeliveryService. Runs as a background tokio task.
use crate::service::DeliveryService;
use common::OrderMessage;
use rdkafka::{
    consumer::{CommitMode, Consumer, StreamConsumer},
    message::Message,
};
use tracing::{Instrument, error, info, warn};

pub const ORDER_REQUESTS_TOPIC: &str = "order-requests";

/// Subscribe to order-requests, spawn the consumer loop, return immediately.
pub async fn start(consumer: StreamConsumer, svc: DeliveryService) -> anyhow::Result<()> {
    // No queue_declare needed. Just subscribe and go.
    consumer.subscribe(&[ORDER_REQUESTS_TOPIC])?;
    info!("Delivery order consumer subscribed to topic '{ORDER_REQUESTS_TOPIC}'");

    tokio::spawn(async move {
        loop {
            match consumer.recv().await {
                Ok(msg) => {
                    let Some(payload) = msg.payload() else {
                        warn!("Empty payload (tombstone?) on order-requests – skipping");
                        // Commit: tombstones are intentional deletions in Kafka,
                        // we never want to retry them.
                        let _ = consumer.commit_message(&msg, CommitMode::Async);
                        continue;
                    };

                    match serde_json::from_slice::<OrderMessage>(payload) {
                        Ok(order) => {
                            // Guard: every valid order must carry a non-empty order_id.
                            if order.order_id.is_empty() {
                                warn!("OrderMessage with empty order_id – skipping (poison pill)");
                                // Commit to skip: this message would fail on every retry.
                                // Equivalent to nack (requeue: false).
                                let _ = consumer.commit_message(&msg, CommitMode::Async);
                                continue;
                            }

                            // Continue the trace started upstream by the Order Service.
                            let trace_id = common::trace::trace_id_or_new(msg.headers());
                            let order_id = order.order_id.clone();
                            let span =
                                tracing::info_span!("delivery_schedule", %trace_id, %order_id);
                            let outcome = async {
                                info!("Order received by Delivery Service");
                                svc.schedule(order, &trace_id).await
                            }
                            .instrument(span)
                            .await;

                            match outcome {
                                Ok(_) => {
                                    // Success → commit.
                                    if let Err(e) = consumer.commit_message(&msg, CommitMode::Async)
                                    {
                                        error!("Failed to commit offset: {e}");
                                    }
                                }
                                Err(e) => {
                                    // ransient error (e.g. Kafka broker unreachable) → no commit.
                                    // Kafka will re-deliver this message after a consumer restart
                                    // or a rebalance. Equivalent to nack(requeue: true).
                                    error!(
                                        "Failed to schedule delivery: {e}. Offset NOT committed."
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            // Malformed JSON → commit to skip forever.
                            // Equivalent to nack(requeue: false).
                            error!("Failed to deserialise OrderMessage: {e}. Skipping.");
                            let _ = consumer.commit_message(&msg, CommitMode::Async);
                        }
                    }
                }
                Err(e) => error!("Kafka receive error: {e}"),
            }
        }
    });

    Ok(())
}
