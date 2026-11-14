//! Bounded automatic retry for descriptors that declare safe read semantics.
//!
//! The policy only decides whether and how long to wait. Callers own the exchange, keep the
//! invocation deadline authoritative and never apply it to journaled or multipart sends.
use std::time::Duration;

use craft_core::control::InvocationControl;
use craft_core::execution::RetryAfter;
use craft_core::registry::{OperationDescriptor, RetryClass};

use craft_core::contracts::EffectClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadRetry {
    /// Total sends, including the first.
    pub(crate) attempts: u8,
    pub(crate) base: Duration,
    pub(crate) cap: Duration,
    /// Provider advice above this is returned to the caller instead of waited on.
    pub(crate) max_wait: Duration,
    /// Budget that must remain after a wait for another attempt to be worth sending.
    pub(crate) min_budget: Duration,
}

impl ReadRetry {
    pub(crate) const STANDARD: Self = Self {
        attempts: 3,
        base: Duration::from_millis(250),
        cap: Duration::from_secs(4),
        max_wait: Duration::from_secs(30),
        min_budget: Duration::from_secs(1),
    };

    /// Only a sealed safe-read descriptor opts in; the HTTP method is never consulted.
    pub(crate) fn for_descriptor(descriptor: &OperationDescriptor) -> Option<Self> {
        (matches!(descriptor.effect, EffectClass::Read)
            && matches!(descriptor.retry_class, RetryClass::SafeRead))
        .then_some(Self::STANDARD)
    }

    pub(crate) fn retryable_status(status: u16) -> bool {
        matches!(status, 429 | 502 | 503 | 504)
    }

    /// Returns the wait before retry number `retry` (1-based), or `None` when the attempt
    /// budget, provider advice or invocation deadline means the last result should stand.
    pub(crate) fn wait(
        &self,
        retry: u8,
        advice: Option<RetryAfter>,
        control: &InvocationControl,
    ) -> Option<Duration> {
        if retry == 0 || retry >= self.attempts {
            return None;
        }
        let wait = match advice {
            Some(RetryAfter::DelaySeconds(seconds)) => Duration::from_secs(seconds),
            Some(RetryAfter::AtUnixMillis(at)) => {
                Duration::from_millis(at.saturating_sub(control.unix_millis()))
            }
            None => {
                let factor = 1u32.checked_shl(u32::from(retry - 1)).unwrap_or(u32::MAX);
                self.base.saturating_mul(factor).min(self.cap)
            }
        };
        if wait > self.max_wait {
            return None;
        }
        let remaining = control.remaining().ok()?;
        (wait.saturating_add(self.min_budget) <= remaining).then_some(wait)
    }
}
