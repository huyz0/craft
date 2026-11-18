use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::storage::SecretBytes;
use zeroize::{Zeroize, Zeroizing};

use super::ServiceCredentials;

const PIPE_LIMIT: usize = super::service::PROVISIONING_LIMIT;

fn invalid_pipe() -> CraftError {
    CraftError::field("/credentials", "expected_bounded_service_credential_pipe")
}

/// Consumes only an explicit native pipe. Terminal and regular-file input are refused before read.
pub fn read_service_credentials_from_stdin(
    control: &InvocationControl,
) -> Result<ServiceCredentials, CraftError> {
    control.check()?;
    if !craft_platform::stdin_is_pipe().map_err(|_| invalid_pipe())? {
        return Err(invalid_pipe());
    }
    ServiceCredentials::from_provisioning(read_secret(std::io::stdin(), control)?)
}

/// Same pipe rules as service credentials, for the provider's static key/token object.
pub fn read_static_credentials_from_stdin(
    provider: &str,
    control: &InvocationControl,
) -> Result<super::StaticCredentials, CraftError> {
    control.check()?;
    if !craft_platform::stdin_is_pipe().map_err(|_| invalid_pipe())? {
        return Err(invalid_pipe());
    }
    super::StaticCredentials::from_provisioning(provider, read_secret(std::io::stdin(), control)?)
}

fn collect(mut reader: impl Read, control: &InvocationControl) -> Result<SecretBytes, CraftError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(PIPE_LIMIT + 1));
    let mut scratch = Zeroizing::new([0_u8; 8_192]);
    loop {
        control.check()?;
        let remaining = (PIPE_LIMIT + 1 - bytes.len()).min(scratch.len());
        if remaining == 0 {
            return Err(invalid_pipe());
        }
        let count = match reader.read(&mut scratch[..remaining]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(invalid_pipe()),
        };
        if count == 0 {
            break;
        }
        control.check()?;
        bytes.extend_from_slice(&scratch[..count]);
        scratch.zeroize();
        if bytes.len() > PIPE_LIMIT {
            return Err(invalid_pipe());
        }
    }
    SecretBytes::new(std::mem::take(&mut *bytes)).map_err(|_| invalid_pipe())
}

fn read_secret(
    reader: impl Read + Send + 'static,
    control: &InvocationControl,
) -> Result<SecretBytes, CraftError> {
    control.check()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_control = control.clone();
    let _worker = std::thread::Builder::new()
        .name("craft-secret-input".into())
        .spawn(move || {
            let _ = sender.send(collect(reader, &worker_control));
        })
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot start secure input worker."))?;
    loop {
        let wait = control.remaining()?.min(Duration::from_millis(20));
        match receiver.recv_timeout(wait) {
            Ok(result) => {
                control.check()?;
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(invalid_pipe()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::SystemClock;
    use craft_core::control::CancellationToken;
    use std::io::{self, Cursor};
    use std::sync::{Arc, mpsc};

    fn control(timeout: u64, token: CancellationToken) -> Result<InvocationControl, CraftError> {
        InvocationControl::new(Arc::new(SystemClock::default()), token, timeout)
    }

    #[test]
    fn secret_pipe_preserves_unicode_and_scrubs_malformed_oversized_or_failed_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid = br#"{"client_id":"synthetic-id","client_secret":"synthetic-secret"}"#;
        let input = read_secret(
            Cursor::new(valid.to_vec()),
            &control(1_000, CancellationToken::default())?,
        )?;
        let credential = ServiceCredentials::from_provisioning(input)?;
        assert!(!format!("{credential:?}").contains("synthetic-secret"));
        let unicode = "{\"client_id\":\"日本語\",\"client_secret\":\"🔑\"}"
            .as_bytes()
            .to_vec();
        ServiceCredentials::from_provisioning(read_secret(
            Cursor::new(unicode),
            &control(1_000, CancellationToken::default())?,
        )?)?;
        for bytes in [
            vec![],
            vec![b'x'; PIPE_LIMIT + 1],
            br#"{"client_id":"id","client_secret":"synthetic-secret","extra":true}"#.to_vec(),
            br#"{"client_id":"id","client_id":"id","client_secret":"synthetic-secret"}"#.to_vec(),
        ] {
            let result = read_secret(
                Cursor::new(bytes),
                &control(1_000, CancellationToken::default())?,
            )
            .and_then(ServiceCredentials::from_provisioning);
            let error = result.err().ok_or("Expected secure input refusal")?;
            assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
        }
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic-secret"))
            }
        }
        let error = read_secret(Failed, &control(1_000, CancellationToken::default())?)
            .err()
            .ok_or("Expected read failure")?;
        assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
        Ok(())
    }

    struct Blocked {
        release: mpsc::Receiver<()>,
        started: mpsc::Sender<()>,
    }
    impl Read for Blocked {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            let _ = self.started.send(());
            let _ = self.release.recv();
            Ok(0)
        }
    }

    #[test]
    fn blocked_secret_pipe_observes_deadline_and_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        for cancel in [false, true] {
            let token = CancellationToken::default();
            let (release, blocked) = mpsc::channel();
            let (started, observed) = mpsc::channel();
            let control = control(if cancel { 5_000 } else { 30 }, token.clone())?;
            let trigger = if cancel {
                Some(std::thread::spawn(move || {
                    let received = observed.recv_timeout(Duration::from_secs(2));
                    token.cancel();
                    received
                }))
            } else {
                None
            };
            let failure = read_secret(
                Blocked {
                    release: blocked,
                    started,
                },
                &control,
            )
            .err()
            .ok_or("Expected bounded input")?;
            assert_eq!(
                failure.code,
                if cancel {
                    ErrorCode::Cancelled
                } else {
                    ErrorCode::DeadlineExceeded
                }
            );
            let _ = release.send(());
            if let Some(trigger) = trigger {
                trigger.join().map_err(|_| "Input trigger panicked")??;
            }
        }
        Ok(())
    }

    #[test]
    fn native_pipe_child() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(mode) = std::env::var("CRAFT_SECRET_INPUT_CASE") else {
            return Ok(());
        };
        if mode == "interactive-reject" {
            let error = super::super::read_service_credentials_interactively(&control(
                1_000,
                CancellationToken::default(),
            )?)
            .err()
            .ok_or("Interactive input accepted a pipe")?;
            assert_eq!(error.code, ErrorCode::InputInvalid);
            println!("secure-input-checked");
            return Ok(());
        }
        let result =
            read_service_credentials_from_stdin(&control(1_000, CancellationToken::default())?);
        assert_eq!(result.is_ok(), mode == "accept");
        if let Err(error) = result {
            assert!(!format!("{error:?}").contains("synthetic-secret"));
        }
        println!("secure-input-checked");
        Ok(())
    }

    #[test]
    fn native_stdin_accepts_a_pipe_and_refuses_a_regular_file_without_echo()
    -> Result<(), Box<dyn std::error::Error>> {
        use craft_test_support::process::{RunOptions, run};
        use std::process::{Command, Stdio};
        let binary = std::env::current_exe()?;
        let arguments = [
            "--exact",
            "auth::input::tests::native_pipe_child",
            "--nocapture",
        ];
        let bytes = br#"{"client_id":"synthetic-id","client_secret":"synthetic-secret"}"#;
        let output = run(
            &binary,
            &arguments,
            Some(bytes),
            RunOptions {
                environment: vec![("CRAFT_SECRET_INPUT_CASE".into(), Some("accept".into()))],
                ..RunOptions::default()
            },
        )?;
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("secure-input-checked"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret"));
        assert!(output.stderr.is_empty());
        let refused = run(
            &binary,
            &arguments,
            Some(bytes),
            RunOptions {
                environment: vec![(
                    "CRAFT_SECRET_INPUT_CASE".into(),
                    Some("interactive-reject".into()),
                )],
                ..RunOptions::default()
            },
        )?;
        assert!(refused.status.success());
        assert!(refused.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&refused.stdout).contains("synthetic-secret"));
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&root)?;
        let path = root.join(crate::profiles::random_id("secret-source-fixture-")?);
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let cleanup = Cleanup(path.clone());
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(bytes)?;
        }
        let mut child = Command::new(&binary)
            .args(arguments)
            .env("CRAFT_SECRET_INPUT_CASE", "reject")
            .stdin(Stdio::from(std::fs::File::open(&path)?))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let started = std::time::Instant::now();
        while child.try_wait()?.is_none() {
            if started.elapsed() > Duration::from_secs(10) {
                child.kill()?;
                child.wait()?;
                return Err("Native source child exceeded its deadline".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.wait_with_output()?;
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret"));
        assert!(output.stderr.is_empty());
        drop(cleanup);
        Ok(())
    }
}
