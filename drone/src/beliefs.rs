//! # Beliefs — the agent's model of the world
//!
//! In BDI, **beliefs** are everything the agent currently knows (or thinks it
//! knows) about itself and its environment.  They are updated when new
//! perceptions arrive and read during deliberation to decide what to do next.

use chrono::{DateTime, Utc};
use common::OrderMessage;

// ─────────────────────────────────────────────────────────────────────────────
// AgentPhase — the drone's belief about its own operational state
// ─────────────────────────────────────────────────────────────────────────────

/// What phase of operation does the drone agent *believe* itself to be in?
///
/// This is not the same as `DroneState` in `model.rs` (which is used by the
/// HTTP API and event store).  `AgentPhase` is the *live, in-memory* belief
/// the agent holds about itself right now.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentPhase {
    /// No active order — the drone is available for assignment.
    Idle,
    /// The drone has accepted an order and is en route to deliver it.
    ExecutingDelivery,
    /// Package delivered; drone is returning to base.
    Returning,
}

impl std::fmt::Display for AgentPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentPhase::Idle => write!(f, "Idle"),
            AgentPhase::ExecutingDelivery => write!(f, "ExecutingDelivery"),
            AgentPhase::Returning => write!(f, "Returning"),
        }
    }
}

pub const MAX_PAYLOAD_WEIGHT: f64 = 100.0;

#[derive(Debug, Clone)]
pub struct DroneBeliefs {
    pub drone_id: Option<String>,
    pub is_available: bool,
    pub current_order: Option<OrderMessage>,
    pub phase: AgentPhase,
    pub dispatch_time: Option<DateTime<Utc>>,
    pub expected_arrival_time: Option<DateTime<Utc>>,
    pub can_carry_payload: bool,
    pub can_meet_deadline: bool,
}

impl PartialEq for DroneBeliefs {
    fn eq(&self, other: &Self) -> bool {
        self.drone_id == other.drone_id
            && self.current_order == other.current_order
            && self.dispatch_time == other.dispatch_time
    }
}

impl DroneBeliefs {
    pub fn new() -> Self {
        Self {
            drone_id: None,
            is_available: true,
            current_order: None,
            phase: AgentPhase::Idle,
            dispatch_time: None,
            expected_arrival_time: None,
            can_carry_payload: false,
            can_meet_deadline: false,
        }
    }

    pub fn from_order(self, order: &OrderMessage, drone_id: Option<String>) -> Self {
        Self {
            drone_id: drone_id,
            is_available: false,
            current_order: Some(order.clone()),
            expected_arrival_time: Some(order.requested_delivery_time),
            can_carry_payload: order.package_weight <= MAX_PAYLOAD_WEIGHT,
            can_meet_deadline: order.max_delivery_time_minutes > 0,
            ..self
        }
    }

    pub fn update_dispatched(self) -> Self {
        Self {
            dispatch_time: Some(Utc::now()),
            phase: AgentPhase::ExecutingDelivery,
            ..self
        }
    }

    pub fn update_returned(self) -> Self {
        Self::new()
    }

    pub fn update_compensated(self) -> Self {
        Self::new()
    }

    pub fn has_arrived(&self) -> bool {
        self.phase == AgentPhase::ExecutingDelivery
            && Utc::now() >= self.expected_arrival_time.unwrap_or(Utc::now())
    }
}
