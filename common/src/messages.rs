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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample(order_id: &str) -> OrderMessage {
        OrderMessage::new(order_id, "cust", "from", "to", 2.5, Utc::now(), 30)
    }

    #[test]
    fn equality_is_based_only_on_order_id() {
        let a = sample("same");
        let mut b = sample("same");
        // Different in every field except the order_id...
        b.customer_id = "different".into();
        b.package_weight = 99.0;
        assert_eq!(a, b); // ...still equal: equality is keyed on order_id only.

        let c = sample("other");
        assert_ne!(a, c);
    }

    #[test]
    fn serde_round_trip_preserves_fields() {
        let original = sample("o-1");
        let json = serde_json::to_string(&original).unwrap();
        let decoded: OrderMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.order_id, "o-1");
        assert_eq!(decoded.customer_id, "cust");
        assert_eq!(decoded.package_weight, 2.5);
        assert_eq!(decoded.max_delivery_time_minutes, 30);
    }
}
