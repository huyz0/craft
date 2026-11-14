use std::io::{BufRead, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use super::*;

struct Probe {
    child: Child,
    updates: mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
    errors: Option<std::thread::JoinHandle<()>>,
}

impl Probe {
    fn spawn(root: &Path, mode: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "journal::tests::process::journal_child",
                "--nocapture",
            ])
            .env("CRAFT_JOURNAL_CHILD_ROOT", root)
            .env("CRAFT_JOURNAL_CHILD_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("Missing journal child output")?;
        let stderr = child.stderr.take().ok_or("Missing journal child errors")?;
        let (sender, updates) = mpsc::channel();
        let error_sender = sender.clone();
        let errors = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr.take(8_192))
                .lines()
                .map_while(Result::ok)
            {
                let _ = error_sender.send(format!("child-stderr:{line}"));
            }
        });
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout.take(8_192))
                .lines()
                .map_while(Result::ok)
            {
                if line.starts_with("journal-") {
                    let _ = sender.send(line);
                }
            }
        });
        Ok(Self {
            child,
            updates,
            reader: Some(reader),
            errors: Some(errors),
        })
    }
    fn next(&self) -> Result<String, Box<dyn std::error::Error>> {
        let message = self.updates.recv_timeout(Duration::from_secs(15))?;
        if message.starts_with("child-stderr:") {
            return Err(message.into());
        }
        Ok(message)
    }
    fn wait(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                assert!(status.success(), "Journal subprocess failed");
                return Ok(());
            }
            if started.elapsed() > Duration::from_secs(15) {
                return Err("Journal child exceeded deadline".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(errors) = self.errors.take() {
            let _ = errors.join();
        }
    }
}

fn signal() -> Result<(), Box<dyn std::error::Error>> {
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte)?;
    Ok(())
}
fn announce(message: &str) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(std::io::stdout(), "{message}")?;
    std::io::stdout().flush()?;
    Ok(())
}

#[test]
fn journal_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("CRAFT_JOURNAL_CHILD_ROOT") else {
        return Ok(());
    };
    let mode = std::env::var("CRAFT_JOURNAL_CHILD_MODE")?;
    let invocation = control(CancellationToken::default(), 30_000)?;
    let profiles = ProfileStore::open(Path::new(&root), &invocation)
        .map_err(|error| format!("Opening child profile state: {error}"))?;
    let grant = profiles.get("team", &invocation)?.grant;
    let journal = NativeJournal::open(&profiles, &invocation)
        .map_err(|error| format!("Opening child journal state: {error}"))?;
    let registry = registry()?;
    let operation = registry.prepare(request())?;
    if mode == "race" {
        announce("journal-ready")?;
        signal()?;
        let outcome =
            journal.claim_send(&registry, &operation, &authority(&grant), "create", &call());
        announce(if outcome.is_ok() {
            "journal-claimed"
        } else {
            "journal-refused"
        })?;
        return Ok(());
    }
    if mode == "uncommitted" {
        journal
            .inner
            .faults
            .fail_on(FaultPoint::JournalUpdateBeforeCommit, 2)?;
        assert!(
            journal
                .claim_send(&registry, &operation, &authority(&grant), "create", &call())
                .is_err()
        );
    } else {
        let claim =
            journal.claim_send(&registry, &operation, &authority(&grant), "create", &call())?;
        if mode == "receipt" {
            claim.record_outcome(&StepOutcome {
                state: SubmissionState::Confirmed,
                confirmed_resources: vec![target()],
                job: None,
            })?;
        }
    }
    if mode == "uncommitted" || mode == "receipt-uncommitted" {
        let guard = journal.lock(false)?;
        let transaction = journal.transaction(&guard.connection, false)?;
        let previous = journal
            .read(&transaction, "create-record")?
            .ok_or("Missing child operation")?;
        let mut next = previous.clone();
        next.revision += 1;
        let step = next.steps.get_mut("create").ok_or("Missing child step")?;
        if mode == "uncommitted" {
            step.state = SubmissionState::SendStarted;
        } else {
            step.state = SubmissionState::Confirmed;
            step.confirmed_resources.push(target());
        }
        journal.update(&transaction, &previous, &next)?;
        announce("journal-held")?;
        signal()?;
        return Ok(());
    }
    announce("journal-held")?;
    signal()?;
    Ok(())
}

#[test]
fn competing_processes_cannot_claim_the_same_native_request_twice()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let invocation = control(CancellationToken::default(), 30_000)?;
    let (_, _, journal) = fixture.setup(&invocation)?;
    let mut first = Probe::spawn(&fixture.root, "race")?;
    let mut second = Probe::spawn(&fixture.root, "race")?;
    assert_eq!(first.next()?, "journal-ready");
    assert_eq!(second.next()?, "journal-ready");
    first
        .child
        .stdin
        .as_mut()
        .ok_or("Missing first stdin")?
        .write_all(b"x")?;
    second
        .child
        .stdin
        .as_mut()
        .ok_or("Missing second stdin")?
        .write_all(b"x")?;
    let outcomes = [first.next()?, second.next()?];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_str() == "journal-claimed")
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_str() == "journal-refused")
            .count(),
        1
    );
    first.wait()?;
    second.wait()?;
    assert_eq!(
        journal.get("create-record")?.ok_or("Missing marker")?.steps["create"].state,
        SubmissionState::SendStarted
    );
    Ok(())
}

#[test]
fn killed_processes_recover_uncommitted_and_committed_send_or_receipt_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["uncommitted", "marker", "receipt-uncommitted", "receipt"] {
        let fixture = Fixture::new()?;
        let invocation = control(CancellationToken::default(), 30_000)?;
        let (profiles, grant, journal) = fixture.setup(&invocation)?;
        let mut child = Probe::spawn(&fixture.root, mode)?;
        assert_eq!(child.next()?, "journal-held");
        child.child.kill()?;
        child.child.wait()?;
        drop(child);
        drop(journal);
        let fresh = NativeJournal::open(&profiles, &invocation)?;
        let record = fresh
            .get("create-record")?
            .ok_or("Lost operation after kill")?;
        assert_eq!(
            record.steps["create"].state,
            match mode {
                "uncommitted" => SubmissionState::Prepared,
                "receipt" => SubmissionState::Confirmed,
                _ => SubmissionState::SendStarted,
            }
        );
        assert_eq!(
            record.steps["create"].confirmed_resources.is_empty(),
            mode != "receipt"
        );
        let registry = registry()?;
        let operation = registry.prepare(request())?;
        assert_eq!(
            fresh
                .claim_send(&registry, &operation, &authority(&grant), "create", &call())
                .is_ok(),
            mode == "uncommitted"
        );
    }
    Ok(())
}
