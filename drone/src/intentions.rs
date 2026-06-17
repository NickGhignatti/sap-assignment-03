use chrono::Utc;
use common::{OrderMessage, SagaEvent};

use crate::beliefs::DroneBeliefs;

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

/// Pure deliberation: read the belief base and select the active intention.
///
/// Extracted from `DroneAgent::deliberate` so the decision logic can be
/// unit-tested in isolation, with no Kafka producer or MongoDB store. This is
/// the BDI "reasoning" step — a guard chain over the agent's beliefs — kept
/// separate from "acting" (`plan_for` + `execute`).
pub fn deliberate(beliefs: &DroneBeliefs) -> DroneIntention {
    match &beliefs.current_order {
        None => DroneIntention::Idle,
        Some(order) => {
            let drone_id = beliefs.drone_id.clone().unwrap_or_default();

            if !beliefs.can_carry_payload {
                DroneIntention::RefuseDelivery {
                    order: order.clone(),
                    reason: format!("{}kg exceeds max payload", order.package_weight),
                }
            } else if !beliefs.can_meet_deadline {
                DroneIntention::RefuseDelivery {
                    order: order.clone(),
                    reason: "Deadline cannot be met".into(),
                }
            } else {
                DroneIntention::AcceptDelivery {
                    order: order.clone(),
                    drone_id,
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beliefs::DroneBeliefs;
    use chrono::Utc;
    use common::OrderMessage;

    fn order(weight: f64, max_minutes: i32) -> OrderMessage {
        OrderMessage::new("o-1", "c-1", "from", "to", weight, Utc::now(), max_minutes)
    }

    // ── deliberate (BDI reasoning) ──────────────────────────────────────────
    #[test]
    fn deliberate_is_idle_without_an_order() {
        assert!(matches!(
            deliberate(&DroneBeliefs::new()),
            DroneIntention::Idle
        ));
    }

    #[test]
    fn deliberate_accepts_a_feasible_order() {
        let beliefs = DroneBeliefs::new().from_order(&order(10.0, 30), Some("d-1".into()));
        assert!(matches!(
            deliberate(&beliefs),
            DroneIntention::AcceptDelivery { .. }
        ));
    }

    #[test]
    fn deliberate_refuses_an_overweight_order() {
        let beliefs = DroneBeliefs::new().from_order(&order(150.0, 30), Some("d-1".into()));
        assert!(matches!(
            deliberate(&beliefs),
            DroneIntention::RefuseDelivery { .. }
        ));
    }

    #[test]
    fn deliberate_refuses_when_deadline_unfeasible() {
        let beliefs = DroneBeliefs::new().from_order(&order(10.0, 0), Some("d-1".into()));
        assert!(matches!(
            deliberate(&beliefs),
            DroneIntention::RefuseDelivery { .. }
        ));
    }

    // ── plan_for (intention → plan) ─────────────────────────────────────────
    #[test]
    fn plan_for_accept_stores_then_publishes_then_dispatches() {
        let plan = plan_for(DroneIntention::AcceptDelivery {
            order: order(10.0, 30),
            drone_id: "d-1".into(),
        });
        assert_eq!(plan.len(), 3);
        assert!(matches!(plan[0], DroneDesire::StoreCreatedEvent { .. }));
        assert!(matches!(plan[1], DroneDesire::PublishDroneAssigned { .. }));
        assert!(matches!(plan[2], DroneDesire::StoreDispatchedEvent { .. }));
    }

    #[test]
    fn plan_for_refuse_publishes_assignment_failed() {
        let plan = plan_for(DroneIntention::RefuseDelivery {
            order: order(150.0, 30),
            reason: "too heavy".into(),
        });
        assert_eq!(plan.len(), 1);
        assert!(matches!(
            plan[0],
            DroneDesire::PublishSagaEvent(SagaEvent::DroneAssignmentFailed { .. })
        ));
    }

    #[test]
    fn plan_for_complete_stores_delivered_then_returned() {
        let plan = plan_for(DroneIntention::CompleteDelivery {
            order_id: "o-1".into(),
            drone_id: "d-1".into(),
        });
        assert_eq!(plan.len(), 2);
        assert!(matches!(plan[0], DroneDesire::StoreDeliveredEvent { .. }));
        assert!(matches!(plan[1], DroneDesire::StoreReturnedEvent { .. }));
    }

    #[test]
    fn plan_for_compensate_records_compensation() {
        let plan = plan_for(DroneIntention::Compensate {
            drone_id: "d-1".into(),
        });
        assert_eq!(plan.len(), 1);
        assert!(matches!(plan[0], DroneDesire::RecordCompensation { .. }));
    }

    #[test]
    fn plan_for_idle_is_empty() {
        assert!(plan_for(DroneIntention::Idle).is_empty());
    }
}
