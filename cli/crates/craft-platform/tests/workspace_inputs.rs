use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use craft_platform::InputRoot;

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
            "workspace-input-{}-{stamp}-{}",
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

#[test]
fn ordinary_workspace_inputs_are_read_only_existing_and_unicode_capable()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let missing = fixture.path.join("missing");
    assert!(InputRoot::open(&missing).is_err());
    assert!(!missing.exists());
    fs::create_dir(fixture.path.join("nested"))?;
    let bytes = "日本語 🦀\n".repeat(5000).into_bytes();
    fs::write(fixture.path.join("nested/日本語 🦀.bin"), &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(
            fixture.path.join("nested/日本語 🦀.bin"),
            fs::Permissions::from_mode(0o644),
        )?;
    }
    let root = InputRoot::open(&fixture.path)?;
    assert!(root.read_file(Path::new("absent")).is_err());
    assert!(!fixture.path.join("absent").exists());
    let mut input = root.read_file(Path::new("nested/日本語 🦀.bin"))?;
    assert_eq!(input.size(), bytes.len() as u64);
    let mut actual = Vec::new();
    input.read_to_end(&mut actual)?;
    input.verify_unchanged()?;
    assert_eq!(actual, bytes);
    assert_eq!(fs::read(fixture.path.join("nested/日本語 🦀.bin"))?, bytes);
    Ok(())
}

#[test]
fn input_selection_refuses_traversal_aliases_and_nonregular_entries()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(fixture.path.join("data"), b"unchanged")?;
    fs::create_dir(fixture.path.join("directory"))?;
    let root = InputRoot::open(&fixture.path)?;
    for value in [
        "",
        ".",
        "..",
        "../data",
        "directory/../data",
        "directory/./data",
        "directory//data",
        "directory/",
        "directory",
    ] {
        assert!(root.read_file(Path::new(value)).is_err(), "{value}");
    }
    assert!(root.read_file(&fixture.path.join("data")).is_err());
    let mut traversal = fixture.path.as_os_str().to_os_string();
    traversal.push(std::path::MAIN_SEPARATOR_STR);
    traversal.push(".");
    traversal.push(std::path::MAIN_SEPARATOR_STR);
    traversal.push("directory");
    assert!(InputRoot::open(&PathBuf::from(traversal)).is_err());
    #[cfg(windows)]
    for value in [
        "data:stream",
        "NUL",
        "CON.txt",
        "COM1",
        "LPT¹.txt",
        "data.",
        "data ",
        "directory\\..\\data",
        "C:relative",
        "\\\\server\\share\\data",
    ] {
        assert!(root.read_file(Path::new(value)).is_err(), "{value}");
    }
    assert_eq!(fs::read(fixture.path.join("data"))?, b"unchanged");
    Ok(())
}

#[test]
fn workspace_hard_links_are_refused_before_bytes_are_read() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::new()?;
    let source = fixture.path.join("source");
    let alias = fixture.path.join("alias");
    fs::write(&source, b"input")?;
    fs::hard_link(&source, &alias)?;
    let root = InputRoot::open(&fixture.path)?;
    assert!(root.read_file(Path::new("source")).is_err());
    assert!(root.read_file(Path::new("alias")).is_err());
    fs::remove_file(&alias)?;
    root.read_file(Path::new("source"))?.verify_unchanged()?;
    Ok(())
}

#[test]
#[cfg(windows)]
fn open_workspace_input_pins_ancestry_and_refuses_source_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let workspace = fixture.path.join("workspace");
    let moved = fixture.path.join("moved");
    fs::create_dir(&workspace)?;
    fs::create_dir(workspace.join("nested"))?;
    fs::write(workspace.join("nested/data"), b"stable")?;
    let root = InputRoot::open(&workspace)?;
    let input = root.read_file(Path::new("nested/data"))?;
    drop(root);
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .open(workspace.join("nested/data"))
            .is_err()
    );
    assert!(fs::rename(workspace.join("nested"), workspace.join("nested-moved")).is_err());
    assert!(fs::rename(&workspace, &moved).is_err());
    input.verify_unchanged()?;
    drop(input);
    fs::rename(&workspace, &moved)?;
    fs::rename(&moved, &workspace)?;
    fs::write(workspace.join("nested/data"), b"later")?;
    Ok(())
}

#[test]
#[cfg(windows)]
fn workspace_junction_roots_and_descendants_never_select_external_data()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    let fixture = Fixture::new()?;
    let workspace = fixture.path.join("workspace");
    let outside = fixture.path.join("outside");
    fs::create_dir(&workspace)?;
    fs::create_dir(&outside)?;
    fs::write(outside.join("data"), b"outside")?;
    let link = workspace.join("linked");
    let link_text = link.to_str().ok_or("Expected input link path")?;
    let target_text = outside.to_str().ok_or("Expected input target path")?;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:CRAFT_TEST_LINK -Target $env:CRAFT_TEST_TARGET | Out-Null"])
        .env("CRAFT_TEST_LINK", link_text.strip_prefix(r"\\?\").unwrap_or(link_text))
        .env("CRAFT_TEST_TARGET", target_text.strip_prefix(r"\\?\").unwrap_or(target_text))
        .creation_flags(0x0800_0000).output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = InputRoot::open(&workspace)?;
    let root_refused = InputRoot::open(&link).is_err();
    let descendant_refused = root.read_file(Path::new("linked/data")).is_err();
    fs::remove_dir(&link)?;
    assert!(root_refused && descendant_refused);
    assert_eq!(fs::read(outside.join("data"))?, b"outside");
    Ok(())
}

#[test]
#[cfg(unix)]
fn unix_descriptor_inputs_refuse_symlinks_and_detect_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(fixture.path.join("source"), b"before")?;
    std::os::unix::fs::symlink("source", fixture.path.join("linked"))?;
    let root = InputRoot::open(&fixture.path)?;
    assert!(root.read_file(Path::new("linked")).is_err());
    let input = root.read_file(Path::new("source"))?;
    fs::write(fixture.path.join("source"), b"changed length")?;
    assert!(input.verify_unchanged().is_err());
    fs::write(fixture.path.join("colon:name"), b"literal Unix name")?;
    root.read_file(Path::new("colon:name"))?
        .verify_unchanged()?;
    Ok(())
}
