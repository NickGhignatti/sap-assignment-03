pub mod agent;
pub mod api;
pub mod beliefs;
pub mod consumer;
pub mod fleet;
pub mod intentions;
pub mod model;
pub mod service;
pub mod store;

use anyhow::Result;
use axum::{Router, routing::get};
use mongodb::Client;
use rdkafka::ClientConfig;
use rdkafka::consumer::StreamConsumer;
use rdkafka::producer::FutureProducer;
use std::{env, sync::Arc, time::Duration};
use store::DroneEventStore;
use tokio::sync::Mutex;
use tracing::info;

use crate::fleet::{DroneFleet, RoundRobinStrategy};

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
    let store = Arc::new(DroneEventStore::new(&db));

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

    // ── Wire up the agent fleet ───────────────────────────────────────────────
    // Wrap store and producer in Arc so the fleet's agents can share them.
    let producer = Arc::new(producer);

    // Fleet of 3 drone agents, round-robin assignment.
    // Wrapped in Arc<Mutex> so it can be shared across async tasks safely.
    let fleet = Arc::new(Mutex::new(DroneFleet::new(
        3,
        Arc::clone(&store),
        Arc::clone(&producer),
        Box::new(RoundRobinStrategy::new()),
    )));

    consumer::start_order_consumer(order_consumer, Arc::clone(&fleet)).await?;
    consumer::start_compensation_consumer(comp_consumer, Arc::clone(&fleet)).await?;

    // ── Arrival scheduler ─────────────────────────────────────────────────────
    let scheduler_fleet = Arc::clone(&fleet);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            if let Err(e) = scheduler_fleet.lock().await.check_arrivals().await {
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
        .with_state(Arc::clone(&store));

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
