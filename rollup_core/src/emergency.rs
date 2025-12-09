use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use dashmap::DashMap;

/// Emergency pause functionality for circuit breaker and security
pub struct EmergencySystem {
    paused: Arc<AtomicBool>,
    pause_events: Arc<DashMap<u64, PauseEvent>>,
    event_counter: AtomicU64,
    circuit_breaker: Arc<CircuitBreaker>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PauseEvent {
    pub event_id: u64,
    pub action: PauseAction,
    pub reason: String,
    pub triggered_by: String,
    pub timestamp: u64,
    pub metadata: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PauseAction {
    Pause,
    Unpause,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: usize,
    pub success_threshold: usize,
    pub timeout_seconds: u64,
}

pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Arc<AtomicU64>, // 0 = Closed, 1 = Open, 2 = HalfOpen
    failure_count: Arc<AtomicU64>,
    success_count: Arc<AtomicU64>,
    last_failure_time: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,  // Normal operation
    Open,    // Circuit tripped, rejecting requests
    HalfOpen, // Testing if system recovered
}

impl EmergencySystem {
    pub fn new(circuit_breaker_config: CircuitBreakerConfig) -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            pause_events: Arc::new(DashMap::new()),
            event_counter: AtomicU64::new(0),
            circuit_breaker: Arc::new(CircuitBreaker::new(circuit_breaker_config)),
        }
    }

    /// Trigger emergency pause
    pub fn pause(&self, reason: String, triggered_by: String) -> Result<PauseEvent> {
        if self.is_paused() {
            return Err(anyhow!("System already paused"));
        }

        self.paused.store(true, Ordering::SeqCst);

        let event_id = self.event_counter.fetch_add(1, Ordering::SeqCst);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let event = PauseEvent {
            event_id,
            action: PauseAction::Pause,
            reason: reason.clone(),
            triggered_by: triggered_by.clone(),
            timestamp: now,
            metadata: String::new(),
        };

        self.pause_events.insert(event_id, event.clone());

        log::warn!(
            "EMERGENCY PAUSE activated by {} - Reason: {}",
            triggered_by,
            reason
        );

        Ok(event)
    }

    /// Resume from emergency pause
    pub fn unpause(&self, triggered_by: String) -> Result<PauseEvent> {
        if !self.is_paused() {
            return Err(anyhow!("System is not paused"));
        }

        self.paused.store(false, Ordering::SeqCst);

        let event_id = self.event_counter.fetch_add(1, Ordering::SeqCst);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let event = PauseEvent {
            event_id,
            action: PauseAction::Unpause,
            reason: "Manual unpause".to_string(),
            triggered_by: triggered_by.clone(),
            timestamp: now,
            metadata: String::new(),
        };

        self.pause_events.insert(event_id, event.clone());

        log::info!("System unpaused by {}", triggered_by);

        Ok(event)
    }

    /// Check if system is paused
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Check if operation is allowed (considering both pause and circuit breaker)
    pub fn is_operation_allowed(&self) -> Result<()> {
        if self.is_paused() {
            return Err(anyhow!("System is in emergency pause mode"));
        }

        if self.circuit_breaker.is_open() {
            return Err(anyhow!("Circuit breaker is open - system overloaded"));
        }

        Ok(())
    }

    /// Get circuit breaker state
    pub fn get_circuit_state(&self) -> CircuitState {
        self.circuit_breaker.get_state()
    }

    /// Record successful operation
    pub fn record_success(&self) {
        self.circuit_breaker.record_success();
    }

    /// Record failed operation
    pub fn record_failure(&self) {
        self.circuit_breaker.record_failure();
    }

    /// Get pause events
    pub fn get_pause_events(&self) -> Vec<PauseEvent> {
        self.pause_events.iter().map(|e| e.value().clone()).collect()
    }

    /// Get system status
    pub fn get_status(&self) -> EmergencyStatus {
        EmergencyStatus {
            is_paused: self.is_paused(),
            circuit_state: self.get_circuit_state(),
            total_pause_events: self.pause_events.len(),
            circuit_failure_count: self.circuit_breaker.failure_count.load(Ordering::Relaxed),
            circuit_success_count: self.circuit_breaker.success_count.load(Ordering::Relaxed),
        }
    }
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(AtomicU64::new(0)), // Closed
            failure_count: Arc::new(AtomicU64::new(0)),
            success_count: Arc::new(AtomicU64::new(0)),
            last_failure_time: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn record_success(&self) {
        self.success_count.fetch_add(1, Ordering::Relaxed);

        let state = self.get_state();

        if state == CircuitState::HalfOpen {
            let successes = self.success_count.load(Ordering::Relaxed);

            if successes >= self.config.success_threshold as u64 {
                // Close the circuit
                self.state.store(0, Ordering::SeqCst);
                self.failure_count.store(0, Ordering::Relaxed);
                self.success_count.store(0, Ordering::Relaxed);
                log::info!("Circuit breaker closed - system recovered");
            }
        }
    }

    pub fn record_failure(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.failure_count.fetch_add(1, Ordering::Relaxed);
        self.last_failure_time.store(now, Ordering::Relaxed);

        let failures = self.failure_count.load(Ordering::Relaxed);

        if failures >= self.config.failure_threshold as u64 {
            // Open the circuit
            self.state.store(1, Ordering::SeqCst);
            log::warn!("Circuit breaker opened - too many failures ({})", failures);
        }
    }

    pub fn get_state(&self) -> CircuitState {
        let state_val = self.state.load(Ordering::Relaxed);

        match state_val {
            0 => CircuitState::Closed,
            1 => {
                // Check if timeout has passed
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                let last_failure = self.last_failure_time.load(Ordering::Relaxed);

                if now - last_failure >= self.config.timeout_seconds {
                    // Move to half-open
                    self.state.store(2, Ordering::SeqCst);
                    self.success_count.store(0, Ordering::Relaxed);
                    log::info!("Circuit breaker half-open - testing recovery");
                    CircuitState::HalfOpen
                } else {
                    CircuitState::Open
                }
            }
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        }
    }

    pub fn is_open(&self) -> bool {
        self.get_state() == CircuitState::Open
    }

    pub fn reset(&self) {
        self.state.store(0, Ordering::SeqCst);
        self.failure_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
        log::info!("Circuit breaker manually reset");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyStatus {
    pub is_paused: bool,
    pub circuit_state: CircuitState,
    pub total_pause_events: usize,
    pub circuit_failure_count: u64,
    pub circuit_success_count: u64,
}

impl Default for EmergencySystem {
    fn default() -> Self {
        Self::new(CircuitBreakerConfig {
            failure_threshold: 10,
            success_threshold: 5,
            timeout_seconds: 60,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emergency_pause() {
        let system = EmergencySystem::default();

        assert!(!system.is_paused());

        system
            .pause("Test pause".to_string(), "admin".to_string())
            .unwrap();

        assert!(system.is_paused());

        let result = system.is_operation_allowed();
        assert!(result.is_err());

        system.unpause("admin".to_string()).unwrap();

        assert!(!system.is_paused());
    }

    #[test]
    fn test_circuit_breaker() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            timeout_seconds: 1,
        };

        let system = EmergencySystem::new(config);

        assert_eq!(system.get_circuit_state(), CircuitState::Closed);

        // Record failures
        for _ in 0..3 {
            system.record_failure();
        }

        assert_eq!(system.get_circuit_state(), CircuitState::Open);

        // Wait for timeout
        std::thread::sleep(std::time::Duration::from_secs(2));

        assert_eq!(system.get_circuit_state(), CircuitState::HalfOpen);

        // Record successes
        for _ in 0..2 {
            system.record_success();
        }

        assert_eq!(system.get_circuit_state(), CircuitState::Closed);
    }

    #[test]
    fn test_pause_events() {
        let system = EmergencySystem::default();

        system
            .pause("Reason 1".to_string(), "admin1".to_string())
            .unwrap();

        system.unpause("admin2".to_string()).unwrap();

        let events = system.get_pause_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, PauseAction::Pause);
        assert_eq!(events[1].action, PauseAction::Unpause);
    }
}
