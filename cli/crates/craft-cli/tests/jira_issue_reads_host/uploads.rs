use super::*;
use craft_cli::attachment_upload::command::UploadArgs;
use craft_cli::attachment_upload::{PreparedUpload, UploadEnvironment};
use craft_cli::profiles::InvocationOptions;
use craft_core::input::content_digest;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyUpdate,
};
use craft_runtime::journal::NativeJournal;
fn project() -> ResourceRef {
    ResourceRef {
        resource_type: "project".into(),
        id: "200".into(),
        ..target()
    }
}
fn allow(setup: &Setup) -> Result<(), CraftError> {
    setup.store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: project(),
            expected_actor: "actor-one".into(),
            operations: vec![Op::AttachmentUpload],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn args(
    setup: &Setup,
    operation: &str,
    bytes: &[u8],
) -> Result<UploadArgs, Box<dyn std::error::Error>> {
    let input = setup.root.join("input");
    fs::create_dir_all(&input)?;
    fs::write(input.join("source.bin"), bytes)?;
    Ok(UploadArgs {
        operation_id: operation.into(),
        project_id: "200".into(),
        issue_id: "100".into(),
        artifact_name: "snapshot.bin".into(),
        file_root: Some(input.to_str().ok_or("path")?.into()),
        file: Some("source.bin".into()),
        from_artifact: false,
        expected_digest: None,
        filename: "設計.bin".into(),
        media_type: "application/octet-stream".into(),
        byte_limit: 1_000_000,
        cloud_id: None,
        apply: true,
    })
}
fn options(setup: &Setup) -> InvocationOptions<'_> {
    InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "upload-original",
        max_bytes: 8_388_608,
    }
}
fn context(enabled: bool) -> Result<Vec<Reply>, Box<dyn std::error::Error>> {
    let issue = json!({"id":"100","fields":{"project":{"id":"200"}}});
    let settings = json!({"enabled":enabled,"uploadLimit":1_000_000});
    Ok(vec![
        reply(issue.clone())?,
        reply(settings.clone())?,
        reply(
            json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"CREATE_ATTACHMENTS":{"key":"CREATE_ATTACHMENTS","havePermission":true}}}),
        )?,
        reply(settings)?,
        reply(issue)?,
    ])
}
fn attachment(id: &str, size: usize) -> Value {
    json!({"id":id,"filename":"設計.bin","size":size,"mimeType":"text/plain","author":{"accountId":"actor-one","displayName":"private-sentinel"},"content":"https://untrusted.invalid/private-sentinel"})
}
fn post_bytes(request: &tls_fixture::Observed) -> Result<&[u8], Box<dyn std::error::Error>> {
    let offset = request
        .body
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("multipart prefix")?
        + 4;
    let boundary = request
        .headers
        .get("content-type")
        .and_then(|v| v.strip_prefix("multipart/form-data; boundary="))
        .ok_or("boundary")?;
    let suffix = format!("\r\n--{boundary}--\r\n");
    if !request.body.ends_with(suffix.as_bytes()) {
        return Err("closing delimiter".into());
    }
    Ok(&request.body[offset..request.body.len() - suffix.len()])
}
#[test]
fn upload_captures_exact_snapshot_and_confirmed_operation_never_sends_again() -> TestResult {
    let setup = Setup::new()?;
    allow(&setup)?;
    let bytes = (0..160_013).map(|n| (n % 256) as u8).collect::<Vec<_>>();
    let input = args(&setup, "upload-success", &bytes)?;
    let mut rows = vec![identity(), identity()];
    rows.extend(context(true)?);
    rows.push(reply(json!([attachment("10001", bytes.len())]))?);
    let product = TlsServer::start(Reply::Sequence(rows))?;
    let issuer = issuer("read:jira-user read:jira-work write:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    let prepared = PreparedUpload::prepare(
        input.clone(),
        &[],
        &options(&setup),
        &setup.store,
        &control()?,
    )?;
    let run = |host: &PreparedUpload| -> Result<craft_cli::dispatch::CommandResult, CraftError> {
        Ok(host.execute(
            UploadEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            || Ok(()),
        ))
    };
    let first = run(&prepared)?;
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    let value = data(&first.envelope)?;
    assert_eq!(value["receipt"]["request_state"], "confirmed");
    assert_eq!(value["receipt"]["confirmed_resources"][0]["id"], "10001");
    assert_eq!(value["snapshot"]["digest"], content_digest(&bytes));
    assert_eq!(value["snapshot_write_state"], "confirmed");
    assert_eq!(value["saved_request"]["write_state"], "confirmed");
    assert_eq!(value["desired_fields_verified"], true);
    assert_eq!(value["remote_digest_verified"], false);
    let mut registry = craft_core::registry::Registry::default();
    craft_cli::attachment_upload::command::register(&mut registry)?;
    assert!(
        jsonschema::validator_for(
            &registry
                .descriptor("jira.attachment.upload")?
                .response_schema
        )?
        .is_valid(&json!(first.envelope))
    );
    let encoded = craft_cli::output::encode(first.envelope, 8_388_608, first.output_policy)?;
    assert_eq!(encoded.exit, 0);
    assert!(!String::from_utf8_lossy(&encoded.bytes).contains("private-sentinel"));
    assert!(serde_json::from_slice::<Envelope>(&encoded.bytes)?.ok);
    fs::write(setup.root.join("input/source.bin"), b"different source")?;
    let repeat = run(&PreparedUpload::prepare(
        input,
        &[],
        &options(&setup),
        &setup.store,
        &control()?,
    )?)?;
    assert_eq!(code(&repeat.envelope), Some(ErrorCode::OperationIdReused));
    assert_eq!(
        data(&repeat.envelope)?["receipt"]["request_state"],
        "confirmed"
    );
    assert_eq!(
        data(&repeat.envelope)?["receipt"]["confirmed_resources"][0]["id"],
        "10001"
    );
    let observed = product.finish()?;
    assert_eq!(observed.len(), 8);
    let uploads = observed
        .iter()
        .filter(|r| r.method == "POST")
        .collect::<Vec<_>>();
    assert_eq!(uploads.len(), 1);
    assert_eq!(post_bytes(uploads[0])?, bytes.as_slice());
    assert_eq!(
        uploads[0]
            .headers
            .get("x-atlassian-token")
            .map(String::as_str),
        Some("no-check")
    );
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn lost_or_sparse_upload_acknowledgment_keeps_source_and_blocks_all_replay() -> TestResult {
    for mode in ["lost", "sparse"] {
        let setup = Setup::new()?;
        allow(&setup)?;
        let input = args(&setup, "upload-uncertain", b"upload-source")?;
        let mut rows = vec![identity(), identity()];
        rows.extend(context(true)?);
        rows.push(if mode == "lost" {
            Reply::Disconnect
        } else {
            reply(json!([{"id":"10001"}]))?
        });
        let product = TlsServer::start(Reply::Sequence(rows))?;
        let issuer = issuer("read:jira-user read:jira-work write:jira-work")?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        let prepared =
            PreparedUpload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?;
        let run = || -> Result<Envelope, CraftError> {
            Ok(prepared
                .execute(
                    UploadEnvironment {
                        store: &setup.store,
                        client: fixture.client(),
                        locked: &locked,
                        journal: &journal,
                        tls: TlsPolicy::AdditionalCaBundle(product.root()),
                        control: control()?,
                    },
                    || Ok(()),
                )
                .envelope)
        };
        let first = run()?;
        assert!(!first.ok);
        assert_eq!(data(&first)?["snapshot_write_state"], "confirmed");
        assert_eq!(data(&first)?["saved_request"]["write_state"], "confirmed");
        assert_eq!(
            data(&first)?["receipt"]["request_state"],
            if mode == "lost" {
                "sent_unknown"
            } else {
                "confirmed"
            }
        );
        assert_eq!(data(&first)?["reconciliation_required"], true);
        let second = run()?;
        assert_eq!(code(&second), Some(ErrorCode::OperationIdReused));
        assert_eq!(product.finish()?.len(), 8);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
#[test]
fn preclaim_refusal_can_resume_only_the_original_frozen_snapshot() -> TestResult {
    let setup = Setup::new()?;
    allow(&setup)?;
    let original = b"original-frozen-source";
    let input = args(&setup, "upload-resume-source", original)?;
    let mut rows = vec![identity(), identity()];
    let disabled = context(false)?;
    rows.extend(disabled.into_iter().take(2));
    rows.push(identity());
    rows.extend(context(true)?);
    rows.push(reply(json!([attachment("10001", original.len())]))?);
    let product = TlsServer::start(Reply::Sequence(rows))?;
    let issuer = issuer("read:jira-user read:jira-work write:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    let run = |input| -> Result<Envelope, CraftError> {
        Ok(
            PreparedUpload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?
                .execute(
                    UploadEnvironment {
                        store: &setup.store,
                        client: fixture.client(),
                        locked: &locked,
                        journal: &journal,
                        tls: TlsPolicy::AdditionalCaBundle(product.root()),
                        control: control()?,
                    },
                    || Ok(()),
                )
                .envelope,
        )
    };
    let refused = run(input.clone())?;
    assert_eq!(code(&refused), Some(ErrorCode::UnsupportedCapability));
    assert_eq!(data(&refused)?["receipt"]["request_state"], "not_sent");
    fs::write(
        setup.root.join("input/source.bin"),
        b"new workspace contents",
    )?;
    let mut changed = input.clone();
    changed.file = Some("other.bin".into());
    let mismatch = run(changed)?;
    assert_eq!(code(&mismatch), Some(ErrorCode::OperationIdReused));
    let mut captured = input;
    captured.from_artifact = true;
    captured.file_root = None;
    captured.file = None;
    captured.expected_digest = Some(content_digest(original));
    let resumed = run(captured)?;
    assert!(resumed.ok, "{:?}", resumed.error);
    assert_eq!(data(&resumed)?["snapshot_reused"], true);
    let observed = product.finish()?;
    assert_eq!(observed.len(), 11);
    let uploaded = observed.iter().find(|r| r.method == "POST").ok_or("post")?;
    assert_eq!(post_bytes(uploaded)?, original);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}
#[test]
fn missing_upload_scope_and_live_permission_refusal_never_reach_post() -> TestResult {
    for mode in ["scope", "permission"] {
        let setup = Setup::new()?;
        allow(&setup)?;
        let input = args(&setup, "upload-refused", b"upload-source")?;
        let mut rows = vec![identity(), identity()];
        if mode == "permission" {
            let mut context = context(true)?;
            context[2] = reply(
                json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"CREATE_ATTACHMENTS":{"key":"CREATE_ATTACHMENTS","havePermission":false}}}),
            )?;
            rows.extend(context.into_iter().take(3));
        }
        let product = TlsServer::start(Reply::Sequence(rows))?;
        let issuer = issuer(if mode == "scope" {
            "read:jira-user read:jira-work"
        } else {
            "read:jira-user read:jira-work write:jira-work"
        })?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        let envelope =
            PreparedUpload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?
                .execute(
                    UploadEnvironment {
                        store: &setup.store,
                        client: fixture.client(),
                        locked: &locked,
                        journal: &journal,
                        tls: TlsPolicy::AdditionalCaBundle(product.root()),
                        control: control()?,
                    },
                    || Ok(()),
                )
                .envelope;
        assert_eq!(code(&envelope), Some(ErrorCode::PermissionDenied));
        assert_eq!(data(&envelope)?["receipt"]["request_state"], "not_sent");
        assert_eq!(data(&envelope)?["snapshot_write_state"], "confirmed");
        let observed = product.finish()?;
        assert_eq!(observed.len(), if mode == "scope" { 2 } else { 5 });
        assert!(observed.iter().all(|r| r.method != "POST"));
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
