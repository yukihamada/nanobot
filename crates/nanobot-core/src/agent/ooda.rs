//! OODA Loop framework for autonomous coding
//!
//! OODA = Observe → Orient → Decide → Act
//! A decision-making cycle for autonomous agents.

use std::collections::HashMap;
use tracing::{info, warn};

/// OODA Loop state for autonomous task execution
#[derive(Debug, Clone)]
pub struct OodaLoop {
    /// Current phase in the loop
    pub phase: OodaPhase,
    /// Observations collected
    pub observations: Vec<Observation>,
    /// Situational assessment
    pub orientation: Option<Orientation>,
    /// Decision made
    pub decision: Option<Decision>,
    /// Actions taken
    pub actions: Vec<Action>,
    /// Loop iteration count
    pub iteration: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OodaPhase {
    Observe,
    Orient,
    Decide,
    Act,
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub source: String,
    pub data: String,
    pub timestamp: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct Orientation {
    pub situation: String,
    pub constraints: Vec<String>,
    pub opportunities: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub goal: String,
    pub strategy: String,
    pub steps: Vec<String>,
    pub tools_needed: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub tool: String,
    pub params: HashMap<String, serde_json::Value>,
    pub result: Option<String>,
    pub success: bool,
}

impl OodaLoop {
    pub fn new() -> Self {
        Self {
            phase: OodaPhase::Observe,
            observations: Vec::new(),
            orientation: None,
            decision: None,
            actions: Vec::new(),
            iteration: 0,
        }
    }

    /// Add an observation
    pub fn observe(&mut self, source: &str, data: &str) {
        info!("👁️ OBSERVE: {} - {}", source, data);
        self.observations.push(Observation {
            source: source.to_string(),
            data: data.to_string(),
            timestamp: std::time::Instant::now(),
        });
    }

    /// Assess the situation (Orient phase)
    pub fn orient(&mut self, situation: &str) {
        info!("🧭 ORIENT: {}", situation);
        self.phase = OodaPhase::Orient;
        self.orientation = Some(Orientation {
            situation: situation.to_string(),
            constraints: Vec::new(),
            opportunities: Vec::new(),
            risks: Vec::new(),
        });
    }

    /// Make a decision (Decide phase)
    pub fn decide(&mut self, goal: &str, strategy: &str, steps: Vec<String>) {
        info!("🎯 DECIDE: Goal={}, Strategy={}", goal, strategy);
        self.phase = OodaPhase::Decide;
        self.decision = Some(Decision {
            goal: goal.to_string(),
            strategy: strategy.to_string(),
            steps,
            tools_needed: Vec::new(),
        });
    }

    /// Execute an action (Act phase)
    pub fn act(&mut self, tool: &str, params: HashMap<String, serde_json::Value>) {
        info!("⚡ ACT: Executing {}", tool);
        self.phase = OodaPhase::Act;
        self.actions.push(Action {
            tool: tool.to_string(),
            params,
            result: None,
            success: false,
        });
    }

    /// Record action result
    pub fn record_result(&mut self, result: &str, success: bool) {
        if let Some(action) = self.actions.last_mut() {
            action.result = Some(result.to_string());
            action.success = success;
        }
    }

    /// Complete one loop iteration and reset for next cycle
    pub fn complete_iteration(&mut self) {
        self.iteration += 1;
        info!("🔄 OODA Loop iteration {} completed", self.iteration);
        self.phase = OodaPhase::Observe;
        // Keep observations for context, clear orientation/decision
        self.orientation = None;
        self.decision = None;
    }

    /// Check if current iteration is successful
    pub fn is_successful(&self) -> bool {
        !self.actions.is_empty() && self.actions.iter().all(|a| a.success)
    }

    /// Get summary of current state
    pub fn summary(&self) -> String {
        format!(
            "OODA Loop Iteration {}: Phase={:?}, Observations={}, Actions={}",
            self.iteration,
            self.phase,
            self.observations.len(),
            self.actions.len()
        )
    }
}

impl Default for OodaLoop {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ooda_loop_phases() {
        let mut ooda = OodaLoop::new();
        assert_eq!(ooda.phase, OodaPhase::Observe);

        ooda.observe("git_status", "Working tree clean");
        assert_eq!(ooda.observations.len(), 1);

        ooda.orient("Need to implement new feature");
        assert_eq!(ooda.phase, OodaPhase::Orient);
        assert!(ooda.orientation.is_some());

        ooda.decide("Add login feature", "TDD approach", vec![
            "Write tests".to_string(),
            "Implement logic".to_string(),
        ]);
        assert_eq!(ooda.phase, OodaPhase::Decide);
        assert!(ooda.decision.is_some());

        let mut params = HashMap::new();
        params.insert("language".to_string(), serde_json::json!("rust"));
        ooda.act("run_tests", params);
        assert_eq!(ooda.phase, OodaPhase::Act);
        assert_eq!(ooda.actions.len(), 1);

        ooda.record_result("Tests passed", true);
        assert!(ooda.actions[0].success);

        ooda.complete_iteration();
        assert_eq!(ooda.iteration, 1);
        assert_eq!(ooda.phase, OodaPhase::Observe);
    }
}
