use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_core::storage::{Clock, StoreError};

struct TimeState {
    wall: AtomicU64,
    monotonic: AtomicU64,
    controller: Mutex<()>,
}

#[derive(Clone)]
pub struct ManualClock(Arc<TimeState>);

impl ManualClock {
    pub fn new(unix_millis: u64) -> Self {
        Self(Arc::new(TimeState {
            wall: AtomicU64::new(unix_millis),
            monotonic: AtomicU64::new(0),
            controller: Mutex::new(()),
        }))
    }

    pub fn advance(&self, duration: Duration) -> Result<(), StoreError> {
        let delta = u64::try_from(duration.as_millis()).map_err(|_| StoreError::TooLarge)?;
        let _controller = self
            .0
            .controller
            .lock()
            .map_err(|_| StoreError::Unavailable)?;
        let wall = self
            .unix_millis()
            .checked_add(delta)
            .ok_or(StoreError::TooLarge)?;
        let monotonic = self
            .monotonic_millis()
            .checked_add(delta)
            .ok_or(StoreError::TooLarge)?;
        self.0.wall.store(wall, Ordering::SeqCst);
        self.0.monotonic.store(monotonic, Ordering::SeqCst);
        Ok(())
    }

    pub fn set_wall(&self, unix_millis: u64) -> Result<(), StoreError> {
        let _controller = self
            .0
            .controller
            .lock()
            .map_err(|_| StoreError::Unavailable)?;
        self.0.wall.store(unix_millis, Ordering::SeqCst);
        Ok(())
    }
}

impl Clock for ManualClock {
    fn unix_millis(&self) -> u64 {
        self.0.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.0.monotonic.load(Ordering::SeqCst)
    }
}
