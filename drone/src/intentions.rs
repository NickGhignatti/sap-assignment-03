use chrono::Utc;
use common::{OrderMessage, SagaEvent};

#[derive(Debug, Clone)]
pub enum DroneIntention {
    AcceptDelivery {
        order: OrderMessage,
        drone_id: String,
    },
    RefuseDelivery {
        order: OrderMessage,
        reason: String,
    },
    CompleteDelivery {
        order_id: String,
        drone_id: String,
    },
    Compensate {
        drone_id: String,
    },
    Idle,
}

/// Atomic steps the agent can execute.
#[derive(Debug, Clone)]
pub enum DroneDesire {
    StoreCreatedEvent {
        drone_id: String,
        order: OrderMessage,
    },
    PublishDroneAssigned {
        drone_id: String,
        order_id: String,
    },
    StoreDispatchedEvent {
        drone_id: String,
        order_id: String,
    },
    StoreDeliveredEvent {
        drone_id: String,
        order_id: String,
    },
    StoreReturnedEvent {
        drone_id: String,
        order_id: String,
    },
    PublishSagaEvent(SagaEvent),
    RecordCompensation {
        drone_id: String,
    },
}

/// Map each intention to its specific sequence of actions.
pub fn plan_for(goal: DroneIntention) -> Vec<DroneDesire> {
    match goal {
        DroneIntention::AcceptDelivery { order, drone_id } => vec![
            DroneDesire::StoreCreatedEvent {
                drone_id: drone_id.clone(),
                order: order.clone(),
            },
            DroneDesire::PublishDroneAssigned {
                drone_id: drone_id.clone(),
                order_id: order.order_id.clone(),
            },
            DroneDesire::StoreDispatchedEvent {
                drone_id,
                order_id: order.order_id,
            },
        ],
        DroneIntention::RefuseDelivery { order, reason } => {
            // the agent publishes a DroneAssignmentFailed saga event
            vec![DroneDesire::PublishSagaEvent(
                SagaEvent::DroneAssignmentFailed {
                    saga_id: uuid::Uuid::new_v4().to_string(),
                    order_id: order.order_id.clone(),
                    reason,
                    timestamp: Utc::now(),
                },
            )]
        }
        DroneIntention::CompleteDelivery { drone_id, order_id } => {
            // two store actions: Delivered then Returned
            vec![
                DroneDesire::StoreDeliveredEvent {
                    drone_id: drone_id.clone(),
                    order_id: order_id.clone(),
                },
                DroneDesire::StoreReturnedEvent {
                    drone_id: drone_id.clone(),
                    order_id: order_id.clone(),
                },
            ]
        }
        DroneIntention::Compensate { drone_id } => {
            // one action: RecordCompensation
            vec![DroneDesire::RecordCompensation { drone_id }]
        }
        DroneIntention::Idle => vec![],
    }
}
