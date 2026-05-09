//! Dependencies are injected through the constructor and shared with `Arc<T>` (atomic zero-cost reference counting).
use crate::{
    repository::SagaRepository,
    saga::{SagaState, SagaStatus, SagaStep},
};
use anyhow::Result;
use chrono::Utc;
use common::{OrderMessage, SagaEvent};
use lapin::{BasicProperties, Channel, options::BasicPublishOptions};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::{error, info, warn};
use uuid::Uuid;

pub const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";
pub const ORDER_QUEUE: &str = "order_queue";

/// Prometheus-style counters for SAGA metrics
#[derive(Default)]
pub struct SagaMetrics {
    pub started: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub compensated: AtomicU64,
}

/// `Clone` is cheap: `SagaRepository` and `lapin::Channel` are both Arc-backed internally.
#[derive(Clone)]
pub struct SagaOrchestrator {
    repo: SagaRepository,
    metrics: Arc<SagaMetrics>,
    channel: Channel,
}

impl SagaOrchestrator {
    pub fn new(repo: SagaRepository, channel: Channel) -> Self {
        Self {
            repo,
            metrics: Arc::new(SagaMetrics::default()),
            channel,
        }
    }

    pub fn metrics(&self) -> Arc<SagaMetrics> {
        Arc::clone(&self.metrics)
    }

    // ── Query helpers ─────────────────────────────────────────────────────────

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
        self.metrics.started.fetch_add(1, Ordering::Relaxed);

        info!(saga_id, order_id, "SAGA avviata");

        self.validate_order(&mut saga).await?;

        Ok(saga_id)
    }

    // ── Step 1: Validazione ordine (sincrono) ─────────────────────────────────

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

        // Sending also the saga_id in the OrderMessage so the delivery service
        // can include it in DeliveryScheduled without an extra query.
        let order = OrderMessage::new(
            saga.order_id.clone(),
            saga.customer_id.clone(),
            saga.from_address.clone(),
            saga.to_address.clone(),
            saga.package_weight,
            saga.requested_delivery_time,
            saga.max_delivery_time_minutes,
        );

        // Hand off to Delivery Service; the orchestrator is now idle until
        // a DeliveryScheduled / DeliverySchedulingFailed event arrives.
        self.publish_to_queue(ORDER_QUEUE, &order).await?;

        info!(
            saga_id = saga.saga_id,
            "Ordine validato. Messaggio inviato al Delivery Service."
        );

        Ok(())
    }

    // ── Async event handler (chiamato dal consumer AMQP) ──────────────────────

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

            _ => Ok(()), // eventi non rilevanti per l'orchestratore
        }
    }

    // ── Percorsi di successo ──────────────────────────────────────────────────

    async fn on_delivery_scheduled(&self, order_id: &str, delivery_id: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA non trovata per DeliveryScheduled");
            return Ok(());
        };

        saga.delivery_id = Some(delivery_id.clone());
        saga.mark_step_completed(SagaStep::DeliveryScheduling);
        self.repo.save(&saga).await?;

        info!(
            saga_id = saga.saga_id,
            delivery_id, "Step 2 completato: delivery schedulata"
        );

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

        info!(
            saga_id = saga.saga_id,
            drone_id, "Step 3 completato: drone assegnato"
        );

        self.complete_saga(saga).await
    }

    async fn complete_saga(&self, mut saga: SagaState) -> Result<()> {
        saga.status = SagaStatus::Completed;
        saga.end_time = Some(Utc::now());
        self.repo.save(&saga).await?;
        self.metrics.completed.fetch_add(1, Ordering::Relaxed);

        self.publish_event(
            "saga.completed",
            &SagaEvent::OrderCompleted {
                saga_id: saga.saga_id.clone(),
                order_id: saga.order_id.clone(),
                timestamp: Utc::now(),
            },
        )
        .await?;

        info!(saga_id = saga.saga_id, "SAGA completata con successo");
        Ok(())
    }

    // ── Percorsi di fallimento ────────────────────────────────────────────────

    async fn handle_validation_failure(&self, saga: &mut SagaState, reason: &str) -> Result<()> {
        saga.mark_failed(reason);
        self.repo.save(saga).await?;
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);

        self.publish_event(
            "saga.validation_failed",
            &SagaEvent::OrderValidationFailed {
                saga_id: saga.saga_id.clone(),
                order_id: saga.order_id.clone(),
                reason: reason.to_string(),
                timestamp: Utc::now(),
            },
        )
        .await?;

        error!(
            saga_id = saga.saga_id,
            reason, "SAGA fallita durante la validazione"
        );

        // Nessuno step completato → la compensazione è un no-op, si cancella direttamente.
        self.cancel_order(saga, reason).await
    }

    async fn on_delivery_failed(&self, order_id: &str, reason: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA non trovata per DeliverySchedulingFailed");
            return Ok(());
        };

        saga.mark_failed(&reason);
        self.repo.save(&saga).await?;
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);

        error!(
            saga_id = saga.saga_id,
            reason, "SAGA fallita durante lo scheduling della delivery"
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
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);

        error!(
            saga_id = saga.saga_id,
            reason, "SAGA fallita durante l'assegnazione del drone"
        );

        self.compensate_saga(saga).await
    }

    // ── Compensazione ─────────────────────────────────────────────────────────

    /// Esegue le transazioni compensative in ordine inverso rispetto agli step completati.
    /// Solo dopo che tutti gli eventi di compensazione sono stati pubblicati con successo
    /// lo stato viene aggiornato a `Compensated`.
    async fn compensate_saga(&self, mut saga: SagaState) -> Result<()> {
        info!(saga_id = saga.saga_id, "Avvio compensazione SAGA");
        saga.start_compensation();
        self.repo.save(&saga).await?;

        for step in saga.steps_to_compensate() {
            match step {
                SagaStep::DroneAssignment => {
                    self.publish_event(
                        "saga.compensate.drone",
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
                        "saga.compensate_delivery",
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
                        "saga.compensate_order",
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

        // Aggiornamento a Compensated solo dopo che tutti gli eventi
        // sono stati pubblicati con successo (grazie al ? sui publish sopra).
        saga.mark_compensated();
        self.repo.save(&saga).await?;
        self.metrics.compensated.fetch_add(1, Ordering::Relaxed);

        info!(saga_id = saga.saga_id, "Compensazione completata");

        let reason = saga.failure_reason.clone().unwrap_or_default();
        self.cancel_order(&saga, &reason).await
    }

    async fn cancel_order(&self, saga: &SagaState, reason: &str) -> Result<()> {
        self.publish_event(
            "saga.cancelled",
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

    // ── Helpers AMQP privati ──────────────────────────────────────────────────

    /// Pubblica un `SagaEvent` sul topic exchange.
    async fn publish_event(&self, routing_key: &str, event: &SagaEvent) -> Result<()> {
        let payload = serde_json::to_vec(event)?;
        self.channel
            .basic_publish(
                SAGA_EVENTS_EXCHANGE,
                routing_key,
                BasicPublishOptions::default(),
                &payload,
                BasicProperties::default().with_content_type("application/json".into()),
            )
            .await?
            .await?;
        Ok(())
    }

    /// Pubblica un messaggio direttamente su una coda (exchange di default,
    /// routing key = nome coda).
    async fn publish_to_queue<T: serde::Serialize>(&self, queue: &str, msg: &T) -> Result<()> {
        let payload = serde_json::to_vec(msg)?;
        self.channel
            .basic_publish(
                "",
                queue,
                BasicPublishOptions::default(),
                &payload,
                BasicProperties::default().with_content_type("application/json".into()),
            )
            .await?
            .await?;
        Ok(())
    }
}
