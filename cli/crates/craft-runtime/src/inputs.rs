//! Host-selected project input, without resolving a provider-supplied filesystem target.

use std::path::{Path, PathBuf};

use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_platform::InputRoot;

fn invalid() -> CraftError {
    CraftError::new(
        ErrorCode::InputInvalid,
        "Input must be an accessible, unchanged regular file without links or traversal.",
    )
}

fn outside() -> CraftError {
    CraftError::new(
        ErrorCode::PolicyDenied,
        "Input file is outside the current project root.",
    )
}

fn relative(root: &Path, selected: &Path) -> Result<PathBuf, CraftError> {
    let text = selected.to_str().ok_or_else(invalid)?;
    if text.len() > 4096
        || text
            .split(|value| value == '/' || cfg!(windows) && value == '\\')
            .any(|part| matches!(part, "." | ".."))
    {
        return Err(invalid());
    }
    if !selected.is_absolute() {
        return Ok(selected.to_owned());
    }
    #[cfg(windows)]
    {
        let root_text = root.to_str().ok_or_else(invalid)?;
        let root = Path::new(root_text.strip_prefix(r"\\?\").unwrap_or(root_text));
        let selected = Path::new(text.strip_prefix(r"\\?\").unwrap_or(text));
        let mut remaining = selected.components();
        for expected in root.components() {
            let actual = remaining.next().ok_or_else(outside)?;
            if !actual
                .as_os_str()
                .to_str()
                .ok_or_else(invalid)?
                .eq_ignore_ascii_case(expected.as_os_str().to_str().ok_or_else(invalid)?)
            {
                return Err(outside());
            }
        }
        Ok(remaining.as_path().to_owned())
    }
    #[cfg(not(windows))]
    {
        selected
            .strip_prefix(root)
            .map(Path::to_owned)
            .map_err(|_| outside())
    }
}

/// Reads selected project input through retained native guards and the bounded worker.
/// Absolute paths must be lexically contained; no canonicalization follows source links.
pub fn read_project_file(
    root: &Path,
    selected: &Path,
    maximum: usize,
    control: &InvocationControl,
) -> Result<Vec<u8>, CraftError> {
    control.check()?;
    let selected = relative(root, selected)?;
    let root = InputRoot::open(root).map_err(|_| invalid())?;
    let source = root.read_file(&selected).map_err(|_| invalid())?;
    crate::control::read_bounded(source, maximum, control)
}
