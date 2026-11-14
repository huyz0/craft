//! Existing-only workspace inputs, separate from private state and output authority.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path};
use std::time::SystemTime;

#[cfg(windows)]
use std::{fs::OpenOptions, path::PathBuf};

pub struct InputRoot {
    #[cfg(windows)]
    path: PathBuf,
    pins: Vec<File>,
}

pub struct InputFile {
    file: File,
    _pins: Vec<File>,
    stamp: Stamp,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    identity: (u64, u64),
    bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "Invalid workspace input path")
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Workspace input contains a link or nonregular entry",
    )
}

fn name(value: &str) -> io::Result<()> {
    if value.is_empty() || matches!(value, "." | "..") || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    #[cfg(windows)]
    if value.ends_with(['.', ' '])
        || value.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'
            )
        })
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    let stem = value
        .split('.')
        .next()
        .ok_or_else(invalid)?
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    #[cfg(windows)]
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.chars().count() == 1
                && suffix
                    .chars()
                    .all(|character| matches!(character, '1'..='9' | '¹' | '²' | '³'))
        })
    }) {
        return Err(invalid());
    }
    Ok(())
}

fn separators(character: char) -> bool {
    character == '/' || cfg!(windows) && character == '\\'
}

pub(super) fn relative(path: &Path) -> io::Result<Vec<&str>> {
    let text = path.to_str().ok_or_else(invalid)?;
    if path.is_absolute() || text.len() > 4096 {
        return Err(invalid());
    }
    let parts: Vec<_> = text.split(separators).collect();
    if parts.is_empty() || parts.len() > 64 {
        return Err(invalid());
    }
    for part in &parts {
        name(part)?;
    }
    Ok(parts)
}

pub(super) fn root_parts(path: &Path) -> io::Result<Vec<&std::ffi::OsStr>> {
    let text = path.to_str().ok_or_else(invalid)?;
    if !path.is_absolute()
        || text.len() > 4096
        || text
            .split(separators)
            .any(|part| matches!(part, "." | ".."))
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if !matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        {
            return Err(invalid());
        }
    }
    let parts: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    if parts.len() > 64 {
        return Err(invalid());
    }
    for part in &parts {
        name(part.to_str().ok_or_else(invalid)?)?;
    }
    Ok(parts)
}

fn stamp(file: &File) -> io::Result<Stamp> {
    let metadata: Metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(refused());
    }
    #[cfg(windows)]
    let identity = super::windows::input_file_identity(file)?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(refused());
        }
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(any(windows, unix)))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Workspace inputs are unsupported",
    ));
    #[cfg(any(windows, unix))]
    Ok(Stamp {
        identity,
        bytes: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        changed: {
            use std::os::unix::fs::MetadataExt;
            (metadata.ctime(), metadata.ctime_nsec())
        },
    })
}

#[cfg(unix)]
fn unix_open(parent: Option<&File>, path: &Path, directory: bool) -> io::Result<File> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::Mode;
    let flags = OFlag::O_RDONLY
        | OFlag::O_NOFOLLOW
        | OFlag::O_CLOEXEC
        | if directory {
            OFlag::O_DIRECTORY
        } else {
            OFlag::O_NONBLOCK
        };
    let descriptor = match parent {
        Some(parent) => openat(parent, path, flags, Mode::empty()),
        None => open(path, flags, Mode::empty()),
    }
    .map_err(|error| io::Error::from_raw_os_error(error as i32))?;
    Ok(File::from(descriptor))
}

impl InputRoot {
    /// Checks absolute input-root syntax without filesystem access.
    pub fn validate_root(path: &Path) -> io::Result<()> {
        root_parts(path).map(|_| ())
    }
    /// Checks relative file syntax without opening or creating the source.
    pub fn validate_path(path: &Path) -> io::Result<()> {
        relative(path).map(|_| ())
    }
    #[cfg(windows)]
    pub(super) fn contains_private_root(&self, protected: &super::PrivateRoot) -> io::Result<bool> {
        let protected =
            super::windows::directory_identity(protected._pins.last().ok_or_else(invalid)?)?;
        for directory in &self.pins {
            if super::windows::directory_identity(directory)? == protected {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Selects an existing input directory without creating it or changing its permissions.
    /// The caller must supply this authority explicitly; remote content must not choose it.
    pub fn open(path: &Path) -> io::Result<Self> {
        let parts = root_parts(path)?;
        let mut pins = Vec::new();
        #[cfg(windows)]
        {
            let mut current = PathBuf::new();
            for component in path.components() {
                current.push(component.as_os_str());
                if !matches!(component, Component::Prefix(_)) {
                    pins.push(super::plain_directory(&current)?);
                }
            }
        }
        #[cfg(unix)]
        {
            pins.push(unix_open(None, Path::new("/"), true)?);
            for part in &parts {
                let parent = pins.last().ok_or_else(invalid)?;
                pins.push(unix_open(Some(parent), Path::new(part), true)?);
            }
        }
        #[cfg(not(any(windows, unix)))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Workspace inputs are unsupported",
        ));
        let _ = parts;
        if pins.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            #[cfg(windows)]
            path: path.to_owned(),
            pins,
        })
    }

    /// Opens one relative regular input read-only, keeping its checked ancestry pinned.
    pub fn read_file(&self, path: &Path) -> io::Result<InputFile> {
        let parts = relative(path)?;
        let (leaf, directories) = parts.split_last().ok_or_else(invalid)?;
        let mut pins = self
            .pins
            .iter()
            .map(File::try_clone)
            .collect::<io::Result<Vec<_>>>()?;
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            let mut current = self.path.clone();
            for directory in directories {
                current.push(directory);
                pins.push(super::plain_directory(&current)?);
            }
            current.push(leaf);
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(current)?
        };
        #[cfg(unix)]
        let file = {
            for directory in directories {
                let parent = pins.last().ok_or_else(invalid)?;
                pins.push(unix_open(Some(parent), Path::new(directory), true)?);
            }
            unix_open(
                Some(pins.last().ok_or_else(invalid)?),
                Path::new(leaf),
                false,
            )?
        };
        #[cfg(not(any(windows, unix)))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Workspace inputs are unsupported",
        ));
        #[cfg(any(windows, unix))]
        Ok(InputFile {
            stamp: stamp(&file)?,
            file,
            _pins: pins,
        })
    }
}

impl InputFile {
    pub fn size(&self) -> u64 {
        self.stamp.bytes
    }

    /// Refuses ordinary concurrent edits or link changes before a snapshot is published.
    /// This is change detection, not a lock against malicious same-principal Unix writers.
    pub fn verify_unchanged(&self) -> io::Result<()> {
        if stamp(&self.file)? != self.stamp {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Workspace input changed while reading",
            ));
        }
        Ok(())
    }
}

impl Read for InputFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read(buffer)?;
        if count == 0 && !buffer.is_empty() {
            self.verify_unchanged()?;
        }
        Ok(count)
    }
}
