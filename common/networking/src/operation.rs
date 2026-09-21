use crate::ratelimit::RateLimiter;
use anyhow::{Context, Result, ensure};
use std::{
    cell::Cell,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

const OPERATION_TIMEOUT: Duration = Duration::from_secs(240);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_UPSTREAM_ATTEMPTS: u8 = 16;

/// One public fetch, including pacing, wrapper hops, solving and recovery.
/// Control-plane RPCs share its deadline but do not spend the website's rate.
pub(crate) struct Operation {
    deadline: Instant,
    limiter: Option<Arc<RateLimiter>>,
    attempts: Cell<u8>,
    rate_retry_used: Cell<bool>,
}

impl Operation {
    pub(crate) fn new(limiter: Option<Arc<RateLimiter>>) -> Self {
        Self {
            deadline: Instant::now() + OPERATION_TIMEOUT,
            limiter,
            attempts: Cell::new(0),
            rate_retry_used: Cell::new(false),
        }
    }

    pub(crate) fn remaining(&self) -> Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "network operation exceeded its 240-second deadline"
        );
        Ok(remaining)
    }

    pub(crate) fn request_timeout(&self) -> Result<Duration> {
        Ok(self.remaining()?.min(REQUEST_TIMEOUT))
    }

    pub(crate) fn before_attempt(&self) -> Result<()> {
        self.remaining()?;
        ensure!(
            self.attempts.get() < MAX_UPSTREAM_ATTEMPTS,
            "network operation exceeded its upstream attempt limit"
        );
        if let Some(limiter) = &self.limiter {
            limiter.acquire(self)?;
        }
        self.remaining()?;
        self.attempts.set(self.attempts.get() + 1);
        Ok(())
    }

    pub(crate) fn backoff(&self, delay: Duration) -> Result<()> {
        if self.rate_retry_used.replace(true) {
            return Err(anyhow::Error::new(crate::backoff::RateLimited)
                .context("the single direct retry was already used"));
        }
        log::warn!(
            "HTTP 429: waiting {:?} before the single direct retry",
            delay
        );
        self.sleep(delay).context(crate::backoff::RateLimited)
    }

    pub(crate) fn sleep(&self, delay: Duration) -> Result<()> {
        ensure!(
            delay < self.remaining()?,
            "required wait exceeds the remaining network operation deadline"
        );
        std::thread::sleep(delay);
        self.remaining()?;
        Ok(())
    }

    /// Bound waits for locks that protect network work. Short client-state
    /// locks are never held during I/O and continue to use normal locking.
    pub(crate) fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> Result<MutexGuard<'a, T>> {
        loop {
            self.remaining()?;
            match mutex.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => {
                    let remaining = self.remaining()?;
                    std::thread::sleep(remaining.min(Duration::from_millis(10)));
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timeout(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            ..Self::new(None)
        }
    }
}
