use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicU64, Ordering};

use craft_test_support::process::{RunOptions, run};
use serde_json::{Value, json};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    parent: PathBuf,
    path: PathBuf,
    repository: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = parent.join(format!(
            "profile-cli-{}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = path.join("repository");
        fs::create_dir_all(&repository)?;
        Ok(Self {
            parent,
            path,
            repository,
        })
    }
    fn options(&self) -> RunOptions {
        RunOptions {
            current_dir: Some(self.repository.clone()),
            environment: vec![
                (
                    "CRAFT_STATE_ROOT".into(),
                    Some(self.path.join("state").into_os_string()),
                ),
                (
                    "LOCALAPPDATA".into(),
                    Some(self.path.join("home").into_os_string()),
                ),
                ("HOME".into(), Some(self.path.join("home").into_os_string())),
                (
                    "XDG_STATE_HOME".into(),
                    Some(self.path.join("home").into_os_string()),
                ),
            ],
            ..RunOptions::default()
        }
    }
    fn invoke(
        &self,
        arguments: &[&str],
        input: Option<&Value>,
    ) -> Result<Output, Box<dyn std::error::Error>> {
        let bytes = input.map(serde_json::to_vec).transpose()?;
        Ok(run(
            Path::new(env!("CARGO_BIN_EXE_craft")),
            arguments,
            bytes.as_deref(),
            self.options(),
        )?)
    }
    fn create(&self, name: &str) -> Result<Value, Box<dyn std::error::Error>> {
        success(&self.invoke(&["profile", "create", "--input", "-"], Some(&json!({"name":name,"provider":"jira","deployment":"cloud","auth_mode":"delegated"})))?)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn envelope(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn success(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    let result = envelope(output)?;
    assert!(output.status.success(), "{result}");
    assert_eq!(result["ok"], true);
    Ok(result)
}

#[test]
fn flags_and_json_create_shared_grant_profiles_with_discoverable_write_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let created = success(&fixture.invoke(
        &[
            "profile",
            "create",
            "--name",
            "finance",
            "--provider",
            "jira",
            "--deployment",
            "cloud",
            "--auth-mode",
            "delegated",
            "--tenant",
            "primary",
        ],
        None,
    )?)?;
    let grant = created["data"]["profile"]["grant"]["id"]
        .as_str()
        .ok_or("Expected grant")?;
    let wiki = json!({"name":"wiki","provider":"confluence","deployment":"cloud","auth_mode":"delegated","tenant":"primary","grant":grant});
    let wiki = success(&fixture.invoke(&["profile", "create", "--input", "-"], Some(&wiki))?)?;
    assert_eq!(wiki["data"]["profile"]["grant"]["id"], grant);
    assert_eq!(wiki["data"]["active"], false);
    let used = success(&fixture.invoke(&["profile", "use", "--name", "finance"], None)?)?;
    assert_eq!(used["data"]["active"], true);
    let shown = success(&fixture.invoke(&["profile", "show"], None)?)?;
    assert_eq!(shown["data"]["selection_source"], "active");
    assert_eq!(shown["data"]["profile"], created["data"]["profile"]);
    for command in ["profile.create", "profile.use"] {
        let capability = success(&fixture.invoke(
            &[
                "capabilities",
                "--command",
                command,
                "--fields",
                "effect,retry_class,reconciliation",
            ],
            None,
        )?)?;
        assert_eq!(capability["data"]["items"][0]["effect"], "write");
        assert_eq!(
            capability["data"]["items"][0]["retry_class"],
            "reconcile_before_replay"
        );
    }
    for (command, actual) in [
        ("profile.create", created),
        ("profile.show", shown),
        ("profile.use", used),
    ] {
        let schema = success(&fixture.invoke(
            &[
                "schema",
                "--command",
                command,
                "--response",
                "--view",
                "full",
                "--max-bytes",
                "65536",
            ],
            None,
        )?)?;
        assert!(
            jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(&actual),
            "{actual}"
        );
    }
    assert_eq!(
        success(&fixture.invoke(&["provider", "list"], None)?)?["data"]["items"],
        json!([
            {"id":"confluence","version":craft_core::VERSION,"deployments":["cloud"]},
            {"id":"jira","version":craft_core::VERSION,"deployments":["cloud"]}
        ])
    );
    assert!(!fixture.path.join("home").exists());
    Ok(())
}

#[test]
fn explicit_selection_overrides_repository_hints_and_ambiguity_never_chooses_first()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.create("alpha")?;
    fixture.create("bravo")?;
    let ambiguous = envelope(&fixture.invoke(&["profile", "show"], None)?)?;
    assert_eq!(ambiguous["error"]["code"], "TARGET_AMBIGUOUS");
    success(&fixture.invoke(&["profile", "use", "--name", "alpha"], None)?)?;
    fs::write(
        fixture.repository.join(".craft.json"),
        br#"{"profile":"bravo"}"#,
    )?;
    let repository = success(&fixture.invoke(&["profile", "show"], None)?)?;
    assert_eq!(repository["data"]["profile"]["name"], "bravo");
    assert_eq!(repository["data"]["selection_source"], "repository");
    let explicit = success(&fixture.invoke(&["profile", "show", "--profile", "alpha"], None)?)?;
    assert_eq!(explicit["data"]["profile"]["name"], "alpha");
    assert_eq!(explicit["data"]["selection_source"], "explicit");
    fs::write(
        fixture.repository.join(".craft.json"),
        br#"{"profile":"bravo","credential_backend":"file"}"#,
    )?;
    assert_eq!(
        envelope(&fixture.invoke(&["profile", "show"], None)?)?["error"]["code"],
        "INPUT_INVALID"
    );
    success(&fixture.invoke(&["profile", "show", "--name", "alpha"], None)?)?;
    assert_eq!(
        envelope(&fixture.invoke(
            &["profile", "show", "--name", "alpha", "--profile", "bravo"],
            None
        )?)?["error"]["code"],
        "INPUT_INVALID"
    );
    let isolated = Fixture::new()?;
    assert_eq!(
        envelope(&isolated.invoke(&["profile", "show", "--name", "alpha"], None)?)?["ok"],
        false
    );
    Ok(())
}

#[test]
fn malformed_input_is_refused_before_state_and_write_budget_failures_leave_no_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let malformed = json!({"name":"bad","provider":"jira","deployment":"cloud","auth_mode":"delegated","secret":"private-sentinel"});
    let output = fixture.invoke(&["profile", "create", "--input", "-"], Some(&malformed))?;
    assert_eq!(envelope(&output)?["error"]["code"], "INPUT_INVALID");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-sentinel"));
    assert!(!fixture.path.join("state").exists());
    let wrong_target = json!({"name":"bad","provider":"jira","deployment":"cloud","auth_mode":"delegated","tenant":"primary","defaults":{"project":{"provider":"jira","deployment":"cloud","tenant":"foreign","resource_type":"project","id":"PAY"}}});
    assert_eq!(
        envelope(&fixture.invoke(&["profile", "create", "--input", "-"], Some(&wrong_target))?)?["ok"],
        false
    );
    assert_eq!(
        envelope(&fixture.invoke(
            &[
                "input",
                "validate",
                "--command",
                "profile.create",
                "--file",
                "-"
            ],
            Some(&wrong_target)
        )?)?["ok"],
        false
    );
    assert!(!fixture.path.join("state").exists());
    let valid = json!({"name":"too-small","provider":"jira","deployment":"cloud","auth_mode":"delegated","tenant":"x".repeat(512)});
    let output = fixture.invoke(
        &["profile", "create", "--input", "-", "--max-bytes", "512"],
        Some(&valid),
    )?;
    assert_eq!(
        envelope(&output)?["error"]["code"],
        "OUTPUT_BUDGET_TOO_SMALL"
    );
    assert!(output.stdout.len() <= 512);
    assert_eq!(
        envelope(&fixture.invoke(&["profile", "show", "--name", "too-small"], None)?)?["ok"],
        false
    );
    fixture.create("alpha")?;
    let mut large = valid;
    large["name"] = json!("bravo");
    success(&fixture.invoke(&["profile", "create", "--input", "-"], Some(&large))?)?;
    success(&fixture.invoke(&["profile", "use", "--name", "alpha"], None)?)?;
    assert_eq!(
        envelope(&fixture.invoke(
            &["profile", "use", "--name", "bravo", "--max-bytes", "512"],
            None
        )?)?["error"]["code"],
        "OUTPUT_BUDGET_TOO_SMALL"
    );
    assert_eq!(
        success(&fixture.invoke(&["profile", "show"], None)?)?["data"]["profile"]["name"],
        "alpha"
    );
    Ok(())
}

#[test]
fn profile_lists_return_schema_valid_pages_and_reject_changed_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    for name in ["alpha", "bravo", "charlie", "delta", "echo"] {
        fixture.create(name)?;
    }
    let first = success(&fixture.invoke(
        &["profile", "list", "--limit", "2", "--fields", "name"],
        None,
    )?)?;
    assert_eq!(first["page"]["returned"], 2);
    assert_eq!(first["page"]["has_more"], true);
    assert_eq!(first["data"]["cached"], false);
    assert!(first["data"]["items"][0].get("auth_mode").is_none());
    let cursor = first["page"]["next_cursor"]
        .as_str()
        .ok_or("Expected opaque cursor")?;
    let next = success(&fixture.invoke(
        &[
            "profile", "list", "--cursor", cursor, "--limit", "100", "--fields", "name",
        ],
        None,
    )?)?;
    assert_eq!(next["data"]["cached"], true);
    assert_eq!(next["data"]["items"][0]["name"], "charlie");
    assert_eq!(next["page"]["returned"], 3);
    assert_eq!(next["page"]["has_more"], false);
    let changed = envelope(&fixture.invoke(
        &[
            "profile",
            "list",
            "--cursor",
            cursor,
            "--fields",
            "auth_mode",
        ],
        None,
    )?)?;
    assert_eq!(
        changed["error"]["details"]["issues"][0]["rule"],
        "cursor_binding_mismatch"
    );
    let schema = success(&fixture.invoke(
        &[
            "schema",
            "--command",
            "profile.list",
            "--response",
            "--view",
            "full",
            "--fields",
            "name",
            "--max-bytes",
            "65536",
        ],
        None,
    )?)?;
    let validator = jsonschema::validator_for(&schema["data"]["schema"])?;
    assert!(validator.is_valid(&first));
    assert!(validator.is_valid(&next));
    let tiny = fixture.invoke(&["profile", "list", "--max-bytes", "512"], None)?;
    let tiny_page = success(&tiny)?;
    assert_eq!(tiny_page["page"]["stop_reason"], "byte_limit");
    assert_eq!(tiny_page["page"]["returned"], 1);
    assert_eq!(tiny_page["page"]["has_more"], true);
    assert!(tiny.stdout.len() <= 512);
    let remainder = success(
        &fixture.invoke(
            &[
                "profile",
                "list",
                "--cursor",
                tiny_page["page"]["next_cursor"]
                    .as_str()
                    .ok_or("Expected continuation")?,
            ],
            None,
        )?,
    )?;
    assert_eq!(remainder["data"]["items"][0]["name"], "bravo");
    assert_eq!(remainder["page"]["returned"], 4);
    Ok(())
}

#[test]
fn discovery_never_opens_state_and_explicit_root_has_precedence()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    for arguments in [
        vec!["version"],
        vec!["help", "--json"],
        vec!["schema", "--command", "profile.create"],
    ] {
        success(&fixture.invoke(&arguments, None)?)?;
    }
    assert!(!fixture.path.join("state").exists());
    let explicit_root = fixture.path.join("explicit");
    let explicit_root = explicit_root.to_str().ok_or("Expected native path")?;
    success(&fixture.invoke(&["profile", "list", "--state-root", explicit_root], None)?)?;
    assert!(!fixture.path.join("state").exists());
    assert!(Path::new(explicit_root).join("state.sqlite3").is_file());
    assert_eq!(
        envelope(&fixture.invoke(&["version", "--state-root", explicit_root], None)?)?["error"]["code"],
        "INPUT_INVALID"
    );
    assert_eq!(
        envelope(&fixture.invoke(&["profile", "list", "--profile", "unselected"], None)?)?["error"]
            ["code"],
        "INPUT_INVALID"
    );
    Ok(())
}

#[test]
fn auth_diagnostics_reject_invalid_context_and_flags_before_state_or_secret_input()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    for (arguments, input) in [
        (vec!["auth", "status", "--name", "../invalid"], None),
        (
            vec!["auth", "doctor", "--name", "alpha", "--profile", "bravo"],
            None,
        ),
        (
            vec!["auth", "status", "--client-secret", "synthetic-secret"],
            None,
        ),
        (
            vec!["auth", "status", "--input", "-"],
            Some(json!({"name":"alpha","verify":true,"secret":"synthetic-secret"})),
        ),
        (vec!["auth", "status", "--fields", "absent"], None),
    ] {
        let output = fixture.invoke(&arguments, input.as_ref())?;
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret"));
        assert_eq!(envelope(&output)?["ok"], false);
        assert!(!fixture.path.join("state").exists());
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn local_auth_reads_report_absence_shared_profiles_and_unknown_live_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let created = fixture.create("alpha")?;
    let grant = &created["data"]["profile"]["grant"]["id"];
    success(&fixture.invoke(&["profile","create","--input","-"],Some(&json!({"name":"wiki","provider":"confluence","deployment":"cloud","auth_mode":"delegated","grant":grant})))?)?;
    fixture.create("bravo")?;
    success(&fixture.invoke(&["profile", "use", "--name", "alpha"], None)?)?;
    let status = success(&fixture.invoke(
        &[
            "auth",
            "status",
            "--fields",
            "credential_state,shared_profiles,token_state,live_verified",
        ],
        None,
    )?)?;
    assert_eq!(status["data"]["credential_state"], "absent");
    assert_eq!(status["data"]["shared_profiles"], json!(["alpha", "wiki"]));
    assert_eq!(status["data"]["token_state"], "unknown");
    assert_eq!(status["data"]["live_verified"], false);
    assert_eq!(status["data"]["grant"]["id"], *grant);
    let shown = success(&fixture.invoke(
        &[
            "auth",
            "status",
            "--profile",
            "wiki",
            "--fields",
            "selection_source",
        ],
        None,
    )?)?;
    assert_eq!(shown["data"]["profile"], "wiki");
    assert_eq!(shown["data"]["selection_source"], "explicit");
    for (arguments, command) in [
        (vec!["auth", "status", "--name", "alpha"], "auth.status"),
        (
            vec!["auth", "doctor", "--name", "alpha", "--fields", "health"],
            "auth.doctor",
        ),
    ] {
        let result = success(&fixture.invoke(&arguments, None)?)?;
        if command == "auth.doctor" {
            assert_eq!(
                result["data"]["health"]["backend"],
                "windows_credential_manager"
            );
            assert_eq!(result["data"]["health"]["write_access"], "unknown");
            assert_eq!(result["data"]["live_verified"], false);
        }
        let mut schema_arguments = vec![
            "schema",
            "--command",
            command,
            "--response",
            "--view",
            "full",
            "--max-bytes",
            "65536",
        ];
        if command == "auth.doctor" {
            schema_arguments.extend(["--fields", "health"]);
        }
        let schema = success(&fixture.invoke(&schema_arguments, None)?)?;
        assert!(jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(&result));
        assert_eq!(result["command"], command);
    }
    Ok(())
}
