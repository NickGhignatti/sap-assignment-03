//! Dependencies are injected through the constructor and shared with `Arc<T>` (atomic zero-cost reference counting).
use crate::{
    repository::SagaRepository,
    saga::{SagaState, SagaStatus, SagaStep},
};
use anyhow::Result;
use chrono::Utc;
use common::{OrderMessage, SagaEvent};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Prometheus-style counters for SAGA metrics
#[derive(Default)]
pub struct SagaMetrics {
    pub started: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub compensated: AtomicU64,
}

// `Clone` is atomic for all `Arc<T>` fields or primitive `Copy`
#[derive(Clone)]
pub struct SagaOrchestrator {
    repo: SagaRepository,
    metrics: Arc<SagaMetrics>,
}

impl SagaOrchestrator {
    pub fn new(repo: SagaRepository) -> Self {
        Self {
            repo,
            metrics: Arc::new(SagaMetrics::default()),
        }
    }

    pub fn metrics(&self) -> Arc<SagaMetrics> {
        Arc::clone(&self.metrics)
    }

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

        info!(
            saga_id = saga.saga_id,
            "Ordine validato. Invio al delivery service..."
        );

        // Sending also the sagaId in the OrderMessage in order to allow the delivery service
        // to include it in the DeliveryScheduled without querying
        let _ = OrderMessage::new(
            saga.order_id.clone(),
            saga.customer_id.clone(),
            saga.from_address.clone(),
            saga.to_address.clone(),
            saga.package_weight,
            saga.requested_delivery_time,
            saga.max_delivery_time_minutes,
        );

        // TODO: Send the message on the message broker

        Ok(())
    }

    pub async fn handle_saga_event(&self, event: SagaEvent) -> Result<()> {
        info!(order_id = event.order_id(), "Evento SAGA ricevuto");

        match event {
            SagaEvent::DeliveryScheduled {
                order_id,
                delivery_id,
                ..
            } => {
                self.on_delivery_scheduled(&order_id, delivery_id).await?;
            }

            SagaEvent::DeliverySchedulingFailed {
                order_id, reason, ..
            } => {
                self.on_delivery_failed(&order_id, reason).await?;
            }

            SagaEvent::DroneAssigned {
                order_id, drone_id, ..
            } => {
                self.on_drone_assigned(&order_id, drone_id).await?;
            }

            SagaEvent::DroneAssignmentFailed {
                order_id, reason, ..
            } => {
                self.on_drone_failed(&order_id, reason).await?;
            }
            _ => {}
        }

        Ok(())
    }

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

        self.complete_saga(saga).await?;
        Ok(())
    }

    async fn complete_saga(&self, mut saga: SagaState) -> Result<()> {
        saga.status = SagaStatus::Completed;
        saga.end_time = Some(Utc::now());
        self.repo.save(&saga).await?;
        self.metrics.completed.fetch_add(1, Ordering::Relaxed);

        // TODO: Publish on the message broker that the SAGA is completed

        info!(saga_id = saga.saga_id, "SAGA successfully completed");
        Ok(())
    }

    async fn handle_validation_failure(&self, saga: &mut SagaState, reason: &str) -> Result<()> {
        saga.mark_failed(reason);
        self.repo.save(saga).await?;
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);

        error!(
            saga_id = saga.saga_id,
            reason, "SAGA failed during validation"
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
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);

        error!(
            saga_id = saga.saga_id,
            reason, "SAGA failed during delivery sheduling"
        );

        self.compensate_saga(saga).await
    }

    async fn on_drone_failed(&self, order_id: &str, reason: String) -> Result<()> {
        let Some(mut saga) = self.repo.find_by_order_id(order_id).await? else {
            warn!(order_id, "SAGA not found for DroneAssignmentFailed");
            return Ok(());
        };

        saga.mark_failed(&reason);
        self.repo.save(&saga).await?;
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);

        error!(
            saga_id = saga.saga_id,
            reason, "SAGA failed during drone assignment"
        );

        self.compensate_saga(saga).await
    }

    async fn compensate_saga(&self, mut saga: SagaState) -> Result<()> {
        info!(saga_id = saga.saga_id, "Avvio compensazione SAGA");
        saga.start_compensation();
        self.repo.save(&saga).await?;

        for step in saga.steps_to_compensate() {
            match step {
                SagaStep::DroneAssignment => {
                    // TODO: Publish on kafka
                }
                SagaStep::DeliveryScheduling => {
                    // TODO: Publish on kafka
                }
                SagaStep::OrderValidation => {
                    // TODO: Publish on kafka
                }
                SagaStep::Completed => {}
            }
        }

        saga.mark_compensated();
        self.repo.save(&saga).await?;
        self.metrics.compensated.fetch_add(1, Ordering::Relaxed);

        info!(saga_id = saga.saga_id, "Compensation done");

        self.cancel_order(&saga, &saga.failure_reason.clone().unwrap_or_default())
            .await
    }

    async fn cancel_order(&self, saga: &SagaState, reason: &str) -> Result<()> {
        // TODO: Publish on the message broker that the order has been cancelled

        warn!(saga_id = saga.saga_id, reason, "Order cancelled");
        Ok(())
    }
}
