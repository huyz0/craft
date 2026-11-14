use super::*;

use clap::{CommandFactory, Parser};
use craft_core::control::CancellationToken;
use craft_runtime::create_policy::CreatePolicyUpdate;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser)]
struct Candidate {
    #[command(flatten)]
    args: IssueCreateArgs,
}
fn args() -> Result<IssueCreateArgs, serde_json::Error> {
    serde_json::from_value(
        json!({"operation_id":"create-report","apply":true,"issue_type_id":"10001","summary":"Example"}),
    )
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}

#[test]
fn candidate_flags_json_presence_and_structured_only_fields_share_the_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    register_candidate(&mut registry)?;
    let example = json!({"operation_id":"create-report","apply":true,"issue_type_id":"10001","summary":"Unicode 世界","description":"null","parent_issue_id":"20000","assignee_account_id":"account-two","project_id":"10000"});
    let mut parser = Candidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &example)?;
    let actual = Candidate::try_parse_from(&flags)?.args;
    assert_eq!(
        serde_json::to_value(actual)?,
        serde_json::to_value(serde_json::from_value::<IssueCreateArgs>(example)?)?
    );
    let null: IssueCreateArgs = serde_json::from_value(
        json!({"operation_id":"create-report","apply":true,"issue_type_id":"10001","summary":"Example","description":null,"labels":null}),
    )?;
    assert!(matches!(null.description, Some(InputField::Null)));
    assert!(matches!(null.labels, Some(InputField::Null)));
    assert_eq!(serde_json::to_value(null)?["description"], Value::Null);
    let omitted = serde_json::to_value(args()?)?;
    assert!(omitted.get("description").is_none());
    assert!(omitted.get("labels").is_none());
    for input in [
        json!({"operation_id":"create-report","apply":true,"issue_type_id":"10001","summary":"Example","client_secret":"forbidden"}),
        json!({"operation_id":"create-report","apply":"true","issue_type_id":"10001","summary":"Example"}),
    ] {
        assert!(registry.validate(COMMAND, &input).is_err());
    }
    assert_eq!(
        registry.descriptor(COMMAND)?.input_schema["properties"]["labels"]["x-craft-input-channel"],
        "json_only"
    );
    assert!(
        crate::catalog::local_registry()?
            .descriptor(COMMAND)
            .is_err()
    );
    assert!(crate::args::Cli::try_parse_from(["craft", "jira", "issue", "create"]).is_ok());
    Ok(())
}

#[test]
fn invalid_or_unapplied_dispatch_preserves_identity_without_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let files = Files::new()?;
    let mut registry = crate::catalog::local_registry()?;
    register_candidate(&mut registry)?;
    let mut input = args()?;
    input.apply = false;
    let options = InvocationOptions {
        state_root: Some(&files.root),
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16_384,
    };
    let result = crate::dispatch::execute(
        &registry,
        crate::args::Action::Jira {
            action: crate::args::JiraAction::Issue {
                action: crate::args::JiraIssueAction::Create(Box::new(input)),
            },
        },
        &[],
        &control()?,
        &options,
        false,
    )?;
    assert!(!files.root.exists());
    assert_eq!(result.envelope.request_id, "original-request");
    assert_eq!(result.envelope.command, COMMAND);
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::ApplyRequired)
    );
    let small = InvocationOptions {
        max_bytes: 512,
        ..options
    };
    let result = execute(args()?, &[], &small, &control()?);
    assert_eq!(
        result.envelope.error.as_ref().map(|error| error.code),
        Some(ErrorCode::OutputBudgetTooSmall)
    );
    assert!(!files.root.exists());
    let mut null = args()?;
    null.project_id = Some(InputField::Null);
    let result = execute(null, &[], &options, &control()?);
    assert!(!result.envelope.ok);
    assert!(!files.root.exists());
    Ok(())
}

struct Files {
    parent: PathBuf,
    root: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            root: parent.join(format!("create-command-{}-{stamp}", std::process::id())),
            parent,
        })
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[test]
fn configured_profile_and_project_need_an_exact_enabled_private_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let files = Files::new()?;
    let control = control()?;
    let store = ProfileStore::open(&files.root, &control)?;
    let profile = store.create(
        &craft_core::profiles::ProfileInput {
            name: "factory".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(target().tenant),
            defaults: BTreeMap::from([("project".into(), target())]),
        },
        &control,
        |_| Ok(()),
    )?;
    let options = InvocationOptions {
        state_root: Some(&files.root),
        profile: Some("factory"),
        request_id: "request",
        max_bytes: 16_384,
    };
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)
            .err()
            .ok_or("Expected missing policy")?
            .code,
        ErrorCode::PolicyDenied
    );
    let update = |revision, enabled| CreatePolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        enabled,
        expected_revision: revision,
    };
    store.set_create_policy("factory", update(None, false), true, &control, |_| Ok(()))?;
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)
            .err()
            .ok_or("Expected disabled policy")?
            .code,
        ErrorCode::PolicyDenied
    );
    store.set_create_policy("factory", update(Some(1), true), true, &control, |_| Ok(()))?;
    let prepared = PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?;
    assert_eq!(prepared.grant(), &profile.grant);
    let mut other = args()?;
    other.project_id = Some(InputField::Value("99999".into()));
    assert_eq!(
        PreparedInvocation::prepare(other, &[], &options, &store, &control)
            .err()
            .ok_or("Expected target refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    Ok(())
}
