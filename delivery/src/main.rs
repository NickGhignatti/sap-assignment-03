mod consumer;
mod service;

use anyhow::Result;
use lapin::{
    Connection, ConnectionProperties,
    options::{ExchangeDeclareOptions, QueueDeclareOptions},
    types::FieldTable,
};
use service::DeliveryService;
use std::env;
use tracing::info;

const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";
const DRONE_QUEUE: &str = "drone_queue";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // ── RabbitMQ ──────────────────────────────────────────────────────────────
    let amqp_uri = env::var("AMQP_URI")
        .unwrap_or_else(|_| "amqp://guest:guest@localhost:5672/%2f".to_string());

    let conn = Connection::connect(&amqp_uri, ConnectionProperties::default()).await?;

    // Two channels: one for publishing (DeliveryService), one for consuming.
    // Keeping them separate isolates back-pressure between the two directions.
    let publish_channel = conn.create_channel().await?;
    let consume_channel = conn.create_channel().await?;

    // ── AMQP topology ─────────────────────────────────────────────────────────
    // Idempotent declarations – safe to call on every startup regardless of
    // whether another service has already created these resources.

    // The topic exchange used for all SAGA events.
    publish_channel
        .exchange_declare(
            SAGA_EVENTS_EXCHANGE,
            lapin::ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    // The queue consumed by the Drone Service.
    // Declared here so the Delivery Service can publish to it even if the
    // Drone Service has not started yet.
    publish_channel
        .queue_declare(
            DRONE_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    // ── Wire up services ──────────────────────────────────────────────────────
    let delivery_svc = DeliveryService::new(publish_channel);
    consumer::start(consume_channel, delivery_svc).await?;

    info!("Delivery Service running – waiting for orders");

    // Keep the process alive until SIGINT / SIGTERM.
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received – goodbye");

    Ok(())
}
