use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use craft_core::contracts::{AuthMode, JobState, RemoteJob, ResourceRef};
use craft_core::control::{CancellationToken, InvocationControl};
use craft_core::input::content_digest;
use craft_core::profiles::ProfileInput;
use craft_core::storage::{
    JournalRecord, JournalStep, OperationBinding, OperationJournal, SubmissionState,
};
use craft_runtime::control::SystemClock;
use craft_runtime::journal::NativeJournal;
use craft_runtime::profiles::ProfileStore;
use craft_test_support::process::{RunOptions, run};
use serde_json::{Value, json};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = parent.join(format!(
            "operation-cli-{}-{stamp}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(Self { parent, root })
    }

    fn invoke(
        &self,
        arguments: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Output, Box<dyn std::error::Error>> {
        Ok(run(
            Path::new(env!("CARGO_BIN_EXE_craft")),
            arguments,
            input,
            RunOptions {
                environment: vec![(
                    "CRAFT_STATE_ROOT".into(),
                    Some(self.root.clone().into_os_string()),
                )],
                ..Default::default()
            },
        )?)
    }

    fn journal(&self) -> Result<NativeJournal, Box<dyn std::error::Error>> {
        let control = control()?;
        let profiles = ProfileStore::open(&self.root, &control)?;
        Ok(NativeJournal::open(&profiles, &control)?)
    }

    fn seed(
        &self,
        count: usize,
        state: SubmissionState,
        job: Option<JobState>,
    ) -> Result<JournalRecord, Box<dyn std::error::Error>> {
        let control = control()?;
        let profiles = ProfileStore::open(&self.root, &control)?;
        let profile = profiles.create(
            &ProfileInput {
                name: "team".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Delegated,
                grant: None,
                tenant: Some("primary".into()),
                defaults: BTreeMap::new(),
            },
            &control,
            |_| Ok(()),
        )?;
        let journal = NativeJournal::open(&profiles, &control)?;
        let mut record = journal.begin(&OperationBinding {
            operation_id: "publish-summary".into(),
            request_digest: content_digest(b"logical"),
            descriptor_digest: content_digest(b"descriptor"),
            target: resource("ISSUE-1"),
            grant: profile.grant,
            actor: "account-one".into(),
        })?;
        for index in 0..count {
            record.steps.insert(
                format!("publish-{index:03}"),
                JournalStep {
                    request_digest: Some(content_digest(format!("native-{index}").as_bytes())),
                    ..Default::default()
                },
            );
        }
        record.revision += 1;
        journal.compare_and_swap(record.revision - 1, &record)?;
        if state != SubmissionState::Prepared {
            for step in record.steps.values_mut() {
                step.state = SubmissionState::SendStarted;
            }
            record.revision += 1;
            journal.compare_and_swap(record.revision - 1, &record)?;
        }
        if matches!(
            state,
            SubmissionState::Confirmed | SubmissionState::Failed | SubmissionState::Unknown
        ) {
            for (name, step) in &mut record.steps {
                step.state = state;
                step.confirmed_resources
                    .push(resource(&format!("{name}-{}", "x".repeat(160))));
                step.job = job.map(|state| RemoteJob {
                    resource: resource("job-one"),
                    state,
                });
            }
            record.revision += 1;
            journal.compare_and_swap(record.revision - 1, &record)?;
        }
        Ok(record)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn control() -> Result<InvocationControl, craft_core::error::CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )
}

fn resource(id: &str) -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "primary".into(),
        resource_type: "issue".into(),
        id: id.into(),
    }
}

fn envelope(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn success(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    let result = envelope(output)?;
    assert!(output.status.success(), "{result}");
    assert_eq!(result["ok"], true);
    Ok(result)
}

fn validate_response(
    fixture: &Fixture,
    fields: Option<&str>,
    actual: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = vec![
        "schema",
        "--command",
        "operation.get",
        "--response",
        "--view",
        "full",
        "--max-bytes",
        "65536",
    ];
    if let Some(fields) = fields {
        args.extend(["--fields", fields]);
    }
    let schema = success(&fixture.invoke(&args, None)?)?;
    assert!(
        jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(actual),
        "{actual}"
    );
    Ok(())
}

#[test]
fn fresh_process_inspection_preserves_binding_steps_receipts_and_projection_guards()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let before = fixture.seed(2, SubmissionState::Unknown, Some(JobState::Running))?;
    let full = success(&fixture.invoke(
        &["operation", "get", "--operation-id", "publish-summary"],
        None,
    )?)?;
    assert_eq!(
        full["data"]["binding"],
        serde_json::to_value(&before.binding)?
    );
    assert_eq!(full["data"]["revision"], before.revision);
    assert_eq!(full["data"]["found"], true);
    assert_eq!(full["data"]["reconciliation_required"], true);
    for (index, (name, step)) in before.steps.iter().enumerate() {
        let output = &full["data"]["steps"][index];
        assert_eq!(output["name"], *name);
        assert_eq!(output["state"], "unknown");
        assert_eq!(
            output["request_digest"],
            serde_json::to_value(&step.request_digest)?
        );
        assert_eq!(
            output["confirmed_resources"],
            serde_json::to_value(&step.confirmed_resources)?
        );
        assert_eq!(output["job"], serde_json::to_value(&step.job)?);
    }
    validate_response(&fixture, None, &full)?;
    let fields = "steps.name,steps.state,steps.confirmed_resources.id,steps.job.state";
    let selected = success(&fixture.invoke(
        &["operation", "get", "--fields", fields, "--input", "-"],
        Some(br#"{"operation_id":"publish-summary"}"#),
    )?)?;
    for field in [
        "operation_id",
        "found",
        "reconciliation_required",
        "evidence_scope",
        "live_verified",
        "replay_authorized",
    ] {
        assert_eq!(selected["data"][field], full["data"][field]);
    }
    assert!(selected["data"].get("binding").is_none());
    assert_eq!(selected["data"]["live_verified"], false);
    assert_eq!(selected["data"]["replay_authorized"], false);
    assert!(selected["data"]["steps"][0].get("request_digest").is_none());
    validate_response(&fixture, Some(fields), &selected)?;
    assert_eq!(fixture.journal()?.get("publish-summary")?, Some(before));
    Ok(())
}

#[test]
fn missing_record_and_distinct_state_root_never_claim_remote_absence_or_replay_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.seed(1, SubmissionState::Confirmed, None)?;
    let other = Fixture::new()?;
    let root = other.root.to_str().ok_or("Expected Unicode fixture path")?;
    let missing = success(&fixture.invoke(
        &[
            "operation",
            "get",
            "--operation-id",
            "publish-summary",
            "--state-root",
            root,
        ],
        None,
    )?)?;
    assert_eq!(
        missing["data"],
        json!({"operation_id":"publish-summary","found":false,"binding":null,"revision":null,"steps":[],"reconciliation_required":null,"evidence_scope":"local_installation","live_verified":false,"replay_authorized":false})
    );
    validate_response(&fixture, None, &missing)?;
    assert_eq!(
        fixture
            .journal()?
            .get("publish-summary")?
            .ok_or("Missing original")?
            .steps
            .len(),
        1
    );
    Ok(())
}

#[test]
fn invalid_identity_input_profile_and_projection_fail_before_state_access()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    for (arguments, input, code) in [
        (vec!["operation", "get"], None, "INPUT_INVALID"),
        (
            vec!["operation", "get", "--operation-id", " trailing "],
            None,
            "INPUT_INVALID",
        ),
        (
            vec!["operation", "get", "--operation-id", "bad#id"],
            None,
            "INPUT_INVALID",
        ),
        (
            vec![
                "operation",
                "get",
                "--operation-id",
                "publish-summary",
                "--profile",
                "team",
            ],
            None,
            "INPUT_INVALID",
        ),
        (
            vec![
                "operation",
                "get",
                "--operation-id",
                "publish-summary",
                "--fields",
                "steps.invented",
            ],
            None,
            "FIELD_UNKNOWN",
        ),
        (
            vec!["operation", "get", "--input", "-"],
            Some(br#"{"operation_id":"a","operation_id":"b"}"#.as_slice()),
            "INPUT_INVALID",
        ),
        (
            vec!["operation", "get", "--input", "-"],
            Some(br#"{"operation_id":"a","apply":true}"#.as_slice()),
            "INPUT_INVALID",
        ),
    ] {
        let output = fixture.invoke(&arguments, input)?;
        assert!(!output.status.success());
        let result = envelope(&output)?;
        assert_eq!(result["error"]["code"], code, "{result}");
        assert!(!fixture.root.exists());
    }
    let input = success(&fixture.invoke(
        &[
            "input",
            "validate",
            "--command",
            "operation.get",
            "--file",
            "-",
        ],
        Some(br#"{"operation_id":"publish-summary"}"#),
    )?)?;
    assert_eq!(input["data"]["live_conditions_checked"], false);
    assert!(!fixture.root.exists());
    let invalid = fixture.invoke(
        &[
            "input",
            "validate",
            "--command",
            "operation.get",
            "--file",
            "-",
        ],
        Some(br#"{"operation_id":"bad#id"}"#),
    )?;
    assert_eq!(envelope(&invalid)?["error"]["code"], "INPUT_INVALID");
    assert!(!fixture.root.exists());
    Ok(())
}

#[test]
fn oversized_evidence_is_refused_intact_and_can_be_read_with_explicit_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let before = fixture.seed(70, SubmissionState::Confirmed, Some(JobState::Succeeded))?;
    let output = fixture.invoke(
        &["operation", "get", "--operation-id", "publish-summary"],
        None,
    )?;
    assert!(!output.status.success());
    assert!(output.stdout.len() <= 16_384);
    let failed = envelope(&output)?;
    assert_eq!(failed["error"]["code"], "OUTPUT_BUDGET_TOO_SMALL");
    assert!(failed["data"].is_null());
    let fields = "steps.name,steps.state";
    let selected = success(&fixture.invoke(
        &[
            "operation",
            "get",
            "--operation-id",
            "publish-summary",
            "--fields",
            fields,
        ],
        None,
    )?)?;
    assert_eq!(
        selected["data"]["steps"]
            .as_array()
            .ok_or("Missing steps")?
            .len(),
        70
    );
    assert_eq!(selected["data"]["reconciliation_required"], false);
    assert_eq!(selected["data"]["replay_authorized"], false);
    validate_response(&fixture, Some(fields), &selected)?;
    let full = success(&fixture.invoke(
        &[
            "operation",
            "get",
            "--operation-id",
            "publish-summary",
            "--max-bytes",
            "65536",
        ],
        None,
    )?)?;
    assert_eq!(
        full["data"]["steps"]
            .as_array()
            .ok_or("Missing steps")?
            .len(),
        70
    );
    assert_eq!(fixture.journal()?.get("publish-summary")?, Some(before));
    Ok(())
}

#[test]
fn corrupt_state_is_an_error_and_submission_and_job_states_remain_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    for (state, job, required) in [
        (SubmissionState::Prepared, None, false),
        (SubmissionState::SendStarted, None, true),
        (SubmissionState::Confirmed, Some(JobState::Accepted), false),
        (SubmissionState::Confirmed, Some(JobState::Running), false),
        (SubmissionState::Confirmed, Some(JobState::Succeeded), false),
        (SubmissionState::Confirmed, Some(JobState::Failed), false),
        (SubmissionState::Confirmed, Some(JobState::Unknown), true),
        (SubmissionState::Failed, None, false),
    ] {
        let fixture = Fixture::new()?;
        fixture.seed(1, state, job)?;
        let result = success(&fixture.invoke(
            &["operation", "get", "--operation-id", "publish-summary"],
            None,
        )?)?;
        assert_eq!(result["data"]["reconciliation_required"], required);
        assert_eq!(
            result["data"]["steps"][0]["state"],
            serde_json::to_value(state)?
        );
        if let Some(job) = job {
            assert_eq!(
                result["data"]["steps"][0]["job"]["state"],
                serde_json::to_value(job)?
            );
        }
        assert_eq!(result["data"]["replay_authorized"], false);
    }
    let fixture = Fixture::new()?;
    fixture.seed(1, SubmissionState::Confirmed, None)?;
    fs::write(fixture.root.join("state.sqlite3"), b"corrupt-metadata")?;
    let result = envelope(&fixture.invoke(
        &["operation", "get", "--operation-id", "publish-summary"],
        None,
    )?)?;
    assert_eq!(result["ok"], false);
    assert!(result["data"].is_null());
    Ok(())
}

#[test]
fn cancelled_inspection_never_initializes_local_state() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let token = CancellationToken::default();
    token.cancel();
    let control = InvocationControl::new(Arc::new(SystemClock::default()), token, 10_000)?;
    let result = craft_cli::operations::execute(
        craft_cli::args::OperationAction::Get(craft_cli::args::OperationGetArgs {
            operation_id: "publish-summary".into(),
        }),
        None,
        &craft_cli::profiles::InvocationOptions {
            state_root: Some(&fixture.root),
            profile: None,
            request_id: "inspect-request",
            max_bytes: 16_384,
        },
        &control,
    );
    assert_eq!(
        result.err().ok_or("Expected cancellation")?.code,
        craft_core::error::ErrorCode::Cancelled
    );
    assert!(!fixture.root.exists());
    Ok(())
}

#[test]
fn operation_list_pages_by_identity_and_filters_unresolved_records_without_replay_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let seeded = fixture.seed(1, SubmissionState::Unknown, None)?;
    let journal = fixture.journal()?;
    for id in ["alpha", "bravo", "charlie"] {
        let mut record = journal.begin(&OperationBinding {
            operation_id: id.into(),
            ..seeded.binding.clone()
        })?;
        record.steps.insert(
            "create".into(),
            JournalStep {
                request_digest: Some(content_digest(id.as_bytes())),
                ..Default::default()
            },
        );
        record.revision += 1;
        journal.compare_and_swap(record.revision - 1, &record)?;
        if id == "bravo" {
            record.steps.get_mut("create").ok_or("step")?.state = SubmissionState::SendStarted;
            record.revision += 1;
            journal.compare_and_swap(record.revision - 1, &record)?;
        }
    }
    let first = success(&fixture.invoke(&["operation", "list", "--limit", "2"], None)?)?;
    let ids = |page: &Value| -> Vec<String> {
        page["data"]["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["operation_id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(ids(&first), ["alpha", "bravo"]);
    assert_eq!(first["data"]["has_more"], true);
    assert_eq!(first["data"]["next_after_operation_id"], "bravo");
    assert_eq!(first["data"]["live_verified"], false);
    assert_eq!(first["data"]["replay_authorized"], false);
    let second = success(&fixture.invoke(
        &[
            "operation",
            "list",
            "--limit",
            "2",
            "--after-operation-id",
            "bravo",
        ],
        None,
    )?)?;
    assert_eq!(ids(&second), ["charlie", "publish-summary"]);
    assert_eq!(second["data"]["has_more"], false);
    assert!(second["data"]["next_after_operation_id"].is_null());
    // The filter keeps records whose sends are started or unknown, and still reports scanning.
    let unresolved = success(&fixture.invoke(
        &["operation", "list", "--input", "-"],
        Some(br#"{"unresolved_only":true}"#),
    )?)?;
    assert_eq!(ids(&unresolved), ["bravo", "publish-summary"]);
    assert_eq!(unresolved["data"]["scanned"], 4);
    let schema = success(&fixture.invoke(
        &[
            "schema",
            "--command",
            "operation.list",
            "--response",
            "--view",
            "full",
            "--max-bytes",
            "65536",
        ],
        None,
    )?)?;
    for page in [&first, &second, &unresolved] {
        assert!(
            jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(page),
            "{page}"
        );
    }
    for bad in [
        vec!["operation", "list", "--limit", "0"],
        vec!["operation", "list", "--limit", "101"],
    ] {
        assert!(!fixture.invoke(&bad, None)?.status.success());
    }
    let refused = envelope(&fixture.invoke(
        &["operation", "list", "--input", "-"],
        Some(br#"{"after_operation_id":"","extra":1}"#),
    )?)?;
    assert_eq!(refused["ok"], false);
    Ok(())
}

const DELETE_INPUT: &[u8] = br#"{"apply":true,"expected":{"id":"10000","issue_type_id":"101","key":"EX-1","project_id":"100","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"},"issue":"10000","operation_id":"issue-change","subtasks":{"mode":"refuse"}}"#;

#[test]
fn local_preview_describes_effect_and_preconditions_without_state_or_write()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let preview = success(&fixture.invoke(
        &[
            "operation",
            "preview",
            "--command",
            "jira.issue.delete",
            "--file",
            "-",
        ],
        Some(DELETE_INPUT),
    )?)?;
    let data = &preview["data"];
    assert_eq!(data["command"], "jira.issue.delete");
    assert_eq!(data["valid"], true);
    assert_eq!(data["effect"], "permanent");
    assert_eq!(data["destructive"], true);
    assert_eq!(data["apply_required"], true);
    assert_eq!(data["operation_id"], "issue-change");
    assert_eq!(data["preconditions"]["expected"]["key"], "EX-1");
    assert!(data["remote"].is_null());
    for flag in ["write_sent", "lock_held", "approval_granted"] {
        assert_eq!(data[flag], false, "{flag}");
    }
    let schema = success(&fixture.invoke(
        &[
            "schema",
            "--command",
            "operation.preview",
            "--response",
            "--view",
            "full",
            "--max-bytes",
            "65536",
        ],
        None,
    )?)?;
    assert!(jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(&preview));
    // Local preview never creates or opens private state.
    assert!(!fixture.root.exists());
    // A different payload is a different request digest; the same payload is stable.
    let changed = String::from_utf8(DELETE_INPUT.to_vec())?.replace("EX-1", "EX-2");
    let other = success(&fixture.invoke(
        &[
            "operation",
            "preview",
            "--command",
            "jira.issue.delete",
            "--file",
            "-",
        ],
        Some(changed.as_bytes()),
    )?)?;
    assert_ne!(other["data"]["input_digest"], data["input_digest"]);
    let again = success(&fixture.invoke(
        &[
            "operation",
            "preview",
            "--command",
            "jira.issue.delete",
            "--file",
            "-",
        ],
        Some(DELETE_INPUT),
    )?)?;
    assert_eq!(again["data"]["input_digest"], data["input_digest"]);
    Ok(())
}

#[test]
fn preview_refuses_reads_bad_input_unobservable_targets_and_implicit_profiles()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let cases: [(&[&str], &[u8], &str); 4] = [
        (&["operation", "preview", "--command", "jira.issue.get", "--file", "-"], br#"{"issue":"1"}"#, "UNSUPPORTED_CAPABILITY"),
        (&["operation", "preview", "--command", "jira.issue.delete", "--file", "-"], br#"{"issue":"1","unknown":true}"#, "INPUT_INVALID"),
        (&["operation", "preview", "--command", "jira.comment.create", "--check-remote", "--file", "-"], br#"{"apply":true,"body":{"content":[{"content":[{"text":"Review complete.","type":"text"}],"type":"paragraph"}],"type":"doc","version":1},"issue_id":"10000","operation_id":"review-comment","project_id":"10000","visibility":{"mode":"project"}}"#, "UNSUPPORTED_CAPABILITY"),
        (&["--profile", "team", "operation", "preview", "--command", "jira.issue.delete", "--file", "-"], DELETE_INPUT, "INPUT_INVALID"),
    ];
    for (arguments, input, code) in cases {
        let output = envelope(&fixture.invoke(arguments, Some(input))?)?;
        assert_eq!(output["ok"], false, "{arguments:?}");
        assert_eq!(output["error"]["code"], code, "{arguments:?} {output}");
    }
    assert!(!fixture.root.exists());
    Ok(())
}

#[test]
fn remote_preview_runs_only_the_mapped_read_and_records_no_operation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.seed(1, SubmissionState::Confirmed, None)?;
    let preview = success(&fixture.invoke(
        &[
            "--profile",
            "team",
            "operation",
            "preview",
            "--command",
            "jira.issue.delete",
            "--check-remote",
            "--file",
            "-",
        ],
        Some(DELETE_INPUT),
    )?)?;
    let remote = &preview["data"]["remote"];
    assert_eq!(remote["command"], "jira.issue.get");
    // No read policy or credential exists here, so the read itself is refused; the preview
    // still reports that refusal as data and sends nothing.
    assert_eq!(remote["ok"], false);
    assert!(remote["result"]["code"].is_string());
    assert_eq!(preview["data"]["write_sent"], false);
    assert!(fixture.journal()?.get("issue-change")?.is_none());
    Ok(())
}
