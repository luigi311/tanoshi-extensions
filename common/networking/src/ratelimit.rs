use std::sync::Mutex;
use std::time::{Duration, Instant};

use log::trace;

const INVALID_RATE: &str = "invalid networking configuration: requests_per_second must be positive and finite with a nonzero, representable scheduling interval; use None for unlimited requests";

#[derive(Debug)]
pub struct RateLimiter {
    interval: Duration,
    next_allowed: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(requests_per_second: f64) -> Result<Self, &'static str> {
        if !requests_per_second.is_finite() || requests_per_second <= 0.0 {
            return Err(INVALID_RATE);
        }
        let interval =
            Duration::try_from_secs_f64(1.0 / requests_per_second).map_err(|_| INVALID_RATE)?;
        let now = Instant::now();
        if interval.is_zero() || now.checked_add(interval).is_none() {
            return Err(INVALID_RATE);
        }
        Ok(Self {
            interval,
            next_allowed: Mutex::new(now),
        })
    }

    pub fn acquire(&self, operation: &crate::operation::Operation) -> anyhow::Result<()> {
        loop {
            operation.remaining()?;
            let now = Instant::now();
            let sleep_for = {
                let mut next = self
                    .next_allowed
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if now >= *next {
                    *next = now
                        .checked_add(self.interval)
                        .ok_or_else(|| anyhow::anyhow!(INVALID_RATE))?;
                    None
                } else {
                    Some(*next - now)
                }
            };

            if let Some(dur) = sleep_for {
                trace!("Rate limiter sleeping for {:?}", dur);
                operation.sleep(dur)?;
            } else {
                return Ok(());
            }
        }
    }
}
