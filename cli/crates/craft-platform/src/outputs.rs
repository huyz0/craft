//! Explicit output authority with private staging and handle-bound publication.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(any(windows, unix))]
use super::InputRoot;
#[cfg(any(windows, unix))]
use std::fs::File;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverwritePolicy {
    #[default]
    CreateNew,
    Replace,
}

#[derive(Debug)]
pub enum OutputCommitError {
    NotPublished(io::Error),
    OutcomeUnknown,
}

impl std::fmt::Display for OutputCommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPublished(error) => write!(formatter, "Output was not published: {error}"),
            Self::OutcomeUnknown => {
                formatter.write_str("Output publication is uncertain; inspect before retry")
            }
        }
    }
}

impl std::error::Error for OutputCommitError {}

pub struct OutputRoot {
    #[cfg(any(windows, unix))]
    protected: Option<super::PrivateRoot>,
    #[cfg(any(windows, unix))]
    _root: InputRoot,
    #[cfg(any(windows, unix))]
    path: PathBuf,
    _private: (),
}

pub struct OutputFile {
    #[cfg(any(windows, unix))]
    file: Option<File>,
    #[cfg(any(windows, unix))]
    _parent: InputRoot,
    /// Unix publication is anchored to the pinned parent handle by these two names.
    #[cfg(unix)]
    staging: String,
    #[cfg(unix)]
    leaf: String,
    destination: PathBuf,
    maximum: u64,
    bytes: u64,
    failed: bool,
    #[cfg(any(windows, unix))]
    prepared: bool,
    #[cfg(any(windows, unix))]
    attempted: bool,
    #[cfg(any(windows, unix))]
    published: bool,
    #[cfg(any(windows, unix))]
    policy: OverwritePolicy,
}

#[cfg(not(any(windows, unix)))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Anchored output publication is unavailable on this platform",
    )
}

impl OutputRoot {
    /// Checks absolute output-root syntax without opening the filesystem.
    pub fn validate_root(path: &Path) -> io::Result<()> {
        super::inputs::root_parts(path).map(|_| ())
    }
    /// Checks portable relative output syntax without opening a directory or creating a file.
    pub fn validate_destination(path: &Path) -> io::Result<()> {
        super::inputs::relative(path).map(|_| ())
    }
    /// Selects an existing no-follow output root. Input authority does not imply output authority.
    pub fn open(path: &Path) -> io::Result<Self> {
        #[cfg(any(windows, unix))]
        {
            Ok(Self {
                _root: InputRoot::open(path)?,
                protected: None,
                path: path.to_owned(),
                _private: (),
            })
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = path;
            Err(unsupported())
        }
    }

    /// Keeps an additional private authority excluded from every destination parent,
    /// even when the selected output root itself is outside that authority.
    pub fn open_excluding(path: &Path, protected: super::PrivateRoot) -> io::Result<Self> {
        #[cfg(any(windows, unix))]
        {
            let mut root = Self::open(path)?;
            root.protected = Some(protected);
            Ok(root)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (path, protected);
            Err(unsupported())
        }
    }

    /// Starts a bounded file in an existing checked parent. The generated staging identity
    /// comes from the host; it cannot be a provider filename or an existing destination.
    pub fn begin(
        &self,
        destination: &Path,
        staging: &str,
        maximum: u64,
        policy: OverwritePolicy,
    ) -> io::Result<OutputFile> {
        self.begin_impl(destination, staging, maximum, policy, None)
    }

    /// Refuses a checked destination whose actual ancestry contains the protected root.
    /// Native identity comparison also covers alternate spellings of the same directory.
    pub fn begin_excluding(
        &self,
        destination: &Path,
        staging: &str,
        maximum: u64,
        policy: OverwritePolicy,
        protected: &super::PrivateRoot,
    ) -> io::Result<OutputFile> {
        self.begin_impl(destination, staging, maximum, policy, Some(protected))
    }

    fn begin_impl(
        &self,
        destination: &Path,
        staging: &str,
        maximum: u64,
        policy: OverwritePolicy,
        protected: Option<&super::PrivateRoot>,
    ) -> io::Result<OutputFile> {
        #[cfg(any(windows, unix))]
        {
            let parts = super::inputs::relative(destination)?;
            let leaf = parts.last().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Missing output filename")
            })?;
            if maximum > 1_073_741_824
                || leaf.eq_ignore_ascii_case(staging)
                || !staging.strip_prefix("export_").is_some_and(|value| {
                    value.len() == 32
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid output staging or byte ceiling",
                ));
            }
            let mut parent_path = self.path.clone();
            for part in parts.iter().take(parts.len() - 1) {
                parent_path.push(part);
            }
            let parent = InputRoot::open(&parent_path)?;
            if let Some(protected) = protected
                && parent.contains_private_root(protected)?
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Output destination is inside protected artifact storage",
                ));
            }
            if let Some(protected) = &self.protected
                && parent.contains_private_root(protected)?
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Output destination is inside protected private state",
                ));
            }
            let destination = parent_path.join(leaf);
            match std::fs::symlink_metadata(&destination) {
                Ok(_) if policy == OverwritePolicy::CreateNew => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Output destination already exists",
                    ));
                }
                Ok(_) => {
                    parent.read_file(Path::new(leaf))?.verify_unchanged()?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            #[cfg(windows)]
            let file = super::windows::create_output_file(&parent_path.join(staging))?;
            #[cfg(unix)]
            let file = unix::create_staging(&parent, staging)?;
            Ok(OutputFile {
                file: Some(file),
                _parent: parent,
                #[cfg(unix)]
                staging: staging.to_owned(),
                #[cfg(unix)]
                leaf: (*leaf).to_owned(),
                destination,
                maximum,
                bytes: 0,
                failed: false,
                prepared: false,
                attempted: false,
                published: false,
                policy,
            })
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (destination, staging, maximum, policy, protected);
            Err(unsupported())
        }
    }
}

impl OutputFile {
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Synchronizes staging without publishing it, so the host can recheck its invocation
    /// after potentially slow I/O. Any later write invalidates this prepared state.
    pub fn prepare(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("Output writer failed"));
        }
        #[cfg(any(windows, unix))]
        {
            self.prepared = false;
            let file = self
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("Output writer is closed"))?;
            #[cfg(windows)]
            let result = file
                .flush()
                .and_then(|()| file.sync_all())
                .and_then(|()| super::windows::input_file_identity(file).map(|_| ()));
            #[cfg(unix)]
            let result = file
                .flush()
                .and_then(|()| file.sync_all())
                .and_then(|()| unix::verify_staging(&self._parent, &self.staging, file));
            self.prepared = result.is_ok();
            if result.is_err() {
                self.failed = true;
            }
            result
        }
        #[cfg(not(any(windows, unix)))]
        {
            Err(unsupported())
        }
    }

    /// Flushes complete staging before atomic rename. Once rename is attempted, an
    /// unclassified error cannot be represented as a known absence of filesystem effects.
    #[cfg(any(windows, unix))]
    pub fn commit(mut self) -> Result<(), OutputCommitError> {
        if self.failed {
            return Err(OutputCommitError::NotPublished(io::Error::other(
                "Output writer failed",
            )));
        }
        if !self.prepared {
            self.prepare().map_err(OutputCommitError::NotPublished)?;
        }
        let file = self.file.as_mut().ok_or_else(|| {
            OutputCommitError::NotPublished(io::Error::other("Output writer is closed"))
        })?;
        self.attempted = true;
        #[cfg(windows)]
        let published = super::windows::publish_output_file(
            file,
            &self.destination,
            self.policy == OverwritePolicy::Replace,
        );
        #[cfg(unix)]
        let published = {
            let _ = file;
            unix::publish(
                &self._parent,
                &self.staging,
                &self.leaf,
                self.policy == OverwritePolicy::Replace,
            )
        };
        match published {
            Ok(()) => {
                self.published = true;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.attempted = false;
                Err(OutputCommitError::NotPublished(error))
            }
            Err(_) => Err(OutputCommitError::OutcomeUnknown),
        }
    }

    #[cfg(not(any(windows, unix)))]
    pub fn commit(self) -> Result<(), OutputCommitError> {
        Err(OutputCommitError::NotPublished(unsupported()))
    }
}

impl Write for OutputFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed
            || self
                .bytes
                .checked_add(bytes.len() as u64)
                .is_none_or(|next| next > self.maximum)
        {
            self.failed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Output byte ceiling exceeded or writer failed",
            ));
        }
        #[cfg(any(windows, unix))]
        {
            self.prepared = false;
            let result = self
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("Output writer is closed"))?
                .write(bytes);
            match result {
                Ok(count) => {
                    self.bytes += count as u64;
                    Ok(count)
                }
                Err(error) => {
                    self.failed = true;
                    Err(error)
                }
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            Err(unsupported())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("Output writer failed"));
        }
        #[cfg(any(windows, unix))]
        {
            let result = self
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("Output writer is closed"))?
                .flush();
            if result.is_err() {
                self.failed = true;
            }
            result
        }
        #[cfg(not(any(windows, unix)))]
        {
            Err(unsupported())
        }
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        #[cfg(windows)]
        if !self.attempted
            && !self.published
            && let Some(file) = self.file.as_ref()
        {
            // Abort refers to the retained staging object, never a re-opened path.
            let _ = super::windows::discard_output_file(file);
        }
        #[cfg(unix)]
        if !self.attempted && !self.published && self.file.take().is_some() {
            // The name is resolved only inside the pinned parent and only while it is ours.
            let _ = unix::discard(&self._parent, &self.staging);
        }
    }
}

/// Directory-handle-relative staging and publication. Every name is resolved inside the
/// pinned no-follow parent, so a concurrently swapped path component cannot redirect bytes.
#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::MetadataExt;

    use nix::fcntl::{AtFlags, OFlag, openat, renameat};
    use nix::sys::stat::{Mode, fstatat};
    use nix::unistd::{UnlinkatFlags, linkat, unlinkat};

    use super::InputRoot;

    fn os(error: nix::Error) -> io::Error {
        io::Error::from_raw_os_error(error as i32)
    }

    pub(super) fn create_staging(parent: &InputRoot, staging: &str) -> io::Result<File> {
        let descriptor = openat(
            parent.directory()?,
            staging,
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(os)?;
        Ok(File::from(descriptor))
    }

    /// The staging name must still be our single-link regular file before publication.
    pub(super) fn verify_staging(parent: &InputRoot, staging: &str, file: &File) -> io::Result<()> {
        let held = file.metadata()?;
        let named =
            fstatat(parent.directory()?, staging, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(os)?;
        if !held.is_file()
            || held.nlink() != 1
            || named.st_dev as u64 != held.dev()
            || named.st_ino as u64 != held.ino()
        {
            return Err(io::Error::other(
                "Output staging changed before publication",
            ));
        }
        Ok(())
    }

    /// Create-new publication uses a hard link, which refuses an existing destination
    /// atomically on every Unix; the staging name is then removed. Replacement uses rename.
    pub(super) fn publish(
        parent: &InputRoot,
        staging: &str,
        leaf: &str,
        replace: bool,
    ) -> io::Result<()> {
        let directory = parent.directory()?;
        if replace {
            return renameat(directory, staging, directory, leaf).map_err(os);
        }
        linkat(directory, staging, directory, leaf, AtFlags::empty()).map_err(os)?;
        // The destination is complete at this point; leftover staging is only clutter.
        let _ = unlinkat(directory, staging, UnlinkatFlags::NoRemoveDir);
        Ok(())
    }

    pub(super) fn discard(parent: &InputRoot, staging: &str) -> io::Result<()> {
        unlinkat(parent.directory()?, staging, UnlinkatFlags::NoRemoveDir).map_err(os)
    }
}
