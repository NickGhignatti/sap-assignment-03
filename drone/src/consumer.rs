use std::sync::Arc;

use crate::service::SAGA_EVENTS_TOPIC;
use common::{OrderMessage, SagaEvent};
use rdkafka::{
    consumer::{CommitMode, Consumer, StreamConsumer},
    message::Message,
};
use tracing::{Instrument, error, info, warn};

pub const DRONE_REQUESTS_TOPIC: &str = "drone-requests";

/// Subscribes to `drone-requests`, spawns the consumer loop, returns immediately.
pub async fn start_order_consumer(
    consumer: StreamConsumer,
    fleet: Arc<tokio::sync::Mutex<crate::fleet::DroneFleet>>,
) -> anyhow::Result<()> {
    consumer.subscribe(&[DRONE_REQUESTS_TOPIC])?;
    info!("Drone order consumer subscribed to '{DRONE_REQUESTS_TOPIC}'");

    tokio::spawn(async move {
        loop {
            match consumer.recv().await {
                Ok(msg) => {
                    let Some(payload) = msg.payload() else {
                        warn!("Empty payload on drone-requests – skipping tombstone");
                        let _ = consumer.commit_message(&msg, CommitMode::Async);
                        continue;
                    };

                    match serde_json::from_slice::<OrderMessage>(payload) {
                        Ok(order) => {
                            // Continue the trace started by the Order Service.
                            let trace_id = common::trace::trace_id_or_new(msg.headers());
                            let order_id = order.order_id.clone();
                            let span = tracing::info_span!("drone_order", %trace_id, %order_id);
                            let outcome = async {
                                info!("Order received by Drone Service");
                                fleet.lock().await.dispatch_order(order, &trace_id).await
                            }
                            .instrument(span)
                            .await;

                            match outcome {
                                Ok(_) => {
                                    // ✅ Success → commit.
                                    if let Err(e) = consumer.commit_message(&msg, CommitMode::Async)
                                    {
                                        error!("Failed to commit offset: {e}");
                                    }
                                }
                                Err(e) => {
                                    // ❌ Transient error → no commit.
                                    error!(
                                        "Failed to start drone delivery: {e}. Offset NOT committed."
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            // ☠️ Poison pill → commit to skip.
                            error!("Failed to deserialise OrderMessage: {e}. Skipping.");
                            let _ = consumer.commit_message(&msg, CommitMode::Async);
                        }
                    }
                }
                Err(e) => error!("Kafka receive error (orders): {e}"),
            }
        }
    });

    Ok(())
}

/// Subscribes to `saga-events`, spawns the consumer loop, returns immediately.
///
/// In AMQP the routing key `saga.compensate.drone` meant only CompensateDrone
/// events arrived on this queue. Here we receive ALL saga-events and filter
/// in application code. Non-drone events are committed immediately (skip).
pub async fn start_compensation_consumer(
    consumer: StreamConsumer,
    fleet: Arc<tokio::sync::Mutex<crate::fleet::DroneFleet>>,
) -> anyhow::Result<()> {
    consumer.subscribe(&[SAGA_EVENTS_TOPIC])?;
    info!("Drone compensation consumer subscribed to '{SAGA_EVENTS_TOPIC}'");

    tokio::spawn(async move {
        loop {
            match consumer.recv().await {
                Ok(msg) => {
                    let Some(payload) = msg.payload() else {
                        let _ = consumer.commit_message(&msg, CommitMode::Async);
                        continue;
                    };

                    // Continue the trace started by the Order Service.
                    let trace_id = common::trace::trace_id_or_new(msg.headers());
                    match serde_json::from_slice::<SagaEvent>(payload) {
                        Ok(SagaEvent::CompensateDrone {
                            drone_id,
                            saga_id,
                            reason,
                            ..
                        }) => {
                            // This is the only event we act on.
                            let span =
                                tracing::info_span!("drone_compensation", %trace_id, %drone_id);
                            let outcome = async {
                                warn!(saga_id, reason, "Drone compensation requested");
                                fleet
                                    .lock()
                                    .await
                                    .compensate(drone_id.clone(), &trace_id)
                                    .await
                            }
                            .instrument(span)
                            .await;
                            if let Err(e) = outcome {
                                error!("Compensation failed: {e}");
                            }

                            // Compensation is synchronous (no async work in compensate()),
                            // so we always commit — retrying a compensation would be
                            // idempotent anyway but unnecessary.
                            if let Err(e) = consumer.commit_message(&msg, CommitMode::Async) {
                                error!("Failed to commit compensation offset: {e}");
                            }
                        }
                        Ok(_other) => {
                            // Any other SagaEvent (OrderCompleted, DeliveryScheduled, etc.)
                            // is not for us. Commit immediately to advance the offset.
                            // This is the Kafka equivalent of "routing key didn't match" —
                            // the broker used to do this for us; now we do it ourselves.
                            let _ = consumer.commit_message(&msg, CommitMode::Async);
                        }
                        Err(e) => {
                            // Malformed JSON → commit and skip.
                            error!("Failed to deserialise SagaEvent: {e}. Skipping.");
                            let _ = consumer.commit_message(&msg, CommitMode::Async);
                        }
                    }
                }
                Err(e) => error!("Kafka receive error (compensation): {e}"),
            }
        }
    });

    Ok(())
}
