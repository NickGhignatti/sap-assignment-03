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

    // Order details in order facilitating the rebuild in case of failure
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
            if next == SagaStep::Completed {
                self.status = SagaStatus::Completed;
                self.end_time = Some(Utc::now());
            } else {
                self.status = SagaStatus::InProgress;
            }
            self.current_step = next;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn new_saga() -> SagaState {
        SagaState::new(
            "saga-1".into(),
            "order-1".into(),
            "cust".into(),
            "from".into(),
            "to".into(),
            2.5,
            Utc::now(),
            30,
        )
    }

    #[test]
    fn starts_in_started_state_at_validation_step() {
        let saga = new_saga();
        assert_eq!(saga.status, SagaStatus::Started);
        assert_eq!(saga.current_step, SagaStep::OrderValidation);
        assert!(saga.completed_steps.is_empty());
        assert!(saga.end_time.is_none());
    }

    #[test]
    fn step_sequence_follows_next() {
        assert_eq!(
            SagaStep::OrderValidation.next(),
            Some(SagaStep::DeliveryScheduling)
        );
        assert_eq!(
            SagaStep::DeliveryScheduling.next(),
            Some(SagaStep::DroneAssignment)
        );
        assert_eq!(
            SagaStep::DroneAssignment.next(),
            Some(SagaStep::Completed)
        );
        assert_eq!(SagaStep::Completed.next(), None);
    }

    #[test]
    fn marking_all_steps_completes_the_saga() {
        let mut saga = new_saga();

        saga.mark_step_completed(SagaStep::OrderValidation);
        assert_eq!(saga.status, SagaStatus::InProgress);
        assert_eq!(saga.current_step, SagaStep::DeliveryScheduling);

        saga.mark_step_completed(SagaStep::DeliveryScheduling);
        assert_eq!(saga.status, SagaStatus::InProgress);
        assert_eq!(saga.current_step, SagaStep::DroneAssignment);

        saga.mark_step_completed(SagaStep::DroneAssignment);
        assert_eq!(saga.status, SagaStatus::Completed);
        assert_eq!(saga.current_step, SagaStep::Completed);
        assert!(saga.end_time.is_some());
        assert_eq!(saga.completed_steps.len(), 3);
    }

    #[test]
    fn compensation_runs_in_reverse_completion_order() {
        let mut saga = new_saga();
        saga.mark_step_completed(SagaStep::OrderValidation);
        saga.mark_step_completed(SagaStep::DeliveryScheduling);
        // Completed in order [OrderValidation, DeliveryScheduling]
        // → compensate in reverse: [DeliveryScheduling, OrderValidation]
        assert_eq!(
            saga.steps_to_compensate(),
            vec![SagaStep::DeliveryScheduling, SagaStep::OrderValidation]
        );
    }

    #[test]
    fn needs_compensation_only_when_failed_with_completed_steps() {
        let mut with_steps = new_saga();
        with_steps.mark_step_completed(SagaStep::OrderValidation);
        with_steps.mark_failed("boom");
        assert!(with_steps.needs_compensation());

        // A validation failure with no completed steps needs no compensation.
        let mut no_steps = new_saga();
        no_steps.mark_failed("invalid input");
        assert!(!no_steps.needs_compensation());
    }
}
