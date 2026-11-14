use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::error::{CraftError, ErrorCode};
use crate::storage::Clock;

pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub const MAX_TIMEOUT_MS: u64 = 86_400_000;

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct InvocationControl {
    clock: Arc<dyn Clock>,
    cancellation: CancellationToken,
    deadline: u64,
}

impl InvocationControl {
    pub fn new(
        clock: Arc<dyn Clock>,
        cancellation: CancellationToken,
        timeout_ms: u64,
    ) -> Result<Self, CraftError> {
        if !(1..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(CraftError::field("/timeout_ms", "timeout_out_of_range"));
        }
        let deadline = clock
            .monotonic_millis()
            .checked_add(timeout_ms)
            .ok_or_else(|| CraftError::field("/timeout_ms", "deadline_out_of_range"))?;
        Ok(Self {
            clock,
            cancellation,
            deadline,
        })
    }

    /// Reports pre-send state; callers that have sent must retain remote effect evidence.
    pub fn remaining(&self) -> Result<Duration, CraftError> {
        if self.cancellation.is_cancelled() {
            let mut error = CraftError::new(ErrorCode::Cancelled, "Invocation cancelled.");
            error.next_action = "Inspect any operation receipt before resuming work.".into();
            return Err(error);
        }
        let remaining = self.deadline.saturating_sub(self.clock.monotonic_millis());
        if remaining == 0 {
            let mut error =
                CraftError::new(ErrorCode::DeadlineExceeded, "Invocation deadline exceeded.");
            error.next_action =
                "Inspect any operation receipt before retrying with a larger --timeout-ms.".into();
            return Err(error);
        }
        Ok(Duration::from_millis(remaining))
    }

    pub fn check(&self) -> Result<(), CraftError> {
        self.remaining().map(|_| ())
    }
}
