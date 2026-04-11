//! Saving directly `DroneEvent` as BSON document in MongoDB (no double serialization)
use anyhow::{Context, Result};
use common::DroneEvent;
use futures::TryStreamExt;
use mongodb::{Collection, Database, bson::doc};
use tracing::{error, info};

const COLLECTION: &str = "drone_events";

#[derive(Clone)]
pub struct DroneEventStore {
    col: Collection<DroneEvent>,
}

impl DroneEventStore {
    pub fn new(db: &Database) -> Self {
        Self {
            col: db.collection(COLLECTION),
        }
    }

    pub async fn save_event(&self, event: &DroneEvent) -> Result<()> {
        self.col
            .insert_one(event)
            .await
            .context("Insert EventDrone in MongoDB failed")?;

        info!(
            drone_id = event.drone_id(),
            event_type = event.event_type(),
            version = event.version(),
            "EventDrone saved"
        );
        Ok(())
    }

    pub async fn get_events_for_drone(&self, drone_id: &str) -> Result<Vec<DroneEvent>> {
        let mut cursor = self
            .col
            .find(doc! { "drone_id": drone_id })
            .sort(doc! { "version": 1 })
            .await
            .context("Event query failed")?;

        let mut events = Vec::new();
        while let Some(event) = cursor.try_next().await.context("Event cursor failed")? {
            events.push(event);
        }
        Ok(events)
    }

    pub async fn get_events_for_order(&self, order_id: &str) -> Result<Vec<DroneEvent>> {
        let mut cursor = self
            .col
            .find(doc! { "order_id": order_id })
            .sort(doc! { "version": 1 })
            .await
            .context("Event OrderEvent failed")?;

        let mut events = Vec::new();
        while let Some(event) = cursor.try_next().await.context("Event cursor failed")? {
            events.push(event);
        }
        Ok(events)
    }

    pub async fn count_events_for_drone(&self, drone_id: &str) -> Result<u64> {
        self.col
            .count_documents(doc! { "drone_id": drone_id })
            .await
            .context("DroneEvent count failed")
    }

    /// State is rebuild not by fetching a table but applying the events in order
    pub async fn rebuild_drone(&self, drone_id: &str) -> Result<Option<crate::model::DroneEntry>> {
        let events = self.get_events_for_drone(drone_id).await?;

        if events.is_empty() {
            return Ok(None);
        }

        let DroneEvent::Created {
            drone_id: did,
            order_id,
            customer_id,
            from_address,
            to_address,
            package_weight,
            requested_delivery_time,
            max_delivery_time_minutes,
            ..
        } = &events[0]
        else {
            error!(
                drone_id,
                "Event store corrupted - First event is not DroneCreated"
            );
            anyhow::bail!("Event store corrupted for drone {}", drone_id);
        };

        let order = common::OrderMessage::new(
            order_id.clone(),
            customer_id.clone(),
            from_address.clone(),
            to_address.clone(),
            *package_weight,
            *requested_delivery_time,
            *max_delivery_time_minutes,
        );

        let mut entry = crate::model::DroneEntry::new(did.clone(), order);

        for event in &events[1..] {
            match event {
                DroneEvent::Dispatched { .. } => entry.start(),
                DroneEvent::Delivered { .. } => entry.end(),
                DroneEvent::Returned { .. } => {}
                _ => {}
            }
        }

        Ok(Some(entry))
    }
}
