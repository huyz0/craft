use super::*;

fn name_path(root: &PrivateRoot, name: &str) -> io::Result<PathBuf> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid private entry name",
        ));
    }
    Ok(root.path.join(name))
}

impl PrivateRoot {
    fn verify_file(&self, file: &File) -> io::Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(denied());
        }
        #[cfg(windows)]
        windows::verify(file, &self.principal, false)?;
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
        Ok(())
    }

    /// Creates one private regular file without reopening or replacing an existing entry.
    pub fn create_new_file(&self, name: &str) -> io::Result<File> {
        let path = name_path(self, name)?;
        #[cfg(windows)]
        let file = windows::create_file(&path, &self.principal)?;
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(path)?
        };
        #[cfg(not(any(windows, unix)))]
        let file: File = return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Private files unsupported",
        ));
        self.verify_file(&file)?;
        Ok(file)
    }

    /// Opens an existing regular file read-only, refusing links and insecure permissions.
    pub fn read_file(&self, name: &str) -> io::Result<File> {
        self.existing_file(name, false)
    }

    /// Opens an existing private file for OS locking without creating a missing lease.
    pub fn lock_file(&self, name: &str) -> io::Result<File> {
        self.existing_file(name, true)
    }

    fn existing_file(&self, name: &str, writable: bool) -> io::Result<File> {
        let path = name_path(self, name)?;
        let mut options = OpenOptions::new();
        options.read(true).write(writable);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .share_mode(FILE_SHARE_READ | if writable { FILE_SHARE_WRITE } else { 0 })
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        self.verify_file(&file)?;
        Ok(file)
    }

    /// Creates one new private child directory; an existing entry is a collision.
    pub fn create_directory(&self, name: &str) -> io::Result<PrivateRoot> {
        let path = name_path(self, name)?;
        #[cfg(windows)]
        windows::create_directory(&path, &self.principal)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        PrivateRoot::open_existing(&path)
    }

    /// Opens a fixed child without creating it; the returned guard pins its checked path.
    pub fn directory(&self, name: &str) -> io::Result<PrivateRoot> {
        PrivateRoot::open_existing(&name_path(self, name)?)
    }

    /// Publishes a closed staging directory atomically, without replacement or copy fallback.
    /// Callers retain this root guard and serialize their own staging ownership transitions.
    pub fn publish_directory(&self, source: &str, destination: &str) -> io::Result<()> {
        let source = name_path(self, source)?;
        let destination = name_path(self, destination)?;
        let checked = PrivateRoot::open_existing(&source)?;
        drop(checked);
        #[cfg(windows)]
        return windows::rename_no_replace(&source, &destination);
        #[cfg(unix)]
        return unix::rename_no_replace(&source, &destination);
        #[cfg(not(any(windows, unix)))]
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Atomic publication unsupported",
        ))
    }
}
