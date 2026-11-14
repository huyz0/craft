//! Native private-state paths. Guards retain checked handles for the caller's state lifetime.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

mod console;
mod entries;
mod inputs;
mod outputs;
pub use console::{ConsoleInput, InputEvent};
pub use inputs::{InputFile, InputRoot};
pub use outputs::{OutputCommitError, OutputFile, OutputRoot, OverwritePolicy};

/// Checks stdin's native source kind without reading bytes or changing terminal state.
pub fn stdin_is_pipe() -> io::Result<bool> {
    #[cfg(windows)]
    {
        windows::stdin_is_pipe()
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        use std::os::unix::fs::FileTypeExt;
        let file = File::from(std::io::stdin().as_fd().try_clone_to_owned()?);
        let kind = file.metadata()?.file_type();
        Ok(kind.is_fifo() || kind.is_socket())
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Native pipe inspection is unavailable",
        ))
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

/// Identifies the process credential principal without enumerating credentials or prompting.
/// Thread impersonation is not part of the supported credential execution model.
pub fn current_process_principal() -> io::Result<String> {
    #[cfg(windows)]
    {
        windows::current_principal()
    }
    #[cfg(unix)]
    {
        Ok(format!("uid:{}", nix::unistd::geteuid().as_raw()))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Native principal identity is unavailable",
        ))
    }
}

pub struct PrivateRoot {
    path: PathBuf,
    _pins: Vec<File>,
    #[cfg(windows)]
    principal: String,
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "State path is not private or contains a link",
    )
}

fn plain_directory(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(denied());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(denied());
        }
    }
    Ok(file)
}

impl PrivateRoot {
    /// Creates missing directories privately; existing entries must already pass verification.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_impl(path, true)
    }

    /// Verifies an existing private root without creating a missing path component.
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        Self::open_impl(path, false)
    }

    fn open_impl(path: &Path, create: bool) -> io::Result<Self> {
        if !path.is_absolute()
            || path.to_str().is_none()
            || path.components().any(|part| {
                matches!(part, Component::CurDir | Component::ParentDir)
                    || part.as_os_str() == "."
                    || part.as_os_str() == ".."
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "State root must be an absolute path without traversal",
            ));
        }
        #[cfg(windows)]
        {
            use std::path::Prefix;
            if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "State root must use a local disk path",
                ));
            }
        }
        #[cfg(windows)]
        let principal = windows::current_principal()?;
        let mut current = PathBuf::new();
        let mut pins = Vec::new();
        for component in path.components() {
            current.push(component.as_os_str());
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            match fs::symlink_metadata(&current) {
                Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                    return Err(denied());
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if !create {
                        return Err(error);
                    }
                    #[cfg(windows)]
                    let result = windows::create_directory(&current, &principal);
                    #[cfg(unix)]
                    let result = {
                        use std::os::unix::fs::DirBuilderExt;
                        fs::DirBuilder::new().mode(0o700).create(&current)
                    };
                    #[cfg(not(any(windows, unix)))]
                    let result: io::Result<()> = Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Private state is unsupported on this OS",
                    ));
                    if let Err(error) = result
                        && error.kind() != io::ErrorKind::AlreadyExists
                    {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
            let pin = plain_directory(&current)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let metadata = pin.metadata()?;
                let uid = nix::unistd::geteuid().as_raw();
                if metadata.uid() != uid && metadata.uid() != 0 {
                    return Err(denied());
                }
                if metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0 {
                    return Err(denied());
                }
            }
            pins.push(pin);
        }
        let root = pins.last().ok_or_else(denied)?;
        #[cfg(windows)]
        windows::verify(root, &principal, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = root.metadata()?;
            if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
                return Err(denied());
            }
        }
        Ok(Self {
            path: path.to_owned(),
            _pins: pins,
            #[cfg(windows)]
            principal,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The fixed-name file is held without Windows delete sharing while its caller uses SQLite.
    pub fn open_file(&self, name: &str) -> io::Result<File> {
        if name.is_empty()
            || name.len() > 128
            || matches!(name, "." | "..")
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid state file name",
            ));
        }
        let path = self.path.join(name);
        #[cfg(windows)]
        let created = windows::create_file(&path, &self.principal);
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(&path)
        };
        #[cfg(not(any(windows, unix)))]
        let created: io::Result<File> = Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Private state is unsupported on this OS",
        ));
        let file = match created {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let mut options = OpenOptions::new();
                options.read(true).write(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    use windows_sys::Win32::Storage::FileSystem::{
                        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
                    };
                    options
                        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(nix::libc::O_NOFOLLOW);
                }
                options.open(&path)?
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(denied());
        }
        #[cfg(windows)]
        windows::verify(&file, &self.principal, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != nix::unistd::geteuid().as_raw()
                || metadata.mode() & 0o077 != 0
                || metadata.nlink() != 1
            {
                return Err(denied());
            }
        }
        Ok(file)
    }
}
