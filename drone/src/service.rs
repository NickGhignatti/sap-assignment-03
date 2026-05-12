use crate::{
    model::{DroneEntry, DroneState},
    store::DroneEventStore,
};
use anyhow::Result;
use chrono::Utc;
use common::{DroneEvent, OrderMessage, SagaEvent};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{info, warn};
use uuid::Uuid;

pub const SAGA_EVENTS_TOPIC: &str = "saga-events";

pub type InFlightMap = Arc<Mutex<HashMap<String, DroneEntry>>>;

#[derive(Clone)]
pub struct DroneService {
    pub store: DroneEventStore,
    producer: FutureProducer, // ← replaces lapin::Channel
    in_flight: InFlightMap,
}

impl DroneService {
    pub fn new(store: DroneEventStore, producer: FutureProducer) -> Self {
        Self {
            store,
            producer,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn in_flight(&self) -> InFlightMap {
        Arc::clone(&self.in_flight)
    }

    // ── Public API ────────────────────────────────────────────────────────────
    // Everything below is identical to the AMQP version except the one
    // publish_saga_event call, which now uses the Kafka helper at the bottom.

    pub async fn start_delivery(&self, order: OrderMessage, delivery_minutes: u32) -> Result<()> {
        let drone_id = Uuid::new_v4().to_string();

        // Event 0: drone created
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

        // Notify SAGA orchestrator: Step 3 success.
        // Key = order_id → same partition as all other events for this order.
        self.publish_to_topic(
            SAGA_EVENTS_TOPIC,
            &order.order_id,
            &SagaEvent::DroneAssigned {
                saga_id: String::new(), // orchestrator correlates by order_id
                order_id: order.order_id.clone(),
                drone_id: drone_id.clone(),
                timestamp: Utc::now(),
            },
        )
        .await?;

        // Event 1: drone dispatched
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

        // Register in in-flight map
        let expected_arrival = Utc::now() + chrono::Duration::minutes(delivery_minutes as i64);
        let mut entry = DroneEntry::new(drone_id.clone(), order.clone());
        entry.start();
        entry.expected_arrival = Some(expected_arrival);
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

    pub async fn settle_arrived(&self) -> Result<()> {
        let now = Utc::now();

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

            self.in_flight.lock().unwrap().remove(&entry.order.order_id);
            info!(drone_id = entry.drone_id, "Drone returned to base");
        }

        Ok(())
    }

    pub fn compensate(&self, drone_id: &str) {
        self.in_flight
            .lock()
            .unwrap()
            .retain(|_, e| e.drone_id != drone_id);
        warn!(drone_id, "Drone removed from in-flight map (compensation)");
    }

    // ── Private Kafka helper ──────────────────────────────────────────────────

    async fn publish_to_topic<T: serde::Serialize>(
        &self,
        topic: &str,
        key: &str,
        msg: &T,
    ) -> Result<()> {
        let payload = serde_json::to_vec(msg)?;
        self.producer
            .send(
                FutureRecord::to(topic).key(key).payload(payload.as_slice()),
                Timeout::After(Duration::from_secs(5)),
            )
            .await
            .map_err(|(e, _)| anyhow::anyhow!("Kafka produce error on '{topic}': {e}"))?;
        Ok(())
    }
}
