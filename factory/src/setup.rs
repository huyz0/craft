use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use crate::process_tree::read_bounded;
use crate::{Result, reject};

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PROBE_OUTPUT: u64 = 64 * 1024;

pub fn doctor(
    state: &Path,
    codex: Option<PathBuf>,
    home: Option<PathBuf>,
    gh: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let (status, git_version) = probe(Command::new("git").arg("--version"), "Git version probe")?;
    if !status.success() {
        return reject("Git is unavailable");
    }
    let state = crate::store::Store::open(state)?;
    state.probe_writes()?;
    let codex_ready = match (codex, home) {
        (None, None) => None,
        (Some(executable), Some(home)) if home.is_absolute() && home.is_dir() => Some(verify(
            &executable,
            &["login", "status"],
            Some(&home),
            "Codex authentication probe",
        )?),
        _ => {
            return reject(
                "supply both an absolute Codex executable and its existing dedicated home",
            );
        }
    };
    let gh_ready = gh
        .as_deref()
        .map(|executable| {
            verify(
                executable,
                &["auth", "status", "--hostname", "github.com"],
                None,
                "GitHub authentication probe",
            )
        })
        .transpose()?;
    Ok(serde_json::json!({
        "schema_version": 1,
        "git": String::from_utf8_lossy(&git_version).trim(),
        "sqlite_writable": true,
        "codex_authenticated": codex_ready,
        "github_authenticated": gh_ready,
        "ready": codex_ready != Some(false) && gh_ready != Some(false)
    }))
}

fn verify(executable: &Path, args: &[&str], home: Option<&Path>, operation: &str) -> Result<bool> {
    if !executable.is_absolute() || !executable.is_file() {
        return reject("setup probes require absolute paths to existing executables");
    }
    #[cfg(windows)]
    if executable
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
    {
        return reject("Windows setup probes require executable files");
    }
    let mut command = Command::new(executable);
    if let Some(home) = home {
        command.env("CODEX_HOME", home);
    }
    let (status, _) = probe(command.args(args), operation)?;
    Ok(status.success())
}

fn probe(command: &mut Command, operation: &str) -> Result<(ExitStatus, Vec<u8>)> {
    command.stdin(Stdio::null()).stderr(Stdio::null());
    read_bounded(command, operation, PROBE_TIMEOUT, MAX_PROBE_OUTPUT)
}
