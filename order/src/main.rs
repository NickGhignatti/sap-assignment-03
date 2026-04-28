pub mod api;
pub mod consumer;
pub mod orchestrator;
pub mod repository;
pub mod saga;
pub mod service;

use anyhow::Result;
use axum::{Router, routing::get, routing::post};
use lapin::{
    Connection, ConnectionProperties,
    options::ExchangeDeclareOptions,
    types::{AMQPValue, FieldTable},
};
use mongodb::Client;
use orchestrator::{SAGA_EVENTS_EXCHANGE, SagaOrchestrator};
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

    // ── RabbitMQ ──────────────────────────────────────────────────────────────
    let amqp_uri = env::var("AMQP_URI")
        .unwrap_or_else(|_| "amqp://guest:guest@localhost:5672/%2f".to_string());
    let conn = Connection::connect(&amqp_uri, ConnectionProperties::default()).await?;

    // Separate channels: one for publishing (orchestrator) and one for consuming.
    let publish_channel = conn.create_channel().await?;
    let consume_channel = conn.create_channel().await?;

    // Declare the topic exchange idempotently on startup.
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
    let orchestrator = SagaOrchestrator::new(repo, publish_channel);
    consumer::start(consume_channel, orchestrator.clone()).await?;

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
    axum::serve(listener, app).await?;

    Ok(())
}
