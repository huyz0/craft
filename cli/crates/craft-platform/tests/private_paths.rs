use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use craft_platform::PrivateRoot;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

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
            "private-state-{}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(Self { parent, path })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(resolved) = self.path.canonicalize()
            && resolved.starts_with(&self.parent)
            && resolved != self.parent
        {
            let _ = fs::remove_dir_all(resolved);
        }
    }
}

#[test]
fn creates_private_state_and_reopens_the_same_owned_files() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::new()?;
    {
        let root = PrivateRoot::open(&fixture.path)
            .map_err(|error| format!("Initial root verification: {error}"))?;
        let file = root
            .open_file("state.sqlite3")
            .map_err(|error| format!("Initial file verification: {error}"))?;
        assert!(file.metadata()?.is_file());
        assert_eq!(root.path(), fixture.path);
        assert!(root.open_file("../outside.sqlite3").is_err());
        assert!(root.open_file("a/b.sqlite3").is_err());
    }
    let reopened = PrivateRoot::open(&fixture.path)
        .map_err(|error| format!("Reopened root verification: {error}"))?;
    assert!(
        reopened
            .open_file("state.sqlite3")
            .map_err(|error| format!("Reopened file verification: {error}"))?
            .metadata()?
            .is_file()
    );
    Ok(())
}

#[test]
fn existing_reads_never_create_and_create_new_never_replaces()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let fixture = Fixture::new()?;
    assert!(PrivateRoot::open_existing(&fixture.path).is_err());
    assert!(!fixture.path.exists());
    let root = PrivateRoot::open(&fixture.path)?;
    assert!(root.directory("absent").is_err());
    assert!(root.read_file("absent").is_err());
    assert!(root.lock_file("absent").is_err());
    assert!(!fixture.path.join("absent").exists());
    let mut file = root.create_new_file("data")?;
    file.write_all(b"first")?;
    drop(file);
    assert!(root.create_new_file("data").is_err());
    let mut read = root.read_file("data")?;
    assert!(read.write_all(b"replacement").is_err());
    let mut bytes = Vec::new();
    read.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"first");
    Ok(())
}

#[test]
fn atomic_directory_publication_never_replaces_an_empty_destination()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let root = PrivateRoot::open(&fixture.path)?;
    drop(root.create_directory("staged")?);
    drop(root.create_directory("existing")?);
    assert!(root.publish_directory("staged", "existing").is_err());
    assert!(fixture.path.join("staged").is_dir());
    assert!(fixture.path.join("existing").is_dir());
    root.publish_directory("staged", "published")?;
    assert!(!fixture.path.join("staged").exists());
    assert!(fixture.path.join("published").is_dir());
    assert!(root.publish_directory("../escape", "destination").is_err());
    Ok(())
}

#[test]
fn refuses_relative_traversal_and_existing_unprotected_roots()
-> Result<(), Box<dyn std::error::Error>> {
    assert!(PrivateRoot::open(Path::new("relative-state")).is_err());
    let fixture = Fixture::new()?;
    fs::create_dir(&fixture.path)?;
    #[cfg(windows)]
    assert!(PrivateRoot::open(&fixture.path).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o755))?;
        assert!(PrivateRoot::open(&fixture.path).is_err());
    }
    let mut traversal = fixture.path.as_os_str().to_os_string();
    traversal.push(std::path::MAIN_SEPARATOR_STR);
    traversal.push("..");
    traversal.push(std::path::MAIN_SEPARATOR_STR);
    traversal.push("other");
    let traversal = PathBuf::from(traversal);
    assert!(PrivateRoot::open(&traversal).is_err());
    Ok(())
}

#[test]
#[cfg(windows)]
fn pinned_root_cannot_be_renamed_during_a_state_operation() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::new()?;
    let destination = fixture.path.with_extension("moved");
    let root = PrivateRoot::open(&fixture.path)?;
    assert!(fs::rename(&fixture.path, &destination).is_err());
    drop(root);
    fs::rename(&fixture.path, &destination)?;
    fs::rename(&destination, &fixture.path)?;
    Ok(())
}

#[test]
fn refuses_an_existing_hard_linked_database_file() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let root = PrivateRoot::open(&fixture.path)?;
    let file = root.open_file("state.sqlite3")?;
    fs::hard_link(
        root.path().join("state.sqlite3"),
        root.path().join("linked.sqlite3"),
    )?;
    assert!(root.open_file("linked.sqlite3").is_err());
    drop(file);
    Ok(())
}

#[test]
#[cfg(windows)]
fn refuses_junction_roots_and_ancestors_without_creating_in_the_target()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    let fixture = Fixture::new()?;
    fs::create_dir(&fixture.path)?;
    let target = fixture.path.join("target");
    let link = fixture.path.join("junction");
    let _target = PrivateRoot::open(&target)?;
    // Windows PowerShell's junction cmdlet does not accept canonical verbatim disk paths.
    let link_text = link.to_str().ok_or("Expected local link path")?;
    let target_text = target.to_str().ok_or("Expected local target path")?;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:CRAFT_TEST_LINK -Target $env:CRAFT_TEST_TARGET | Out-Null"])
        .env("CRAFT_TEST_LINK", link_text.strip_prefix(r"\\?\").unwrap_or(link_text))
        .env("CRAFT_TEST_TARGET", target_text.strip_prefix(r"\\?\").unwrap_or(target_text))
        .creation_flags(0x0800_0000)
        .output()?;
    assert!(
        output.status.success(),
        "Junction setup: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(PrivateRoot::open(&link).is_err());
    assert!(PrivateRoot::open(&link.join("nested")).is_err());
    assert!(!target.join("nested").exists());
    // Remove the junction entry itself before the fixture performs recursive cleanup.
    fs::remove_dir(&link)?;
    Ok(())
}
