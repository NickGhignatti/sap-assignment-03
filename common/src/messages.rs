use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderMessage {
    pub order_id: String,
    pub customer_id: String,
    pub from_address: String,
    pub to_address: String,
    pub package_weight: f64,
    pub requested_delivery_time: DateTime<Utc>,
    pub max_delivery_time_minutes: i32,
}

impl OrderMessage {
    pub fn new(
        order_id: impl Into<String>,
        customer_id: impl Into<String>,
        from_address: impl Into<String>,
        to_address: impl Into<String>,
        package_weight: f64,
        requested_delivery_time: DateTime<Utc>,
        max_delivery_time_minutes: i32,
    ) -> Self {
        Self {
            order_id: order_id.into(),
            customer_id: customer_id.into(),
            from_address: from_address.into(),
            to_address: to_address.into(),
            package_weight,
            requested_delivery_time,
            max_delivery_time_minutes,
        }
    }
}

/// Comparison based only on the orderId, if other fields are needed add
impl PartialEq for OrderMessage {
    fn eq(&self, other: &Self) -> bool {
        self.order_id == other.order_id
    }
}

impl std::fmt::Display for OrderMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OrderMessage {{ order_id: '{}', customer_id: '{}', from: '{}', to: '{}', weight: {}, max_time: {} min }}",
            self.order_id,
            self.customer_id,
            self.from_address,
            self.to_address,
            self.package_weight,
            self.max_delivery_time_minutes
        )
    }
}
