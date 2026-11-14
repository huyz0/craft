use std::io;
use std::mem::{align_of, size_of};
use std::os::windows::io::AsRawHandle;
use std::time::Duration;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{
    ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
    FlushConsoleInputBuffer, GetConsoleMode, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW,
    SetConsoleMode,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use zeroize::Zeroizing;

use super::InputEvent;

#[cfg(test)]
mod native_tests;

#[repr(C, align(8))]
struct RecordBuffer(Zeroizing<[u8; size_of::<INPUT_RECORD>()]>);
const _: () = assert!(align_of::<INPUT_RECORD>() <= align_of::<RecordBuffer>());

pub(super) struct NativeConsole {
    input: io::Stdin,
    original: u32,
}

impl NativeConsole {
    pub(super) fn open() -> io::Result<Self> {
        let input = io::stdin();
        let mut original = 0;
        // The borrowed stdin handle stays live in this guard; Windows writes one mode word.
        if unsafe { GetConsoleMode(input.as_raw_handle(), &mut original) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mode = original
            & !(ENABLE_ECHO_INPUT
                | ENABLE_LINE_INPUT
                | ENABLE_PROCESSED_INPUT
                | ENABLE_VIRTUAL_TERMINAL_INPUT);
        // Only the input mode changes; the exact prior mode is retained for checked restoration.
        if unsafe { SetConsoleMode(input.as_raw_handle(), mode) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { input, original })
    }

    pub(super) fn poll(&mut self, wait: Duration) -> io::Result<Option<InputEvent>> {
        // Console input is waitable. Exclusive foreground ownership avoids a competing reader.
        let status = unsafe {
            WaitForSingleObject(self.input.as_raw_handle(), wait.as_millis().min(100) as u32)
        };
        if status == WAIT_TIMEOUT {
            return Ok(None);
        }
        if status != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = RecordBuffer(Zeroizing::new([0; size_of::<INPUT_RECORD>()]));
        if !bytes
            .0
            .as_ptr()
            .addr()
            .is_multiple_of(align_of::<INPUT_RECORD>())
        {
            return Err(io::Error::other("Console record buffer is not aligned"));
        }
        let mut count = 0;
        // The initialized, suitably aligned buffer holds one INPUT_RECORD and is exclusive here.
        if unsafe {
            ReadConsoleInputW(
                self.input.as_raw_handle(),
                bytes.0.as_mut_ptr().cast(),
                1,
                &mut count,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if count != 1 {
            return Ok(None);
        }
        // The successful API call initialized one complete record; only the matching union is read.
        let record = unsafe { &*bytes.0.as_ptr().cast::<INPUT_RECORD>() };
        if u32::from(record.EventType) != KEY_EVENT {
            return Ok(None);
        }
        // KEY_EVENT identifies the KeyEvent union representation.
        let key = unsafe { &record.Event.KeyEvent };
        if key.bKeyDown == 0 {
            return Ok(None);
        }
        // The documented key character union contains a UTF-16 code unit for this wide API.
        Ok(super::event(
            unsafe { key.uChar.UnicodeChar },
            key.wRepeatCount,
        ))
    }

    pub(super) fn restore(&mut self) -> io::Result<()> {
        // Discard queued prompt keystrokes before echo returns, including after cancellation.
        let flushed = unsafe { FlushConsoleInputBuffer(self.input.as_raw_handle()) } != 0;
        // Restores the same borrowed input handle's exact saved mode before ownership is released.
        if unsafe { SetConsoleMode(self.input.as_raw_handle(), self.original) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if !flushed {
            return Err(io::Error::other("Cannot discard pending terminal input"));
        }
        Ok(())
    }
}
