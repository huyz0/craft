use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{Result, reject};

pub(crate) fn credential_path(variable: &str) -> Option<PathBuf> {
    std::env::var_os(format!("{variable}_FILE"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("CREDENTIALS_DIRECTORY").map(|path| PathBuf::from(path).join(variable))
        })
}

pub fn require_new_directory(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => reject("evidence destination already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn create_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    Ok(())
}

pub fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

pub fn replace_json(path: &Path, report: &serde_json::Value) -> Result<()> {
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).map_err(|_| "could not generate a report filename")?;
    let temporary = path.with_extension(format!("{:016x}.pending", u64::from_be_bytes(nonce)));
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    write_new(&temporary, &bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

pub(crate) fn check_output_paths<'a>(
    outputs: &[Option<&Path>],
    inputs: impl IntoIterator<Item = &'a Path>,
    conflict: &'static str,
) -> Result<()> {
    let resolve = |path: &Path| -> Result<PathBuf> {
        match fs::canonicalize(path) {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = path.file_name().ok_or("provide an output filename")?;
                let parent = path
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                Ok(fs::canonicalize(parent)?.join(name))
            }
            Err(error) => Err(error.into()),
        }
    };
    let same_path = |first: &Path, second: &Path| {
        first == second
            || (cfg!(windows)
                && first
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&second.to_string_lossy()))
    };
    let mut resolved = Vec::new();
    for output in outputs.iter().flatten() {
        let path = resolve(output)?;
        if resolved
            .iter()
            .any(|prior: &PathBuf| same_path(prior, &path))
        {
            return reject(conflict);
        }
        resolved.push(path);
    }
    if !resolved.is_empty()
        && inputs.into_iter().any(|input| {
            resolve(input)
                .is_ok_and(|input| resolved.iter().any(|output| same_path(output, &input)))
        })
    {
        return reject(conflict);
    }
    Ok(())
}

pub fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
