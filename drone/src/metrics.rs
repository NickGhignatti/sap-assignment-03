//! Metrics for the drone-assignment success SLI.
//!
//! The agent's `deliberate()` decides to either accept or refuse a delivery.
//! We count both outcomes; the SLI is `assigned / (assigned + refused)`.

use std::sync::Arc;

use prometheus::{IntCounter, Registry};

pub struct DroneMetrics {
    /// Deliveries the fleet accepted (a drone was assigned).
    pub assigned: IntCounter,
    /// Deliveries the fleet refused (payload too heavy, deadline unmeetable,
    /// or no drone available).
    pub refused: IntCounter,
}

impl DroneMetrics {
    pub fn new(registry: &Registry) -> Arc<Self> {
        let assigned = IntCounter::new(
            "drone_assignment_assigned_total",
            "Number of deliveries accepted and assigned to a drone",
        )
        .unwrap();
        registry.register(Box::new(assigned.clone())).unwrap();

        let refused = IntCounter::new(
            "drone_assignment_refused_total",
            "Number of deliveries refused by the fleet",
        )
        .unwrap();
        registry.register(Box::new(refused.clone())).unwrap();

        Arc::new(Self { assigned, refused })
    }
}
