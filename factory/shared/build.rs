use std::{env, fmt::Write, fs, path::PathBuf};

use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=migrations");
    let mut paths = fs::read_dir("migrations")?.collect::<Result<Vec<_>, _>>()?;
    paths.sort_by_key(|entry| entry.file_name());
    let mut manifest = Vec::new();
    for entry in paths {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "sql") {
            continue;
        }
        let number = manifest.len() + 1;
        let filename = entry.file_name();
        let filename = filename.to_str().ok_or("non-UTF-8 migration filename")?;
        if !filename.starts_with(&format!("{number:03}_")) {
            return Err(format!("unexpected migration order at {filename}").into());
        }
        let text = fs::read_to_string(path)?.replace("\r\n", "\n");
        let mut row = format!("{number} {filename} ");
        for byte in Sha256::digest(text) {
            write!(row, "{byte:02x}")?;
        }
        manifest.push(row);
    }
    if manifest.is_empty() {
        return Err("no queue migrations found".into());
    }
    let destination =
        PathBuf::from(env::var_os("OUT_DIR").ok_or("missing Cargo output directory")?);
    fs::write(
        destination.join("queue-migration-manifest.txt"),
        manifest.join("\n"),
    )?;
    Ok(())
}
