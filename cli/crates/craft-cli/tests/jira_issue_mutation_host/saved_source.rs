use super::*;
use craft_cli::issue_mutation::command::{IssueMutationArgs, PreparedInvocation};
use craft_cli::issue_recovery::command::{OperationReconcileArgs, PreparedInvocation as Recovery};
use craft_cli::profiles::InvocationOptions;
use craft_runtime::issue_mutation_policy::{IssueMutationOperation, IssueMutationPolicyUpdate};
use craft_runtime::issue_recovery_policy::{IssueRecoveryOperation, IssueRecoveryPolicyUpdate};

#[test]
fn lost_edit_uses_saved_source_for_read_only_inspection_without_another_put() -> TestResult {
    let setup = Setup::new()?;
    let c = control(CancellationToken::default())?;
    let project = ResourceRef {
        resource_type: "project".into(),
        id: "100".into(),
        ..target()
    };
    setup.profiles.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: project.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![IssueMutationOperation::Edit],
            expected_revision: None,
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    let issuer = issuer(SCOPES)?;
    let mut replies = vec![identity(), identity()];
    replies.extend(metadata(EDIT_ISSUE, false)?);
    replies.push(Reply::Disconnect);
    replies.push(identity());
    let mut observed = state();
    observed["fields"]["assignee"] = Value::Null;
    replies.push(reply(observed)?);
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
    let locked = fixture.lock(&setup.grant, &c)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let mut value = request(EDIT_ISSUE).input;
    value["operation_id"] = json!(OPERATION);
    value["apply"] = json!(true);
    let options = InvocationOptions {
        state_root: Some(&setup.files.root),
        profile: Some("factory"),
        request_id: "saved-edit",
        max_bytes: 262_144,
    };
    let candidate = PreparedInvocation::prepare(
        IssueMutationArgs::Edit(serde_json::from_value(value)?),
        &["saved_request.write_state".into()],
        &options,
        &setup.profiles,
        &c,
    )?;
    let result = candidate.execute_saved(
        &setup.profiles,
        fixture.client(),
        &locked,
        &setup.journal,
        TlsPolicy::AdditionalCaBundle(product.root()),
        c.clone(),
    );
    assert!(!result.envelope.ok);
    let data = result.envelope.data.as_ref().ok_or("data")?;
    assert_eq!(data["receipt"]["request_state"], "sent_unknown");
    assert_eq!(data["saved_request"]["write_state"], "confirmed");
    let source = craft_runtime::artifacts::NativeArtifacts::open(
        &setup.profiles.path().join("requests"),
        2_097_152,
        &c,
    )?;
    let blob = craft_core::storage::ArtifactStore::read(
        &source,
        data["saved_request"]["name"].as_str().ok_or("name")?,
        2_097_152,
    )?
    .ok_or("source")?;
    for secret in [CLIENT_ID, CLIENT_SECRET, ACCESS_TOKEN] {
        assert!(
            !blob
                .bytes
                .windows(secret.len())
                .any(|bytes| bytes == secret.as_bytes())
        );
    }
    let schema = craft_cli::catalog::registry()?
        .descriptor(EDIT_ISSUE)?
        .response_schema
        .clone();
    assert!(jsonschema::validator_for(&schema)?.is_valid(&serde_json::to_value(&result.envelope)?));
    let prior = setup.journal.get(OPERATION)?.ok_or("prior")?;
    setup.profiles.set_issue_recovery_policy(
        "factory",
        IssueRecoveryPolicyUpdate {
            target: project,
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![IssueRecoveryOperation::Edit],
            expected_revision: None,
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    let recovery = Recovery::prepare(
        OperationReconcileArgs {
            operation_id: OPERATION.into(),
            original: None,
            saved: true,
            candidate_issue_ids: vec![],
        },
        &[],
        &options,
        &setup.profiles,
        &c,
    )?;
    let inspected = recovery.execute_cached(
        &setup.profiles,
        fixture.client(),
        &locked,
        &setup.journal,
        TlsPolicy::AdditionalCaBundle(product.root()),
        c,
    );
    assert!(inspected.envelope.ok, "{:?}", inspected.envelope.error);
    let inspected = inspected.envelope.data.ok_or("inspection")?;
    assert_eq!(inspected["assessment"], "unresolved");
    assert_eq!(inspected["observations_complete"], true);
    assert_eq!(inspected["replay_authorized"], false);
    assert_eq!(setup.journal.get(OPERATION)?, Some(prior));
    let requests = product.finish()?;
    assert_eq!(requests.len(), count);
    let write = requests
        .iter()
        .position(|r| r.method == "PUT")
        .ok_or("write")?;
    assert_eq!(requests.iter().filter(|r| r.method == "PUT").count(), 1);
    assert!(requests.iter().skip(write + 1).all(|r| r.method == "GET"));
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    setup.no_plaintext()?;
    Ok(())
}
