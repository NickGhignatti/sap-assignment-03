mod consumer;
mod service;

use anyhow::Result;
use rdkafka::ClientConfig;
use rdkafka::consumer::StreamConsumer;
use rdkafka::producer::FutureProducer;
use service::DeliveryService;
use std::env;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // ── Kafka ─────────────────────────────────────────────────────────────────
    let brokers = env::var("KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".to_string());

    // Producer shared by DeliveryService to publish to saga-events
    // and drone-requests topics.
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("message.timeout.ms", "5000")
        .set("acks", "all")
        .create()?;

    // Consumer belonging to "delivery-service" group.
    // Note: different group.id from the order service → each service gets its
    // own independent cursor through the same topics.
    // If you ran two replicas of delivery-service they'd SHARE this group,
    // so Kafka would split the partitions between them automatically.
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("group.id", "delivery-service")
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.commit", "false")
        .create()?;

    // ── Wire up services ──────────────────────────────────────────────────────
    // No topology declarations needed — topics are created automatically.
    let delivery_svc = DeliveryService::new(producer);
    consumer::start(consumer, delivery_svc).await?;

    info!("Delivery Service running – waiting for orders");

    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received – goodbye");

    Ok(())
}
