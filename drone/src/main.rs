pub mod api;
pub mod consumer;
pub mod model;
pub mod service;
pub mod store;

use anyhow::Result;
use axum::{Router, routing::get};
use lapin::{
    Connection, ConnectionProperties,
    options::ExchangeDeclareOptions,
    types::FieldTable,
};
use mongodb::Client;
use service::DroneService;
use std::{env, sync::Arc, time::Duration};
use store::DroneEventStore;
use tracing::info;

const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // ── MongoDB ───────────────────────────────────────────────────────────────
    let mongo_uri = env::var("MONGODB_URI")
        .unwrap_or_else(|_| "mongodb://localhost:27017".to_string());
    let mongo = Client::with_uri_str(&mongo_uri).await?;
    let db = mongo.database("drone_service");
    let store = DroneEventStore::new(&db);

    // ── RabbitMQ ──────────────────────────────────────────────────────────────
    let amqp_uri = env::var("AMQP_URI")
        .unwrap_or_else(|_| "amqp://guest:guest@localhost:5672/%2f".to_string());
    let conn = Connection::connect(&amqp_uri, ConnectionProperties::default()).await?;

    // One channel per logical concern – isolates back-pressure between them.
    let publish_channel = conn.create_channel().await?;
    let order_sub_channel = conn.create_channel().await?;
    let comp_sub_channel = conn.create_channel().await?;

    // ── AMQP topology ─────────────────────────────────────────────────────────
    // Idempotent – safe to declare even if another service already created this.
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

    // ── Wire up services ──────────────────────────────────────────────────────
    let svc = DroneService::new(store, publish_channel);

    // Order consumer: receives OrderMessage from the Delivery Service.
    consumer::start_order_consumer(order_sub_channel, svc.clone()).await?;

    // Compensation consumer: receives CompensateDrone from the orchestrator.
    consumer::start_compensation_consumer(comp_sub_channel, svc.clone()).await?;

    // ── Arrival scheduler ─────────────────────────────────────────────────────
    // Replaces Spring's @Scheduled(fixedDelay = ...).
    // Checks every 10 s for drones that have reached their destination.
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
            tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl_c");
            info!("Shutdown signal received – goodbye");
        })
        .await?;

    Ok(())
}
