use std::io::{Read, Write};
use std::time::{Duration, Instant};

use craft_test_support::process::{RunOptions, run};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("emit") => {
            std::io::stdout().lock().write_all(&vec![b'o'; 131_072])?;
            std::io::stderr().lock().write_all(&vec![b'e'; 131_072])?;
            return Ok(());
        }
        Some("echo") => {
            let mut input = Vec::new();
            std::io::stdin().lock().read_to_end(&mut input)?;
            std::io::stdout().lock().write_all(&input)?;
            return Ok(());
        }
        Some("sleep") => {
            std::thread::sleep(Duration::from_secs(3));
            return Ok(());
        }
        Some("close") => return Ok(()),
        _ => {}
    }
    let binary = std::env::current_exe()?;
    let output = run(&binary, &["emit"], None, RunOptions::default())?;
    assert!(output.status.success());
    assert_eq!(output.stdout, vec![b'o'; 131_072]);
    assert_eq!(output.stderr, vec![b'e'; 131_072]);
    let input = vec![b'i'; 131_072];
    let output = run(&binary, &["echo"], Some(&input), RunOptions::default())?;
    assert!(output.status.success());
    assert_eq!(output.stdout, input);
    let started = Instant::now();
    let error = run(
        &binary,
        &["sleep"],
        None,
        RunOptions {
            timeout: Duration::from_millis(100),
            ..RunOptions::default()
        },
    )
    .err()
    .ok_or("Expected subprocess deadline")?;
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        run(
            &binary,
            &["emit"],
            None,
            RunOptions {
                max_stream_bytes: 1024,
                ..RunOptions::default()
            }
        )
        .is_err()
    );
    assert!(
        run(&binary, &["close"], Some(&input), RunOptions::default())?
            .status
            .success()
    );
    println!(
        "Subprocess controls passed: concurrent streams, stdin/EOF, deadline, capture bounds, early close"
    );
    Ok(())
}
