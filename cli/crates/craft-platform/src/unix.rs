use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(super) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    // The NUL-terminated paths remain live through the synchronous call. Exclusive flags
    // preserve an existing destination; unsupported filesystems fail without a fallback.
    #[cfg(target_os = "linux")]
    let status = unsafe {
        nix::libc::renameat2(
            nix::libc::AT_FDCWD,
            source.as_ptr(),
            nix::libc::AT_FDCWD,
            destination.as_ptr(),
            nix::libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let status = unsafe {
        nix::libc::renamex_np(
            source.as_ptr(),
            destination.as_ptr(),
            nix::libc::RENAME_EXCL,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Exclusive rename unsupported",
    ));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
