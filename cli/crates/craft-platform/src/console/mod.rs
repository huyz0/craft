use std::io::{self, IsTerminal};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use zeroize::Zeroizing;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
use windows::NativeConsole;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix::NativeConsole;

pub enum InputEvent {
    Text {
        units: Zeroizing<[u16; 2]>,
        length: usize,
        repeat: u16,
    },
    Enter,
    Backspace,
    Clear,
    Cancel,
}

static INPUT_OWNER: Mutex<()> = Mutex::new(());

/// Owns exclusive foreground terminal input within this process. Never moves to a worker.
pub struct ConsoleInput {
    #[cfg(any(windows, unix))]
    native: NativeConsole,
    restored: bool,
    _owner: MutexGuard<'static, ()>,
}

impl ConsoleInput {
    /// Opens only actual terminal stdin and saves its state before disabling echo.
    pub fn open() -> io::Result<Self> {
        if !io::stdin().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Interactive input requires a terminal",
            ));
        }
        let owner = INPUT_OWNER.try_lock().map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "Terminal input is already owned")
        })?;
        #[cfg(any(windows, unix))]
        {
            Ok(Self {
                native: NativeConsole::open()?,
                restored: false,
                _owner: owner,
            })
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = owner;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Interactive terminal input is unavailable",
            ))
        }
    }

    /// Requires exclusive ownership of the terminal, including against other processes/readers.
    pub fn poll(&mut self, wait: Duration) -> io::Result<Option<InputEvent>> {
        if self.restored {
            return Err(io::Error::other("Terminal input has ended"));
        }
        #[cfg(any(windows, unix))]
        {
            self.native.poll(wait)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = wait;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Interactive terminal input is unavailable",
            ))
        }
    }

    /// A restore failure must prevent the caller from using the collected credentials.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        #[cfg(any(windows, unix))]
        self.native.restore()?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for ConsoleInput {
    fn drop(&mut self) {
        if !self.restored {
            #[cfg(any(windows, unix))]
            {
                let _ = self.native.restore();
            }
        }
    }
}

fn event(unit: u16, repeat: u16) -> Option<InputEvent> {
    Some(match unit {
        0 => return None,
        3 | 4 | 26 | 27 => InputEvent::Cancel,
        8 | 127 => InputEvent::Backspace,
        10 | 13 => InputEvent::Enter,
        21 => InputEvent::Clear,
        _ => InputEvent::Text {
            units: Zeroizing::new([unit, 0]),
            length: 1,
            repeat: repeat.max(1),
        },
    })
}
