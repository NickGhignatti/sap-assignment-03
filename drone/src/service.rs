//! Drone business logic.
//!
//! `DroneService` owns three pieces of shared state, all cheaply cloneable:
//!   - `store`     – event store backed by MongoDB  (`Collection<T>` is Arc-backed)
//!   - `channel`   – AMQP publish channel           (`lapin::Channel` is Arc-backed)
//!   - `in_flight` – in-memory map of active drones (`Arc<Mutex<HashMap>>`)
//!
//! All clones of `DroneService` share the same `in_flight` map, so the scheduler
//! task and the consumer task always see the same view of the world.
use crate::{
    model::{DroneEntry, DroneState},
    store::DroneEventStore,
};
use anyhow::Result;
use chrono::Utc;
use common::{DroneEvent, OrderMessage, SagaEvent};
use lapin::{BasicProperties, Channel, options::BasicPublishOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::{info, warn};
use uuid::Uuid;

const SAGA_EVENTS_EXCHANGE: &str = "saga_events_exchange";

/// Thread-safe map of currently in-flight drones, keyed by `order_id`.
/// Using `Mutex<HashMap>` rather than `DashMap` because all lock scopes
/// are intentionally short – no await is ever held while the lock is active.
pub type InFlightMap = Arc<Mutex<HashMap<String, DroneEntry>>>;

#[derive(Clone)]
pub struct DroneService {
    pub store: DroneEventStore,
    channel: Channel,
    in_flight: InFlightMap,
}

impl DroneService {
    pub fn new(store: DroneEventStore, channel: Channel) -> Self {
        Self {
            store,
            channel,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Expose the in-flight map for the HTTP API layer (read-only access).
    pub fn in_flight(&self) -> InFlightMap {
        Arc::clone(&self.in_flight)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Full delivery lifecycle kick-off:
    ///   1. Persist `DroneCreated` event.
    ///   2. Notify the SAGA orchestrator (`DroneAssigned`).
    ///   3. Persist `DroneDispatched` event.
    ///   4. Register the drone in the in-flight map with its expected arrival time.
    ///
    /// `delivery_minutes` is chosen randomly by the consumer based on the
    /// order's `max_delivery_time_minutes`.
    pub async fn start_delivery(&self, order: OrderMessage, delivery_minutes: u32) -> Result<()> {
        let drone_id = Uuid::new_v4().to_string();

        // ── Event 0: drone created ────────────────────────────────────────────
        let version = self.store.count_events_for_drone(&drone_id).await?;
        self.store
            .save_event(&DroneEvent::Created {
                drone_id: drone_id.clone(),
                order_id: order.order_id.clone(),
                customer_id: order.customer_id.clone(),
                from_address: order.from_address.clone(),
                to_address: order.to_address.clone(),
                package_weight: order.package_weight,
                requested_delivery_time: order.requested_delivery_time,
                max_delivery_time_minutes: order.max_delivery_time_minutes,
                timestamp: Utc::now(),
                version,
            })
            .await?;

        // ── Notify SAGA orchestrator (Step 3 success) ─────────────────────────
        // saga_id is left empty: the orchestrator correlates by order_id.
        self.publish_saga_event(
            "saga.drone_assigned",
            &SagaEvent::DroneAssigned {
                saga_id: String::new(),
                order_id: order.order_id.clone(),
                drone_id: drone_id.clone(),
                timestamp: Utc::now(),
            },
        )
        .await?;

        // ── Event 1: drone dispatched ─────────────────────────────────────────
        let version = self.store.count_events_for_drone(&drone_id).await?;
        self.store
            .save_event(&DroneEvent::Dispatched {
                drone_id: drone_id.clone(),
                order_id: order.order_id.clone(),
                dispatch_time: Utc::now(),
                timestamp: Utc::now(),
                version,
            })
            .await?;

        // ── Register in in-flight map ─────────────────────────────────────────
        let expected_arrival = Utc::now() + chrono::Duration::minutes(delivery_minutes as i64);
        let mut entry = DroneEntry::new(drone_id.clone(), order.clone());
        entry.start();
        entry.expected_arrival = Some(expected_arrival);

        // Lock scope is intentionally minimal – no await inside.
        self.in_flight
            .lock()
            .unwrap()
            .insert(order.order_id.clone(), entry);

        info!(
            drone_id,
            order_id = order.order_id,
            delivery_minutes,
            "Drone dispatched"
        );

        Ok(())
    }

    /// Called periodically by the scheduler task (every 10 s).
    /// Finds all drones whose `expected_arrival` has passed, persists the
    /// `Delivered` and `Returned` events, and removes them from the map.
    ///
    /// Correctness note: the lock is released before any `await` so that
    /// other tasks are not starved while MongoDB writes are in progress.
    pub async fn settle_arrived(&self) -> Result<()> {
        let now = Utc::now();

        // Collect arrived entries without holding the lock across awaits.
        let arrived: Vec<DroneEntry> = {
            let map = self.in_flight.lock().unwrap();
            map.values()
                .filter(|e| {
                    e.state == DroneState::InTransit
                        && e.expected_arrival.map_or(false, |t| now >= t)
                })
                .cloned()
                .collect()
        };

        for mut entry in arrived {
            info!(drone_id = entry.drone_id, "Drone arrived at destination");
            entry.end();

            // ── Event 2: delivered ────────────────────────────────────────────
            let version = self.store.count_events_for_drone(&entry.drone_id).await?;
            self.store
                .save_event(&DroneEvent::Delivered {
                    drone_id: entry.drone_id.clone(),
                    order_id: entry.order.order_id.clone(),
                    delivery_time: Utc::now(),
                    timestamp: Utc::now(),
                    version,
                })
                .await?;

            // ── Event 3: returned ─────────────────────────────────────────────
            let version = self.store.count_events_for_drone(&entry.drone_id).await?;
            self.store
                .save_event(&DroneEvent::Returned {
                    drone_id: entry.drone_id.clone(),
                    order_id: entry.order.order_id.clone(),
                    return_time: Utc::now(),
                    timestamp: Utc::now(),
                    version,
                })
                .await?;

            // Remove from map only after both events are safely persisted.
            self.in_flight.lock().unwrap().remove(&entry.order.order_id);

            info!(drone_id = entry.drone_id, "Drone returned to base");
        }

        Ok(())
    }

    /// Remove a drone from the in-flight map during SAGA compensation.
    /// The drone_id is used instead of order_id because the compensation
    /// event carries the drone_id assigned at Step 3.
    pub fn compensate(&self, drone_id: &str) {
        self.in_flight
            .lock()
            .unwrap()
            .retain(|_, entry| entry.drone_id != drone_id);

        warn!(drone_id, "Drone removed from in-flight map (compensation)");
    }

    // ── Private AMQP helper ───────────────────────────────────────────────────

    async fn publish_saga_event(&self, routing_key: &str, event: &SagaEvent) -> Result<()> {
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
}
