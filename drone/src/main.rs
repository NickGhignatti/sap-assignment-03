pub mod api;
pub mod consumer;
pub mod model;
pub mod service;
pub mod store;

use anyhow::Result;
use axum::{Router, routing::get};
use mongodb::Client;
use rdkafka::ClientConfig;
use rdkafka::consumer::StreamConsumer;
use rdkafka::producer::FutureProducer;
use service::DroneService;
use std::{env, sync::Arc, time::Duration};
use store::DroneEventStore;
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
    let db = mongo.database("drone_service");
    let store = DroneEventStore::new(&db);

    // ── Kafka ─────────────────────────────────────────────────────────────────
    let brokers = env::var("KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".to_string());

    // One producer, shared by DroneService for publishing DroneAssigned events.
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("message.timeout.ms", "5000")
        .set("acks", "all")
        .create()?;

    // Consumer 1: receives OrderMessage from the Delivery Service.
    // group.id "drone-service-orders" → distinct cursor, visible in Kafka tooling.
    let order_consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("group.id", "drone-service-orders")
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.commit", "false")
        .create()?;

    // Consumer 2: receives CompensateDrone events from the SAGA orchestrator.
    // Different group.id → independent offset from consumer 1, even though
    // both consume from saga-events, they are in different groups.
    // Wait — consumer 1 reads drone-requests and consumer 2 reads saga-events,
    // so there is no overlap. The distinct group.id is still good practice
    // for clear Kafka consumer-group dashboards.
    let comp_consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &brokers)
        .set("group.id", "drone-service-compensation")
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.commit", "false")
        .create()?;

    // ── Wire up services ──────────────────────────────────────────────────────
    // No topology declarations — topics are auto-created or pre-existing.
    let svc = DroneService::new(store, producer);

    consumer::start_order_consumer(order_consumer, svc.clone()).await?;
    consumer::start_compensation_consumer(comp_consumer, svc.clone()).await?;

    // ── Arrival scheduler ─────────────────────────────────────────────────────
    // Unchanged: checks every 10s for drones that have reached their destination.
    let scheduler_svc = svc.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            if let Err(e) = scheduler_svc.settle_arrived().await {
                tracing::error!("Arrival scheduler error: {e}");
            }
        }
    });

    // ── HTTP server ───────────────────────────────────────────────────────────
    let app = Router::new()
        .route("/order/{orderId}/status", get(api::get_order_status))
        .route("/drone/{droneId}/events", get(api::drone_events))
        .route("/drone/{droneId}/rebuild", get(api::rebuild_drone))
        .route("/order/{orderId}/events", get(api::order_events))
        .route("/health", get(api::health))
        .with_state(Arc::new(svc));

    let port = env::var("PORT").unwrap_or_else(|_| "8082".to_string());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("Drone Service listening on port {port}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for ctrl_c");
            info!("Shutdown signal received – goodbye");
        })
        .await?;

    Ok(())
}
