use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::Utc;
use common::{DroneEvent, OrderMessage, SagaEvent};
use rdkafka::{
    producer::{FutureProducer, FutureRecord},
    util::Timeout,
};
use tracing::{info, warn};

const SAGA_EVENTS_TOPIC: &str = "saga-events";

use crate::{
    beliefs::DroneBeliefs,
    intentions::{DroneDesire, DroneIntention, plan_for},
    store::DroneEventStore,
};

/// The BDI cycle runs continuously. Each tick:
/// 1. perceive()       → get new signals from the environment
/// 2. update_beliefs() → revise the belief base (AGM expansion)
/// 3. deliberate()     → select active goal from desires
/// 4. plan()           → map goal → Vec<DroneAction>  (your plan_for)
/// 5. execute()        → run each action (Kafka publish / MongoDB write)
#[allow(async_fn_in_trait)]
pub trait Agent {
    type Perception;
    type Goal;

    fn update_beliefs(&mut self, perception: Self::Perception);
    fn deliberate(&self) -> Self::Goal;
    fn plan(&self, goal: Self::Goal) -> Vec<DroneDesire>;
    async fn execute(&mut self, actions: Vec<DroneDesire>) -> Result<()>;
}

pub struct DroneAgent {
    pub beliefs: DroneBeliefs,
    store: Arc<DroneEventStore>,
    producer: Arc<FutureProducer>,
}

impl DroneAgent {
    pub fn new(store: Arc<DroneEventStore>, producer: Arc<FutureProducer>) -> Self {
        Self {
            beliefs: DroneBeliefs::new(),
            store,
            producer,
        }
    }
}

impl Agent for DroneAgent {
    type Perception = OrderMessage;
    type Goal = DroneIntention;

    fn update_beliefs(&mut self, order: OrderMessage) {
        self.beliefs = self
            .beliefs
            .clone()
            .from_order(&order, Some(uuid::Uuid::new_v4().to_string()));
    }

    fn deliberate(&self) -> Self::Goal {
        match &self.beliefs.current_order {
            None => DroneIntention::Idle,
            Some(order) => {
                let drone_id = self.beliefs.drone_id.clone().unwrap_or_default();

                if !self.beliefs.can_carry_payload {
                    DroneIntention::RefuseDelivery {
                        order: order.clone(),
                        reason: format!("{}kg exceeds max payload", order.package_weight),
                    }
                } else if !self.beliefs.can_meet_deadline {
                    DroneIntention::RefuseDelivery {
                        order: order.clone(),
                        reason: "Deadline cannot be met".into(),
                    }
                } else {
                    DroneIntention::AcceptDelivery {
                        order: order.clone(),
                        drone_id: drone_id,
                    }
                }
            }
        }
    }

    fn plan(&self, goal: Self::Goal) -> Vec<DroneDesire> {
        plan_for(goal)
    }

    async fn execute(&mut self, actions: Vec<DroneDesire>) -> Result<()> {
        for action in actions {
            match action {
                // ── Store the Created event in MongoDB ────────────────────────
                DroneDesire::StoreCreatedEvent { drone_id, order } => {
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
                    info!(drone_id, order_id = order.order_id, "Drone created");
                }

                // ── Notify the SAGA orchestrator: step 3 done ────────────────
                DroneDesire::PublishDroneAssigned { drone_id, order_id } => {
                    let event = SagaEvent::DroneAssigned {
                        saga_id: String::new(), // orchestrator correlates by order_id
                        order_id: order_id.clone(),
                        drone_id: drone_id.clone(),
                        timestamp: Utc::now(),
                    };
                    let payload = serde_json::to_vec(&event)?;
                    self.producer
                        .send(
                            FutureRecord::to(SAGA_EVENTS_TOPIC)
                                .key(&order_id)
                                .payload(&payload),
                            Timeout::After(Duration::from_secs(5)),
                        )
                        .await
                        .map_err(|(e, _)| anyhow::anyhow!("Kafka error: {e}"))?;
                    info!(drone_id, order_id, "DroneAssigned published");
                }

                // ── Store the Dispatched event + update beliefs ───────────────
                DroneDesire::StoreDispatchedEvent { drone_id, order_id } => {
                    let version = self.store.count_events_for_drone(&drone_id).await?;
                    self.store
                        .save_event(&DroneEvent::Dispatched {
                            drone_id: drone_id.clone(),
                            order_id: order_id.clone(),
                            dispatch_time: Utc::now(),
                            timestamp: Utc::now(),
                            version,
                        })
                        .await?;
                    // Belief revision: the drone now believes it is in flight.
                    self.beliefs = self.beliefs.clone().update_dispatched();
                    info!(drone_id, order_id, "Drone dispatched");
                }

                // ── Store the Delivered event ─────────────────────────────────
                DroneDesire::StoreDeliveredEvent { drone_id, order_id } => {
                    let version = self.store.count_events_for_drone(&drone_id).await?;
                    self.store
                        .save_event(&DroneEvent::Delivered {
                            drone_id: drone_id.clone(),
                            order_id: order_id.clone(),
                            delivery_time: Utc::now(),
                            timestamp: Utc::now(),
                            version,
                        })
                        .await?;
                    info!(drone_id, order_id, "Drone delivered package");
                }

                // ── Store the Returned event + reset beliefs ──────────────────
                DroneDesire::StoreReturnedEvent { drone_id, order_id } => {
                    let version = self.store.count_events_for_drone(&drone_id).await?;
                    self.store
                        .save_event(&DroneEvent::Returned {
                            drone_id: drone_id.clone(),
                            order_id: order_id.clone(),
                            return_time: Utc::now(),
                            timestamp: Utc::now(),
                            version,
                        })
                        .await?;
                    // Belief revision: drone is home, available again.
                    self.beliefs = self.beliefs.clone().update_returned();
                    info!(drone_id, order_id, "Drone returned to base");
                }

                // ── Generic saga event publish (e.g. DroneAssignmentFailed) ───
                DroneDesire::PublishSagaEvent(event) => {
                    // Extract order_id for the Kafka partition key.
                    let key = match &event {
                        SagaEvent::DroneAssignmentFailed { order_id, .. } => order_id.clone(),
                        _ => String::new(),
                    };
                    let payload = serde_json::to_vec(&event)?;
                    self.producer
                        .send(
                            FutureRecord::to(SAGA_EVENTS_TOPIC)
                                .key(&key)
                                .payload(&payload),
                            Timeout::After(Duration::from_secs(5)),
                        )
                        .await
                        .map_err(|(e, _)| anyhow::anyhow!("Kafka error: {e}"))?;
                }

                // ── Compensation: free the drone, reset beliefs ───────────────
                DroneDesire::RecordCompensation { drone_id } => {
                    // No event stored — compensation is a control signal, not a
                    // domain fact.  Just reset beliefs so the drone is available.
                    self.beliefs = self.beliefs.clone().update_compensated();
                    warn!(drone_id, "Drone compensated, returned to idle");
                }
            }
        }
        Ok(())
    }
}
