use super::*;
use crate::ConsoleInput;
use std::fs::{File, OpenOptions};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    COORD, GetConsoleMode, GetConsoleProcessList, GetNumberOfConsoleInputEvents, GetStdHandle,
    INPUT_RECORD_0, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, ReadConsoleOutputCharacterW,
    STD_INPUT_HANDLE, SetStdHandle, WriteConsoleInputW,
};

struct InputHandle {
    original: HANDLE,
    input: File,
}
impl Drop for InputHandle {
    fn drop(&mut self) {
        // Restore the child's previous standard handle before its owned CONIN$ handle closes.
        unsafe {
            SetStdHandle(STD_INPUT_HANDLE, self.original);
        }
    }
}

fn mode(input: &File) -> io::Result<u32> {
    let mut value = 0;
    // The file owns a live console handle and the output word is exclusive for this call.
    if unsafe { GetConsoleMode(input.as_raw_handle(), &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn inject(input: &File, text: &str) -> io::Result<()> {
    let records: Vec<INPUT_RECORD> = text
        .encode_utf16()
        .map(|unit| INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: 1,
                    wRepeatCount: 1,
                    wVirtualKeyCode: 0,
                    wVirtualScanCode: 0,
                    uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
                    dwControlKeyState: 0,
                },
            },
        })
        .collect();
    let mut written = 0;
    // All records are initialized synthetic events; Windows borrows this slice only here.
    if unsafe {
        WriteConsoleInputW(
            input.as_raw_handle(),
            records.as_ptr(),
            records.len() as u32,
            &mut written,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if written as usize != records.len() {
        return Err(io::Error::other("Incomplete synthetic input"));
    }
    Ok(())
}

#[test]
#[ignore = "Requires an owned hidden Windows console; run explicitly with the acceptance environment"]
fn owned_console_preserves_unicode_suppresses_echo_discards_pending_input_and_restores_modes()
-> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CRAFT_CONSOLE_ACCEPTANCE").as_deref() != Ok("owned-hidden") {
        return Err("Run only in an explicitly owned hidden console".into());
    }
    let mut processes = [0_u32; 8];
    // The output buffer is initialized and its capacity matches the explicit word count.
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    if count != 1 || processes[0] != std::process::id() {
        return Err("Console is not exclusively owned by this test process".into());
    }
    let input = OpenOptions::new().read(true).write(true).open("CONIN$")?;
    // Standard handles change only inside this isolated child and are restored before close.
    let original_handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    // The owned file remains live until after InputHandle restores the old standard handle.
    if unsafe { SetStdHandle(STD_INPUT_HANDLE, input.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let handle = InputHandle {
        original: original_handle,
        input,
    };
    let original = mode(&handle.input)?;
    let mut terminal = ConsoleInput::open()?;
    assert_eq!(
        mode(&handle.input)? & (ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT),
        0
    );
    let sentinel = "synthetic-日本-🔑";
    inject(&handle.input, &format!("{sentinel}\rdiscard-this"))?;
    let started = std::time::Instant::now();
    let mut units = Zeroizing::new(Vec::new());
    loop {
        if started.elapsed() > Duration::from_secs(2) {
            return Err("Native event read exceeded its budget".into());
        }
        match terminal.poll(Duration::from_millis(20))? {
            Some(InputEvent::Text {
                units: input,
                length,
                repeat,
            }) => {
                for _ in 0..repeat {
                    units.extend_from_slice(&input[..length]);
                }
            }
            Some(InputEvent::Enter) => break,
            _ => {}
        }
    }
    assert_eq!(String::from_utf16(&units)?, sentinel);
    terminal.finish()?;
    assert_eq!(mode(&handle.input)?, original);
    let mut pending = 0;
    // This private console has no other client; inspect only its remaining event count.
    if unsafe { GetNumberOfConsoleInputEvents(handle.input.as_raw_handle(), &mut pending) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    assert_eq!(pending, 0);
    drop(terminal);
    {
        let mut terminal = ConsoleInput::open()?;
        let started = std::time::Instant::now();
        assert!(terminal.poll(Duration::from_millis(20))?.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        inject(&handle.input, "unread-synthetic-input")?;
        // Dropping an unfinished guard models a handled read/error unwinding path.
    }
    assert_eq!(mode(&handle.input)?, original);
    let output = OpenOptions::new().read(true).write(true).open("CONOUT$")?;
    let mut screen = [0_u16; 4_096];
    let mut read = 0;
    // The UTF-16 buffer is initialized and bounded; no screen content is printed or persisted.
    if unsafe {
        ReadConsoleOutputCharacterW(
            output.as_raw_handle(),
            screen.as_mut_ptr(),
            screen.len() as u32,
            COORD { X: 0, Y: 0 },
            &mut read,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let text = String::from_utf16_lossy(&screen[..read as usize]);
    assert!(!text.contains("synthetic"));
    assert!(!text.contains("discard-this"));
    Ok(())
}
