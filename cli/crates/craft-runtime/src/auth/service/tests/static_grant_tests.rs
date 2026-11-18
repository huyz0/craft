use super::identity_tests::{api, grant, trust};
use super::*;
use crate::transport::CredentialScheme;

const MEMBER: &str = "5f1d2c3b4a5968778695a4b3";

fn static_tenant(provider: &str, id: &str) -> TenantRef {
    TenantRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: id.into(),
    }
}

fn probe_client(server: &TlsServer) -> Result<ServiceOAuthClient, Box<dyn std::error::Error>> {
    let mut client = client(
        server,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    trust(&mut client, &[server])?;
    api(&mut client, server)?;
    Ok(client)
}

fn session(
    tenant: &TenantRef,
    secret: &str,
    scheme: CredentialScheme,
) -> Result<HostSession, Box<dyn std::error::Error>> {
    Ok(HostSession::with_scheme(
        tenant.clone(),
        grant(),
        SecretBytes::new(secret.as_bytes().to_vec())?,
        scheme,
    )?)
}

#[test]
fn trello_probe_sends_header_credentials_and_binds_the_member_id()
-> Result<(), Box<dyn std::error::Error>> {
    let body = format!(r#"{{"id":"{MEMBER}","username":"ignored","fullName":"Ignored"}}"#);
    let server = TlsServer::start(Reply::json(200, body.as_bytes(), &[]))?;
    let client = probe_client(&server)?;
    let tenant = static_tenant("trello", MEMBER);
    let session = session(
        &tenant,
        "keysentinel:tokensentinel",
        CredentialScheme::TrelloKeyToken,
    )?;
    assert_eq!(client.probe_static(&tenant, &session)?, MEMBER);
    let requests = server.finish()?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/1/members/me");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("OAuth oauth_consumer_key=\"keysentinel\", oauth_token=\"tokensentinel\"")
    );
    Ok(())
}

#[test]
fn a_credential_for_another_member_is_refused_without_leaking_it()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(
        200,
        br#"{"id":"aaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        &[],
    ))?;
    let client = probe_client(&server)?;
    let tenant = static_tenant("trello", MEMBER);
    let session = session(
        &tenant,
        "keysentinel:tokensentinel",
        CredentialScheme::TrelloKeyToken,
    )?;
    let failure = client
        .probe_static(&tenant, &session)
        .err()
        .ok_or("Expected identity mismatch")?;
    let text = format!("{failure:?} {}", serde_json::to_string(&failure.error)?);
    assert!(!text.contains("keysentinel") && !text.contains("tokensentinel"));
    assert_eq!(failure.error.code, ErrorCode::PolicyDenied);
    server.finish()?;
    Ok(())
}

#[test]
fn bitbucket_probe_uses_basic_auth_and_takes_the_workspace_uuid_as_actor()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(
        200,
        br#"{"slug":"team-space","uuid":"{11111111-2222-3333-4444-555555555555}","name":"Team"}"#,
        &[],
    ))?;
    let client = probe_client(&server)?;
    let tenant = static_tenant("bitbucket", "team-space");
    let session = session(
        &tenant,
        "agent@example.test:apitokensentinel",
        CredentialScheme::Basic,
    )?;
    assert_eq!(
        client.probe_static(&tenant, &session)?,
        "11111111-2222-3333-4444-555555555555"
    );
    let requests = server.finish()?;
    assert_eq!(requests[0].target, "/2.0/workspaces/team-space");
    assert!(
        requests[0]
            .headers
            .get("authorization")
            .is_some_and(|value| value.starts_with("Basic "))
    );
    Ok(())
}

#[test]
fn a_rejected_credential_is_a_failed_probe_with_no_retry() -> Result<(), Box<dyn std::error::Error>>
{
    let server = TlsServer::start(Reply::json(401, br#"{"message":"invalid token"}"#, &[]))?;
    let client = probe_client(&server)?;
    let tenant = static_tenant("trello", MEMBER);
    let session = session(
        &tenant,
        "keysentinel:tokensentinel",
        CredentialScheme::TrelloKeyToken,
    )?;
    let failure = client
        .probe_static(&tenant, &session)
        .err()
        .ok_or("Expected a rejected credential")?;
    assert_eq!(failure.error.code, ErrorCode::AuthRequired);
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}
