//! Token-bucket rate limiter.
//!
//! The bucket holds up to `capacity` tokens and refills continuously at `refill_per_sec`.
//! Each message costs one token. This allows short bursts ("hi" "how are you" "lol") while
//! capping the sustained rate.

#[derive(Debug, Clone)]
pub struct RateLimiter {
    capacity: f32,
    refill_per_sec: f32,
    tokens: f32,
    last_secs: f64,
}

impl RateLimiter {
    pub fn new(capacity: f32, refill_per_sec: f32, now_secs: f64) -> Self {
        Self { capacity, refill_per_sec, tokens: capacity, last_secs: now_secs }
    }

    /// Takes a token if one is available. `now_secs` is a monotonic clock.
    pub fn try_take(&mut self, now_secs: f64) -> bool {
        let elapsed = (now_secs - self.last_secs).max(0.0) as f32;
        self.last_secs = now_secs;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_burst_then_blocks() {
        let mut rl = RateLimiter::new(3.0, 1.0, 0.0);
        assert!(rl.try_take(0.0));
        assert!(rl.try_take(0.0));
        assert!(rl.try_take(0.0));
        assert!(!rl.try_take(0.0));
    }

    #[test]
    fn refills_over_time() {
        let mut rl = RateLimiter::new(2.0, 1.0, 0.0);
        assert!(rl.try_take(0.0));
        assert!(rl.try_take(0.0));
        assert!(!rl.try_take(0.5));
        assert!(rl.try_take(1.6));
        assert!(!rl.try_take(1.6));
    }

    #[test]
    fn never_exceeds_capacity() {
        let mut rl = RateLimiter::new(2.0, 1.0, 0.0);
        // A long idle period must not bank more than `capacity` tokens.
        assert!(rl.try_take(1000.0));
        assert!(rl.try_take(1000.0));
        assert!(!rl.try_take(1000.0));
    }
}
