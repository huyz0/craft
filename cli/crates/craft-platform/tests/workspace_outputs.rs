#![cfg(windows)]

use craft_platform::{OutputRoot, OverwritePolicy};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    parent: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new() -> std::io::Result<Self> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        let path = parent.join(format!(
            "workspace-output-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self { parent, path })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}
fn staging(value: u32) -> String {
    format!("export_{value:032x}")
}

#[test]
fn complete_output_is_published_by_handle_with_unicode_and_pinned_parents()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.path.join("nested"))?;
    let root = OutputRoot::open(&fixture.path)?;
    let bytes = "日本語 🦀\n".repeat(5000).into_bytes();
    let temporary = staging(1);
    let mut writer = root.begin(
        Path::new("nested/日本語 🦀.bin"),
        &temporary,
        bytes.len() as u64,
        OverwritePolicy::CreateNew,
    )?;
    drop(root);
    for part in bytes.chunks(1701) {
        writer.write_all(part)?;
    }
    assert!(!fixture.path.join("nested/日本語 🦀.bin").exists());
    assert!(fs::rename(fixture.path.join("nested"), fixture.path.join("moved")).is_err());
    assert!(
        fs::rename(
            fixture.path.join("nested").join(&temporary),
            fixture.path.join("other-stage")
        )
        .is_err()
    );
    writer.commit()?;
    assert_eq!(fs::read(fixture.path.join("nested/日本語 🦀.bin"))?, bytes);
    assert!(!fixture.path.join("nested").join(temporary).exists());
    Ok(())
}

#[test]
fn output_collisions_require_explicit_replace_and_keep_existing_directories()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(fixture.path.join("selected"), b"old")?;
    fs::create_dir(fixture.path.join("directory"))?;
    let root = OutputRoot::open(&fixture.path)?;
    assert!(
        root.begin(
            Path::new("selected"),
            &staging(1),
            3,
            OverwritePolicy::CreateNew
        )
        .is_err()
    );
    assert!(
        root.begin(
            Path::new("directory"),
            &staging(1),
            3,
            OverwritePolicy::Replace
        )
        .is_err()
    );
    let mut writer = root.begin(
        Path::new("selected"),
        &staging(2),
        3,
        OverwritePolicy::Replace,
    )?;
    writer.write_all(b"new")?;
    assert_eq!(fs::read(fixture.path.join("selected"))?, b"old");
    writer.commit()?;
    assert_eq!(fs::read(fixture.path.join("selected"))?, b"new");
    assert!(fixture.path.join("directory").is_dir());
    Ok(())
}

#[test]
fn abandoned_or_failed_output_removes_only_its_owned_staging()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(fixture.path.join("unrelated"), b"keep")?;
    let root = OutputRoot::open(&fixture.path)?;
    let stage = staging(1);
    let mut writer = root.begin(Path::new("absent"), &stage, 3, OverwritePolicy::CreateNew)?;
    writer.write_all(b"abc")?;
    assert!(
        root.begin(Path::new("other"), &stage, 3, OverwritePolicy::CreateNew)
            .is_err()
    );
    drop(writer);
    assert!(!fixture.path.join(&stage).exists());
    let mut writer = root.begin(
        Path::new("absent"),
        &staging(2),
        2,
        OverwritePolicy::CreateNew,
    )?;
    assert!(writer.write_all(b"too large").is_err());
    assert!(writer.commit().is_err());
    assert!(!fixture.path.join("absent").exists());
    assert!(!fixture.path.join(staging(2)).exists());
    assert_eq!(fs::read(fixture.path.join("unrelated"))?, b"keep");
    Ok(())
}

#[test]
fn racing_create_new_outputs_have_exactly_one_complete_winner()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let winners = std::thread::scope(|scope| -> Result<usize, Box<dyn std::error::Error>> {
        let (ready, prepared) = std::sync::mpsc::channel();
        let mut workers = Vec::new();
        let mut starts = Vec::new();
        for value in [1, 2] {
            let path = fixture.path.clone();
            let ready = ready.clone();
            let (start, released) = std::sync::mpsc::channel();
            starts.push(start);
            workers.push(scope.spawn(
                move || -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
                    let root = OutputRoot::open(&path)?;
                    let mut writer = root.begin(
                        Path::new("selected"),
                        &staging(value),
                        200_000,
                        OverwritePolicy::CreateNew,
                    )?;
                    writer.write_all(&vec![value as u8; 200_000])?;
                    ready.send(())?;
                    released.recv_timeout(std::time::Duration::from_secs(5))?;
                    Ok(writer.commit().is_ok())
                },
            ));
        }
        for _ in 0..2 {
            prepared.recv_timeout(std::time::Duration::from_secs(5))?;
        }
        for start in starts {
            start.send(())?;
        }
        let mut winners = 0;
        for worker in workers {
            if worker
                .join()
                .map_err(|_| "Output worker panicked")?
                .map_err(|error| error.to_string())?
            {
                winners += 1;
            }
        }
        Ok(winners)
    })?;
    assert_eq!(winners, 1);
    let bytes = fs::read(fixture.path.join("selected"))?;
    assert_eq!(bytes.len(), 200_000);
    assert!(bytes.iter().all(|byte| *byte == 1) || bytes.iter().all(|byte| *byte == 2));
    Ok(())
}

#[test]
fn output_paths_and_linked_destinations_cannot_escape_the_selected_root()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    let fixture = Fixture::new()?;
    let output = fixture.path.join("output");
    let outside = fixture.path.join("outside");
    fs::create_dir(&output)?;
    fs::create_dir(&outside)?;
    fs::write(outside.join("data"), b"outside")?;
    fs::hard_link(outside.join("data"), output.join("linked-file"))?;
    let root = OutputRoot::open(&output)?;
    for selected in [
        "../escaped",
        "NUL.txt",
        "data:stream",
        "missing/selected",
        "linked-file",
    ] {
        assert!(
            root.begin(
                Path::new(selected),
                &staging(1),
                10,
                OverwritePolicy::Replace
            )
            .is_err(),
            "{selected}"
        );
    }
    assert!(
        root.begin(
            &outside.join("data"),
            &staging(1),
            10,
            OverwritePolicy::Replace
        )
        .is_err()
    );
    let link = output.join("junction");
    let link_text = link.to_str().ok_or("Expected link path")?;
    let target_text = outside.to_str().ok_or("Expected target path")?;
    let created = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:CRAFT_TEST_LINK -Target $env:CRAFT_TEST_TARGET | Out-Null"])
        .env("CRAFT_TEST_LINK", link_text.strip_prefix(r"\\?\").unwrap_or(link_text))
        .env("CRAFT_TEST_TARGET", target_text.strip_prefix(r"\\?\").unwrap_or(target_text))
        .creation_flags(0x0800_0000).output()?;
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let root_refused = OutputRoot::open(&link).is_err();
    let child_refused = root
        .begin(
            Path::new("junction/new"),
            &staging(1),
            10,
            OverwritePolicy::CreateNew,
        )
        .is_err();
    fs::remove_dir(&link)?;
    assert!(root_refused && child_refused);
    assert!(!outside.join("new").exists());
    assert_eq!(fs::read(outside.join("data"))?, b"outside");
    Ok(())
}
