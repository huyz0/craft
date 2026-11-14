use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Barrier, mpsc};

use craft_core::control::CancellationToken;

use super::*;
use crate::control::SystemClock;

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = parent.join(random_id("artifact-fixture-")?);
        Ok(Self { parent, root })
    }

    fn open(&self) -> Result<NativeArtifacts, StoreError> {
        NativeArtifacts::open(
            &self.root,
            2_000_000,
            &control(CancellationToken::default()),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.root.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(windows)]
struct OutputFixture {
    parent: PathBuf,
    path: PathBuf,
}

#[cfg(windows)]
impl OutputFixture {
    fn new(source: &Fixture) -> Result<Self, Box<dyn std::error::Error>> {
        let path = source.parent.join(random_id("export-fixture-")?);
        fs::create_dir(&path)?;
        Ok(Self {
            parent: source.parent.clone(),
            path,
        })
    }
}

#[cfg(windows)]
impl Drop for OutputFixture {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn control(token: CancellationToken) -> InvocationControl {
    InvocationControl::new(Arc::new(SystemClock::default()), token, 10_000)
        .unwrap_or_else(|_| unreachable!("Static test timeout is valid"))
}

fn spec(name: &str, max_bytes: u64) -> ArtifactSpec {
    ArtifactSpec {
        name: name.into(),
        media_type: "application/octet-stream".into(),
        max_bytes,
        source: None,
    }
}

#[test]
fn workspace_snapshots_keep_original_bytes_after_source_edits_and_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let workspace = fixture.root.join("workspace");
    fs::create_dir(&workspace)?;
    let input = craft_platform::InputRoot::open(&workspace)?;
    for (name, bytes) in [
        ("empty-input", Vec::new()),
        ("unicode-input", "日本語 🦀\n".repeat(5000).into_bytes()),
        (
            "binary-input",
            (0..200_000).map(|value| (value % 251) as u8).collect(),
        ),
    ] {
        fs::write(workspace.join("source"), &bytes)?;
        let receipt = store.import_file(
            &input,
            Path::new("source"),
            spec(name, 300_000),
            Some(&content_digest(&bytes)),
        )?;
        assert_eq!(receipt.digest, content_digest(&bytes));
        assert_eq!(receipt.bytes, bytes.len() as u64);
        fs::write(workspace.join("source"), b"later edit")?;
        let reopened = fixture.open()?;
        let snapshot = reopened.read(name, 300_000)?.ok_or("Missing snapshot")?;
        assert_eq!(snapshot.bytes, bytes);
        assert_eq!(snapshot.receipt, receipt);
    }
    Ok(())
}

#[test]
fn invalid_or_mismatched_workspace_inputs_publish_no_complete_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let workspace = fixture.root.join("workspace");
    fs::create_dir(&workspace)?;
    fs::write(workspace.join("source"), b"input bytes")?;
    let input = craft_platform::InputRoot::open(&workspace)?;
    assert_eq!(
        store
            .import_file(&input, Path::new("source"), spec("oversized", 2), None)
            .err(),
        Some(StoreError::TooLarge)
    );
    assert_eq!(
        store
            .import_file(
                &input,
                Path::new("source"),
                spec("mismatched", 100),
                Some(&format!("sha256:{}", "a".repeat(64)))
            )
            .err(),
        Some(StoreError::Conflict)
    );
    assert_eq!(
        store
            .import_file(
                &input,
                Path::new("missing"),
                spec("bad-digest", 100),
                Some("not-a-digest")
            )
            .err(),
        Some(StoreError::Invalid)
    );
    assert_eq!(
        store
            .import_file(&input, Path::new("../source"), spec("escaped", 100), None)
            .err(),
        Some(StoreError::Invalid)
    );
    for name in ["oversized", "mismatched", "bad-digest", "escaped"] {
        assert!(store.receipt(name)?.is_none());
    }
    assert!(!workspace.join("missing").exists());
    assert_eq!(fs::read(workspace.join("source"))?, b"input bytes");
    Ok(())
}

#[test]
fn workspace_import_preserves_collisions_and_cancelled_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let workspace = fixture.root.join("workspace");
    fs::create_dir(&workspace)?;
    fs::write(workspace.join("source"), b"first")?;
    let input = craft_platform::InputRoot::open(&workspace)?;
    let receipt = store.import_file(&input, Path::new("source"), spec("selected", 100), None)?;
    fs::write(workspace.join("source"), b"second")?;
    assert_eq!(
        store
            .import_file(&input, Path::new("source"), spec("selected", 100), None)
            .err(),
        Some(StoreError::Conflict)
    );
    let token = CancellationToken::default();
    let cancelled = NativeArtifacts::open(&fixture.root, 100, &control(token.clone()))?;
    token.cancel();
    assert_eq!(
        cancelled
            .import_file(&input, Path::new("missing"), spec("cancelled", 100), None)
            .err(),
        Some(StoreError::Cancelled)
    );
    assert_eq!(
        store
            .read("selected", 100)?
            .ok_or("Missing original")?
            .bytes,
        b"first"
    );
    assert_eq!(store.receipt("selected")?, Some(receipt));
    assert!(store.receipt("cancelled")?.is_none());
    assert!(!workspace.join("missing").exists());
    Ok(())
}

#[test]
#[cfg(windows)]
fn complete_artifacts_export_to_raw_files_with_explicit_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_platform::{OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let output_fixture = OutputFixture::new(&fixture)?;
    let output_path = output_fixture.path.clone();
    let output = OutputRoot::open(&output_path)?;
    for (name, bytes) in [
        ("empty-export", Vec::new()),
        ("unicode-export", "日本語 🦀\n".repeat(5000).into_bytes()),
        (
            "binary-export",
            (0..200_000).map(|value| (value % 251) as u8).collect(),
        ),
    ] {
        let mut writer = store.begin(spec(name, 300_000))?;
        writer.write_all(&bytes)?;
        let original = writer.finish()?;
        let destination = format!("{name}-日本語.bin");
        let exported = store.export_file(
            name,
            &output,
            Path::new(&destination),
            OverwritePolicy::default(),
        )?;
        assert_eq!(exported.bytes, original.bytes);
        assert_eq!(exported.digest, original.digest);
        assert_eq!(
            fs::read(exported.local_path.as_ref().ok_or("Missing export path")?)?,
            bytes
        );
        assert_eq!(
            store
                .export_file(
                    name,
                    &output,
                    Path::new(&destination),
                    OverwritePolicy::CreateNew
                )
                .err(),
            Some(StoreError::Conflict)
        );
        fs::write(output_path.join(&destination), b"selected old bytes")?;
        store.export_file(
            name,
            &output,
            Path::new(&destination),
            OverwritePolicy::Replace,
        )?;
        assert_eq!(fs::read(output_path.join(destination))?, bytes);
    }
    Ok(())
}

#[test]
#[cfg(windows)]
fn corrupt_artifact_export_never_replaces_a_selected_file() -> Result<(), Box<dyn std::error::Error>>
{
    use craft_platform::{OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("source", 100))?;
    writer.write_all(b"original")?;
    let receipt = writer.finish()?;
    fs::write(
        receipt.local_path.as_ref().ok_or("Missing source")?,
        b"tampered",
    )?;
    let output_fixture = OutputFixture::new(&fixture)?;
    let output_path = output_fixture.path.clone();
    fs::write(output_path.join("selected"), b"preserved")?;
    let output = OutputRoot::open(&output_path)?;
    assert_eq!(
        store
            .export_file(
                "source",
                &output,
                Path::new("selected"),
                OverwritePolicy::Replace
            )
            .err(),
        Some(StoreError::Corrupt)
    );
    assert_eq!(fs::read(output_path.join("selected"))?, b"preserved");
    assert_eq!(fs::read_dir(&output_path)?.count(), 1);
    Ok(())
}

#[test]
#[cfg(windows)]
fn export_failures_distinguish_before_publication_from_lost_acknowledgment()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_platform::{InputRoot, OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("source", 100))?;
    writer.write_all(b"complete")?;
    let source = writer.finish()?;
    let output_fixture = OutputFixture::new(&fixture)?;
    let output_path = output_fixture.path.clone();
    fs::write(output_path.join("selected"), b"old")?;
    let output = OutputRoot::open(&output_path)?;
    store.fault.store(3, Ordering::SeqCst);
    assert_eq!(
        store
            .export_file(
                "source",
                &output,
                Path::new("selected"),
                OverwritePolicy::Replace
            )
            .err(),
        Some(StoreError::Unavailable)
    );
    assert_eq!(fs::read(output_path.join("selected"))?, b"old");
    store.fault.store(4, Ordering::SeqCst);
    assert_eq!(
        store
            .export_file(
                "source",
                &output,
                Path::new("selected"),
                OverwritePolicy::Replace
            )
            .err(),
        Some(StoreError::OutcomeUnknown)
    );
    let inspection = InputRoot::open(&output_path)?;
    let mut selected = inspection.read_file(Path::new("selected"))?;
    let mut bytes = Vec::new();
    selected.read_to_end(&mut bytes)?;
    assert_eq!(content_digest(&bytes), source.digest);
    assert_eq!(bytes.len() as u64, source.bytes);
    assert_eq!(fs::read_dir(&output_path)?.count(), 1);
    Ok(())
}

#[test]
#[cfg(windows)]
fn export_cancellation_preserves_the_confirmed_receipt_after_publication()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_platform::{OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    let store = NativeArtifacts::open(&fixture.root, 100, &control(token.clone()))?;
    let mut writer = store.begin(spec("source", 100))?;
    writer.write_all(b"complete")?;
    writer.finish()?;
    let output_fixture = OutputFixture::new(&fixture)?;
    let output_path = output_fixture.path.clone();
    let output = OutputRoot::open(&output_path)?;
    *store
        .cancel_after_publish
        .lock()
        .map_err(|_| "Poisoned cancellation fixture")? = Some(token.clone());
    let receipt = store.export_file(
        "source",
        &output,
        Path::new("selected"),
        OverwritePolicy::CreateNew,
    )?;
    assert!(token.is_cancelled());
    assert_eq!(
        fs::read(receipt.local_path.as_ref().ok_or("Missing output path")?)?,
        b"complete"
    );
    assert_eq!(
        store
            .export_file(
                "source",
                &output,
                Path::new("absent"),
                OverwritePolicy::CreateNew
            )
            .err(),
        Some(StoreError::Cancelled)
    );
    assert!(!output_path.join("absent").exists());
    assert_eq!(fs::read_dir(&output_path)?.count(), 1);
    Ok(())
}

#[test]
#[cfg(windows)]
fn export_cancellation_after_staging_sync_preserves_the_existing_destination()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_platform::{OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    let store = NativeArtifacts::open(&fixture.root, 100, &control(token.clone()))?;
    let mut writer = store.begin(spec("source", 100))?;
    writer.write_all(b"complete")?;
    writer.finish()?;
    let output_fixture = OutputFixture::new(&fixture)?;
    let output_path = output_fixture.path.clone();
    fs::write(output_path.join("selected"), b"preserved")?;
    let output = OutputRoot::open(&output_path)?;
    *store
        .cancel_before_export
        .lock()
        .map_err(|_| "Poisoned cancellation fixture")? = Some(token.clone());
    assert_eq!(
        store
            .export_file(
                "source",
                &output,
                Path::new("selected"),
                OverwritePolicy::Replace
            )
            .err(),
        Some(StoreError::Cancelled)
    );
    assert!(token.is_cancelled());
    assert_eq!(fs::read(output_path.join("selected"))?, b"preserved");
    assert_eq!(fs::read_dir(&output_path)?.count(), 1);
    Ok(())
}

#[test]
#[cfg(windows)]
fn exports_cannot_replace_or_create_inside_immutable_artifact_storage()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_platform::{OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    for (name, bytes) in [
        ("source", b"new bytes".as_slice()),
        ("preserved", b"old bytes".as_slice()),
    ] {
        let mut writer = store.begin(spec(name, 100))?;
        writer.write_all(bytes)?;
        writer.finish()?;
    }
    let output = OutputRoot::open(&fixture.root)?;
    let selected = PathBuf::from(object_name("preserved")?).join("data");
    assert_eq!(
        store
            .export_file("source", &output, &selected, OverwritePolicy::Replace)
            .err(),
        Some(StoreError::Invalid)
    );
    assert_eq!(
        store
            .export_file(
                "source",
                &output,
                Path::new("new-file"),
                OverwritePolicy::CreateNew
            )
            .err(),
        Some(StoreError::Invalid)
    );
    let nested = OutputRoot::open(&fixture.root.join(object_name("preserved")?))?;
    assert_eq!(
        store
            .export_file(
                "source",
                &nested,
                Path::new("data"),
                OverwritePolicy::Replace
            )
            .err(),
        Some(StoreError::Invalid)
    );
    assert_eq!(
        store
            .read("preserved", 100)?
            .ok_or("Missing protected artifact")?
            .bytes,
        b"old bytes"
    );
    assert!(!fixture.root.join("new-file").exists());
    assert_eq!(
        fs::read_dir(fixture.root.join(object_name("preserved")?))?.count(),
        3
    );
    Ok(())
}

#[test]
fn complete_artifacts_reopen_with_exact_digest_and_direct_data_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("unicode", "日本語🦀\n".repeat(5_000).into_bytes()),
        (
            "binary",
            (0..150_000).map(|index| (index % 251) as u8).collect(),
        ),
    ] {
        let mut writer = store.begin(spec(name, 200_000))?;
        for chunk in bytes.chunks(1_701) {
            writer.write_all(chunk)?;
        }
        let receipt = writer.finish()?;
        assert_eq!(receipt.digest, content_digest(&bytes));
        assert_eq!(receipt.bytes, bytes.len() as u64);
        assert_eq!(
            fs::read(receipt.local_path.as_ref().ok_or("Missing data path")?)?,
            bytes
        );
        let reopened = fixture.open()?;
        assert_eq!(reopened.receipt(name)?, Some(receipt.clone()));
        let blob = reopened
            .read(name, 200_000)?
            .ok_or("Missing reopened artifact")?;
        assert_eq!(blob.bytes, bytes);
        assert_eq!(blob.receipt, receipt);
        if !bytes.is_empty() {
            assert_eq!(reopened.read(name, 1).err(), Some(StoreError::TooLarge));
        }
    }
    let before = fs::read_dir(&fixture.root)?.count();
    assert!(store.receipt("missing")?.is_none());
    assert!(store.read("missing", 100)?.is_none());
    assert_eq!(fs::read_dir(&fixture.root)?.count(), before);
    Ok(())
}

#[test]
fn traversal_aliases_and_oversized_or_failed_writes_cannot_publish()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "a/b",
        "a\\b",
        "C:stream",
        "a\0b",
    ] {
        assert!(store.begin(spec(name, 100)).is_err());
        assert!(store.receipt(name).is_err());
    }
    assert!(store.begin(spec("zero", 0)).is_err());
    assert!(store.begin(spec("large", 2_000_001)).is_err());
    for name in ["CON", "NUL", "Foo", "foo", "trailing."] {
        let mut writer = store.begin(spec(name, 100))?;
        writer.write_all(name.as_bytes())?;
        writer.finish()?;
        assert_eq!(
            store.read(name, 100)?.ok_or("Lost logical alias")?.bytes,
            name.as_bytes()
        );
    }
    let mut writer = store.begin(spec("oversized", 3))?;
    writer.write_all(b"ok")?;
    assert!(writer.write_all(b"over").is_err());
    assert_eq!(writer.finish().err(), Some(StoreError::TooLarge));
    assert!(store.receipt("oversized")?.is_none());
    let mut dropped = store.begin(spec("dropped", 100))?;
    dropped.write_all(b"partial")?;
    drop(dropped);
    assert!(store.receipt("dropped")?.is_none());
    assert_eq!(store.cleanup_pending()?, 0);
    Ok(())
}

#[test]
fn before_and_after_publication_failures_are_reconciled_without_overwrite()
-> Result<(), Box<dyn std::error::Error>> {
    for fault in [1, 2] {
        let fixture = Fixture::new()?;
        let store = fixture.open()?;
        let mut writer = store.begin(spec("receipt", 100))?;
        writer.write_all(b"confirmed")?;
        store.fault.store(fault, Ordering::SeqCst);
        assert_eq!(
            writer.finish().err(),
            Some(if fault == 1 {
                StoreError::Unavailable
            } else {
                StoreError::OutcomeUnknown
            })
        );
        let fresh = fixture.open()?;
        if fault == 1 {
            assert!(fresh.receipt("receipt")?.is_none());
        } else {
            assert_eq!(
                fresh.read("receipt", 100)?.ok_or("Lost publication")?.bytes,
                b"confirmed"
            );
            assert_eq!(
                fresh.begin(spec("receipt", 100)).err(),
                Some(StoreError::Conflict)
            );
        }
    }
    Ok(())
}

#[test]
fn competing_native_writers_have_one_winner_and_cannot_replace_it()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [b"first".as_slice(), b"second".as_slice()]
        .into_iter()
        .map(|bytes| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || -> Result<ArtifactReceipt, StoreError> {
                let mut writer = store.begin(spec("contended", 100))?;
                writer
                    .write_all(bytes)
                    .map_err(|_| StoreError::Unavailable)?;
                barrier.wait();
                writer.finish()
            })
        })
        .collect();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().map_err(|_| "Writer thread panicked"))
        .collect::<Result<_, _>>()?;
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| **result == Err(StoreError::Conflict))
            .count(),
        1
    );
    let original = store.read("contended", 100)?.ok_or("Missing winner")?;
    assert!(original.bytes == b"first" || original.bytes == b"second");
    assert_eq!(
        store.begin(spec("contended", 100)).err(),
        Some(StoreError::Conflict)
    );
    assert_eq!(
        store.read("contended", 100)?.ok_or("Lost winner")?.bytes,
        original.bytes
    );
    Ok(())
}

#[test]
fn cancellation_refuses_uncommitted_artifacts_and_does_not_block_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    let store = NativeArtifacts::open(&fixture.root, 1_000, &control(token.clone()))?;
    let mut writer = store.begin(spec("cancelled", 100))?;
    writer.write_all(b"pending")?;
    token.cancel();
    assert_eq!(writer.finish().err(), Some(StoreError::Cancelled));
    assert_eq!(
        store.receipt("cancelled").err(),
        Some(StoreError::Cancelled)
    );
    let fresh = fixture.open()?;
    assert!(fresh.receipt("cancelled")?.is_none());
    fresh.cleanup_pending()?;
    assert!(
        !fs::read_dir(&fixture.root)?.any(
            |entry| entry.is_ok_and(|entry| staging_name(&entry.file_name().to_string_lossy()))
        )
    );
    Ok(())
}

#[test]
fn cancellation_after_publication_preserves_the_confirmed_local_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    let store = NativeArtifacts::open(&fixture.root, 1_000, &control(token.clone()))?;
    *store
        .cancel_after_publish
        .lock()
        .map_err(|_| "Poisoned test hook")? = Some(token.clone());
    let mut writer = store.begin(spec("confirmed", 100))?;
    writer.write_all(b"complete")?;
    let receipt = writer.finish()?;
    assert!(token.is_cancelled());
    assert_eq!(fixture.open()?.receipt("confirmed")?, Some(receipt));
    Ok(())
}

#[test]
fn root_lock_waits_observe_deadlines_and_cancellation() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let _held = store.lock()?;
    let deadline = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30,
    )?;
    let waiting = NativeArtifacts::open(&fixture.root, 1_000, &deadline)?;
    assert_eq!(
        waiting.begin(spec("deadline", 100)).err(),
        Some(StoreError::DeadlineExceeded)
    );
    let token = CancellationToken::default();
    let waiting = NativeArtifacts::open(&fixture.root, 1_000, &control(token.clone()))?;
    let cancel = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(15));
        token.cancel();
    });
    assert_eq!(
        waiting.begin(spec("cancelled-wait", 100)).err(),
        Some(StoreError::Cancelled)
    );
    cancel.join().map_err(|_| "Cancellation thread panicked")?;
    Ok(())
}

#[test]
fn publication_preserves_even_empty_destinations_and_cleanup_refuses_unknown_children()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("collision", 100))?;
    writer.write_all(b"candidate")?;
    let destination = store.root.create_directory(&object_name("collision")?)?;
    drop(destination);
    assert_eq!(writer.finish().err(), Some(StoreError::Conflict));
    assert_eq!(
        fs::read_dir(fixture.root.join(object_name("collision")?))?.count(),
        0
    );
    assert!(store.receipt("collision").is_err());
    let pending = random_id("pending_")?;
    let directory = store.root.create_directory(&pending)?;
    let mut data = directory.create_new_file("data")?;
    data.write_all(b"preserve")?;
    drop(data);
    let unknown = directory.create_new_file("unrecognized")?;
    drop(unknown);
    drop(directory);
    assert_eq!(store.cleanup_pending().err(), Some(StoreError::Corrupt));
    assert_eq!(
        fs::read(fixture.root.join(&pending).join("data"))?,
        b"preserve"
    );
    assert!(!fixture.root.join(&pending).join("lease").exists());
    fs::remove_file(fixture.root.join(&pending).join("unrecognized"))?;
    assert_eq!(store.cleanup_pending()?, 1);
    Ok(())
}

#[test]
#[cfg(windows)]
fn junctions_cannot_redirect_reads_publication_or_staging_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    fn junction(link: &Path, target: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let link = link.to_str().ok_or("Invalid fixture link")?;
        let target = target.to_str().ok_or("Invalid fixture target")?;
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:CRAFT_TEST_LINK -Target $env:CRAFT_TEST_TARGET | Out-Null"])
            .env("CRAFT_TEST_LINK", link.strip_prefix(r"\\?\").unwrap_or(link))
            .env("CRAFT_TEST_TARGET", target.strip_prefix(r"\\?\").unwrap_or(target))
            .creation_flags(0x0800_0000)
            .output()?;
        assert!(
            output.status.success(),
            "Junction fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let target = store.root.create_directory("target")?;
    let mut data = target.create_new_file("data")?;
    data.write_all(b"untouched")?;
    drop(data);
    let target_path = target.path().to_owned();
    drop(target);
    let mut writer = store.begin(spec("junction", 100))?;
    writer.write_all(b"refused")?;
    let destination = fixture.root.join(object_name("junction")?);
    junction(&destination, &target_path)?;
    assert!(store.receipt("junction").is_err());
    assert_eq!(writer.finish().err(), Some(StoreError::Conflict));
    fs::remove_dir(&destination)?;
    let abandoned_link = fixture.root.join(random_id("pending_")?);
    junction(&abandoned_link, &target_path)?;
    assert!(store.cleanup_pending().is_err());
    assert_eq!(fs::read(target_path.join("data"))?, b"untouched");
    assert!(!target_path.join("lease").exists());
    fs::remove_dir(abandoned_link)?;
    Ok(())
}

#[test]
fn corrupt_receipts_data_and_hard_links_never_return_complete_artifacts()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("bound", 100))?;
    writer.write_all(b"payload")?;
    let receipt = writer.finish()?;
    let directory = fixture.root.join(object_name("bound")?);
    let metadata = directory.join("receipt.json");
    let original = fs::read(&metadata)?;
    let record: serde_json::Value = serde_json::from_slice(&original)?;
    let mut variants = Vec::new();
    for (field, value) in [
        ("local_path", serde_json::json!("C:\\untrusted\\escape")),
        ("name", serde_json::json!("different")),
        ("digest", serde_json::json!(content_digest(b"different"))),
        ("bytes", serde_json::json!(1000)),
        ("media_type", serde_json::json!("bad\nheader")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut corrupt = record.clone();
        corrupt["receipt"][field] = value;
        variants.push(serde_json::to_vec(&corrupt)?);
    }
    variants.push(vec![b'x'; METADATA_LIMIT as usize + 1]);
    variants.push(b"{\"format\":1,\"format\":1}".to_vec());
    for bytes in variants {
        fs::write(&metadata, bytes)?;
        assert!(store.receipt("bound").is_err());
        assert!(store.read("bound", 100).is_err());
    }
    fs::write(&metadata, original)?;
    assert_eq!(store.receipt("bound")?, Some(receipt));
    let data = directory.join("data");
    fs::write(&data, b"corrupt")?;
    assert_eq!(store.receipt("bound").err(), Some(StoreError::Corrupt));
    fs::write(&data, b"payload")?;
    let alias = fixture.root.join("hard-link");
    fs::hard_link(&data, &alias)?;
    assert!(store.read("bound", 100).is_err());
    fs::remove_file(alias)?;
    assert!(store.read("bound", 100)?.is_some());
    Ok(())
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn artifact_writer_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("CRAFT_ARTIFACT_CHILD_ROOT") else {
        return Ok(());
    };
    let store = NativeArtifacts::open(
        Path::new(&root),
        1_000,
        &control(CancellationToken::default()),
    )?;
    if std::env::var("CRAFT_ARTIFACT_CHILD_MODE").as_deref() == Ok("read") {
        let artifact = store
            .read("restart", 100)?
            .ok_or("Missing fresh-process artifact")?;
        assert_eq!(artifact.bytes, b"child-visible");
        assert_eq!(artifact.receipt.digest, content_digest(b"child-visible"));
        return Ok(());
    }
    let mut writer = store.begin(spec("crashed", 100))?;
    writer.write_all(b"unfinished")?;
    writeln!(std::io::stdout(), "artifact-writer-ready")?;
    std::io::stdout().flush()?;
    let mut signal = [0];
    std::io::stdin().read_exact(&mut signal)?;
    Ok(())
}

#[test]
fn cleanup_skips_live_processes_and_reclaims_only_abandoned_staging()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut child = ChildGuard(
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "artifacts::tests::artifact_writer_child",
                "--nocapture",
            ])
            .env("CRAFT_ARTIFACT_CHILD_ROOT", &fixture.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let stdout = child.0.stdout.take().ok_or("Missing child stdout")?;
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout.take(4_096))
            .lines()
            .map_while(Result::ok)
        {
            if line == "artifact-writer-ready" {
                let _ = sender.send(());
                break;
            }
        }
    });
    receiver.recv_timeout(Duration::from_secs(15))?;
    assert_eq!(store.cleanup_pending()?, 0);
    assert!(store.receipt("crashed")?.is_none());
    child.0.kill()?;
    child.0.wait()?;
    reader.join().map_err(|_| "Child output reader panicked")?;
    assert_eq!(store.cleanup_pending()?, 1);
    assert_eq!(store.cleanup_pending()?, 0);
    assert!(store.receipt("crashed")?.is_none());
    let mut writer = store.begin(spec("final", 100))?;
    writer.write_all(b"retained")?;
    writer.finish()?;
    assert_eq!(store.cleanup_pending()?, 0);
    assert!(store.receipt("final")?.is_some());
    Ok(())
}

#[test]
fn published_receipts_and_bytes_survive_fresh_process_reads()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_test_support::process::{RunOptions, run};
    use std::ffi::OsString;
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("restart", 100))?;
    writer.write_all(b"child-visible")?;
    writer.finish()?;
    let output = run(
        &std::env::current_exe()?,
        &["--exact", "artifacts::tests::artifact_writer_child"],
        None,
        RunOptions {
            max_stream_bytes: 16_384,
            environment: vec![
                (
                    OsString::from("CRAFT_ARTIFACT_CHILD_ROOT"),
                    Some(fixture.root.clone().into()),
                ),
                (
                    OsString::from("CRAFT_ARTIFACT_CHILD_MODE"),
                    Some("read".into()),
                ),
            ],
            ..RunOptions::default()
        },
    )?;
    assert!(
        output.status.success(),
        "Artifact read child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn verification_ceiling_does_not_misclassify_a_complete_artifact_as_corrupt()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("bounded", 100))?;
    writer.write_all(b"complete")?;
    let receipt = writer.finish()?;
    let smaller = NativeArtifacts::open(&fixture.root, 2, &control(CancellationToken::default()))?;
    assert_eq!(smaller.receipt("bounded").err(), Some(StoreError::TooLarge));
    assert_eq!(store.receipt("bounded")?, Some(receipt));
    Ok(())
}
#[test]
#[cfg(windows)]
fn digest_bound_export_rejects_a_changed_observation_before_output_staging()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_platform::{OutputRoot, OverwritePolicy};
    let fixture = Fixture::new()?;
    let store = fixture.open()?;
    let mut writer = store.begin(spec("bound", 100))?;
    writer.write_all(b"complete")?;
    let receipt = writer.finish()?;
    let output_fixture = OutputFixture::new(&fixture)?;
    let output = OutputRoot::open(&output_fixture.path)?;
    fs::write(output_fixture.path.join("existing"), b"preserve")?;
    let before = fs::read_dir(&output_fixture.path)?.count();
    for digest in ["invalid".to_owned(), format!("sha256:{}", "0".repeat(64))] {
        let result = store.export_file_checked(
            "bound",
            &output,
            Path::new("existing"),
            OverwritePolicy::Replace,
            &digest,
        );
        assert_eq!(
            result.err(),
            Some(if digest == "invalid" {
                StoreError::Invalid
            } else {
                StoreError::Conflict
            })
        );
        assert_eq!(fs::read(output_fixture.path.join("existing"))?, b"preserve");
        assert_eq!(fs::read_dir(&output_fixture.path)?.count(), before);
    }
    let exported = store.export_file_checked(
        "bound",
        &output,
        Path::new("existing"),
        OverwritePolicy::Replace,
        &receipt.digest,
    )?;
    assert_eq!(exported.digest, receipt.digest);
    assert_eq!(fs::read(output_fixture.path.join("existing"))?, b"complete");
    Ok(())
}

#[test]
fn verified_input_stream_retains_exact_snapshot_rejects_wrong_digest_and_obeys_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    let control = control(token.clone());
    let store = NativeArtifacts::open(&fixture.root, 200_000, &control)?;
    let bytes = (0..160_013).map(|n| (n % 256) as u8).collect::<Vec<_>>();
    let mut writer = store.begin(spec("streamed", 200_000))?;
    writer.write_all(&bytes)?;
    let receipt = writer.finish()?;
    assert!(matches!(
        store.open_verified_stream("streamed", &format!("sha256:{}", "0".repeat(64))),
        Err(StoreError::Conflict)
    ));
    let mut source = store.open_verified_stream("streamed", &receipt.digest)?;
    let mut observed = Vec::new();
    source.read_to_end(&mut observed)?;
    assert_eq!(observed, bytes);
    assert_eq!(source.receipt(), &receipt);
    assert!(!source.is_fresh());
    let mut cancelled = store.open_verified_stream("streamed", &receipt.digest)?;
    token.cancel();
    let mut byte = [0];
    let error = cancelled.read(&mut byte).err().ok_or("cancelled")?;
    assert_eq!(
        error
            .get_ref()
            .and_then(|v| v.downcast_ref::<StoreError>())
            .copied(),
        Some(StoreError::Cancelled)
    );
    Ok(())
}
