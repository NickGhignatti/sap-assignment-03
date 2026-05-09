//! Two AMQP consumers in one module:
//!
//!  - `start_order_consumer`        – listens on `drone_queue` for new orders
//!                                    (replaces Java `DroneMessageConsumer`)
//!  - `start_compensation_consumer` – listens on `drone_compensation_queue`
//!                                    for SAGA rollback signals
//!                                    (replaces Java `DroneCompensationListener`)
use crate::service::DroneService;
use common::{OrderMessage, SagaEvent};
use lapin::{
    Channel,
    options::{
        BasicAckOptions, BasicConsumeOptions, BasicNackOptions, QueueBindOptions,
        QueueDeclareOptions,
    },
    types::FieldTable,
};
use rand::Rng;
use tracing::{error, info, warn};

const DRONE_QUEUE: &str = "drone_queue";
const DRONE_COMPENSATION_QUEUE: &str = "drone_compensation_queue";
const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";

/// Declare `drone_queue`, then spawn a background task that consumes orders
/// and calls `DroneService::start_delivery` for each valid one.
pub async fn start_order_consumer(channel: Channel, svc: DroneService) -> anyhow::Result<()> {
    use futures::StreamExt;

    channel
        .queue_declare(
            DRONE_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    let mut consumer = channel
        .basic_consume(
            DRONE_QUEUE,
            // Unique tag – avoids conflicts when multiple instances run in parallel.
            &format!("drone-service-orders-{}", uuid::Uuid::new_v4()),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;

    info!("Drone order consumer started");

    tokio::spawn(async move {
        while let Some(delivery) = consumer.next().await {
            match delivery {
                Ok(delivery) => {
                    match serde_json::from_slice::<OrderMessage>(&delivery.data) {
                        Ok(order) => {
                            info!(order_id = order.order_id, "Order received by Drone Service");

                            // Randomise delivery duration within the allowed window.
                            let minutes = rand::thread_rng()
                                .gen_range(1..=order.max_delivery_time_minutes.max(1) as u32);

                            if let Err(e) = svc.start_delivery(order, minutes).await {
                                // Transient error – requeue so the order is not lost.
                                error!("Failed to start drone delivery: {e}");
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
                            // Malformed message – discard to avoid poison-pill loops.
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

        error!("Drone order consumer stopped – channel closed");
    });

    Ok(())
}

/// Declare `drone_compensation_queue`, bind it to the SAGA exchange on
/// `saga.compensate.drone`, then spawn a task that calls `DroneService::compensate`
/// for every `CompensateDrone` event received.
pub async fn start_compensation_consumer(
    channel: Channel,
    svc: DroneService,
) -> anyhow::Result<()> {
    use futures::StreamExt;

    channel
        .queue_declare(
            DRONE_COMPENSATION_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    // Bind to the topic exchange so we only receive drone compensation events.
    channel
        .queue_bind(
            DRONE_COMPENSATION_QUEUE,
            SAGA_EVENTS_EXCHANGE,
            "saga.compensate.drone",
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await?;

    let mut consumer = channel
        .basic_consume(
            DRONE_COMPENSATION_QUEUE,
            &format!("drone-service-compensation-{}", uuid::Uuid::new_v4()),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;

    info!("Drone compensation consumer started");

    tokio::spawn(async move {
        while let Some(delivery) = consumer.next().await {
            match delivery {
                Ok(delivery) => {
                    match serde_json::from_slice::<SagaEvent>(&delivery.data) {
                        Ok(SagaEvent::CompensateDrone {
                            drone_id,
                            saga_id,
                            reason,
                            ..
                        }) => {
                            warn!(drone_id, saga_id, reason, "Drone compensation requested");
                            svc.compensate(&drone_id);
                            info!(drone_id, "Drone compensation complete");
                        }

                        Ok(other) => {
                            // Should never happen given the routing key binding,
                            // but log and discard rather than crash.
                            warn!(
                                "Unexpected event on compensation queue: {:?}",
                                other
                            );
                        }

                        Err(e) => {
                            error!("Failed to deserialise compensation event: {e}");
                        }
                    }

                    // Always ack: compensation is best-effort and retrying a
                    // malformed message would not help.
                    let _ = delivery.ack(BasicAckOptions::default()).await;
                }

                Err(e) => error!("AMQP compensation delivery error: {e}"),
            }
        }

        error!("Drone compensation consumer stopped – channel closed");
    });

    Ok(())
}
