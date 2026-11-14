use std::io::{self, Write};
use std::time::Duration;

use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_platform::{ConsoleInput, InputEvent};
use zeroize::{Zeroize, Zeroizing};

use super::ServiceCredentials;

trait Terminal {
    fn poll(&mut self, wait: Duration) -> io::Result<Option<InputEvent>>;
    fn finish(&mut self) -> io::Result<()>;
}
impl Terminal for ConsoleInput {
    fn poll(&mut self, wait: Duration) -> io::Result<Option<InputEvent>> {
        ConsoleInput::poll(self, wait)
    }
    fn finish(&mut self) -> io::Result<()> {
        ConsoleInput::finish(self)
    }
}

fn terminal_error() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::InputInvalid,
        "Cannot read credentials securely from this terminal.",
    );
    error.next_action="Use an explicit foreground terminal or the secure provisioning pipe; reset the terminal if its settings changed.".into();
    error
}

/// Explicit human-only entry. It never runs implicitly during an agent operation or login retry.
pub fn read_service_credentials_interactively(
    control: &InvocationControl,
) -> Result<ServiceCredentials, CraftError> {
    control.check()?;
    let mut terminal = ConsoleInput::open().map_err(|_| terminal_error())?;
    prompt(&mut terminal, &mut io::stderr().lock(), control)
}

fn prompt(
    terminal: &mut impl Terminal,
    writer: &mut impl Write,
    control: &InvocationControl,
) -> Result<ServiceCredentials, CraftError> {
    let result = (|| {
        control.check()?;
        writer
            .write_all(b"Client ID: ")
            .and_then(|_| writer.flush())
            .map_err(|_| terminal_error())?;
        let client_id = read_line(terminal, 1_024, control)?;
        writer
            .write_all(b"\nClient secret: ")
            .and_then(|_| writer.flush())
            .map_err(|_| terminal_error())?;
        let client_secret = read_line(terminal, 65_536, control)?;
        ServiceCredentials::from_secret_parts(client_id, client_secret)
    })();
    // Restoration precedes returning either credentials or an input/cancellation failure.
    if terminal.finish().is_err() {
        let mut error = terminal_error();
        error.message = "Terminal input mode could not be restored.".into();
        error.next_action =
            "Reset or close this terminal before continuing; no credentials were returned.".into();
        return Err(error);
    }
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|_| terminal_error())?;
    result
}

fn read_line(
    terminal: &mut impl Terminal,
    maximum: usize,
    control: &InvocationControl,
) -> Result<Zeroizing<String>, CraftError> {
    let mut units = Zeroizing::new(Vec::<u16>::with_capacity(maximum));
    loop {
        control.check()?;
        let event = terminal
            .poll(control.remaining()?.min(Duration::from_millis(20)))
            .map_err(|_| terminal_error())?;
        control.check()?;
        match event {
            None => {}
            Some(InputEvent::Enter) => break,
            Some(InputEvent::Cancel) => {
                return Err(CraftError::new(
                    ErrorCode::Cancelled,
                    "Credential entry was cancelled.",
                ));
            }
            Some(InputEvent::Clear) => units.zeroize(),
            Some(InputEvent::Backspace) => {
                let remove = if units.len() >= 2
                    && (0xdc00..=0xdfff).contains(&units[units.len() - 1])
                    && (0xd800..=0xdbff).contains(&units[units.len() - 2])
                {
                    2
                } else {
                    1
                };
                for _ in 0..remove {
                    if let Some(last) = units.last_mut() {
                        last.zeroize();
                    }
                    units.pop();
                }
            }
            Some(InputEvent::Text {
                units: input,
                length,
                repeat,
            }) => {
                if length == 0 || length > input.len() {
                    return Err(terminal_error());
                }
                for _ in 0..repeat {
                    control.check()?;
                    if units.len() + length > maximum {
                        return Err(CraftError::field(
                            "/credentials",
                            "credential_text_limit_exceeded",
                        ));
                    }
                    units.extend_from_slice(&input[..length]);
                }
            }
        }
    }
    // Preallocation plus the byte check prevents a secret-bearing UTF-8 reallocation.
    let mut text = Zeroizing::new(String::with_capacity(maximum));
    for character in char::decode_utf16(units.iter().copied()) {
        let character = character.map_err(|_| terminal_error())?;
        if character.len_utf8() > maximum.saturating_sub(text.len()) {
            return Err(CraftError::field(
                "/credentials",
                "credential_text_limit_exceeded",
            ));
        }
        text.push(character);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_core::control::CancellationToken;
    use craft_test_support::state::ManualClock;
    use std::collections::VecDeque;
    use std::sync::Arc;

    struct FakeTerminal {
        events: VecDeque<InputEvent>,
        clock: Arc<ManualClock>,
        restored: bool,
        fail_restore: bool,
        fail_read: bool,
        cancel: Option<CancellationToken>,
        token: CancellationToken,
    }
    impl Terminal for FakeTerminal {
        fn poll(&mut self, wait: Duration) -> io::Result<Option<InputEvent>> {
            if self.fail_read {
                return Err(io::Error::other("synthetic-secret"));
            }
            if let Some(token) = self.cancel.take() {
                token.cancel();
            }
            if self.events.is_empty() {
                self.clock.advance(wait).map_err(io::Error::other)?;
            }
            Ok(self.events.pop_front())
        }
        fn finish(&mut self) -> io::Result<()> {
            self.restored = true;
            if self.fail_restore {
                Err(io::Error::other("synthetic-secret"))
            } else {
                Ok(())
            }
        }
    }
    fn fixture(text: &str) -> Result<(FakeTerminal, InvocationControl), CraftError> {
        let clock = Arc::new(ManualClock::new(1_000));
        let token = CancellationToken::default();
        let control = InvocationControl::new(clock.clone(), token.clone(), 100)?;
        let events = text
            .encode_utf16()
            .map(|unit| {
                if unit == 13 {
                    InputEvent::Enter
                } else {
                    InputEvent::Text {
                        units: Zeroizing::new([unit, 0]),
                        length: 1,
                        repeat: 1,
                    }
                }
            })
            .collect();
        Ok((
            FakeTerminal {
                events,
                clock,
                restored: false,
                fail_restore: false,
                fail_read: false,
                cancel: None,
                token,
            },
            control,
        ))
    }

    #[test]
    fn prompt_restores_before_return_and_never_echoes_either_field()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut terminal, control) = fixture("synthetic-id\rsynthetic-secret\r")?;
        let mut output = Vec::new();
        let credentials = prompt(&mut terminal, &mut output, &control)?;
        assert!(terminal.restored);
        assert_eq!(output, b"Client ID: \nClient secret: \n");
        assert!(!format!("{credentials:?}").contains("synthetic-secret"));
        Ok(())
    }

    #[test]
    fn unicode_editing_handles_surrogates_clear_and_exact_byte_bounds()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut terminal, control) = fixture("old")?;
        terminal.events.push_back(InputEvent::Clear);
        terminal
            .events
            .extend("日本🔑".encode_utf16().map(|unit| InputEvent::Text {
                units: Zeroizing::new([unit, 0]),
                length: 1,
                repeat: 1,
            }));
        terminal.events.push_back(InputEvent::Backspace);
        terminal.events.push_back(InputEvent::Enter);
        assert_eq!(read_line(&mut terminal, 6, &control)?.as_str(), "日本");
        let (mut terminal, control) = fixture("🔑\r")?;
        assert_eq!(read_line(&mut terminal, 4, &control)?.as_str(), "🔑");
        let (mut terminal, control) = fixture("🔑\r")?;
        assert!(read_line(&mut terminal, 3, &control).is_err());
        Ok(())
    }

    #[test]
    fn cancellation_deadline_read_and_restore_failures_never_return_credentials()
    -> Result<(), Box<dyn std::error::Error>> {
        for case in [
            "cancel", "deadline", "read", "restore", "encoding", "oversize", "control",
        ] {
            let (mut terminal, control) = fixture(match case {
                "restore" => "id\rsynthetic-secret\r",
                "control" => "id\t\rsynthetic-secret\r",
                _ => "",
            })?;
            match case {
                "cancel" => terminal.cancel = Some(terminal.token.clone()),
                "read" => terminal.fail_read = true,
                "restore" => terminal.fail_restore = true,
                "encoding" => terminal.events.extend([
                    InputEvent::Text {
                        units: Zeroizing::new([0xd800, 0]),
                        length: 1,
                        repeat: 1,
                    },
                    InputEvent::Enter,
                ]),
                "oversize" => terminal.events.push_back(InputEvent::Text {
                    units: Zeroizing::new([b'x' as u16, 0]),
                    length: 1,
                    repeat: 1_025,
                }),
                _ => {}
            }
            let mut output = Vec::new();
            let error = prompt(&mut terminal, &mut output, &control)
                .err()
                .ok_or("Expected prompt refusal")?;
            assert!(terminal.restored);
            assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
            assert!(!String::from_utf8_lossy(&output).contains("synthetic-secret"));
            if case == "deadline" {
                assert_eq!(error.code, ErrorCode::DeadlineExceeded);
            }
            if case == "cancel" {
                assert_eq!(error.code, ErrorCode::Cancelled);
            }
        }
        Ok(())
    }

    #[test]
    fn prompt_output_failure_still_restores_and_does_not_expose_io_error_text()
    -> Result<(), Box<dyn std::error::Error>> {
        struct FailedWriter;
        impl Write for FailedWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic-secret"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (mut terminal, control) = fixture("id\rsynthetic-secret\r")?;
        let failure = prompt(&mut terminal, &mut FailedWriter, &control)
            .err()
            .ok_or("Expected output failure")?;
        assert!(terminal.restored);
        assert!(!format!("{failure:?} {failure}").contains("synthetic-secret"));
        Ok(())
    }
}
