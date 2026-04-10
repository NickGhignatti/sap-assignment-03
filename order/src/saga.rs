use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SagaStatus {
    Started,
    InProgress,
    Completed,
    Failed,
    Compensating,
    Compensated,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SagaStep {
    OrderValidation,
    DeliveryScheduling,
    DroneAssignment,
    Completed,
}

impl SagaStep {
    pub fn next(&self) -> Option<SagaStep> {
        match self {
            SagaStep::OrderValidation => Some(SagaStep::DeliveryScheduling),
            SagaStep::DeliveryScheduling => Some(SagaStep::DroneAssignment),
            SagaStep::DroneAssignment => Some(SagaStep::Completed),
            SagaStep::Completed => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SagaState {
    /// mapping with the MongoDB one
    #[serde(rename = "_id")]
    pub saga_id: String,
    pub order_id: String,
    pub status: SagaStatus,
    pub current_step: SagaStep,
    pub completed_steps: Vec<SagaStep>,
    /// Valid only in case of SAGA failure
    pub failure_reason: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,

    // Order details in order failictating the rebuild in case of failure
    pub customer_id: String,
    pub from_address: String,
    pub to_address: String,
    pub package_weight: f64,
    pub requested_delivery_time: DateTime<Utc>,
    pub max_delivery_time_minutes: i32,

    /// `None` until step 2 is completed
    pub delivery_id: Option<String>,
    /// `None` until step 3 is completed
    pub drone_id: Option<String>,
}

impl SagaState {
    pub fn new(
        saga_id: String,
        order_id: String,
        customer_id: String,
        from_address: String,
        to_address: String,
        package_weight: f64,
        requested_delivery_time: DateTime<Utc>,
        max_delivery_time_minutes: i32,
    ) -> Self {
        Self {
            saga_id,
            order_id,
            status: SagaStatus::Started,
            current_step: SagaStep::OrderValidation,
            completed_steps: Vec::new(),
            failure_reason: None,
            start_time: Utc::now(),
            end_time: None,
            customer_id,
            from_address,
            to_address,
            package_weight,
            requested_delivery_time,
            max_delivery_time_minutes,
            delivery_id: None,
            drone_id: None,
        }
    }

    pub fn mark_step_completed(&mut self, step: SagaStep) {
        self.completed_steps.push(step);
        if let Some(next) = self.current_step.next() {
            self.current_step = next.clone();
            if next == SagaStep::Completed {
                self.status = SagaStatus::Completed;
                self.end_time = Some(Utc::now());
            } else {
                self.status = SagaStatus::InProgress;
            }
        }
    }

    pub fn mark_failed(&mut self, reason: impl Into<String>) {
        self.status = SagaStatus::Failed;
        self.failure_reason = Some(reason.into());
        self.end_time = Some(Utc::now());
    }

    pub fn start_compensation(&mut self) {
        self.status = SagaStatus::Compensating;
    }

    pub fn mark_compensated(&mut self) {
        self.status = SagaStatus::Compensated;
        self.end_time = Some(Utc::now());
    }

    pub fn steps_to_compensate(&self) -> Vec<SagaStep> {
        let mut steps = self.completed_steps.clone();
        steps.sort();
        steps.reverse(); // Reverse order compensation (Drone -> Delivery -> Order)
        steps
    }

    pub fn needs_compensation(&self) -> bool {
        self.status == SagaStatus::Failed && !self.completed_steps.is_empty()
    }
}
