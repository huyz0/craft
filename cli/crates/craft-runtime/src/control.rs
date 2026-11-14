use std::io::Read;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::storage::Clock;

pub struct SystemClock {
    started: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }
    fn monotonic_millis(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

pub fn install_control(timeout_ms: u64) -> Result<InvocationControl, CraftError> {
    let cancellation = CancellationToken::default();
    let signal = cancellation.clone();
    ctrlc::set_handler(move || signal.cancel()).map_err(|_| {
        CraftError::new(ErrorCode::Internal, "Cannot install cancellation handler.")
    })?;
    InvocationControl::new(Arc::new(SystemClock::default()), cancellation, timeout_ms)
}

/// A blocked input worker cannot prevent the main invocation from observing cancellation.
/// CLI process exit releases a still-blocked stdin worker; this is not a general task executor.
pub fn read_bounded(
    reader: impl Read + Send + 'static,
    maximum: usize,
    control: &InvocationControl,
) -> Result<Vec<u8>, CraftError> {
    control.check()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let _worker = thread::Builder::new()
        .name("craft-input".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = reader
                .take(maximum.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = sender.send(result);
        })
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot start input worker."))?;
    loop {
        let wait = control.remaining()?.min(Duration::from_millis(20));
        match receiver.recv_timeout(wait) {
            Ok(result) => {
                control.check()?;
                let bytes = result.map_err(|_| {
                    CraftError::new(ErrorCode::InputInvalid, "Cannot read command input.")
                })?;
                if bytes.len() > maximum {
                    return Err(CraftError::field("", "input_byte_limit_exceeded"));
                }
                return Ok(bytes);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "Input worker ended without a result.",
                ));
            }
        }
    }
}
