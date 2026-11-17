use super::*;
use crate::conformance::Observed;
use craft_cli::issue_create::command::{IssueCreateArgs, PreparedInvocation};
use craft_cli::issue_recovery::command::{OperationReconcileArgs, PreparedInvocation as Recovery};
use craft_cli::profiles::InvocationOptions;
use craft_runtime::issue_recovery_policy::{IssueRecoveryOperation, IssueRecoveryPolicyUpdate};

#[test]
fn saved_create_source_precedes_native_access_and_recovers_a_lost_post_without_replay() -> TestResult
{
    for blocked in [false, true] {
        let setup = Setup::new()?;
        let c = control(CancellationToken::default(), 30_000)?;
        setup.profiles.set_create_policy(
            "factory",
            CreatePolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                enabled: true,
                expected_revision: None,
            },
            true,
            &c,
            |_| Ok(()),
        )?;
        let issuer = issuer(SCOPES)?;
        let mut replies = vec![identity()];
        if !blocked {
            replies.push(identity());
            replies.extend(metadata()?);
            replies.push(Reply::Disconnect);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1_000)))?;
        let locked = fixture.lock(&setup.grant, &c)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        if blocked {
            fs::write(setup.profiles.path().join("requests"), b"blocked directory")?;
        }
        let mut value = input();
        value["operation_id"] = json!(OPERATION);
        value["apply"] = json!(true);
        value["project_id"] = json!("10000");
        let options = InvocationOptions {
            state_root: Some(&setup.files.root),
            profile: Some("factory"),
            request_id: "saved-create",
            max_bytes: 262_144,
        };
        let prepared = PreparedInvocation::prepare(
            serde_json::from_value::<IssueCreateArgs>(value)?,
            &[],
            &options,
            &setup.profiles,
            &c,
        )?;
        let result = prepared
            .execute_saved(
                &setup.profiles,
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                c.clone(),
            )
            .observed();
        assert!(!result.envelope.ok);
        let data = result.envelope.data.as_ref().ok_or("data")?;
        assert_eq!(
            data["receipt"]["request_state"],
            if blocked { "not_sent" } else { "sent_unknown" }
        );
        if blocked {
            assert!(data["saved_request"].is_null());
            assert!(setup.journal.get(OPERATION)?.is_none());
        } else {
            assert_eq!(data["saved_request"]["write_state"], "confirmed");
            let source = craft_runtime::artifacts::NativeArtifacts::open(
                &setup.profiles.path().join("requests"),
                1_048_576,
                &c,
            )?;
            let blob = craft_core::storage::ArtifactStore::read(
                &source,
                data["saved_request"]["name"].as_str().ok_or("name")?,
                1_048_576,
            )?
            .ok_or("source")?;
            let saved: Value = serde_json::from_slice(&blob.bytes)?;
            for secret in [CLIENT_ID, CLIENT_SECRET, ACCESS_TOKEN] {
                assert!(
                    !blob
                        .bytes
                        .windows(secret.len())
                        .any(|bytes| bytes == secret.as_bytes())
                );
            }
            let record = setup.journal.get(OPERATION)?.ok_or("journal")?;
            assert_eq!(saved["binding"], json!(record.binding));
            assert!(saved["original"]["input"].get("apply").is_none());
            setup.profiles.set_issue_recovery_policy(
                "factory",
                IssueRecoveryPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    enabled: true,
                    operations: vec![IssueRecoveryOperation::Create],
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
            let inspected = recovery
                .execute_cached(
                    &setup.profiles,
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    c.clone(),
                )
                .observed();
            assert!(inspected.envelope.ok, "{:?}", inspected.envelope.error);
            let inspected = inspected.envelope.data.ok_or("inspection")?;
            assert_eq!(inspected["assessment"], "unresolved");
            assert_eq!(inspected["replay_authorized"], false);
            assert_eq!(setup.journal.get(OPERATION)?, Some(record));
        }
        let requests = product.finish()?;
        assert_eq!(requests.len(), if blocked { 1 } else { 8 });
        assert_eq!(
            requests.iter().filter(|r| r.method == "POST").count(),
            usize::from(!blocked)
        );
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        setup.assert_no_plaintext()?;
    }
    Ok(())
}
