use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    pub name: String,
    state: CircuitState,
    failure_count: u32,
    success_count_in_half_open: u32,
    last_failure: Option<Instant>,
    failure_threshold: u32,
    recovery_timeout: Duration,
    success_threshold: u32,
    // Adaptive timeout
    latencies: VecDeque<u64>, // last N latencies in ms
    base_timeout: Duration,
    current_timeout: Duration,
    max_timeout: Duration,
    consecutive_timeouts: u32,
}

impl CircuitBreaker {
    pub fn new(name: &str, base_timeout: Duration) -> Self {
        Self {
            name: name.to_string(),
            state: CircuitState::Closed,
            failure_count: 0,
            success_count_in_half_open: 0,
            last_failure: None,
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(60),
            success_threshold: 2,
            latencies: VecDeque::with_capacity(50),
            base_timeout,
            current_timeout: base_timeout,
            max_timeout: Duration::from_secs(300),
            consecutive_timeouts: 0,
        }
    }

    /// Check if we can make a request through this circuit
    pub fn can_execute(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if recovery timeout has elapsed
                if let Some(last) = self.last_failure {
                    if last.elapsed() >= self.recovery_timeout {
                        self.state = CircuitState::HalfOpen;
                        self.success_count_in_half_open = 0;
                        log::info!("[circuit:{}] Transitioning to HALF_OPEN", self.name);
                        true
                    } else {
                        false
                    }
                } else {
                    // No failure recorded, shouldn't be open
                    self.state = CircuitState::Closed;
                    true
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful call
    pub fn record_success(&mut self, latency_ms: u64) {
        self.latencies.push_back(latency_ms);
        if self.latencies.len() > 50 {
            self.latencies.pop_front();
        }
        self.consecutive_timeouts = 0;
        self.update_adaptive_timeout();

        match self.state {
            CircuitState::HalfOpen => {
                self.success_count_in_half_open += 1;
                if self.success_count_in_half_open >= self.success_threshold {
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    log::info!("[circuit:{}] CLOSED (recovered)", self.name);
                }
            }
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            _ => {}
        }
    }

    /// Record a failed call
    pub fn record_failure(&mut self, was_timeout: bool) {
        self.failure_count += 1;
        self.last_failure = Some(Instant::now());

        if was_timeout {
            self.consecutive_timeouts += 1;
            // Increase timeout on consecutive timeouts
            if self.consecutive_timeouts >= 3 {
                let new_timeout = self.current_timeout.mul_f64(1.5).min(self.max_timeout);
                if new_timeout != self.current_timeout {
                    log::warn!("[circuit:{}] Increasing timeout to {}s (consecutive timeouts: {})",
                        self.name, new_timeout.as_secs(), self.consecutive_timeouts);
                    self.current_timeout = new_timeout;
                }
            }
        }

        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.failure_threshold {
                    self.state = CircuitState::Open;
                    log::warn!("[circuit:{}] OPEN (after {} failures)", self.name, self.failure_count);
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open goes back to open
                self.state = CircuitState::Open;
                log::warn!("[circuit:{}] Back to OPEN (failed in half-open)", self.name);
            }
            _ => {}
        }
    }

    /// Get the current adaptive timeout
    pub fn timeout(&self) -> Duration {
        self.current_timeout
    }

    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Read-only availability check (doesn't mutate state like `can_execute`).
    /// Use for scoring/ranking providers without triggering Open→HalfOpen transition.
    pub fn is_available(&self) -> bool {
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(last) = self.last_failure {
                    last.elapsed() >= self.recovery_timeout
                } else {
                    true
                }
            }
        }
    }

    /// Average latency from recent calls (0.0 if no data).
    pub fn avg_latency_ms(&self) -> f64 {
        if self.latencies.is_empty() {
            return 0.0;
        }
        let sum: u64 = self.latencies.iter().sum();
        sum as f64 / self.latencies.len() as f64
    }

    fn update_adaptive_timeout(&mut self) {
        if self.latencies.len() >= 5 {
            // Calculate p95
            let mut sorted: Vec<u64> = self.latencies.iter().copied().collect();
            sorted.sort();
            let p95_idx = (sorted.len() as f64 * 0.95) as usize;
            let p95 = sorted.get(p95_idx).copied().unwrap_or(0);

            // Set timeout to max(p95 * 2, 30s)
            let adaptive = Duration::from_millis(p95 * 2).max(Duration::from_secs(30));
            self.current_timeout = adaptive.min(self.max_timeout);
        }
    }
}
