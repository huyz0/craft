use super::*;

const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
pub(super) fn tenant(provider: &str) -> TenantRef {
    TenantRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
    }
}
pub(super) fn grant() -> GrantRef {
    GrantRef {
        id: "gr_00000000000000000000000000000000".into(),
    }
}
pub(super) fn trust(
    client: &mut ServiceOAuthClient,
    servers: &[&TlsServer],
) -> Result<(), Box<dyn std::error::Error>> {
    let roots = servers.iter().flat_map(|server| server.root()).collect();
    client.client = client_builder(TlsPolicy::AdditionalCaBundle(roots))?.build()?;
    Ok(())
}
pub(super) fn api(
    client: &mut ServiceOAuthClient,
    server: &TlsServer,
) -> Result<(), Box<dyn std::error::Error>> {
    client.api_origin = Some(reqwest::Url::parse(&format!(
        "https://localhost:{}",
        server.address().port()
    ))?);
    Ok(())
}

#[test]
fn each_product_requires_exact_scope_identity_and_cloud_proof_before_session_creation()
-> Result<(), Box<dyn std::error::Error>> {
    for scope in [
        "read:jira-user read:confluence-user",
        "read:application-role:jira read:group:jira read:user:jira read:avatar:jira read:content-details:confluence",
    ] {
        let mut response = valid();
        response["scope"] = json!(scope);
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&response)?, &[]))?;
        let product = TlsServer::start(Reply::json(200, br#"{"accountId":"actor-one","active":true,"type":"known","email":"ignored@example.com"}"#, &[]))?;
        let mut client = client(
            &issuer,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        trust(&mut client, &[&issuer, &product])?;
        api(&mut client, &product)?;
        let unverified = client.acquire(&credentials()?)?;
        assert!(
            unverified
                .into_host_session(tenant("jira"), grant())
                .is_err()
        );
        let mut token = client.acquire(&credentials()?)?;
        for provider in ["jira", "confluence"] {
            let proof = client.verify_product(&mut token, &tenant(provider), Some("actor-one"))?;
            assert_eq!(proof.actor(), "actor-one");
            assert_eq!(proof.tenant(), &tenant(provider));
        }
        assert_eq!(token.verified_products().len(), 2);
        token.into_host_session(tenant("jira"), grant())?;
        let requests = product.finish()?;
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].target,
            format!("/ex/jira/{CLOUD}/rest/api/3/myself")
        );
        assert_eq!(
            requests[1].target,
            format!("/ex/confluence/{CLOUD}/wiki/rest/api/user/current")
        );
        assert!(requests.iter().all(|request| {
            request.method == "GET"
                && request.body.is_empty()
                && request
                    .headers
                    .get("authorization")
                    .is_some_and(|value| value == &format!("Bearer {TOKEN}"))
        }));
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}

#[test]
fn missing_or_unknown_scopes_and_invalid_tenants_make_no_product_requests()
-> Result<(), Box<dyn std::error::Error>> {
    for scopes in [None, Some("read:user:jira")] {
        let mut response = valid();
        if let Some(scopes) = scopes {
            response["scope"] = json!(scopes);
        } else {
            response
                .as_object_mut()
                .ok_or("Expected object")?
                .remove("scope");
        }
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&response)?, &[]))?;
        let product = TlsServer::start(Reply::Disconnect)?;
        let mut client = client(
            &issuer,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        trust(&mut client, &[&issuer, &product])?;
        api(&mut client, &product)?;
        let mut token = client.acquire(&credentials()?)?;
        let failure = client
            .verify_product(&mut token, &tenant("jira"), None)
            .err()
            .ok_or("Expected scope refusal")?;
        assert_eq!(failure.error.code, ErrorCode::PermissionDenied);
        no_leaks(&failure)?;
        let mut bad = tenant("jira");
        bad.tenant = "guessed-site".into();
        assert!(client.verify_product(&mut token, &bad, None).is_err());
        assert!(
            client
                .verify_product(&mut token, &tenant("projects"), None)
                .is_err()
        );
        assert_eq!(product.connections(), 0);
        assert!(token.verified_products().is_empty());
    }
    Ok(())
}

#[test]
fn inactive_anonymous_missing_or_changed_actors_are_refused_without_metadata_leaks()
-> Result<(), Box<dyn std::error::Error>> {
    for body in [
        br#"{"accountId":"actor-two","active":true}"#.to_vec(),
        br#"{"accountId":"actor-one","active":false}"#.to_vec(),
        br#"{"accountId":"unknown","active":true}"#.to_vec(),
        br#"{"accountId":"actor-one","active":true,"type":"anonymous"}"#.to_vec(),
        br#"{"accountId":"actor-one"}"#.to_vec(),
        br#"{"active":true}"#.to_vec(),
        br#"{"accountId":"actor-one","accountId":"actor-one","active":true}"#.to_vec(),
        serde_json::to_vec(&json!({"accountId":TOKEN,"active":true}))?,
    ] {
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
        let product = TlsServer::start(Reply::json(200, &body, &[]))?;
        let mut client = client(
            &issuer,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        trust(&mut client, &[&issuer, &product])?;
        api(&mut client, &product)?;
        let mut token = client.acquire(&credentials()?)?;
        let failure = client
            .verify_product(&mut token, &tenant("jira"), Some("actor-one"))
            .err()
            .ok_or("Expected actor refusal")?;
        no_leaks(&failure)?;
        assert_ne!(failure.error.details.write_state, RequestState::SentUnknown);
        assert!(token.verified_products().is_empty());
        assert_eq!(product.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn verifying_another_product_cannot_silently_switch_an_existing_actor()
-> Result<(), Box<dyn std::error::Error>> {
    let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let first = TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-one","active":true}"#,
        &[],
    ))?;
    let changed = TlsServer::start(Reply::json(
        200,
        br#"{"accountId":"actor-two","type":"known"}"#,
        &[],
    ))?;
    let mut client = client(
        &issuer,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    trust(&mut client, &[&issuer, &first, &changed])?;
    let mut token = client.acquire(&credentials()?)?;
    api(&mut client, &first)?;
    client.verify_product(&mut token, &tenant("jira"), None)?;
    api(&mut client, &changed)?;
    assert!(
        client
            .verify_product(&mut token, &tenant("confluence"), None)
            .is_err()
    );
    assert_eq!(token.verified_products().len(), 1);
    assert_eq!(token.verified_products()[0].actor(), "actor-one");
    token.into_host_session(tenant("jira"), grant())?;
    assert_eq!(first.finish()?.len(), 1);
    assert_eq!(changed.finish()?.len(), 1);
    Ok(())
}

#[test]
fn failed_revalidation_removes_access_proof_but_preserves_the_actor_binding()
-> Result<(), Box<dyn std::error::Error>> {
    for reply in [
        Reply::json(200, br#"{"active":true}"#, &[]),
        Reply::json(429, b"error-body-sentinel", &[("Retry-After", "7")]),
    ] {
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
        let first = TlsServer::start(Reply::json(
            200,
            br#"{"accountId":"actor-one","active":true}"#,
            &[],
        ))?;
        let failed = TlsServer::start(reply)?;
        let changed = TlsServer::start(Reply::json(
            200,
            br#"{"accountId":"actor-two","active":true}"#,
            &[],
        ))?;
        let mut client = client(
            &issuer,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        trust(&mut client, &[&issuer, &first, &failed, &changed])?;
        let mut token = client.acquire(&credentials()?)?;
        api(&mut client, &first)?;
        client.verify_product(&mut token, &tenant("jira"), None)?;
        api(&mut client, &failed)?;
        let failure = client
            .verify_product(&mut token, &tenant("jira"), None)
            .err()
            .ok_or("Expected read refusal")?;
        no_leaks(&failure)?;
        assert_eq!(failure.error.details.write_state, RequestState::Failed);
        assert!(!failure.error.next_action.contains("issuance"));
        assert!(token.verified_products().is_empty());
        api(&mut client, &changed)?;
        assert!(
            client
                .verify_product(&mut token, &tenant("jira"), None)
                .is_err()
        );
        assert!(token.verified_products().is_empty());
        api(&mut client, &first)?;
        client.verify_product(&mut token, &tenant("jira"), None)?;
        token.into_host_session(tenant("jira"), grant())?;
        assert_eq!(first.finish()?.len(), 2);
        assert_eq!(failed.finish()?.len(), 1);
        assert_eq!(changed.finish()?.len(), 1);
    }
    Ok(())
}
