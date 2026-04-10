use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `#[serde(tag = "type")]` add the field to the JSON
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SagaEvent {
    #[serde(rename = "ORDER_SAGA_STARTED")]
    OrderSagaStarted {
        saga_id: String,
        order_id: String,
        customer_id: String,
        from_address: String,
        to_address: String,
        package_weight: f64,
        requested_delivery_time: DateTime<Utc>,
        max_delivery_time_minutes: i32,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "ORDER_VALIDATED")]
    OrderValidated {
        saga_id: String,
        order_id: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "DELIVERY_SCHEDULED")]
    DeliveryScheduled {
        saga_id: String,
        order_id: String,
        delivery_id: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "DRONE_ASSIGNED")]
    DroneAssigned {
        saga_id: String,
        order_id: String,
        drone_id: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "ORDER_COMPLETED")]
    OrderCompleted {
        saga_id: String,
        order_id: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "ORDER_VALIDATION_FAILED")]
    OrderValidationFailed {
        saga_id: String,
        order_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "DELIVERY_SCHEDULING_FAILED")]
    DeliverySchedulingFailed {
        saga_id: String,
        order_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "DRONE_ASSIGNMENT_FAILED")]
    DroneAssignmentFailed {
        saga_id: String,
        order_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "ORDER_CANCELLED")]
    OrderCancelled {
        saga_id: String,
        order_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "COMPENSATE_ORDER")]
    CompensateOrder {
        saga_id: String,
        order_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "COMPENSATE_DELIVERY")]
    CompensateDelivery {
        saga_id: String,
        order_id: String,
        delivery_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },

    #[serde(rename = "COMPENSATE_DRONE")]
    CompensateDrone {
        saga_id: String,
        order_id: String,
        drone_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
}

impl SagaEvent {
    pub fn order_id(&self) -> &str {
        match self {
            SagaEvent::OrderSagaStarted { order_id, .. }
            | SagaEvent::OrderValidated { order_id, .. }
            | SagaEvent::DeliveryScheduled { order_id, .. }
            | SagaEvent::DroneAssigned { order_id, .. }
            | SagaEvent::OrderCompleted { order_id, .. }
            | SagaEvent::OrderValidationFailed { order_id, .. }
            | SagaEvent::DeliverySchedulingFailed { order_id, .. }
            | SagaEvent::DroneAssignmentFailed { order_id, .. }
            | SagaEvent::OrderCancelled { order_id, .. }
            | SagaEvent::CompensateOrder { order_id, .. }
            | SagaEvent::CompensateDelivery { order_id, .. }
            | SagaEvent::CompensateDrone { order_id, .. } => order_id,
        }
    }

    pub fn saga_id(&self) -> &str {
        match self {
            SagaEvent::OrderSagaStarted { saga_id, .. }
            | SagaEvent::OrderValidated { saga_id, .. }
            | SagaEvent::DeliveryScheduled { saga_id, .. }
            | SagaEvent::DroneAssigned { saga_id, .. }
            | SagaEvent::OrderCompleted { saga_id, .. }
            | SagaEvent::OrderValidationFailed { saga_id, .. }
            | SagaEvent::DeliverySchedulingFailed { saga_id, .. }
            | SagaEvent::DroneAssignmentFailed { saga_id, .. }
            | SagaEvent::OrderCancelled { saga_id, .. }
            | SagaEvent::CompensateOrder { saga_id, .. }
            | SagaEvent::CompensateDelivery { saga_id, .. }
            | SagaEvent::CompensateDrone { saga_id, .. } => saga_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DroneEvent {
    #[serde(rename = "DRONE_CREATED")]
    Created {
        drone_id: String,
        order_id: String,
        customer_id: String,
        from_address: String,
        to_address: String,
        package_weight: f64,
        requested_delivery_time: DateTime<Utc>,
        max_delivery_time_minutes: i32,
        timestamp: DateTime<Utc>,
        version: u64,
    },

    #[serde(rename = "DRONE_DISPATCHED")]
    Dispatched {
        drone_id: String,
        order_id: String,
        dispatch_time: DateTime<Utc>,
        timestamp: DateTime<Utc>,
        version: u64,
    },

    #[serde(rename = "DRONE_DELIVERED")]
    Delivered {
        drone_id: String,
        order_id: String,
        delivery_time: DateTime<Utc>,
        timestamp: DateTime<Utc>,
        version: u64,
    },

    #[serde(rename = "DRONE_RETURNED")]
    Returned {
        drone_id: String,
        order_id: String,
        return_time: DateTime<Utc>,
        timestamp: DateTime<Utc>,
        version: u64,
    },
}

impl DroneEvent {
    pub fn drone_id(&self) -> &str {
        match self {
            DroneEvent::Created { drone_id, .. }
            | DroneEvent::Dispatched { drone_id, .. }
            | DroneEvent::Delivered { drone_id, .. }
            | DroneEvent::Returned { drone_id, .. } => drone_id,
        }
    }

    pub fn order_id(&self) -> &str {
        match self {
            DroneEvent::Created { order_id, .. }
            | DroneEvent::Dispatched { order_id, .. }
            | DroneEvent::Delivered { order_id, .. }
            | DroneEvent::Returned { order_id, .. } => order_id,
        }
    }

    pub fn version(&self) -> u64 {
        match self {
            DroneEvent::Created { version, .. }
            | DroneEvent::Dispatched { version, .. }
            | DroneEvent::Delivered { version, .. }
            | DroneEvent::Returned { version, .. } => *version,
        }
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            DroneEvent::Created { .. } => "DRONE_CREATED",
            DroneEvent::Dispatched { .. } => "DRONE_DISPATCHED",
            DroneEvent::Delivered { .. } => "DRONE_DELIVERED",
            DroneEvent::Returned { .. } => "DRONE_RETURNED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checking if type is present in the serialization
    #[test]
    fn saga_event_serializes_with_type_field() {
        let event = SagaEvent::OrderValidated {
            saga_id: "s1".into(),
            order_id: "o1".into(),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ORDER_VALIDATED\""));
        assert!(json.contains("\"order_id\":\"o1\""));
    }

    #[test]
    fn drone_event_round_trip() {
        let event = DroneEvent::Created {
            drone_id: "d1".into(),
            order_id: "o1".into(),
            customer_id: "c1".into(),
            from_address: "Via Roma 1".into(),
            to_address: "Via Milano 2".into(),
            package_weight: 2.5,
            requested_delivery_time: Utc::now(),
            max_delivery_time_minutes: 60,
            timestamp: Utc::now(),
            version: 0,
        };

        let json = serde_json::to_string(&event).unwrap();
        let decoded: DroneEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.drone_id(), "d1");
        assert_eq!(decoded.event_type(), "DRONE_CREATED");
    }
}
