use craft_test_support::state::{FaultPoint, Faults, VaultMode};

use super::identity_tests::{api, grant, tenant, trust};
use super::*;
use crate::secrets::IncompletePolicy;
use crate::secrets::tests::Fixture;

fn product() -> Result<TlsServer, Box<dyn std::error::Error>> {
    TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-one","active":true,"type":"known"}"#,
        &[],
    ))
}

#[test]
fn service_bundle_reopens_reuses_then_reacquires_and_logout_never_logs_in()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let original;
    {
        let locked = fixture.lock(&grant())?;
        let receipt = client.provision_grant(
            &locked,
            credentials()?,
            &[tenant("jira"), tenant("confluence")],
            Some("actor-one"),
        )?;
        assert_eq!(receipt.products.len(), 2);
        assert!(!receipt.generation.cleanup_pending);
        assert!(!format!("{receipt:?}").contains(TOKEN));
        original = receipt.generation.version;
    }
    // A different client and monotonic origin represent a newly opened invocation.
    let new_clock = Arc::new(ManualClock::new(2_000));
    let mut reopened = super::client(
        &issuer,
        new_clock.clone(),
        CancellationToken::default(),
        10_000,
    )?;
    trust(&mut reopened, &[&issuer, &product])?;
    api(&mut reopened, &product)?;
    let locked = fixture.lock(&grant())?;
    let session = reopened.open_service_session(&locked, &tenant("jira"))?;
    assert!(session.cache_update.is_none());
    assert_eq!(locked.read()?.version, original);
    assert_eq!(session.identity.actor(), "actor-one");
    assert!(!format!("{session:?}").contains(TOKEN));
    assert_eq!(issuer.request_count(), 1);
    new_clock.advance(Duration::from_millis(3_600_000))?;
    let renewed = reopened.open_service_session(&locked, &tenant("confluence"))?;
    let update = renewed.cache_update.ok_or("Expected cache activation")?;
    assert_ne!(update.version, original);
    assert_eq!(renewed.token.issued_at_unix_ms, 3_602_000);
    assert_eq!(issuer.request_count(), 2);
    reopened.open_service_session(&locked, &tenant("jira"))?;
    assert_eq!(issuer.request_count(), 2);
    let version = locked.read()?.version;
    locked.logout(&version)?;
    let failure = reopened
        .open_service_session(&locked, &tenant("jira"))
        .err()
        .ok_or("Expected explicit login refusal")?;
    assert_eq!(failure.error.code, ErrorCode::AuthRequired);
    no_leaks(&failure)?;
    assert_eq!(issuer.finish()?.len(), 2);
    assert_eq!(product.finish()?.len(), 6);
    drop(locked);
    for entry in std::fs::read_dir(&fixture.root)? {
        let path = entry?.path();
        if path.is_file() {
            let bytes = std::fs::read(path)?;
            for sentinel in [CLIENT_ID, CLIENT_SECRET, TOKEN] {
                assert!(
                    !bytes
                        .windows(sentinel.len())
                        .any(|window| window == sentinel.as_bytes())
                );
            }
        }
    }
    Ok(())
}

#[test]
fn invalid_targets_missing_corrupt_or_locked_state_make_no_exchange()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::Disconnect)?;
    let client = client(
        &issuer,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    let locked = fixture.lock(&grant())?;
    for targets in [
        vec![],
        vec![tenant("jira"), tenant("jira")],
        vec![tenant("projects")],
    ] {
        let failure = client
            .provision_grant(&locked, credentials()?, &targets, None)
            .err()
            .ok_or("Expected local refusal")?;
        no_leaks(&failure)?;
    }
    assert!(
        client
            .open_service_session(&locked, &tenant("jira"))
            .is_err()
    );
    let previous = locked.read()?;
    locked.replace(
        &previous.version,
        &SecretBytes::new(b"not-service-json".to_vec())?,
        IncompletePolicy::KeepPrevious,
    )?;
    let failure = client
        .open_service_session(&locked, &tenant("jira"))
        .err()
        .ok_or("Expected cache refusal")?;
    assert_eq!(failure.error.code, ErrorCode::KeyringUnavailable);
    fixture.vault.set_mode(VaultMode::Locked)?;
    assert!(
        client
            .open_service_session(&locked, &tenant("jira"))
            .is_err()
    );
    fixture.vault.set_mode(VaultMode::Available)?;
    assert_eq!(issuer.connections(), 0);
    Ok(())
}

#[test]
fn unconfigured_target_and_changed_actor_never_replace_the_committed_grant()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let changed = TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-two","active":true}"#,
        &[],
    ))?;
    let clock = Arc::new(ManualClock::new(1_000));
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product, &changed])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
    let before = locked.read()?;
    assert!(
        client
            .open_service_session(&locked, &tenant("confluence"))
            .is_err()
    );
    let mut other_cloud = tenant("jira");
    other_cloud.tenant = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into();
    assert!(client.open_service_session(&locked, &other_cloud).is_err());
    assert_eq!(issuer.request_count(), 1);
    assert_eq!(product.request_count(), 1);
    api(&mut client, &changed)?;
    for advance in [0, 3_600_000] {
        clock.advance(Duration::from_millis(advance))?;
        let failure = client
            .open_service_session(&locked, &tenant("jira"))
            .err()
            .ok_or("Expected actor refusal")?;
        no_leaks(&failure)?;
        let after = locked.read()?;
        assert_eq!(after.version, before.version);
        assert_eq!(
            after.secret.ok_or("Expected secret")?.expose(),
            before.secret.as_ref().ok_or("Expected secret")?.expose()
        );
    }
    assert_eq!(issuer.finish()?.len(), 2);
    assert_eq!(changed.finish()?.len(), 2);
    Ok(())
}

#[test]
fn invalid_bundle_fields_and_lifetime_fail_before_exchange_or_product_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let mut client = client(
        &issuer,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
    let original = locked.read()?.secret.ok_or("Expected bundle")?;
    let value: Value = serde_json::from_slice(original.expose())?;
    let cases = [
        ("/format", json!(2)),
        ("/grant/id", json!("gr_11111111111111111111111111111111")),
        ("/actor", json!("unknown")),
        ("/credentials/client_secret", json!("")),
        ("/token/access_token", json!("bad bearer")),
        ("/token/expires_at_unix_ms", json!(1)),
        ("/token/reacquire_at_unix_ms", json!(42)),
        ("/token/scopes", json!(["read:jira-user", "read:jira-user"])),
    ];
    let mut bytes = Vec::new();
    for (pointer, replacement) in cases {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).ok_or("Missing test pointer")? = replacement;
        bytes.push(serde_json::to_vec(&changed)?);
    }
    let mut missing = value.clone();
    missing["token"]
        .as_object_mut()
        .ok_or("Expected token object")?
        .remove("scopes");
    bytes.push(serde_json::to_vec(&missing)?);
    let encoded = String::from_utf8(original.expose().to_vec())?;
    bytes.push(
        encoded
            .replacen("\"format\":1", "\"format\":1,\"format\":1", 1)
            .into_bytes(),
    );
    for bytes in bytes {
        let before = locked.read()?;
        locked.replace(
            &before.version,
            &SecretBytes::new(bytes)?,
            IncompletePolicy::KeepPrevious,
        )?;
        let failure = client
            .open_service_session(&locked, &tenant("jira"))
            .err()
            .ok_or("Expected corrupt cache refusal")?;
        assert_eq!(failure.error.code, ErrorCode::KeyringUnavailable);
        no_leaks(&failure)?;
    }
    assert_eq!(issuer.finish()?.len(), 1);
    assert_eq!(product.finish()?.len(), 1);
    Ok(())
}

#[test]
fn interrupted_cache_replacement_requires_local_recovery_without_oauth_replay()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
    clock.advance(Duration::from_millis(3_600_000))?;
    fixture
        .faults
        .fail_on(FaultPoint::VaultWriteBeforeCommit, 2)?;
    let failure = client
        .open_service_session(&locked, &tenant("jira"))
        .err()
        .ok_or("Expected storage refusal")?;
    no_leaks(&failure)?;
    assert_eq!(failure.error.code, ErrorCode::WriteOutcomeUnknown);
    assert_eq!(
        failure.credential_write_state,
        Some(RequestState::SentUnknown)
    );
    assert!(locked.inspect()?.recovery_required);
    assert!(
        client
            .open_service_session(&locked, &tenant("jira"))
            .is_err()
    );
    assert_eq!(issuer.request_count(), 2);
    assert!(locked.recover()?.recovered);
    client.open_service_session(&locked, &tenant("jira"))?;
    assert_eq!(issuer.finish()?.len(), 3);
    assert_eq!(product.finish()?.len(), 3);
    Ok(())
}

#[test]
fn frozen_or_backward_wall_time_cannot_extend_a_cached_token()
-> Result<(), Box<dyn std::error::Error>> {
    for wall in [500, 1_000] {
        let fixture = Fixture::new()?;
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
        let product = product()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
        trust(&mut client, &[&issuer, &product])?;
        api(&mut client, &product)?;
        let locked = fixture.lock(&grant())?;
        client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
        clock.advance(Duration::from_secs(3_600))?;
        clock.set_wall(wall)?;
        let session = client.open_service_session(&locked, &tenant("jira"))?;
        assert!(session.cache_update.is_some());
        assert_eq!(session.token.issued_at_unix_ms, wall);
        assert_eq!(issuer.finish()?.len(), 2);
        assert_eq!(product.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn cleanup_pending_activation_is_returned_without_reacquiring_another_token()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
    clock.advance(Duration::from_secs(3_600))?;
    fixture
        .faults
        .fail_on(FaultPoint::VaultDeleteBeforeCommit, 1)?;
    let session = client.open_service_session(&locked, &tenant("jira"))?;
    let receipt = session.cache_update.ok_or("Expected activation receipt")?;
    assert!(receipt.cleanup_pending);
    assert_eq!(locked.read()?.version, receipt.version);
    assert!(locked.inspect()?.cleanup_pending);
    let reused = client.open_service_session(&locked, &tenant("jira"))?;
    assert!(reused.cache_update.is_none());
    assert!(!locked.recover()?.cleanup_pending);
    assert_eq!(issuer.finish()?.len(), 2);
    assert_eq!(product.finish()?.len(), 3);
    Ok(())
}

#[test]
fn sealed_cache_access_reuses_one_grant_and_retains_a_renewal_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    let original = client
        .provision_grant(
            &locked,
            credentials()?,
            &[tenant("jira")],
            Some("actor-one"),
        )?
        .generation
        .version;
    let access = client.open_service_access(&locked, &tenant("jira"))?;
    assert!(access.cache_update().is_none());
    assert_eq!(access.grant(), &locked.grant());
    assert_eq!(access.identity().actor(), "actor-one");
    clock.advance(Duration::from_secs(3_600))?;
    let renewed = client.open_service_access(&locked, &tenant("jira"))?;
    let update = renewed.cache_update().ok_or("Expected renewal receipt")?;
    assert_ne!(update.version, original);
    assert_eq!(locked.read()?.version, update.version);
    let version = locked.read()?.version;
    locked.logout(&version)?;
    assert!(
        client
            .open_service_access(&locked, &tenant("jira"))
            .is_err()
    );
    assert_eq!(issuer.finish()?.len(), 2);
    assert_eq!(product.finish()?.len(), 3);
    Ok(())
}

#[test]
fn sealed_access_expiry_after_cache_activation_preserves_the_confirmed_update()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let clock = Arc::new(ExpireAfterCleanup {
        clock: ManualClock::new(1_000),
        faults: fixture.faults.clone(),
    });
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
    clock.clock.advance(Duration::from_secs(3_600))?;
    let failure = client
        .open_service_access(&locked, &tenant("jira"))
        .err()
        .ok_or("Expected post-activation expiry")?;
    no_leaks(&failure)?;
    assert_eq!(failure.error.code, ErrorCode::AuthRequired);
    assert_eq!(
        failure.credential_write_state,
        Some(RequestState::Confirmed)
    );
    let update = failure
        .credential_update
        .ok_or("Expected confirmed update")?;
    assert_eq!(locked.read()?.version, update.version);
    assert_eq!(issuer.finish()?.len(), 2);
    assert_eq!(product.finish()?.len(), 2);
    Ok(())
}

struct ExpireAfterCleanup {
    clock: ManualClock,
    faults: Faults,
}

impl Clock for ExpireAfterCleanup {
    fn unix_millis(&self) -> u64 {
        if self
            .faults
            .calls(FaultPoint::VaultDeleteBeforeCommit)
            .is_ok_and(|count| count > 0)
        {
            20_000_000
        } else {
            self.clock.unix_millis()
        }
    }
    fn monotonic_millis(&self) -> u64 {
        self.clock.monotonic_millis()
    }
}

#[test]
fn confirmed_cache_update_survives_a_later_session_expiry_error()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let product = product()?;
    let clock = Arc::new(ExpireAfterCleanup {
        clock: ManualClock::new(1_000),
        faults: fixture.faults.clone(),
    });
    let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
    trust(&mut client, &[&issuer, &product])?;
    api(&mut client, &product)?;
    let locked = fixture.lock(&grant())?;
    client.provision_grant(&locked, credentials()?, &[tenant("jira")], None)?;
    clock.clock.advance(Duration::from_secs(3_600))?;
    let failure = client
        .open_service_session(&locked, &tenant("jira"))
        .err()
        .ok_or("Expected session expiry")?;
    no_leaks(&failure)?;
    assert_eq!(failure.error.code, ErrorCode::AuthRequired);
    assert_eq!(
        failure.credential_write_state,
        Some(RequestState::Confirmed)
    );
    let receipt = failure
        .credential_update
        .ok_or("Expected confirmed activation receipt")?;
    assert_eq!(locked.read()?.version, receipt.version);
    assert!(!receipt.cleanup_pending);
    assert_eq!(issuer.finish()?.len(), 2);
    assert_eq!(product.finish()?.len(), 2);
    Ok(())
}

#[test]
fn provisioning_preflight_runs_after_proof_and_refuses_writes_or_stale_activation()
-> Result<(), Box<dyn std::error::Error>> {
    for stale in [false, true] {
        let fixture = Fixture::new()?;
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
        let product = product()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let mut client = client(&issuer, clock.clone(), CancellationToken::default(), 10_000)?;
        trust(&mut client, &[&issuer, &product])?;
        api(&mut client, &product)?;
        let locked = fixture.lock(&grant())?;
        let failure = client
            .provision_grant_with_preflight(
                &locked,
                credentials()?,
                &[tenant("jira")],
                Some("actor-one"),
                |metadata, proofs| {
                    assert_eq!(proofs.len(), 1);
                    assert_eq!(proofs[0].actor(), "actor-one");
                    assert!(metadata.scopes.is_some());
                    if stale {
                        clock.advance(Duration::from_secs(3_600)).map_err(|_| {
                            CraftError::new(ErrorCode::Internal, "Fixture clock failed")
                        })?;
                        Ok(())
                    } else {
                        Err(CraftError::new(
                            ErrorCode::OutputBudgetTooSmall,
                            "Fixture output limit",
                        ))
                    }
                },
            )
            .err()
            .ok_or("Expected pre-activation refusal")?;
        assert_eq!(failure.credential_write_state, None);
        assert!(!locked.inspect()?.active);
        assert_eq!(fixture.faults.calls(FaultPoint::VaultWriteBeforeCommit)?, 0);
        assert_eq!(issuer.finish()?.len(), 1);
        assert_eq!(product.finish()?.len(), 1);
    }
    Ok(())
}
