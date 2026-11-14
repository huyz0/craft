use super::*;
use clap::Parser;
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::issue_mutation_policy::IssueMutationPolicyUpdate;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
mod actions;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct CandidateParser {
    #[command(subcommand)]
    action: CandidateAction,
}
#[derive(clap::Subcommand)]
enum CandidateAction {
    Issue {
        #[command(subcommand)]
        action: CandidateIssue,
    },
    Transition {
        #[command(subcommand)]
        action: CandidateTransition,
    },
}
#[derive(clap::Subcommand)]
enum CandidateIssue {
    Update(Box<IssueUpdateArgs>),
    Assign(Box<IssueAssignArgs>),
    Unassign(Box<IssueUnassignArgs>),
    Delete(Box<IssueDeleteArgs>),
}
#[derive(clap::Subcommand)]
enum CandidateTransition {
    Apply(Box<TransitionApplyArgs>),
}
impl From<CandidateAction> for crate::args::JiraAction {
    fn from(value: CandidateAction) -> Self {
        use crate::args::{JiraIssueAction as I, JiraTransitionAction as T};
        match value {
            CandidateAction::Issue { action } => Self::Issue {
                action: match action {
                    CandidateIssue::Update(v) => I::Update(v),
                    CandidateIssue::Assign(v) => I::Assign(v),
                    CandidateIssue::Unassign(v) => I::Unassign(v),
                    CandidateIssue::Delete(v) => I::Delete(v),
                },
            },
            CandidateAction::Transition {
                action: CandidateTransition::Apply(v),
            } => Self::Transition {
                action: T::Apply(v),
            },
        }
    }
}
fn state() -> Value {
    json!({"id":"10000","key":"ED-7","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"})
}
fn arguments(command: &str) -> Value {
    let mut value = json!({"operation_id":"issue-change","apply":true,"issue":"10000","expected":state(),"changes":{"description":null}});
    if command == TRANSITION_ISSUE {
        value["transition_id"] = json!("31");
        value["expected_status_id"] = json!("103");
    }
    value
}
fn args(command: &str, value: Value) -> Result<IssueMutationArgs, serde_json::Error> {
    if command == EDIT_ISSUE {
        serde_json::from_value(value).map(IssueMutationArgs::Edit)
    } else {
        serde_json::from_value(value).map(IssueMutationArgs::Transition)
    }
}
fn options(root: Option<&Path>) -> InvocationOptions<'_> {
    InvocationOptions {
        state_root: root,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16_384,
    }
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
struct Files {
    root: PathBuf,
    parent: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            root: craft_test_support::state::fixture_root(&parent, "mutation-command", stamp),
            parent,
        })
    }
    fn setup(&self) -> Result<ProfileStore, Box<dyn std::error::Error>> {
        let store = ProfileStore::open(&self.root, &control()?)?;
        store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some("00000000-1111-2222-3333-444444444444".into()),
                defaults: BTreeMap::new(),
            },
            &control()?,
            |_| Ok(()),
        )?;
        Ok(store)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
#[test]
fn flags_and_structured_mutations_have_the_same_typed_changes_and_required_schema() -> TestResult {
    let mut registry = Registry::default();
    register_candidates(&mut registry)?;
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        let expected = serde_json::to_string(&state())?;
        let changes = r#"{"description":null}"#;
        let mut flags = vec![
            "candidate",
            if command == EDIT_ISSUE {
                "issue"
            } else {
                "transition"
            },
            if command == EDIT_ISSUE {
                "update"
            } else {
                "apply"
            },
            "--operation-id",
            "issue-change",
            "--apply",
            "--issue",
            "10000",
            "--expected",
            &expected,
            "--changes",
            changes,
        ];
        if command == TRANSITION_ISSUE {
            flags.extend(["--transition-id", "31", "--expected-status-id", "103"]);
        }
        let parsed = CandidateParser::try_parse_from(flags)?;
        let action: crate::args::JiraAction = parsed.action.into();
        let parsed = match action {
            crate::args::JiraAction::Issue {
                action: crate::args::JiraIssueAction::Update(args),
            } => IssueMutationArgs::Edit(*args),
            crate::args::JiraAction::Transition {
                action: crate::args::JiraTransitionAction::Apply(args),
            } => IssueMutationArgs::Transition(*args),
            _ => return Err("mutation action".into()),
        };
        let json = args(command, arguments(command))?;
        assert_eq!(parsed.input()?, json.input()?);
        json.validate(&[], &options(None))?;
        let schema = &registry.descriptor(command)?.input_schema;
        let validator = jsonschema::validator_for(schema)?;
        assert!(validator.is_valid(&arguments(command)));
        for field in ["expected", "changes", "issue", "operation_id"] {
            let mut missing = arguments(command);
            missing.as_object_mut().ok_or("object")?.remove(field);
            assert!(!validator.is_valid(&missing), "{field}");
        }
    }
    Ok(())
}
#[test]
fn malformed_input_apply_cloud_projection_and_budget_fail_before_private_state() -> TestResult {
    let files = Files::new()?;
    let absent = files.root.join("not-created");
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        for (field, value) in [
            ("issue", json!("ED-7")),
            ("apply", json!(false)),
            ("expected", json!({})),
            ("changes", json!({"status":"Done"})),
            ("cloud_id", json!("bad")),
            ("operation_id", json!("")),
        ] {
            let mut bad = arguments(command);
            bad[field] = value;
            if let Ok(args) = args(command, bad) {
                assert!(
                    !execute(args, &[], &options(Some(&absent)), &control()?)
                        .envelope
                        .ok
                );
            }
            assert!(!absent.exists());
        }
        for fields in [vec!["unknown".into()], vec![]] {
            let mut options = options(Some(&absent));
            options.max_bytes = 512;
            assert!(
                !execute(
                    args(command, arguments(command))?,
                    &fields,
                    &options,
                    &control()?
                )
                .envelope
                .ok
            );
            assert!(!absent.exists());
        }
    }
    assert!(expected(r#"{"id":"1","id":"2"}"#).is_err());
    assert!(changes(r#"{"summary":"a","summary":"b"}"#).is_err());
    Ok(())
}
#[test]
fn create_or_read_configuration_does_not_grant_mutation_and_captured_policy_is_exact() -> TestResult
{
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let project = ResourceRef {
        resource_type: "project".into(),
        id: "100".into(),
        ..site("00000000-1111-2222-3333-444444444444")
    };
    store.set_create_policy(
        "factory",
        craft_runtime::create_policy::CreatePolicyUpdate {
            target: project.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(
        PreparedInvocation::prepare(
            args(EDIT_ISSUE, arguments(EDIT_ISSUE))?,
            &[],
            &options(None),
            &store,
            &control
        )
        .is_err()
    );
    let policy = store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: project.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![Operation::Edit],
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let prepared = PreparedInvocation::prepare(
        args(EDIT_ISSUE, arguments(EDIT_ISSUE))?,
        &[],
        &options(None),
        &store,
        &control,
    )?;
    assert_eq!(prepared.policy, policy);
    assert!(
        PreparedInvocation::prepare(
            args(TRANSITION_ISSUE, arguments(TRANSITION_ISSUE))?,
            &[],
            &options(None),
            &store,
            &control
        )
        .is_err()
    );
    store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: project,
            expected_actor: "actor-one".into(),
            enabled: false,
            operations: vec![Operation::Edit],
            expected_revision: Some(1),
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(
        store
            .verify_issue_mutation_policy(&prepared.policy, prepared.operation, &control)
            .is_err()
    );
    Ok(())
}
