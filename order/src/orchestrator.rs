use crate::{
    repository::SagaRepository,
    saga::{SagaState, SagaStatus, SagaStep},
};
use anyhow::Result;
use chrono::Utc;
use common::{OrderMessage, SagaEvent};
use prometheus::{IntCounter, Registry};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use std::{sync::Arc, time::Duration};
use tracing::{error, info, warn};
use uuid::Uuid;

// Two topics replace the one exchange + one direct queue from AMQP.
// "saga-events"    ← all SagaEvent variants (what was saga_events_exchange)
// "order-requests" ← OrderMessage to delivery service (what was order_queue)
pub const SAGA_EVENTS_TOPIC: &str = "saga-events";
pub const ORDER_REQUESTS_TOPIC: &str = "order-requests";

pub struct SagaMetrics {
    pub started: IntCounter,
    pub completed: IntCounter,
    pub failed: IntCounter,
    pub compensated: IntCounter,
}

impl SagaMetrics {
    pub fn new(registry: &Registry) -> Self {
        let started =
            IntCounter::new("order_saga_started_total", "Number of saga started").unwrap();
        let _ = registry.register(Box::new(started.clone()));
        let completed =
            IntCounter::new("order_saga_completed_total", "Number of saga completed").unwrap();
        let _ = registry.register(Box::new(completed.clone()));
        let failed = IntCounter::new("order_saga_failed_total", "Number of saga failed").unwrap();
        let _ = registry.register(Box::new(failed.clone()));
        let compensated =
            IntCounter::new("order_saga_compensated_total", "Number of saga compensated").unwrap();
        let _ = registry.register(Box::new(compensated.clone()));
        Self {
            started,
            completed,
            failed,
            compensated,
        }
    }
}

// FutureProducer is Arc-backed internally, so Clone is cheap.
#[derive(Clone)]
pub struct SagaOrchestrator {
    repo: SagaRepository,
    metrics: Arc<SagaMetrics>,
    producer: FutureProducer,
}

impl SagaOrchestrator {
    pub fn new(repo: SagaRepository, producer: FutureProducer, registry: &Registry) -> Self {
        Self {
            repo,
            metrics: Arc::new(SagaMetrics::new(registry)),
            producer,
        }
    }

    pub fn metrics(&self) -> Arc<SagaMetrics> {
        Arc::clone(&self.metrics)
    }

    pub async fn get_saga_by_order_id(&self, order_id: &str) -> Result<Option<SagaState>> {
        self.repo.find_by_order_id(order_id).await
    }

    // ── SAGA entry point ──────────────────────────────────────────────────────

    pub async fn start_order_saga(
        &self,
        order_id: String,
        customer_id: String,
        from_address: String,
        to_address: String,
        package_weight: f64,
        requested_delivery_time: chrono::DateTime<Utc>,
        max_delivery_time_minutes: i32,
    ) -> Result<String> {
        let saga_id = Uuid::new_v4().to_string();
        let mut saga = SagaState::new(
            saga_id.clone(),
            order_id.clone(),
            customer_id.clone(),
            from_address.clone(),
            to_address.clone(),
            package_weight,
            requested_delivery_time,
            max_delivery_time_minutes,
        );

        self.repo.save(&saga).await?;
        self.metrics.started.inc();
        info!(saga_id, order_id, "SAGA avviata");
        self.validate_order(&mut saga).await?;
        Ok(saga_id)
    }

    // ── Step 1: Order validation ──────────────────────────────────────────────

    async fn validate_order(&self, saga: &mut SagaState) -> Result<()> {
        info!(saga_id = saga.saga_id, "Step 1: validazione ordine");

        if saga.package_weight <= 0.0 {
            return self
                .handle_validation_failure(saga, "Peso pacco non valido")
                .await;
        }
        if saga.from_address.is_empty() || saga.to_address.is_empty() {
            return self
                .handle_validation_failure(saga, "Indirizzi mancanti")
                .await;
        }

        saga.mark_step_completed(SagaStep::OrderValidation);
        self.repo.save(saga).await?;

        let order = OrderMessage::new(
            saga.order_id.clone(),
            saga.customer_id.clone(),
            saga.from_address.clone(),
            saga.to_address.clone(),
            saga.package_weight,
            saga.requested_delivery_time,
            saga.max_delivery_time_minutes,
        );

        // Publish to order-requests topic; delivery service will consume it.
        // Key = order_id ensures partition affinity for this order's messages.
        self.publish_to_topic(ORDER_REQUESTS_TOPIC, &saga.order_id, &order)
            .await?;

        info!(
            saga_id = saga.saga_id,
            "Ordine validato. Messaggio inviato al Delivery Service."
        );
        Ok(())
    }

    // ── Async event handler (called by the Kafka consumer) ────────────────────

    pub async fn handle_saga_event(&self, event: SagaEvent) -> Result<()> {
        info!(order_id = event.order_id(), "Evento SAGA ricevuto");

        match event {
            SagaEvent::DeliveryScheduled {
                order_id,
                delivery_id,
                ..
            } => self.on_delivery_scheduled(&order_id, delivery_id).await,
            SagaEvent::DeliverySchedulingFailed {
                order_id, reason, ..
            } => self.on_delivery_failed(&order_id, reason).await,
            SagaEvent::DroneAssigned {
                order_id, drone_id, ..
            } => self.on_drone_assigned(&order_id, drone_id).await,
            SagaEvent::DroneAssignmentFailed {
                order_id, reason, ..
            } => self.on_drone_failed(&order_id, reason).await,
            _ => Ok(()),
        }
    }

    // ── Success paths ─────────────────────────────────────────────────────────
    // (on_delivery_scheduled, on_drone_assigned, complete_saga — unchanged
    //  except publish_event calls, updated below)

    async fn on_delivery_scheduled(&self, order_id: &str, delivery_id: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA non trovata per DeliveryScheduled");
            return Ok(());
        };
        saga.delivery_id = Some(delivery_id.clone());
        saga.mark_step_completed(SagaStep::DeliveryScheduling);
        self.repo.save(&saga).await?;
        info!(saga_id = saga.saga_id, delivery_id, "Step 2 completato");
        Ok(())
    }

    async fn on_drone_assigned(&self, order_id: &str, drone_id: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA non trovata per DroneAssigned");
            return Ok(());
        };
        saga.drone_id = Some(drone_id.clone());
        saga.mark_step_completed(SagaStep::DroneAssignment);
        self.repo.save(&saga).await?;
        info!(saga_id = saga.saga_id, drone_id, "Step 3 completato");
        self.complete_saga(saga).await
    }

    async fn complete_saga(&self, mut saga: SagaState) -> Result<()> {
        saga.status = SagaStatus::Completed;
        saga.end_time = Some(Utc::now());
        self.repo.save(&saga).await?;
        self.metrics.completed.inc();

        let event = SagaEvent::OrderCompleted {
            saga_id: saga.saga_id.clone(),
            order_id: saga.order_id.clone(),
            timestamp: Utc::now(),
        };
        // Key = order_id: all saga events for this order go to the same partition.
        self.publish_event(&saga.order_id, &event).await?;
        info!(saga_id = saga.saga_id, "SAGA completata con successo");
        Ok(())
    }

    // ── Failure paths ─────────────────────────────────────────────────────────

    async fn handle_validation_failure(&self, saga: &mut SagaState, reason: &str) -> Result<()> {
        saga.mark_failed(reason);
        self.repo.save(saga).await?;
        self.metrics.failed.inc();

        let event = SagaEvent::OrderValidationFailed {
            saga_id: saga.saga_id.clone(),
            order_id: saga.order_id.clone(),
            reason: reason.to_string(),
            timestamp: Utc::now(),
        };
        self.publish_event(&saga.order_id, &event).await?;
        error!(
            saga_id = saga.saga_id,
            reason, "SAGA fallita durante la validazione"
        );
        self.cancel_order(saga, reason).await
    }

    async fn on_delivery_failed(&self, order_id: &str, reason: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA non trovata per DeliverySchedulingFailed");
            return Ok(());
        };
        saga.mark_failed(&reason);
        self.repo.save(&saga).await?;
        self.metrics.failed.inc();
        error!(
            saga_id = saga.saga_id,
            reason, "SAGA fallita durante lo scheduling"
        );
        self.compensate_saga(saga).await
    }

    async fn on_drone_failed(&self, order_id: &str, reason: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA non trovata per DroneAssignmentFailed");
            return Ok(());
        };
        saga.mark_failed(&reason);
        self.repo.save(&saga).await?;
        self.metrics.failed.inc();
        error!(
            saga_id = saga.saga_id,
            reason, "SAGA fallita durante l'assegnazione del drone"
        );
        self.compensate_saga(saga).await
    }

    // ── Compensation ──────────────────────────────────────────────────────────

    async fn compensate_saga(&self, mut saga: SagaState) -> Result<()> {
        info!(saga_id = saga.saga_id, "Avvio compensazione SAGA");
        saga.start_compensation();
        self.repo.save(&saga).await?;

        for step in saga.steps_to_compensate() {
            match step {
                SagaStep::DroneAssignment => {
                    self.publish_event(
                        &saga.order_id,
                        &SagaEvent::CompensateDrone {
                            saga_id: saga.saga_id.clone(),
                            order_id: saga.order_id.clone(),
                            drone_id: saga.drone_id.clone().unwrap_or_default(),
                            reason: saga.failure_reason.clone().unwrap_or_default(),
                            timestamp: Utc::now(),
                        },
                    )
                    .await?;
                }
                SagaStep::DeliveryScheduling => {
                    self.publish_event(
                        &saga.order_id,
                        &SagaEvent::CompensateDelivery {
                            saga_id: saga.saga_id.clone(),
                            order_id: saga.order_id.clone(),
                            delivery_id: saga.delivery_id.clone().unwrap_or_default(),
                            reason: saga.failure_reason.clone().unwrap_or_default(),
                            timestamp: Utc::now(),
                        },
                    )
                    .await?;
                }
                SagaStep::OrderValidation => {
                    self.publish_event(
                        &saga.order_id,
                        &SagaEvent::CompensateOrder {
                            saga_id: saga.saga_id.clone(),
                            order_id: saga.order_id.clone(),
                            reason: saga.failure_reason.clone().unwrap_or_default(),
                            timestamp: Utc::now(),
                        },
                    )
                    .await?;
                }
                SagaStep::Completed => {}
            }
        }

        saga.mark_compensated();
        self.repo.save(&saga).await?;
        self.metrics.compensated.inc();
        info!(saga_id = saga.saga_id, "Compensazione completata");

        let reason = saga.failure_reason.clone().unwrap_or_default();
        self.cancel_order(&saga, &reason).await
    }

    async fn cancel_order(&self, saga: &SagaState, reason: &str) -> Result<()> {
        self.publish_event(
            &saga.order_id,
            &SagaEvent::OrderCancelled {
                saga_id: saga.saga_id.clone(),
                order_id: saga.order_id.clone(),
                reason: reason.to_string(),
                timestamp: Utc::now(),
            },
        )
        .await?;
        warn!(saga_id = saga.saga_id, reason, "Ordine annullato");
        Ok(())
    }

    // ── Private Kafka helpers ─────────────────────────────────────────────────

    /// Publishes a SagaEvent to the saga-events topic.
    /// `key` should be the order_id — ensures partition affinity.
    async fn publish_event(&self, key: &str, event: &SagaEvent) -> Result<()> {
        self.publish_to_topic(SAGA_EVENTS_TOPIC, key, event).await
    }

    /// Generic publish: serialise `msg` as JSON and send to `topic` with `key`.
    async fn publish_to_topic<T: serde::Serialize>(
        &self,
        topic: &str,
        key: &str,
        msg: &T,
    ) -> Result<()> {
        let payload = serde_json::to_vec(msg)?;

        // FutureRecord is the Kafka equivalent of BasicProperties + routing info.
        // .to(topic)    → which topic to write to  (replaces exchange + routing key)
        // .key(key)     → partition affinity key    (replaces routing key's routing role)
        // .payload(...)  → message body             (same as before)
        self.producer
            .send(
                FutureRecord::to(topic).key(key).payload(payload.as_slice()),
                Timeout::After(Duration::from_secs(5)),
            )
            .await
            .map_err(|(e, _)| anyhow::anyhow!("Kafka produce error on topic '{topic}': {e}"))?;
        Ok(())
    }
}
