use std::io::{self, Read};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::error::ErrorCode;
use craft_runtime::control::read_bounded;
use craft_test_support::state::ManualClock;

#[test]
fn monotonic_budget_survives_wall_adjustment_and_cancellation_takes_priority()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = ManualClock::new(1_000);
    let token = CancellationToken::default();
    let control = InvocationControl::new(Arc::new(clock.clone()), token.clone(), 100)?;
    clock.advance(Duration::from_millis(90))?;
    clock.set_wall(1)?;
    assert_eq!(control.remaining()?, Duration::from_millis(10));
    clock.advance(Duration::from_millis(10))?;
    assert_eq!(
        control.check().err().ok_or("Expected deadline")?.code,
        ErrorCode::DeadlineExceeded
    );
    token.cancel();
    assert_eq!(
        control.check().err().ok_or("Expected cancellation")?.code,
        ErrorCode::Cancelled
    );
    Ok(())
}

struct HeldReader {
    clock: ManualClock,
    token: CancellationToken,
    cancel: bool,
    release: mpsc::Receiver<()>,
}

impl Read for HeldReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel {
            self.token.cancel();
        } else {
            self.clock
                .advance(Duration::from_millis(100))
                .map_err(io::Error::other)?;
        }
        let _ = self.release.recv();
        Ok(0)
    }
}

#[test]
fn blocked_input_cannot_hide_deadline_or_cancellation() -> Result<(), Box<dyn std::error::Error>> {
    for cancel in [false, true] {
        let clock = ManualClock::new(1_000);
        let token = CancellationToken::default();
        let control = InvocationControl::new(Arc::new(clock.clone()), token.clone(), 100)?;
        let (release, receiver) = mpsc::channel();
        let result = read_bounded(
            HeldReader {
                clock,
                token,
                cancel,
                release: receiver,
            },
            1024,
            &control,
        );
        let _ = release.send(());
        assert_eq!(
            result.err().ok_or("Expected interrupted input")?.code,
            if cancel {
                ErrorCode::Cancelled
            } else {
                ErrorCode::DeadlineExceeded
            }
        );
    }
    let control = InvocationControl::new(
        Arc::new(ManualClock::new(0)),
        CancellationToken::default(),
        1000,
    )?;
    assert_eq!(read_bounded(io::Cursor::new(b"{}"), 1024, &control)?, b"{}");
    assert!(read_bounded(io::Cursor::new(vec![b' '; 1025]), 1024, &control).is_err());
    Ok(())
}
