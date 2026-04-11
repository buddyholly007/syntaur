use std::collections::HashMap;
use std::time::Instant;

pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(capacity: u32, per_seconds: u64) -> Self {
        Self {
            capacity: capacity as f64,
            tokens: capacity as f64,
            refill_rate: capacity as f64 / per_seconds as f64,
            last_refill: Instant::now(),
        }
    }

    pub fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn time_until_available(&self) -> f64 {
        if self.tokens >= 1.0 {
            0.0
        } else {
            (1.0 - self.tokens) / self.refill_rate
        }
    }
}

pub struct RateLimiter {
    buckets: HashMap<String, TokenBucket>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: HashMap::new(),
        }
    }

    /// Check rate limit for a key. Returns Ok(()) or Err(wait_seconds)
    pub fn check(&mut self, key: &str, capacity: u32, per_seconds: u64) -> Result<(), f64> {
        let bucket = self.buckets
            .entry(key.to_string())
            .or_insert_with(|| TokenBucket::new(capacity, per_seconds));

        if bucket.try_consume() {
            Ok(())
        } else {
            Err(bucket.time_until_available())
        }
    }

    /// Convenience: check user message rate (30/minute)
    pub fn check_user_message(&mut self, user_id: i64) -> Result<(), f64> {
        self.check(&format!("user_msg:{}", user_id), 30, 60)
    }

    /// Convenience: check LLM call rate (10/minute per provider)
    pub fn check_llm_call(&mut self, provider: &str) -> Result<(), f64> {
        self.check(&format!("llm:{}", provider), 10, 60)
    }

    /// Convenience: check tool execution rate (5/minute per tool)
    pub fn check_tool_exec(&mut self, tool_name: &str) -> Result<(), f64> {
        self.check(&format!("tool:{}", tool_name), 5, 60)
    }

    /// Clean up old buckets (call periodically)
    pub fn cleanup(&mut self) {
        // Remove buckets that haven't been used in 10 minutes
        // (they'll be at full capacity anyway)
        self.buckets.retain(|_, b| b.last_refill.elapsed().as_secs() < 600);
    }
}
