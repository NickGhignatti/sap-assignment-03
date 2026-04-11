use chrono::{DateTime, Utc};
use common::OrderMessage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DroneState {
    Sleeping,
    InTransit,
    Returning,
}

impl std::fmt::Display for DroneState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DroneState::Sleeping => write!(f, "Sleeping"),
            DroneState::InTransit => write!(f, "InTransit"),
            DroneState::Returning => write!(f, "Returning"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DroneEntry {
    pub drone_id: String,
    pub order: OrderMessage,
    pub state: DroneState,
    pub dispatch_time: Option<DateTime<Utc>>,
    pub delivery_time: Option<DateTime<Utc>>,
    pub expected_arrival: Option<DateTime<Utc>>,
}

impl DroneEntry {
    pub fn new(drone_id: String, order: OrderMessage) -> Self {
        Self {
            drone_id,
            order,
            state: DroneState::Sleeping,
            dispatch_time: None,
            delivery_time: None,
            expected_arrival: None,
        }
    }

    pub fn start(&mut self) {
        self.state = DroneState::InTransit;
        self.dispatch_time = Some(Utc::now());
    }

    pub fn end(&mut self) {
        self.state = DroneState::Returning;
        self.delivery_time = Some(Utc::now());
    }

    pub fn display(&self) -> String {
        let elapsed = self
            .dispatch_time
            .map(|t| {
                let secs = (Utc::now() - t).num_seconds();
                format!("{}s elapsed", secs)
            })
            .unwrap_or_else(|| "not dispatched".to_string());

        format!(
            "Drone {} {} from '{}' to '{}' — {}",
            self.drone_id, self.state, self.order.from_address, self.order.to_address, elapsed
        )
    }
}
