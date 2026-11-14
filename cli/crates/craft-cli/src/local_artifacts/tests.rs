use super::*;
use craft_core::control::CancellationToken;
use craft_core::input::content_digest;
use craft_core::storage::ArtifactSpec;
use craft_runtime::control::SystemClock;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
type TestResult = Result<(), Box<dyn std::error::Error>>;
struct Files {
    parent: PathBuf,
    base: PathBuf,
    root: PathBuf,
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
        let base = parent.join(format!("artifact-command-{stamp}"));
        fs::create_dir(&base)?;
        let root = base.join("state");
        let output = base.join("output");
        fs::create_dir(&output)?;
        Ok(Self {
            parent,
            base,
            root,
            output,
        })
    }
    fn options(&self) -> InvocationOptions<'_> {
        InvocationOptions {
            state_root: Some(&self.root),
            profile: None,
            request_id: "local-artifact",
            max_bytes: 16384,
        }
    }
    fn put(
        &self,
        collection: Collection,
        bytes: &[u8],
    ) -> Result<ArtifactReceipt, Box<dyn std::error::Error>> {
        let store = NativeArtifacts::open(
            &self.root.join(collection.directory()),
            200_000,
            &control()?,
        )?;
        let mut writer = store.begin(ArtifactSpec {
            name: "source.bin".into(),
            media_type: "application/octet-stream".into(),
            max_bytes: 200_000,
            source: None,
        })?;
        writer.write_all(bytes)?;
        Ok(writer.finish()?)
    }
    fn export(&self, digest: &str) -> ExportArgs {
        ExportArgs {
            source: get(),
            expected_digest: digest.into(),
            output_root: self.output.to_string_lossy().into(),
            output: "result.bin".into(),
            overwrite: false,
        }
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
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )
}
fn get() -> GetArgs {
    GetArgs {
        artifact_name: "source.bin".into(),
        collection: Collection::Downloads,
        byte_limit: 200_000,
    }
}
fn data(result: &CommandResult) -> Result<&Value, Box<dyn std::error::Error>> {
    result.envelope.data.as_ref().ok_or_else(|| "data".into())
}
#[test]
fn local_inspection_verifies_exact_binary_and_keeps_missing_corrupt_and_oversized_distinct()
-> TestResult {
    let files = Files::new()?;
    let bytes = (0..160_013).map(|i| (i % 256) as u8).collect::<Vec<_>>();
    let receipt = files.put(Collection::Downloads, &bytes)?;
    let result = execute(
        ArtifactAction::Get(get()),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(result.envelope.ok, "{:?}", result.envelope.error);
    assert_eq!(data(&result)?["artifact"]["digest"], content_digest(&bytes));
    assert_eq!(data(&result)?["found"], true);
    assert_eq!(data(&result)?["integrity_verified"], true);
    assert_eq!(data(&result)?["replay_authorized"], false);
    let mut registry = Registry::default();
    register(&mut registry)?;
    assert!(
        jsonschema::validator_for(&registry.descriptor(GET)?.response_schema)?
            .is_valid(&json!(result.envelope))
    );
    let missing = execute(
        ArtifactAction::Get(GetArgs {
            collection: Collection::Reviews,
            ..get()
        }),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(missing.envelope.ok);
    assert_eq!(data(&missing)?["found"], false);
    assert!(!files.root.join("reviews").exists());
    let bounded = execute(
        ArtifactAction::Get(GetArgs {
            byte_limit: 2,
            ..get()
        }),
        &[],
        &files.options(),
        &control()?,
    );
    assert_eq!(
        bounded.envelope.error.as_ref().map(|v| v.code),
        Some(ErrorCode::ItemTooLarge)
    );
    assert!(data(&bounded)?["found"].is_null());
    fs::write(receipt.local_path.ok_or("source")?, vec![0; bytes.len()])?;
    let corrupt = execute(
        ArtifactAction::Get(get()),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(!corrupt.envelope.ok);
    assert!(data(&corrupt)?["found"].is_null());
    assert_eq!(data(&corrupt)?["integrity_verified"], false);
    Ok(())
}
#[test]
fn strict_export_inputs_roundtrip_without_unknown_or_missing_fields() -> TestResult {
    let files = Files::new()?;
    let args = files.export(&content_digest(b"abc"));
    let encoded = serde_json::to_value(&args)?;
    let decoded: ExportArgs = serde_json::from_value(encoded.clone())?;
    assert_eq!(decoded.expected_digest, args.expected_digest);
    let mut registry = Registry::default();
    register(&mut registry)?;
    registry.validate(EXPORT, &encoded)?;
    for mode in ["unknown", "digest", "collection"] {
        let mut invalid = encoded.clone();
        match mode {
            "unknown" => {
                invalid["url"] = json!("https://invalid.example");
            }
            "digest" => {
                invalid
                    .as_object_mut()
                    .ok_or("object")?
                    .remove("expected_digest");
            }
            _ => {
                invalid["collection"] = json!("../requests");
            }
        };
        assert!(registry.validate(EXPORT, &invalid).is_err());
        assert!(serde_json::from_value::<ExportArgs>(invalid).is_err());
    }
    Ok(())
}
#[test]
#[cfg(windows)]
fn local_export_requires_exact_digest_and_retains_source_on_collision_or_explicit_replacement()
-> TestResult {
    let files = Files::new()?;
    let bytes = b"local-binary-sentinel\0\xff";
    let receipt = files.put(Collection::Downloads, bytes)?;
    let wrong = execute(
        ArtifactAction::Export(files.export(&format!("sha256:{}", "0".repeat(64)))),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(!wrong.envelope.ok);
    assert_eq!(data(&wrong)?["export_write_state"], "not_sent");
    assert!(!files.output.join("result.bin").exists());
    let exported = execute(
        ArtifactAction::Export(files.export(&receipt.digest)),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(exported.envelope.ok, "{:?}", exported.envelope.error);
    assert_eq!(fs::read(files.output.join("result.bin"))?, bytes);
    assert_eq!(data(&exported)?["export_write_state"], "confirmed");
    let collision = execute(
        ArtifactAction::Export(files.export(&receipt.digest)),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(!collision.envelope.ok);
    assert_eq!(data(&collision)?["export_write_state"], "failed");
    assert_eq!(data(&collision)?["integrity_verified"], true);
    let mut args = files.export(&receipt.digest);
    args.overwrite = true;
    let replaced = execute(
        ArtifactAction::Export(args),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(replaced.envelope.ok);
    let encoded = crate::output::encode(replaced.envelope, 16384, OutputPolicy::PreserveEvidence)?;
    assert_eq!(encoded.exit, 0);
    assert!(!String::from_utf8_lossy(&encoded.bytes).contains("local-binary-sentinel"));
    assert!(serde_json::from_slice::<Envelope>(&encoded.bytes)?.ok);
    Ok(())
}
#[test]
#[cfg(windows)]
fn export_from_an_ancestor_cannot_replace_private_state_and_small_budget_precedes_publication()
-> TestResult {
    let files = Files::new()?;
    let receipt = files.put(Collection::Downloads, b"abc")?;
    fs::write(files.root.join("control.json"), b"preserve")?;
    let mut args = files.export(&receipt.digest);
    args.output_root = files.base.to_string_lossy().into();
    args.output = "state/control.json".into();
    args.overwrite = true;
    let protected = execute(
        ArtifactAction::Export(args),
        &[],
        &files.options(),
        &control()?,
    );
    assert!(!protected.envelope.ok);
    assert_eq!(fs::read(files.root.join("control.json"))?, b"preserve");
    assert_eq!(data(&protected)?["export_write_state"], "failed");
    let options = InvocationOptions {
        max_bytes: 2048,
        ..files.options()
    };
    let small = execute(
        ArtifactAction::Export(files.export(&receipt.digest)),
        &[],
        &options,
        &control()?,
    );
    assert_eq!(
        small.envelope.error.as_ref().map(|v| v.code),
        Some(ErrorCode::OutputBudgetTooSmall)
    );
    assert_eq!(data(&small)?["export_write_state"], "not_sent");
    assert!(!files.output.join("result.bin").exists());
    Ok(())
}
#[test]
fn invalid_controls_and_paths_precede_state_and_collection_creation() -> TestResult {
    let files = Files::new()?;
    let args = files.export(&content_digest(b"abc"));
    for mode in ["profile", "fields", "path", "digest", "limit", "name"] {
        let mut args = args.clone();
        let mut options = files.options();
        let mut fields = vec![];
        match mode {
            "profile" => options.profile = Some("factory"),
            "fields" => fields.push("artifact".into()),
            "path" => args.output = "../escape".into(),
            "digest" => args.expected_digest = "bad".into(),
            "limit" => args.source.byte_limit = 0,
            _ => args.source.artifact_name = "..".into(),
        };
        let result = execute(ArtifactAction::Export(args), &fields, &options, &control()?);
        assert!(!result.envelope.ok, "{mode}");
        assert!(!files.root.exists(), "{mode}");
    }
    Ok(())
}

#[test]
fn export_error_envelopes_retain_unknown_and_confirmed_publication_states() -> TestResult {
    let files = Files::new()?;
    let receipt = files.put(Collection::Downloads, b"abc")?;
    let action = ArtifactAction::Export(files.export(&receipt.digest));
    for (state, code) in [
        (RequestState::SentUnknown, ErrorCode::WriteOutcomeUnknown),
        (RequestState::Confirmed, ErrorCode::Cancelled),
    ] {
        let mut retained = ArtifactData::empty(action.source());
        retained.artifact = Some(receipt.clone());
        retained.found = Some(true);
        retained.integrity_verified = true;
        retained.export_write_state = state;
        if state == RequestState::Confirmed {
            retained.exported = Some(ArtifactReceipt {
                local_path: Some(files.output.join("result.bin").to_string_lossy().into()),
                ..receipt.clone()
            });
        }
        let result = finish(
            &action,
            retained,
            Err(CraftError::new(
                code,
                "Retain the local publication disposition.",
            )),
            &files.options(),
        );
        assert_eq!(
            result
                .envelope
                .error
                .as_ref()
                .ok_or("error")?
                .details
                .write_state,
            state
        );
        assert_eq!(
            data(&result)?["export_write_state"],
            serde_json::to_value(state)?
        );
        assert_eq!(
            data(&result)?["exported"].is_null(),
            state != RequestState::Confirmed
        );
        let encoded =
            crate::output::encode(result.envelope, 16384, OutputPolicy::PreserveEvidence)?;
        assert_eq!(
            serde_json::from_slice::<Envelope>(&encoded.bytes)?
                .error
                .ok_or("error")?
                .details
                .write_state,
            state
        );
    }
    Ok(())
}
