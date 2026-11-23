use super::tests;
use super::*;
use craft_core::contracts::TenantRef;

fn secret(text: &str) -> Result<SecretBytes, Box<dyn std::error::Error>> {
    Ok(SecretBytes::new(text.as_bytes().to_vec())?)
}

fn tenant(provider: &str, id: &str) -> TenantRef {
    TenantRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: id.into(),
    }
}

#[test]
fn trello_secret_becomes_header_fields_not_url_parts() -> Result<(), Box<dyn std::error::Error>> {
    let header = trello_header(&secret("abc123:tok456")?)?;
    assert_eq!(
        header.to_str()?,
        "OAuth oauth_consumer_key=\"abc123\", oauth_token=\"tok456\""
    );
    assert!(header.is_sensitive());
    Ok(())
}

#[test]
fn static_secrets_that_could_break_a_header_field_are_refused()
-> Result<(), Box<dyn std::error::Error>> {
    for bad in [
        "nocolon",
        ":token",
        "key:",
        "k\"y:t",
        "k,y:t",
        "key:to:ken",
        "ke y:t",
    ] {
        assert!(trello_header(&secret(bad)?).is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn basic_header_matches_the_standard_encoding() -> Result<(), Box<dyn std::error::Error>> {
    // RFC 7617 example plus padding cases.
    assert_eq!(
        base64(b"Aladdin:open sesame"),
        "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    );
    assert_eq!(base64(b"a"), "YQ==");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"abc"), "YWJj");
    let header = basic_header(&secret("agent@example.test:apitoken")?)?;
    assert_eq!(
        header.to_str()?,
        "Basic YWdlbnRAZXhhbXBsZS50ZXN0OmFwaXRva2Vu"
    );
    Ok(())
}

#[test]
fn fixed_hosts_ignore_everything_but_the_provider() -> Result<(), Box<dyn std::error::Error>> {
    let trello = routing::Endpoint::for_tenant(&tenant("trello", &"a".repeat(24)))?;
    assert_eq!(trello.prefix.as_str(), "https://api.trello.com/1/");
    let bitbucket = routing::Endpoint::for_tenant(&tenant("bitbucket", "team-space"))?;
    assert_eq!(bitbucket.prefix.as_str(), "https://api.bitbucket.org/2.0/");
    for bad in ["", "Team", "-x", "a/b", "..", "a b", "a?b"] {
        assert!(
            routing::Endpoint::for_tenant(&tenant("bitbucket", bad)).is_err(),
            "{bad}"
        );
    }
    assert!(routing::Endpoint::for_tenant(&tenant("trello", "short")).is_err());
    Ok(())
}

#[test]
fn identity_probe_urls_use_the_fixed_hosts() -> Result<(), Box<dyn std::error::Error>> {
    let url = product_identity_url(&tenant("trello", &"0".repeat(24)))?;
    assert_eq!(url.as_str(), "https://api.trello.com/1/members/me");
    let url = product_identity_url(&tenant("bitbucket", "team-space"))?;
    assert_eq!(
        url.as_str(),
        "https://api.bitbucket.org/2.0/workspaces/team-space"
    );
    Ok(())
}

#[test]
fn a_scheme_cannot_be_bound_to_another_providers_tenant() -> Result<(), Box<dyn std::error::Error>>
{
    let grant = GrantRef {
        id: "static-grant".into(),
    };
    let result = HostSession::with_scheme(
        tenant("bitbucket", "team-space"),
        grant,
        secret("abc123:tok456")?,
        CredentialScheme::TrelloKeyToken,
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn a_trello_read_reaches_the_fixed_route_with_header_credentials_only()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::tls_fixture::{Reply, TlsServer};
    use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
    use craft_core::execution::OperationContext;
    use craft_core::registry::Registry;

    let member = "5f1d2c3b4a5968778695a4b3";
    let board = "5f1d2c3b4a5968778695a4b4";
    let mut registry = Registry::default();
    registry.register_provider(&craft_provider_trello::TrelloContext)?;
    let operation = registry.prepare(CanonicalRequest {
        command: "trello.board.get".into(),
        target: ResourceRef {
            provider: "trello".into(),
            deployment: "cloud".into(),
            tenant: member.into(),
            resource_type: "account".into(),
            id: member.into(),
        },
        input: serde_json::json!({"id": board}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    let grant = GrantRef {
        id: "gr_00000000000000000000000000000000".into(),
    };
    let server = TlsServer::start(Reply::json(
        200,
        format!(r#"{{"id":"{board}","name":"Sprint"}}"#).as_bytes(),
        &[],
    ))?;
    let session = HostSession::with_scheme(
        tenant("trello", member),
        grant.clone(),
        secret("keysentinel:tokensentinel")?,
        CredentialScheme::TrelloKeyToken,
    )?;
    let control = tests::control(2_000)?;
    let mut context = HttpContext::for_operation(
        &registry,
        &operation,
        &grant,
        session,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control,
    )?;
    context
        .endpoint
        .prefix
        .set_host(Some("localhost"))
        .map_err(|_| "Invalid fixture host")?;
    context
        .endpoint
        .prefix
        .set_port(Some(server.address().port()))
        .map_err(|_| "Invalid fixture port")?;
    let call = craft_core::execution::NativeCall {
        route: "trello-board-get".into(),
        path_parameters: std::collections::BTreeMap::from([("id".into(), board.into())]),
        query: std::collections::BTreeMap::from([("fields".into(), "name".into())]),
        repeated_query: std::collections::BTreeMap::new(),
        body: None,
    };
    let response = context.invoke(&call)?;
    assert_eq!(response.status, 200);
    let records = server.finish()?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "GET");
    assert_eq!(records[0].target, format!("/1/boards/{board}?fields=name"));
    assert_eq!(
        records[0].headers.get("authorization").map(String::as_str),
        Some("OAuth oauth_consumer_key=\"keysentinel\", oauth_token=\"tokensentinel\"")
    );
    // Credentials never ride in the URL.
    assert!(!records[0].target.contains("sentinel"));
    Ok(())
}
