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

#[test]
#[cfg(windows)]
fn scan_checkpoints_export_only_with_their_digest_and_never_claim_a_partial_is_complete()
-> TestResult {
    use craft_core::contracts::{GrantRef, ResourceRef};
    use craft_core::pagination::ListBinding;
    use craft_core::scan::ScanCaps;
    use craft_runtime::scan::{self, ScanCheckpoint, ScanInputs};
    use craft_test_support::paging::{Style, SyntheticService, dataset};
    let files = Files::new()?;
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
    )?;
    craft_runtime::profiles::ProfileStore::open(&files.state, &control)?;
    let digest = craft_core::input::content_digest(b"binding");
    let binding = ListBinding {
        operation: "jira.issue.search".into(),
        descriptor_digest: digest.clone(),
        schema_digest: digest.clone(),
        projection_digest: digest,
        api_revision: "v3".into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        },
        grant: GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        actor: "actor-one".into(),
        query: Default::default(),
    };
    let start = ScanCheckpoint::start(
        "jira.issue.search",
        json!({"input":{"jql":"project = ED"},"record_fields":[]}),
        &binding,
        ScanCaps {
            max_items: 3,
            max_pages: 10,
            max_stored_bytes: 65_536,
        },
        1,
    )?;
    let mut service = SyntheticService::new(Style::Token, 4, dataset(5, 0));
    let project = |items: &[Value]| Ok(items.to_vec());
    let id = |item: &Value| item["id"].as_str().map(String::from);
    let advice = || None;
    let clock = SystemClock::default();
    let mut advanced = scan::advance(
        &start,
        ScanInputs {
            source: &mut service,
            binding: &binding,
            project: &project,
            item_id: &id,
            rate_advice: &advice,
            control: &control,
            clock: &clock,
        },
    )?;
    let artifacts = NativeArtifacts::open(&files.state.join("scans"), 1 << 27, &control)?;
    let published = scan::publish(&artifacts, &mut advanced)?;
    let name = published.checkpoint.name.as_str();
    let wrong = format!("sha256:{}", "0".repeat(64));
    let root = files.output.to_str().ok_or("output")?;
    let mismatch = files.run(
        &[
            "scan",
            "export",
            "--checkpoint",
            name,
            "--checkpoint-digest",
            &wrong,
            "--output-root",
            root,
            "--output",
            "records.jsonl",
        ],
        None,
    )?;
    assert_eq!(mismatch["ok"], false);
    assert_eq!(mismatch["error"]["code"], "CURSOR_MISMATCH");
    let digest = published.checkpoint.digest.as_str();
    let refused = files.run(
        &[
            "scan",
            "export",
            "--checkpoint",
            name,
            "--checkpoint-digest",
            digest,
            "--output-root",
            root,
            "--output",
            "records.jsonl",
        ],
        None,
    )?;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["data"]["complete"], false);
    assert!(!files.output.join("records.jsonl").exists());
    let partial = files.run(
        &[
            "scan",
            "export",
            "--checkpoint",
            name,
            "--checkpoint-digest",
            digest,
            "--output-root",
            root,
            "--output",
            "records.jsonl",
            "--allow-partial",
        ],
        None,
    )?;
    assert_eq!(partial["ok"], true, "{partial}");
    assert_eq!(partial["data"]["complete"], false);
    assert_eq!(partial["data"]["items"], 3);
    assert_eq!(partial["data"]["export_write_state"], "confirmed");
    let text = fs::read_to_string(files.output.join("records.jsonl"))?;
    assert_eq!(text.lines().count(), 3);
    assert!(!partial.to_string().contains("Record 0001"));
    let again = files.run(
        &[
            "scan",
            "export",
            "--checkpoint",
            name,
            "--checkpoint-digest",
            digest,
            "--output-root",
            root,
            "--output",
            "records.jsonl",
            "--allow-partial",
        ],
        None,
    )?;
    assert_eq!(again["ok"], false);
    let segment = published.segment.ok_or("segment")?;
    let inspected = files.run(
        &[
            "artifact",
            "get",
            "--collection",
            "scans",
            "--artifact-name",
            &segment.name,
        ],
        None,
    )?;
    assert_eq!(inspected["data"]["artifact"]["digest"], segment.digest);
    // Resume refuses a mismatched digest before any profile or credential access.
    let resume = files.run(
        &[
            "scan",
            "resume",
            "--checkpoint",
            name,
            "--checkpoint-digest",
            &wrong,
        ],
        None,
    )?;
    assert_eq!(resume["error"]["code"], "CURSOR_MISMATCH");
    for arguments in [
        vec![
            "scan",
            "start",
            "--command",
            "jira.issue.get",
            "--arguments",
            "{}",
        ],
        vec![
            "scan",
            "start",
            "--command",
            "jira.issue.search",
            "--arguments",
            r#"{"jql":"x","cursor":"cur_00000000000000000000000000000000"}"#,
        ],
        vec![
            "scan",
            "start",
            "--command",
            "jira.issue.search",
            "--arguments",
            r#"{"jql":"x","extra":1}"#,
        ],
        vec![
            "scan",
            "start",
            "--command",
            "jira.issue.search",
            "--arguments",
            r#"{"jql":"x"}"#,
            "--max-items",
            "0",
        ],
        vec![
            "scan",
            "start",
            "--command",
            "jira.issue.search",
            "--arguments",
            r#"{"jql":"x"}"#,
            "--fields",
            "totals",
        ],
    ] {
        let refused = files.run(&arguments, None)?;
        assert_eq!(refused["ok"], false, "{arguments:?}: {refused}");
    }
    Ok(())
}
