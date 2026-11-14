use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct RunOptions {
    pub timeout: Duration,
    pub max_stream_bytes: u64,
    pub current_dir: Option<PathBuf>,
    pub environment: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_stream_bytes: 4 * 1024 * 1024,
            current_dir: None,
            environment: Vec::new(),
        }
    }
}

fn capture(reader: impl Read, maximum: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(io::Error::other(
            "Subprocess output exceeded its capture budget",
        ));
    }
    Ok(bytes)
}

/// Reads both streams while the child runs so a full pipe cannot deadlock its deadline check.
pub fn run(
    binary: &Path,
    arguments: &[&str],
    input: Option<&[u8]>,
    options: RunOptions,
) -> io::Result<Output> {
    let mut command = Command::new(binary);
    if let Some(directory) = &options.current_dir {
        command.current_dir(directory);
    }
    for (name, value) in &options.environment {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let mut child = command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Child stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("Child stderr was not piped"))?;
    let stdin = child.stdin.take();
    let input = input.map(<[u8]>::to_vec);
    let writer = thread::spawn(move || -> io::Result<()> {
        if let Some(mut stdin) = stdin
            && let Some(input) = input
        {
            match stdin.write_all(&input) {
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                result => result?,
            }
        }
        Ok(())
    });
    let maximum = options.max_stream_bytes;
    let output_reader = thread::spawn(move || capture(stdout, maximum));
    let error_reader = thread::spawn(move || capture(stderr, maximum));
    let started = Instant::now();
    let status: io::Result<ExitStatus> = (|| loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= options.timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Subprocess exceeded its deadline",
            ));
        }
        thread::sleep(Duration::from_millis(2));
    })();
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let write_result = writer
        .join()
        .map_err(|_| io::Error::other("Subprocess input worker failed"))?;
    let output_result = output_reader
        .join()
        .map_err(|_| io::Error::other("Subprocess output worker failed"))?;
    let error_result = error_reader
        .join()
        .map_err(|_| io::Error::other("Subprocess error worker failed"))?;
    let status = status?;
    write_result?;
    Ok(Output {
        status,
        stdout: output_result?,
        stderr: error_result?,
    })
}
