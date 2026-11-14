use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::storage::{ArtifactSpec, ArtifactStore};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::control::SystemClock;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
type TestResult = Result<(), Box<dyn std::error::Error>>;
struct Files {
    parent: PathBuf,
    base: PathBuf,
    state: PathBuf,
    output: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let base = parent.join(format!("artifact-process-{stamp}"));
        fs::create_dir(&base)?;
        let state = base.join("state");
        let output = base.join("output");
        fs::create_dir(&output)?;
        Ok(Self {
            parent,
            base,
            state,
            output,
        })
    }
    fn seed(&self) -> Result<craft_core::storage::ArtifactReceipt, Box<dyn std::error::Error>> {
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            10_000,
        )?;
        let artifacts = NativeArtifacts::open(&self.state.join("downloads"), 100, &control)?;
        let mut writer = artifacts.begin(ArtifactSpec {
            name: "recovered.bin".into(),
            media_type: "application/octet-stream".into(),
            max_bytes: 100,
            source: None,
        })?;
        writer.write_all(b"recovery-binary\0\xff")?;
        Ok(writer.finish()?)
    }
    fn run(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let mut arguments = vec!["--state-root", self.state.to_str().ok_or("state")?];
        arguments.extend_from_slice(args);
        let result = craft_test_support::process::run(
            Path::new(env!("CARGO_BIN_EXE_craft")),
            &arguments,
            input,
            Default::default(),
        )?;
        assert!(
            result.stderr.is_empty(),
            "{:?}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(serde_json::from_slice(&result.stdout)?)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if let Ok(path) = self.base.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}
#[test]
#[cfg(windows)]
fn executable_recovers_saved_bytes_with_strict_json_and_never_acquires_a_profile() -> TestResult {
    let files = Files::new()?;
    let receipt = files.seed()?;
    let inspected = files.run(
        &["artifact", "get", "--artifact-name", "recovered.bin"],
        None,
    )?;
    assert_eq!(inspected["ok"], true);
    assert_eq!(inspected["data"]["artifact"]["digest"], receipt.digest);
    assert_eq!(inspected["data"]["replay_authorized"], false);
    let input = serde_json::to_vec(
        &json!({"artifact_name":"recovered.bin","expected_digest":receipt.digest,"output_root":files.output,"output":"copied.bin"}),
    )?;
    let exported = files.run(&["artifact", "export", "--input", "-"], Some(&input))?;
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(exported["data"]["export_write_state"], "confirmed");
    assert_eq!(
        fs::read(files.output.join("copied.bin"))?,
        b"recovery-binary\0\xff"
    );
    assert!(!exported.to_string().contains("recovery-binary"));
    let collision = files.run(&["artifact", "export", "--input", "-"], Some(&input))?;
    assert_eq!(collision["ok"], false);
    assert_eq!(collision["data"]["export_write_state"], "failed");
    assert_eq!(collision["data"]["integrity_verified"], true);
    let flags = files.run(
        &[
            "artifact",
            "export",
            "--artifact-name",
            "recovered.bin",
            "--expected-digest",
            &receipt.digest,
            "--output-root",
            files.output.to_str().ok_or("output")?,
            "--output",
            "flags.bin",
        ],
        None,
    )?;
    assert_eq!(flags["ok"], true, "{flags}");
    let profile = files.run(
        &[
            "artifact",
            "get",
            "--artifact-name",
            "recovered.bin",
            "--profile",
            "factory",
        ],
        None,
    )?;
    assert_eq!(profile["ok"], false);
    assert!(!files.state.join("state.sqlite3").exists());
    let mut contracts = craft_core::registry::Registry::default();
    craft_cli::local_artifacts::register(&mut contracts)?;
    for (value, command) in [
        (&inspected, "artifact.get"),
        (&exported, "artifact.export"),
        (&collision, "artifact.export"),
    ] {
        assert!(
            jsonschema::validator_for(&contracts.descriptor(command)?.response_schema)?
                .is_valid(value)
        );
    }
    Ok(())
}
#[test]
fn malformed_artifact_input_and_projection_fail_before_private_state_creation() -> TestResult {
    let files = Files::new()?;
    let output = files.output.to_str().ok_or("output")?;
    for args in [
        vec!["artifact", "get", "--artifact-name", ".."],
        vec![
            "artifact",
            "get",
            "--artifact-name",
            "valid.bin",
            "--fields",
            "artifact",
        ],
        vec![
            "artifact",
            "export",
            "--artifact-name",
            "valid.bin",
            "--expected-digest",
            "invalid",
            "--output-root",
            output,
            "--output",
            "file.bin",
        ],
        vec![
            "artifact",
            "export",
            "--artifact-name",
            "valid.bin",
            "--expected-digest",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "--output-root",
            output,
            "--output",
            "../escape",
        ],
    ] {
        assert_eq!(files.run(&args, None)?["ok"], false);
        assert!(!files.state.exists());
    }
    let unknown =
        serde_json::to_vec(&json!({"artifact_name":"valid.bin","collection":"../requests"}))?;
    assert_eq!(
        files.run(&["artifact", "get", "--input", "-"], Some(&unknown))?["ok"],
        false
    );
    assert!(!files.state.exists());
    Ok(())
}
#[test]
fn executable_reports_corruption_as_unknown_presence_and_missing_as_a_local_observation()
-> TestResult {
    let files = Files::new()?;
    let receipt = files.seed()?;
    let missing = files.run(&["artifact", "get", "--artifact-name", "missing.bin"], None)?;
    assert_eq!(missing["ok"], true);
    assert_eq!(missing["data"]["found"], false);
    let size = fs::metadata(receipt.local_path.as_ref().ok_or("source")?)?.len();
    fs::write(receipt.local_path.ok_or("source")?, vec![0; size as usize])?;
    let corrupt = files.run(
        &["artifact", "get", "--artifact-name", "recovered.bin"],
        None,
    )?;
    assert_eq!(corrupt["ok"], false);
    assert!(corrupt["data"]["found"].is_null());
    assert_eq!(corrupt["data"]["integrity_verified"], false);
    Ok(())
}
