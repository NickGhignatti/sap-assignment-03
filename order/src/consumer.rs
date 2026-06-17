//! Kafka consumer: subscribes to saga-events and forwards events to the
//! orchestrator. Runs as a background tokio task.
use crate::orchestrator::{SAGA_EVENTS_TOPIC, SagaOrchestrator};
use common::SagaEvent;
use rdkafka::{
    consumer::{CommitMode, Consumer, StreamConsumer},
    message::Message,
};
use tracing::{Instrument, error, info, warn};

/// Subscribe to the saga-events topic, then spawn a task that drives the
/// consumer loop. The task runs until the process exits.
pub async fn start(consumer: StreamConsumer, orchestrator: SagaOrchestrator) -> anyhow::Result<()> {
    // No wildcards needed: we get every message on the topic and filter
    // in application code (orchestrator.handle_saga_event already ignores
    // events it doesn't care about via the `_ => Ok(())` arm).
    consumer.subscribe(&[SAGA_EVENTS_TOPIC])?;
    info!("SAGA event consumer subscribed to topic '{SAGA_EVENTS_TOPIC}'");

    tokio::spawn(async move {
        loop {
            // recv() is the pull: we ask Kafka for the next message.
            // This blocks (asynchronously) until one arrives or an error occurs.
            match consumer.recv().await {
                Ok(msg) => {
                    // msg.payload() returns Option<&[u8]> — None for tombstone records.
                    let Some(payload) = msg.payload() else {
                        warn!("Received empty payload (tombstone?), skipping");
                        // Commit to advance past this message.
                        let _ = consumer.commit_message(&msg, CommitMode::Async);
                        continue;
                    };

                    match serde_json::from_slice::<SagaEvent>(payload) {
                        Ok(event) => {
                            // Continue the distributed trace: read the id the producer
                            // attached as a Kafka header and run the handler inside a span
                            // carrying it, so every log line correlates to this order.
                            let trace_id = common::trace::trace_id_or_new(msg.headers());
                            let span = tracing::info_span!(
                                "saga_event",
                                %trace_id,
                                order_id = %event.order_id()
                            );
                            let outcome = orchestrator
                                .handle_saga_event(event, &trace_id)
                                .instrument(span)
                                .await;
                            match outcome {
                                Ok(_) => {
                                    // Processing succeeded → commit the offset.
                                    // CommitMode::Async: fire-and-forget commit (higher throughput).
                                    // Use CommitMode::Sync if you need the strongest guarantee.
                                    if let Err(e) = consumer.commit_message(&msg, CommitMode::Async)
                                    {
                                        error!("Failed to commit Kafka offset: {e}");
                                    }
                                }
                                Err(e) => {
                                    // Processing failed → do NOT commit.
                                    // Kafka will re-deliver this message on the next poll
                                    // (after a consumer restart or rebalance).
                                    // This is the equivalent of basic_nack { requeue: true }.
                                    error!("Error handling SAGA event: {e}. Offset NOT committed.");
                                }
                            }
                        }
                        Err(e) => {
                            // Poison pill: malformed JSON that will never parse.
                            // Commit to skip it — retrying would loop forever.
                            // This is the equivalent of basic_nack { requeue: false }.
                            error!(
                                "Failed to deserialise SAGA event: {e}. Skipping poison-pill message."
                            );
                            let _ = consumer.commit_message(&msg, CommitMode::Async);
                        }
                    }
                }
                Err(e) => {
                    // Kafka errors here are usually transient (e.g. rebalance).
                    // Log and keep looping — rdkafka handles reconnection internally.
                    error!("Kafka receive error: {e}");
                }
            }
        }
    });

    Ok(())
}
