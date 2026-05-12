pub mod api;
pub mod consumer;
pub mod orchestrator;
pub mod repository;
pub mod saga;
pub mod service;

use anyhow::Result;
use axum::{Router, routing::get, routing::post};
use mongodb::Client;
use orchestrator::SagaOrchestrator;
use rdkafka::ClientConfig;
use rdkafka::consumer::StreamConsumer;
use rdkafka::producer::FutureProducer;
use repository::SagaRepository;
use service::OrderService;
use std::{env, sync::Arc};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // ── MongoDB ───────────────────────────────────────────────────────────────
    let mongo_uri =
        env::var("MONGODB_URI").unwrap_or_else(|_| "mongodb://localhost:27017".to_string());
    let mongo = Client::with_uri_str(&mongo_uri).await?;
    let db = mongo.database("order_service");
    let repo = SagaRepository::new(&db);

    // A single broker address (or comma-separated list for a cluster).
    // Kafka clients auto-discover the full cluster topology from this seed.
    let brokers = env::var("KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".to_string());

    // Producer: sends events to Kafka topics.
    // acks=all → waits for the leader AND all in-sync replicas to acknowledge.
    // This is the strongest durability guarantee (no data loss on broker failure).
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("message.timeout.ms", "5000")
        .set("acks", "all")
        .create()?;

    // Consumer: receives events from Kafka topics.
    //
    // group.id → "order-service" is our consumer group name.
    //   All replicas of this service share the same group.id, so Kafka
    //   distributes partitions among them (load balancing). Each partition
    //   is assigned to exactly one consumer in the group at a time.
    //
    // auto.offset.reset → "earliest": if this group has no committed offset
    //   (first run, or after a reset), start reading from the beginning of
    //   the topic. Use "latest" if you only care about new messages.
    //
    // enable.auto.commit → "false": we commit offsets manually, AFTER
    //   successfully processing each message. This is the equivalent of
    //   basic_ack in AMQP and is critical for at-least-once delivery.
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("group.id", "order-service")
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.commit", "false")
        .create()?;

    // ── Wire up services ──────────────────────────────────────────────────────
    let orchestrator = SagaOrchestrator::new(repo, producer);
    consumer::start(consumer, orchestrator.clone()).await?;

    let svc = Arc::new(OrderService::new(orchestrator));

    // ── HTTP server ───────────────────────────────────────────────────────────
    let app = Router::new()
        .route("/", post(api::create_order))
        .route("/{orderId}/saga-status", get(api::saga_status))
        .route("/health", get(api::health))
        .with_state(svc);

    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("Order Service listening on port {port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    info!("Shutdown signal received");
}
