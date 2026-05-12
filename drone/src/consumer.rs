use crate::service::{DroneService, SAGA_EVENTS_TOPIC};
use common::{OrderMessage, SagaEvent};
use rand::Rng;
use rdkafka::{
    consumer::{CommitMode, Consumer, StreamConsumer},
    message::Message,
};
use tracing::{error, info, warn};

pub const DRONE_REQUESTS_TOPIC: &str = "drone-requests";

/// Subscribes to `drone-requests`, spawns the consumer loop, returns immediately.
pub async fn start_order_consumer(
    consumer: StreamConsumer,
    svc: DroneService,
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
                            info!(order_id = order.order_id, "Order received by Drone Service");

                            // Randomise delivery duration within the allowed window.
                            let minutes = rand::thread_rng()
                                .gen_range(1..=order.max_delivery_time_minutes.max(1) as u32);

                            match svc.start_delivery(order, minutes).await {
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
    svc: DroneService,
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

                    match serde_json::from_slice::<SagaEvent>(payload) {
                        Ok(SagaEvent::CompensateDrone {
                            drone_id,
                            saga_id,
                            reason,
                            ..
                        }) => {
                            // This is the only event we act on.
                            warn!(drone_id, saga_id, reason, "Drone compensation requested");
                            svc.compensate(&drone_id);
                            info!(drone_id, "Drone compensation complete");

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
