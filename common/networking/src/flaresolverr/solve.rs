use anyhow::Result;
use std::{
    error::Error,
    sync::{Arc, Mutex},
};

/// One solve shared by requests that started before it completed. A later
/// request gets a fresh attempt, so failures do not become permanent caches.
#[derive(Default)]
pub(super) struct SolveAttempt {
    result: Mutex<Option<Result<bool, SharedSolveError>>>,
}

impl SolveAttempt {
    pub(super) fn is_complete(&self) -> bool {
        match self.result.try_lock() {
            Ok(result) => result.is_some(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().is_some(),
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
    }

    pub(super) fn run(
        &self,
        budget: &crate::operation::Operation,
        operation: impl FnOnce() -> Result<bool>,
    ) -> Result<bool> {
        let mut result = budget.lock(&self.result)?;
        result
            .get_or_insert_with(|| operation().map_err(|error| SharedSolveError(Arc::new(error))))
            .clone()
            .map_err(anyhow::Error::new)
    }
}

#[derive(Clone, Debug)]
struct SharedSolveError(Arc<anyhow::Error>);

impl std::fmt::Display for SharedSolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl Error for SharedSolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

/// Anyhow context downcasts do not cross a shared error's source boundary.
/// Keep typed session recovery decisions intact for leaders and waiters.
pub(super) fn error_is<E: Error + Send + Sync + 'static>(error: &anyhow::Error) -> bool {
    error.is::<E>()
        || error
            .downcast_ref::<SharedSolveError>()
            .is_some_and(|shared| error_is::<E>(&shared.0))
}
