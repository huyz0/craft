use super::*;
use craft_cli::attachment_download::command::{AttachmentDownloadArgs, PreparedDownload};
use craft_cli::attachment_download::{ARTIFACT_DIRECTORY, PreparedAttachmentDownload};
use craft_cli::attachment_reads::ReadEnvironment;
use craft_cli::profiles::InvocationOptions;
use craft_core::input::content_digest;
use craft_core::storage::ArtifactStore;
use craft_runtime::artifacts::NativeArtifacts;
fn args(setup: &Setup, name: &str) -> Result<AttachmentDownloadArgs, Box<dyn std::error::Error>> {
    let output = setup.root.with_extension("exports");
    fs::create_dir_all(&output)?;
    Ok(AttachmentDownloadArgs {
        id: "101".into(),
        artifact_name: name.into(),
        output_root: output.to_str().ok_or("path")?.into(),
        output: "result.bin".into(),
        byte_limit: 200_000,
        expected_digest: None,
        overwrite: false,
        cloud_id: None,
    })
}
fn options(setup: &Setup) -> InvocationOptions<'_> {
    InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "binary-download",
        max_bytes: 16384,
    }
}
fn allow(
    setup: &Setup,
) -> Result<craft_runtime::issue_read_policy::IssueReadPolicyRecord, CraftError> {
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::AttachmentDownload],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )
}
fn binary(bytes: &[u8]) -> Reply {
    let mut wire=format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=../../provider-name\r\nConnection: close\r\n\r\n",bytes.len()).into_bytes();
    wire.extend_from_slice(bytes);
    Reply::Raw(wire)
}

#[test]
fn attachment_download_streams_exact_bytes_exports_atomically_and_retains_artifact_on_collision()
-> TestResult {
    let setup = Setup::new()?;
    allow(&setup)?;
    let bytes = (0..160_013).map(|i| (i % 256) as u8).collect::<Vec<_>>();
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        binary(&bytes),
        identity(),
        binary(&bytes),
        identity(),
        identity(),
        binary(&bytes),
    ]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let run = |input: AttachmentDownloadArgs| -> Result<Envelope, CraftError> {
        Ok(
            PreparedDownload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?
                .execute(ReadEnvironment {
                    store: &setup.store,
                    client: fixture.client(),
                    locked: &locked,
                    tls: TlsPolicy::AdditionalCaBundle(product.root()),
                    control: control()?,
                })
                .envelope,
        )
    };
    let mut first_args = args(&setup, "first.bin")?;
    first_args.expected_digest = Some(content_digest(&bytes));
    let first = run(first_args)?;
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(data(&first)?["artifact_write_state"], "confirmed");
    assert_eq!(data(&first)?["export_write_state"], "confirmed");
    assert_eq!(
        data(&first)?["response_media_type"],
        "application/octet-stream"
    );
    assert_eq!(
        data(&first)?["artifact"]["source"]["resource_type"],
        "attachment"
    );
    assert_eq!(data(&first)?["artifact"]["source"]["id"], "101");
    assert_eq!(
        fs::read(setup.root.with_extension("exports").join("result.bin"))?,
        bytes
    );
    let registry = craft_cli::attachment_download::command::register;
    let mut contracts = craft_core::registry::Registry::default();
    registry(&mut contracts)?;
    assert!(
        jsonschema::validator_for(
            &contracts
                .descriptor("jira.attachment.download")?
                .response_schema
        )?
        .is_valid(&json!(first))
    );
    let encoded = craft_cli::output::encode(
        first,
        16384,
        craft_cli::output::OutputPolicy::PreserveEvidence,
    )?;
    assert_eq!(encoded.exit, 0);
    assert!(serde_json::from_slice::<Envelope>(&encoded.bytes)?.ok);
    let second = run(args(&setup, "second.bin")?)?;
    assert!(!second.ok);
    assert_eq!(data(&second)?["artifact_write_state"], "confirmed");
    assert_eq!(data(&second)?["export_write_state"], "failed");
    assert!(!data(&second)?["artifact"].is_null());
    assert_eq!(
        fs::read(setup.root.with_extension("exports").join("result.bin"))?,
        bytes
    );
    let collision = run(args(&setup, "first.bin")?)?;
    assert!(!collision.ok);
    assert_eq!(code(&collision), Some(ErrorCode::VersionConflict));
    assert_eq!(data(&collision)?["artifact_write_state"], "not_sent");
    let mut replacement = args(&setup, "third.bin")?;
    replacement.overwrite = true;
    let third = run(replacement)?;
    assert!(third.ok, "{:?}", third.error);
    let artifacts =
        NativeArtifacts::open(&setup.root.join(ARTIFACT_DIRECTORY), 200_000, &control()?)?;
    assert_eq!(
        artifacts
            .read("second.bin", 200_000)?
            .ok_or("artifact")?
            .bytes,
        bytes
    );
    let observed = product.finish()?;
    assert_eq!(observed.len(), 8);
    let downloads = observed
        .iter()
        .filter(|r| r.target.contains("/attachment/content/"))
        .collect::<Vec<_>>();
    assert_eq!(downloads.len(), 3);
    for request in downloads {
        assert_eq!(request.method, "GET");
        assert!(
            request
                .target
                .ends_with("/attachment/content/101?redirect=false")
        );
        assert_eq!(
            request.headers.get("accept-encoding").map(String::as_str),
            Some("identity")
        );
    }
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn attachment_download_refuses_bad_digest_oversize_partial_and_redirect_without_publication()
-> TestResult {
    for mode in ["digest", "oversize", "partial", "redirect"] {
        let setup = Setup::new()?;
        allow(&setup)?;
        let response = match mode {
            "partial" => Reply::json(206, b"abc", &[]),
            "redirect" => Reply::json(
                303,
                b"",
                &[("Location", "https://untrusted.invalid/secret")],
            ),
            _ => binary(b"abc"),
        };
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), response]))?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let mut input = args(&setup, "failed.bin")?;
        if mode == "digest" {
            input.expected_digest = Some(format!("sha256:{}", "0".repeat(64)));
        }
        if mode == "oversize" {
            input.byte_limit = 2;
        }
        let envelope =
            PreparedDownload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?
                .execute(ReadEnvironment {
                    store: &setup.store,
                    client: fixture.client(),
                    locked: &locked,
                    tls: TlsPolicy::AdditionalCaBundle(product.root()),
                    control: control()?,
                })
                .envelope;
        assert!(!envelope.ok, "{mode}");
        assert!(data(&envelope)?["artifact"].is_null());
        assert_eq!(data(&envelope)?["artifact_write_state"], "not_sent");
        assert_eq!(data(&envelope)?["export_write_state"], "not_sent");
        assert!(
            !setup
                .root
                .with_extension("exports")
                .join("result.bin")
                .exists()
        );
        let artifacts =
            NativeArtifacts::open(&setup.root.join(ARTIFACT_DIRECTORY), 200_000, &control()?)?;
        assert!(artifacts.receipt("failed.bin")?.is_none());
        assert_eq!(product.finish()?.len(), 3);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
#[test]
fn download_permission_and_scope_refusals_precede_binary_get() -> TestResult {
    let setup = Setup::new()?;
    let input = args(&setup, "refused.bin")?;
    assert!(
        PreparedDownload::prepare(
            input.clone(),
            &[],
            &options(&setup),
            &setup.store,
            &control()?
        )
        .is_err()
    );
    let old = setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::AttachmentGet],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    assert!(
        PreparedDownload::prepare(
            input.clone(),
            &[],
            &options(&setup),
            &setup.store,
            &control()?
        )
        .is_err()
    );
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::AttachmentDownload],
            enabled: true,
            expected_revision: Some(old.revision),
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![identity(), identity()]))?;
    let issuer = issuer("read:jira-user")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let envelope =
        PreparedDownload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?
            .execute(ReadEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            })
            .envelope;
    assert_eq!(code(&envelope), Some(ErrorCode::PermissionDenied));
    assert!(data(&envelope)?["artifact"].is_null());
    assert_eq!(product.finish()?.len(), 2);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}
#[test]
fn download_policy_revocation_after_artifact_commit_retains_receipt_and_suppresses_export()
-> TestResult {
    let setup = Setup::new()?;
    let captured = allow(&setup)?;
    let input = args(&setup, "retained.bin")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        binary(b"abc"),
    ]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let host = PreparedAttachmentDownload::prepare(
        CanonicalRequest {
            command: "jira.attachment.download".into(),
            target: target(),
            input: json!({"id":"101"}),
            selection: Selection {
                fields: vec![],
                limit: 20,
                cursor: None,
                max_bytes: 16384,
            },
            preconditions: None,
        },
        "retained-download",
        IssueReadPolicy {
            target: target(),
            grant: setup.grant.clone(),
            expected_actor: "actor-one".into(),
        },
        input,
    )?;
    let probe = NativeArtifacts::open(&setup.root.join(ARTIFACT_DIRECTORY), 200_000, &control()?)?;
    let changed = AtomicBool::new(false);
    let envelope = host
        .execute(
            ReadEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            || {
                if probe
                    .receipt("retained.bin")
                    .map_err(craft_cli::attachment_download::storage)?
                    .is_some()
                    && !changed.swap(true, Ordering::SeqCst)
                {
                    setup.store.set_issue_read_policy(
                        "factory",
                        IssueReadPolicyUpdate {
                            target: target(),
                            expected_actor: "actor-one".into(),
                            operations: vec![IssueReadOperation::AttachmentDownload],
                            enabled: false,
                            expected_revision: Some(captured.revision),
                        },
                        true,
                        &control()?,
                        |_| Ok(()),
                    )?;
                }
                setup.store.verify_issue_read_policy(
                    &captured,
                    IssueReadOperation::AttachmentDownload,
                    &control()?,
                )
            },
        )
        .envelope;
    assert!(!envelope.ok);
    assert!(changed.load(Ordering::SeqCst));
    assert_eq!(data(&envelope)?["artifact_write_state"], "confirmed");
    assert!(!data(&envelope)?["artifact"].is_null());
    assert_eq!(data(&envelope)?["export_write_state"], "not_sent");
    assert!(probe.receipt("retained.bin")?.is_some());
    assert!(
        !setup
            .root
            .with_extension("exports")
            .join("result.bin")
            .exists()
    );
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn download_revoked_preparation_and_renewed_cache_keep_independent_effects() -> TestResult {
    let setup = Setup::new()?;
    let captured = allow(&setup)?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        binary(b"abc"),
    ]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let host = PreparedDownload::prepare(
        args(&setup, "renewed.bin")?,
        &[],
        &options(&setup),
        &setup.store,
        &control()?,
    )?;
    clock.advance(Duration::from_secs(3600))?;
    let result = host.execute(ReadEnvironment {
        store: &setup.store,
        client: fixture.client(),
        locked: &locked,
        tls: TlsPolicy::AdditionalCaBundle(product.root()),
        control: control()?,
    });
    assert!(result.envelope.ok, "{:?}", result.envelope.error);
    assert_eq!(
        data(&result.envelope)?["credential_cache_update"]["write_state"],
        "confirmed"
    );
    let refused = PreparedDownload::prepare(
        args(&setup, "revoked.bin")?,
        &[],
        &options(&setup),
        &setup.store,
        &control()?,
    )?;
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::AttachmentDownload],
            enabled: false,
            expected_revision: Some(captured.revision),
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let envelope = refused
        .execute(ReadEnvironment {
            store: &setup.store,
            client: fixture.client(),
            locked: &locked,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        })
        .envelope;
    assert!(!envelope.ok);
    assert_eq!(data(&envelope)?["artifact_write_state"], "not_sent");
    assert!(data(&envelope)?["credential_cache_update"].is_null());
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 2);
    Ok(())
}

#[test]
fn download_export_cannot_replace_control_state_from_an_ancestor_output_root() -> TestResult {
    let setup = Setup::new()?;
    allow(&setup)?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        binary(b"hostile replacement"),
    ]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let mut input = args(&setup, "protected.bin")?;
    input.output_root = setup.parent.to_str().ok_or("path")?.into();
    input.output = format!(
        "{}/state.sqlite3",
        setup
            .root
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("name")?
    );
    input.overwrite = true;
    let envelope =
        PreparedDownload::prepare(input, &[], &options(&setup), &setup.store, &control()?)?
            .execute(ReadEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            })
            .envelope;
    assert!(!envelope.ok);
    assert_eq!(data(&envelope)?["artifact_write_state"], "confirmed");
    assert_eq!(data(&envelope)?["export_write_state"], "failed");
    assert_eq!(setup.store.get("factory", &control()?)?.grant, setup.grant);
    assert!(fs::read(setup.root.join("state.sqlite3"))?.starts_with(b"SQLite format 3"));
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}

#[test]
fn interrupted_composed_download_discards_disk_staging_without_export_or_refetch() -> TestResult {
    for mode in ["cancel", "deadline"] {
        let setup = Setup::new()?;
        let captured = allow(&setup)?;
        let product =
            TlsServer::start(Reply::Sequence(vec![identity(), identity(), Reply::Stall]))?;
        let issuer = issuer("read:jira-user read:jira-work")?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let token = CancellationToken::default();
        let clock = Arc::new(ManualClock::new(1000));
        let invocation = InvocationControl::new(clock.clone(), token.clone(), 10_000)?;
        let host = PreparedAttachmentDownload::prepare(
            CanonicalRequest {
                command: "jira.attachment.download".into(),
                target: target(),
                input: json!({"id":"101"}),
                selection: Selection {
                    fields: vec![],
                    limit: 20,
                    cursor: None,
                    max_bytes: 16384,
                },
                preconditions: None,
            },
            "interrupted-download",
            IssueReadPolicy {
                target: target(),
                grant: setup.grant.clone(),
                expected_actor: "actor-one".into(),
            },
            args(&setup, "interrupted.bin")?,
        )?;
        let changed = AtomicBool::new(false);
        let envelope = host
            .execute(
                ReadEnvironment {
                    store: &setup.store,
                    client: fixture.client(),
                    locked: &locked,
                    tls: TlsPolicy::AdditionalCaBundle(product.root()),
                    control: invocation,
                },
                || {
                    if product.request_count() >= 3 && !changed.swap(true, Ordering::SeqCst) {
                        if mode == "cancel" {
                            token.cancel();
                        } else {
                            clock
                                .advance(Duration::from_secs(11))
                                .map_err(craft_cli::attachment_download::storage)?;
                        }
                    }
                    setup.store.verify_issue_read_policy(
                        &captured,
                        IssueReadOperation::AttachmentDownload,
                        &control()?,
                    )
                },
            )
            .envelope;
        assert_eq!(
            code(&envelope),
            Some(if mode == "cancel" {
                ErrorCode::Cancelled
            } else {
                ErrorCode::DeadlineExceeded
            })
        );
        assert!(data(&envelope)?["artifact"].is_null());
        assert_eq!(data(&envelope)?["artifact_write_state"], "not_sent");
        assert_eq!(data(&envelope)?["export_write_state"], "not_sent");
        let artifacts =
            NativeArtifacts::open(&setup.root.join(ARTIFACT_DIRECTORY), 200_000, &control()?)?;
        assert!(artifacts.receipt("interrupted.bin")?.is_none());
        assert!(
            !setup
                .root
                .with_extension("exports")
                .join("result.bin")
                .exists()
        );
        assert_eq!(product.finish()?.len(), 3);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}
