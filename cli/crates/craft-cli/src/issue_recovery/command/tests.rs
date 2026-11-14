use super::*;
use crate::issue_recovery::tests::{original as original_request, project};
use clap::Parser;
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::issue_recovery_policy::{
    IssueRecoveryOperation as Op, IssueRecoveryPolicyUpdate,
};
use std::collections::BTreeMap;
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Parser)]
struct Candidate {
    #[command(flatten)]
    args: OperationReconcileArgs,
}
fn args() -> Result<OperationReconcileArgs, serde_json::Error> {
    serde_json::from_value(json!({"operation_id":"recover-create","original":original_request()}))
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        30_000,
    )
}
#[test]
fn recovery_flags_json_and_schema_require_one_source_before_state() -> TestResult {
    let original_json = serde_json::to_string(&original_request())?;
    let flags = Candidate::try_parse_from([
        "candidate",
        "--operation-id",
        "recover-create",
        "--original",
        &original_json,
    ])?
    .args;
    assert_eq!(serde_json::to_value(flags)?, serde_json::to_value(args()?)?);
    let mut registry = Registry::default();
    register_candidate(&mut registry)?;
    let descriptor = registry.descriptor(COMMAND)?;
    let schema = &descriptor.input_schema;
    let validator = jsonschema::validator_for(schema)?;
    assert!(validator.is_valid(&json!({"operation_id":"recover-create","saved":true})));
    assert!(!validator.is_valid(&json!({"operation_id":"recover-create"})));
    assert!(!validator.is_valid(
        &json!({"operation_id":"recover-create","saved":true,"original":original_request()})
    ));
    for bad in [
        json!({"operation_id":"recover-create","original":null}),
        json!({"operation_id":"recover-create","original":original_request(),"actor":"injected"}),
    ] {
        assert!(serde_json::from_value::<OperationReconcileArgs>(bad).is_err());
    }
    let missing = Candidate::try_parse_from(["candidate"])?.args;
    let options = InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "recover-request",
        max_bytes: 16_777_216,
    };
    assert!(missing.validate(&[], &options).is_err());
    for patch in [
        json!({"candidate_issue_ids":["1","1"]}),
        json!({"candidate_issue_ids":["x"]}),
        json!({"candidate_issue_ids":vec!["1";21]}),
        json!({"operation_id":""}),
    ] {
        let mut value = serde_json::to_value(args()?)?;
        for (key, replacement) in patch.as_object().ok_or("patch")? {
            value[key] = replacement.clone();
        }
        let bad: OperationReconcileArgs = serde_json::from_value(value)?;
        assert!(bad.validate(&[], &options).is_err());
    }
    assert!(args()?.validate(&["unexpected".into()], &options).is_err());
    assert!(
        args()?
            .validate(
                &[],
                &InvocationOptions {
                    max_bytes: 512,
                    ..options
                }
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn recovery_candidate_requires_exact_private_policy_and_keeps_missing_evidence_local() -> TestResult
{
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let root = craft_test_support::state::fixture_root(
        &parent,
        "recovery-candidate",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let control = control()?;
    let invalid_options = InvocationOptions {
        state_root: Some(&root),
        profile: Some("factory"),
        request_id: "recover-request",
        max_bytes: 16_777_216,
    };
    let invalid = Candidate::try_parse_from(["candidate"])?.args;
    assert!(
        !execute(invalid, &[], &invalid_options, &control)
            .envelope
            .ok
    );
    assert!(!root.exists());
    let store = ProfileStore::open(&root, &control)?;
    store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(project().tenant),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    let options = InvocationOptions {
        state_root: Some(&root),
        profile: Some("factory"),
        request_id: "recover-request",
        max_bytes: 16_777_216,
    };
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let record = store.set_issue_recovery_policy(
        "factory",
        IssueRecoveryPolicyUpdate {
            target: project(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![Op::Create],
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let prepared = PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?;
    assert_eq!(prepared.grant(), &store.get("factory", &control)?.grant);
    let journal = NativeJournal::open(&store, &control)?;
    let result = prepared.execute_with_credentials(
        &store,
        &journal,
        TlsPolicy::Platform,
        control.clone(),
        |_| panic!("local recovery must not open credentials"),
    );
    assert!(result.envelope.ok);
    let data = result.envelope.data.ok_or("data")?;
    assert_eq!(data["assessment"], "local_evidence_missing");
    assert_eq!(data["replay_authorized"], false);
    assert!(journal.get("recover-create")?.is_none());
    let initial = journal.begin(&prepared.host.binding)?;
    let mut started = initial.clone();
    started.revision += 1;
    started.steps.insert(
        created_issue::ROUTE.into(),
        craft_core::storage::JournalStep {
            request_digest: Some(craft_core::input::content_digest(b"native-create")),
            state: craft_core::storage::SubmissionState::Prepared,
            confirmed_resources: vec![],
            job: None,
        },
    );
    journal.compare_and_swap(initial.revision, &started)?;
    let previous = started.clone();
    started.revision += 1;
    started
        .steps
        .get_mut(created_issue::ROUTE)
        .ok_or("step")?
        .state = craft_core::storage::SubmissionState::SendStarted;
    journal.compare_and_swap(previous.revision, &started)?;
    let previous = started.clone();
    started.revision += 1;
    started
        .steps
        .get_mut(created_issue::ROUTE)
        .ok_or("step")?
        .state = craft_core::storage::SubmissionState::Unknown;
    journal.compare_and_swap(previous.revision, &started)?;
    let result = prepared.execute_with_credentials(
        &store,
        &journal,
        TlsPolicy::Platform,
        control.clone(),
        |_| panic!("unknown create without identities must not open credentials"),
    );
    assert!(result.envelope.ok);
    let data = result.envelope.data.ok_or("data")?;
    assert_eq!(data["assessment"], "unresolved");
    assert_eq!(data["initial_record"], json!(started));
    assert_eq!(journal.get("recover-create")?, Some(started));
    let mut wrong = args()?;
    wrong.original.as_mut().ok_or("original")?.target.tenant =
        "11111111-1111-2222-3333-444444444444".into();
    assert!(PreparedInvocation::prepare(wrong, &[], &options, &store, &control).is_err());
    let prepared = PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?;
    store.set_issue_recovery_policy(
        "factory",
        IssueRecoveryPolicyUpdate {
            target: project(),
            expected_actor: "actor-one".into(),
            enabled: false,
            operations: vec![Op::Create],
            expected_revision: Some(record.revision),
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let result = prepared.execute_with_credentials(
        &store,
        &journal,
        TlsPolicy::Platform,
        control.clone(),
        |_| panic!("revoked recovery must not open credentials"),
    );
    assert!(!result.envelope.ok);
    drop(journal);
    drop(store);
    let resolved = root.canonicalize()?;
    assert!(resolved.starts_with(&parent) && resolved != parent);
    std::fs::remove_dir_all(&resolved)?;
    Ok(())
}
