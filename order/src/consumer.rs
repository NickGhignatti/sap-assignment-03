//! AMQP consumer: listens on the saga_events_queue and forwards events to the
//! orchestrator. Runs as a background tokio task.
use crate::orchestrator::SagaOrchestrator;
use common::SagaEvent;
use lapin::{
    Channel,
    options::{
        BasicAckOptions, BasicConsumeOptions, BasicNackOptions, QueueBindOptions,
        QueueDeclareOptions,
    },
    types::FieldTable,
};
use tracing::{error, info};

pub const SAGA_EVENTS_QUEUE: &str = "saga_events_queue";
pub const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";

/// Declare the queue + binding, then spawn a task that drives the consumer.
/// The task runs until the channel is closed or the process exits.
pub async fn start(channel: Channel, orchestrator: SagaOrchestrator) -> anyhow::Result<()> {
    use futures::StreamExt;

    // Idempotent declarations – safe to call on every startup.
    channel
        .queue_declare(
            SAGA_EVENTS_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    channel
        .queue_bind(
            SAGA_EVENTS_QUEUE,
            SAGA_EVENTS_EXCHANGE,
            "saga.*",
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await?;

    let mut consumer = channel
        .basic_consume(
            SAGA_EVENTS_QUEUE,
            "order-service",
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;

    info!("SAGA event consumer started");

    tokio::spawn(async move {
        while let Some(delivery) = consumer.next().await {
            match delivery {
                Ok(delivery) => {
                    match serde_json::from_slice::<SagaEvent>(&delivery.data) {
                        Ok(event) => {
                            if let Err(e) = orchestrator.handle_saga_event(event).await {
                                error!("Error handling SAGA event: {e}");
                                // nack without requeue to avoid poison-pill loops
                                let _ = delivery
                                    .nack(BasicNackOptions {
                                        requeue: false,
                                        ..Default::default()
                                    })
                                    .await;
                                continue;
                            }
                        }
                        Err(e) => {
                            error!("Failed to deserialise SAGA event: {e}");
                        }
                    }
                    let _ = delivery.ack(BasicAckOptions::default()).await;
                }
                Err(e) => error!("AMQP delivery error: {e}"),
            }
        }
        error!("SAGA event consumer stopped – channel closed");
    });

    Ok(())
}
