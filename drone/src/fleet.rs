use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use common::OrderMessage;
use rdkafka::producer::FutureProducer;
use tracing::info;

use crate::{
    agent::{Agent, DroneAgent},
    intentions::DroneIntention,
    metrics::DroneMetrics,
    store::DroneEventStore,
};

/// Selects which agent in the pool should handle the next order.
/// Returns the index into the agents slice, or None if no agent is available.
pub trait AssignmentStrategy: Send + Sync {
    fn select(&self, agents: &[DroneAgent]) -> Option<usize>;
}

/// Always picks the first available (idle) agent.
pub struct FirstAvailableStrategy;

impl AssignmentStrategy for FirstAvailableStrategy {
    fn select(&self, agents: &[DroneAgent]) -> Option<usize> {
        agents.iter().position(|a| a.beliefs.is_available)
    }
}

/// Cycles through agents in order — spreads load evenly.
pub struct RoundRobinStrategy {
    next: AtomicUsize,
}

impl RoundRobinStrategy {
    pub fn new() -> Self {
        Self {
            next: AtomicUsize::new(0),
        }
    }
}

impl AssignmentStrategy for RoundRobinStrategy {
    fn select(&self, agents: &[DroneAgent]) -> Option<usize> {
        let len = agents.len();
        for i in 0..len {
            let idx = (self.next.fetch_add(1, Ordering::Relaxed) + i) % len;
            if agents[idx].beliefs.is_available {
                return Some(idx);
            }
        }
        None // all drones busy
    }
}

pub struct DroneFleet {
    agents: Vec<DroneAgent>,
    strategy: Box<dyn AssignmentStrategy>,
    metrics: Arc<DroneMetrics>,
}

impl DroneFleet {
    pub fn new(
        size: usize,
        store: Arc<DroneEventStore>,
        producer: Arc<FutureProducer>,
        strategy: Box<dyn AssignmentStrategy>,
        metrics: Arc<DroneMetrics>,
    ) -> Self {
        let agents = (0..size)
            .map(|_| DroneAgent::new(Arc::clone(&store), Arc::clone(&producer)))
            .collect();
        info!(size, "DroneFleet initialised");
        Self {
            agents,
            strategy,
            metrics,
        }
    }

    pub async fn dispatch_order(&mut self, order: OrderMessage) -> Result<()> {
        let Some(idx) = self.strategy.select(&self.agents) else {
            // No idle drone = the fleet cannot serve this delivery → a refusal.
            self.metrics.refused.inc();
            return Err(anyhow::anyhow!("No available drones"));
        };

        self.agents[idx].update_beliefs(order);
        let goal = self.agents[idx].deliberate();

        // SLI #4: count the assignment outcome decided by the agent.
        match &goal {
            DroneIntention::AcceptDelivery { .. } => self.metrics.assigned.inc(),
            DroneIntention::RefuseDelivery { .. } => self.metrics.refused.inc(),
            _ => {}
        }

        let plan = self.agents[idx].plan(goal);
        self.agents[idx].execute(plan).await?;
        Ok(())
    }

    pub async fn check_arrivals(&mut self) -> Result<()> {
        for agent in &mut self.agents {
            if agent.beliefs.has_arrived() {
                if let (Some(drone_id), Some(order_id)) = (
                    agent.beliefs.drone_id.clone(),
                    agent
                        .beliefs
                        .current_order
                        .as_ref()
                        .map(|o| o.order_id.clone()),
                ) {
                    let intention = DroneIntention::CompleteDelivery { drone_id, order_id };
                    let plan = agent.plan(intention);
                    agent.execute(plan).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn compensate(&mut self, drone_id: String) -> Result<()> {
        for agent in &mut self.agents {
            if agent.beliefs.drone_id.clone().unwrap_or_default() == drone_id.clone() {
                let intention = DroneIntention::Compensate {
                    drone_id: drone_id.clone(),
                };
                let plan = agent.plan(intention);
                agent.execute(plan).await?;
            }
        }
        Ok(())
    }
}
