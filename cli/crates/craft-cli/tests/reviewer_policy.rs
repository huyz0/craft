use craft_test_support::process::{RunOptions, run};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
struct Fixture {
    parent: PathBuf,
    root: PathBuf,
    repository: PathBuf,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = craft_test_support::state::fixture_root(
            &parent,
            "public-reviewer-policy",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        );
        let repository = root.with_extension("repository");
        std::fs::create_dir_all(&repository)?;
        Ok(Self {
            parent,
            root,
            repository,
        })
    }
    fn invoke(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        profile: bool,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let root = self.root.to_str().ok_or("root")?;
        let mut arguments = if args.first().is_some_and(|a| {
            matches!(
                *a,
                "schema" | "help" | "provider" | "command" | "capabilities" | "version"
            )
        }) {
            Vec::new()
        } else {
            vec!["--state-root", root]
        };
        if profile {
            arguments.extend(["--profile", "factory"]);
        }
        arguments.extend(args);
        let output = run(
            Path::new(env!("CARGO_BIN_EXE_craft")),
            &arguments,
            input,
            RunOptions {
                current_dir: Some(self.repository.clone()),
                timeout: std::time::Duration::from_secs(30),
                ..Default::default()
            },
        )?;
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(serde_json::from_slice(&output.stdout)?)
    }
    fn profile(&self) -> TestResult {
        let result = self.invoke(
            &[
                "profile",
                "create",
                "--name",
                "factory",
                "--provider",
                "confluence",
                "--deployment",
                "cloud",
                "--auth-mode",
                "service",
                "--tenant",
                CLOUD,
            ],
            None,
            false,
        )?;
        assert_eq!(result["ok"], true, "{result}");
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for path in [&self.root, &self.repository] {
            if let Ok(actual) = path.canonicalize()
                && actual.starts_with(&self.parent)
                && actual != self.parent
            {
                let _ = std::fs::remove_dir_all(actual);
            }
        }
    }
}
fn request() -> Value {
    json!({"space_id":"100","expected_actor":"worker-agent","enabled":true,"reviewers":["bob","alice"],"excluded_actors":["other-agent"],"quorum":2,"expected_revision":null,"apply":true})
}

#[test]
fn public_reviewer_configuration_persists_without_credentials_or_repository_authority() -> TestResult
{
    let fixture = Fixture::new()?;
    fixture.profile()?;
    std::fs::write(
        fixture.repository.join(".craft.json"),
        b"deliberately-invalid-repository-hints",
    )?;
    let absent = fixture.invoke(
        &["review", "policy", "get", "--space-id", "100"],
        None,
        true,
    )?;
    assert_eq!(absent["ok"], true, "{absent}");
    assert!(absent["data"]["policy"].is_null());
    let created = fixture.invoke(
        &["review", "policy", "set", "--input", "-"],
        Some(&serde_json::to_vec(&request())?),
        true,
    )?;
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(
        created["data"]["policy"]["reviewers"],
        json!(["alice", "bob"])
    );
    assert_eq!(created["data"]["configuration_write_state"], "confirmed");
    assert_eq!(created["data"]["live_verified"], false);
    assert_eq!(created["data"]["review_verified"], false);
    assert!(created["data"]["proposed_policy"].is_null());
    let read = fixture.invoke(
        &["review", "policy", "get", "--space-id", "100"],
        None,
        true,
    )?;
    assert_eq!(read["data"]["policy"], created["data"]["policy"]);
    let control = craft_core::control::InvocationControl::new(
        std::sync::Arc::new(craft_runtime::control::SystemClock::default()),
        craft_core::control::CancellationToken::default(),
        60_000,
    )?;
    let store = craft_runtime::profiles::ProfileStore::open(&fixture.root, &control)?;
    let captured = store
        .review_policy("factory", "100", &control)?
        .ok_or("captured policy")?;
    craft_cli::review_policy::trusted_policy(&store, &captured, &control)?;
    let mut forged = captured.clone();
    forged.reviewers = vec!["worker-agent".into()];
    assert!(craft_cli::review_policy::trusted_policy(&store, &forged, &control).is_err());
    let schema = fixture.invoke(
        &[
            "schema",
            "--command",
            "review.policy.get",
            "--response",
            "--view",
            "full",
        ],
        None,
        false,
    )?;
    assert!(jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(&read));
    let stale = fixture.invoke(
        &["review", "policy", "set", "--input", "-"],
        Some(&serde_json::to_vec(&request())?),
        true,
    )?;
    assert_eq!(stale["error"]["code"], "VERSION_CONFLICT");
    let disabled = fixture.invoke(
        &[
            "review",
            "policy",
            "set",
            "--space-id",
            "100",
            "--expected-actor",
            "worker-agent",
            "--enabled",
            "false",
            "--reviewers",
            "alice,bob",
            "--excluded-actors",
            "other-agent",
            "--quorum",
            "2",
            "--expected-revision",
            "1",
            "--apply",
        ],
        None,
        true,
    )?;
    assert_eq!(disabled["ok"], true, "{disabled}");
    assert_eq!(disabled["data"]["policy"]["revision"], 2);
    assert_eq!(disabled["data"]["policy"]["enabled"], false);
    assert!(craft_cli::review_policy::trusted_policy(&store, &captured, &control).is_err());
    let no_profile = fixture.invoke(
        &["review", "policy", "get", "--space-id", "100"],
        None,
        false,
    )?;
    assert_eq!(no_profile["error"]["code"], "INPUT_INVALID");
    std::fs::remove_file(fixture.repository.join(".craft.json"))?;
    let auth = fixture.invoke(&["auth", "status", "--name", "factory"], None, false)?;
    assert_eq!(auth["data"]["credential_state"], "absent", "{auth}");
    Ok(())
}
#[test]
fn invalid_reviewer_intent_and_inadequate_receipts_never_initialize_state() -> TestResult {
    for mode in [
        "self",
        "overlap",
        "duplicate",
        "missing_enabled",
        "missing_apply",
        "quorum",
        "budget",
        "unknown_field",
        "projection",
        "profile",
    ] {
        let fixture = Fixture::new()?;
        let mut value = request();
        match mode {
            "self" => value["reviewers"] = json!(["worker-agent", "alice"]),
            "overlap" => value["excluded_actors"] = json!(["alice"]),
            "duplicate" => value["reviewers"] = json!(["alice", "alice"]),
            "missing_enabled" => {
                value.as_object_mut().ok_or("request")?.remove("enabled");
            }
            "missing_apply" => value["apply"] = json!(false),
            "quorum" => value["quorum"] = json!(3),
            "unknown_field" => value["review_verified"] = json!(true),
            _ => {}
        }
        let mut args = vec!["review", "policy", "set", "--input", "-"];
        if mode == "budget" {
            args.extend(["--max-bytes", "512"]);
        }
        if mode == "projection" {
            args.extend(["--fields", "policy.reviewers"]);
        }
        let result =
            fixture.invoke(&args, Some(&serde_json::to_vec(&value)?), mode != "profile")?;
        assert_eq!(result["ok"], false, "{mode}: {result}");
        assert!(!fixture.root.exists(), "{mode}");
    }
    Ok(())
}
