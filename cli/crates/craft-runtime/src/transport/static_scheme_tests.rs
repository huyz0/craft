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

type WireOutcome<T> = (Result<T, CraftError>, Vec<crate::tls_fixture::Observed>);

/// Runs one Bitbucket read through the real transport against the TLS fixture, with the fixed
/// origin rewritten to the fixture. Returns the operation outcome and the recorded requests.
fn bitbucket_wire<T>(
    command: &str,
    input: serde_json::Value,
    reply: impl FnOnce(u16) -> Vec<u8>,
    run: impl FnOnce(
        &craft_core::execution::PreparedOperation,
        &mut HttpContext,
    ) -> Result<T, CraftError>,
) -> Result<WireOutcome<T>, Box<dyn std::error::Error>> {
    use crate::tls_fixture::{Reply, TlsServer};
    use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
    use craft_core::registry::Registry;

    let mut registry = Registry::default();
    registry.register_provider(&craft_provider_bitbucket::BitbucketContext)?;
    let operation = registry.prepare(CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "bitbucket".into(),
            deployment: "cloud".into(),
            tenant: "team".into(),
            resource_type: "workspace".into(),
            id: "team".into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    })?;
    let grant = GrantRef {
        id: "gr_00000000000000000000000000000000".into(),
    };
    let server = TlsServer::start(Reply::json(200, &reply(0), &[]))?;
    let port = server.address().port();
    let session = HostSession::with_scheme(
        tenant("bitbucket", "team"),
        grant.clone(),
        secret("agent@example.test:tokensentinel")?,
        CredentialScheme::Basic,
    )?;
    let mut context = HttpContext::for_operation(
        &registry,
        &operation,
        &grant,
        session,
        TlsPolicy::AdditionalCaBundle(server.root()),
        tests::control(2_000)?,
    )?;
    context
        .endpoint
        .prefix
        .set_host(Some("localhost"))
        .map_err(|_| "Invalid fixture host")?;
    context
        .endpoint
        .prefix
        .set_port(Some(port))
        .map_err(|_| "Invalid fixture port")?;
    let outcome = run(&operation, &mut context);
    Ok((outcome, server.finish()?))
}

#[test]
fn a_bitbucket_pipeline_read_uses_basic_credentials_and_an_encoded_uuid_path()
-> Result<(), Box<dyn std::error::Error>> {
    let uuid = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
    let (outcome, records) = bitbucket_wire(
        "bitbucket.pipeline.get",
        serde_json::json!({"repo": "service", "pipeline_uuid": uuid}),
        |_| format!(r#"{{"uuid":"{{{uuid}}}","build_number":3}}"#).into_bytes(),
        |operation, context| craft_provider_bitbucket::read_get(operation, context),
    )?;
    let item = outcome?;
    assert_eq!(item["id"], uuid);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "GET");
    assert_eq!(
        records[0].target,
        format!("/2.0/repositories/team/service/pipelines/%7B{uuid}%7D")
    );
    assert_eq!(
        records[0].headers.get("authorization").map(String::as_str),
        Some("Basic YWdlbnRAZXhhbXBsZS50ZXN0OnRva2Vuc2VudGluZWw=")
    );
    assert!(!records[0].target.contains("sentinel"));
    Ok(())
}

#[test]
fn a_bitbucket_next_link_on_the_same_route_yields_its_page_token()
-> Result<(), Box<dyn std::error::Error>> {
    // The fixture origin is only known once the server runs, so the link is built from the
    // call's own recorded origin by the closure below.
    let (outcome, records) = bitbucket_wire(
        "bitbucket.pullrequest.list",
        serde_json::json!({"repo": "service", "page_size": 2}),
        |_| {
            br#"{"values":[],"pagelen":2,"next":"?state=OPEN&sort=-updated_on&pagelen=2&page=2"}"#
                .to_vec()
        },
        |operation, context| craft_provider_bitbucket::read_page(operation, context, None),
    )?;
    let page = outcome?;
    assert_eq!(page.next.ok_or("expected a next position")?.0["page"], "2");
    assert_eq!(
        records[0].target,
        "/2.0/repositories/team/service/pullrequests?pagelen=2&sort=-updated_on&state=OPEN"
    );
    Ok(())
}

#[test]
fn a_bitbucket_next_link_cannot_change_host_route_or_filters()
-> Result<(), Box<dyn std::error::Error>> {
    for link in [
        "https://evil.example/2.0/repositories/team/service/pullrequests?page=2",
        "/2.0/repositories/team/other/pullrequests?page=2",
        "?state=MERGED&sort=-updated_on&pagelen=2&page=2",
        "?state=OPEN&sort=-updated_on&pagelen=2&page=2&extra=1",
    ] {
        let body = format!(r#"{{"values":[],"next":"{link}"}}"#).into_bytes();
        let (outcome, _) = bitbucket_wire(
            "bitbucket.pullrequest.list",
            serde_json::json!({"repo": "service", "page_size": 2}),
            |_| body,
            |operation, context| craft_provider_bitbucket::read_page(operation, context, None),
        )?;
        assert!(outcome.is_err(), "{link}");
    }
    Ok(())
}
