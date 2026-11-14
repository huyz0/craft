use std::collections::BTreeMap;
use std::sync::Arc;

use craft_core::contracts::{CanonicalRequest, ResourceRef, SCHEMA_VERSION, Selection};
use craft_core::control::CancellationToken;
use craft_core::execution::{OperationHandler, RouteSegment};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, RetryClass,
    response_schema, schema_for,
};
use serde_json::{Value, json};

use super::*;
use crate::control::SystemClock;

mod repeated_queries;

#[test]
fn empty_created_response_preserves_status_without_relaxing_json_entity_checks()
-> Result<(), Box<dyn std::error::Error>> {
    for status in [201, 204, 205] {
        assert_eq!(decode_entity(status, "", b"", 1024, false)?, Value::Null);
    }
    assert!(decode_entity(200, "application/json", b"", 1024, false).is_err());
    assert!(decode_entity(201, "text/plain", b"unexpected", 1024, false).is_err());
    assert!(
        decode_entity(
            201,
            "application/json",
            br#"{"duplicate":1,"duplicate":2}"#,
            1024,
            false
        )
        .is_err()
    );
    assert!(decode_entity(201, "application/json", br#"{} trailing"#, 1024, false).is_err());
    assert!(
        decode_entity(
            201,
            "application/json",
            br#"{"value":"oversized"}"#,
            8,
            false
        )
        .is_err()
    );
    assert_eq!(decode_entity(200, "", b"", 1024, true)?, Value::Null);
    Ok(())
}

#[test]
fn link_observation_distinguishes_absence_from_unusable_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let mut headers = reqwest::header::HeaderMap::new();
    assert_eq!(observed_link_headers(&headers), Some(vec![]));
    headers.append("link", HeaderValue::from_static("</next>; rel=\"next\""));
    assert_eq!(
        observed_link_headers(&headers),
        Some(vec!["</next>; rel=\"next\"".into()])
    );
    headers.append("link", HeaderValue::from_bytes(&[0xff])?);
    assert!(observed_link_headers(&headers).is_none());
    headers.clear();
    for _ in 0..17 {
        headers.append("link", HeaderValue::from_static("</next>; rel=next"));
    }
    assert!(observed_link_headers(&headers).is_none());
    headers.clear();
    headers.append("link", HeaderValue::from_str(&"x".repeat(8193))?);
    assert!(observed_link_headers(&headers).is_none());
    Ok(())
}

const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
const OTHER_CLOUD: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

#[test]
fn field_site_links_bind_exact_array_filters_without_extending_project_site_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let site = JiraFieldSite::parse("https://craft-test.atlassian.net")?;
    let route = craft_provider_jira::fields::route(craft_provider_jira::fields::SEARCH)?;
    let call = NativeCall {
        route: route.id.clone(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::from([
            ("startAt".into(), "0".into()),
            ("maxResults".into(), "50".into()),
            ("orderBy".into(), "name".into()),
        ]),
        repeated_query: BTreeMap::from([(
            "id".into(),
            vec!["customfield_1".into(), "customfield_2".into()],
        )]),
        body: None,
    };
    let next = site.normalize(&route,&call,"https://craft-test.atlassian.net/rest/api/3/field/search?startAt=50&id=customfield_1&id=customfield_2")?;
    assert_eq!(next.repeated_query, call.repeated_query);
    assert_eq!(
        Endpoint::atlassian(&tenant())?
            .route_url(&route, &next)?
            .host_str(),
        Some("api.atlassian.com")
    );
    for link in [
        "https://other.atlassian.net/rest/api/3/field/search?startAt=50",
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=50",
        "?startAt=50&id=customfield_1",
        "?startAt=50&id=customfield_2&id=customfield_1",
        "?startAt=50&type=custom",
        "?startAt=50&maxResults=100",
        "?startAt=50&%73tartAt=100",
        "?startAt=%FF",
        "?startAt=50#fragment",
        "https://user:secret@craft-test.atlassian.net/rest/api/3/field/search?startAt=50",
    ] {
        assert!(site.normalize(&route, &call, link).is_err(), "{link}");
    }
    assert!(
        JiraProjectSite::parse(site.as_str())?
            .normalize(&route, &call, "?startAt=50")
            .is_err()
    );
    let mut changed = route.clone();
    changed.repeated_query_parameters.clear();
    assert!(site.normalize(&changed, &call, "?startAt=50").is_err());
    let mut changed = route;
    changed.query_parameters.push("extra".into());
    assert!(site.normalize(&changed, &call, "?startAt=50").is_err());
    Ok(())
}

#[cfg(feature = "synthetic-acceptance")]
#[test]
fn ordinary_host_sessions_cannot_be_marked_for_synthetic_routing()
-> Result<(), Box<dyn std::error::Error>> {
    let ordinary = session()?;
    assert_eq!(
        ordinary
            .with_synthetic(Some(crate::synthetic_acceptance::Loopback::new(1, 2)?))
            .err()
            .ok_or("Expected foreign bearer refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert!(crate::synthetic_acceptance::Loopback::new(0, 2).is_err());
    assert!(crate::synthetic_acceptance::Loopback::new(1, 0).is_err());
    Ok(())
}

fn tenant() -> TenantRef {
    TenantRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
    }
}
fn grant() -> GrantRef {
    GrantRef {
        id: "gr_00000000000000000000000000000000".into(),
    }
}
pub(super) fn session() -> Result<HostSession, Box<dyn std::error::Error>> {
    Ok(HostSession::new(
        tenant(),
        grant(),
        SecretBytes::new(b"synthetic-bearer-sentinel".to_vec())?,
    )?)
}
pub(super) fn control(timeout: u64) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        timeout,
    )
}

fn route() -> NativeRoute {
    NativeRoute {
        repeated_query_parameters: Vec::new(),
        id: "search".into(),
        method: HttpMethod::Get,
        path: vec![
            RouteSegment::Literal("rest".into()),
            RouteSegment::Literal("api".into()),
            RouteSegment::Literal("3".into()),
            RouteSegment::Literal("issue".into()),
            RouteSegment::Parameter("id".into()),
        ],
        query_parameters: vec!["q".into(), "cursor".into()],
        continuation_parameters: vec!["cursor".into()],
        response_max_bytes: 512,
    }
}

pub(super) fn call() -> NativeCall {
    NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: "search".into(),
        path_parameters: BTreeMap::from([("id".into(), "PAY-1".into())]),
        query: BTreeMap::from([("q".into(), "project = PAY".into())]),
        body: None,
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, _: &CanonicalRequest) -> Result<(), CraftError> {
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Ok(context.invoke(&call())?.body)
    }
}

struct Provider {
    effect: EffectClass,
    changed: bool,
}
impl ProviderModule for Provider {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "jira".into(),
            version: "0.1.0".into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        registry.register_typed_handler::<Empty>(
            OperationDescriptor {
                id: "jira.fixture".into(),
                provider: "jira".into(),
                contract_version: SCHEMA_VERSION.into(),
                api_revision: "fixture".into(),
                description: if self.changed {
                    "Changed fixture."
                } else {
                    "Bound HTTPS fixture."
                }
                .into(),
                effect: self.effect,
                input_schema: schema_for::<Empty>()?,
                response_schema: response_schema::<Value>()?,
                example: json!({}),
                response_example: serde_json::to_value(craft_core::contracts::Envelope::success(
                    "jira.fixture",
                    "example".into(),
                    json!({}),
                ))
                .map_err(|_| internal())?,
                projection: None,
                scope_alternatives: vec![vec!["read:jira".into()]],
                permissions: vec!["browse".into()],
                tenant_conditions: vec!["fixture".into()],
                routes: vec![route()],
                retry_class: if matches!(self.effect, EffectClass::Read) {
                    RetryClass::SafeRead
                } else {
                    RetryClass::ReconcileBeforeReplay
                },
                reconciliation: if matches!(self.effect, EffectClass::Read) {
                    ReconciliationClass::NotApplicable
                } else {
                    ReconciliationClass::KnownResource
                },
            },
            Arc::new(Handler),
        )
    }
}

pub(super) fn prepared(effect: EffectClass) -> Result<(Registry, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&Provider {
        effect,
        changed: false,
    })?;
    let operation = registry.prepare(CanonicalRequest {
        command: "jira.fixture".into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: CLOUD.into(),
            resource_type: "issue".into(),
            id: "PAY-1".into(),
        },
        input: json!({}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((registry, operation))
}

#[test]
fn tenant_templates_and_encoded_components_cannot_replace_host_or_path()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::atlassian(&tenant())?;
    assert_eq!(
        endpoint.prefix.as_str(),
        format!("https://api.atlassian.com/ex/jira/{CLOUD}/")
    );
    let mut native = call();
    native
        .path_parameters
        .insert("id".into(), "équipe space".into());
    native
        .query
        .insert("q".into(), "a&host=evil#fragment".into());
    let url = endpoint.route_url(&route(), &native)?;
    assert_eq!(url.host_str(), Some("api.atlassian.com"));
    assert!(url.path().ends_with("/%C3%A9quipe%20space"));
    assert_eq!(
        url.query_pairs()
            .collect::<BTreeMap<_, _>>()
            .get("q")
            .map(|value| value.as_ref()),
        Some("a&host=evil#fragment")
    );
    for id in [
        ".",
        "..",
        "a/b",
        "a\\b",
        "a%2fb",
        "a?host=other",
        "a#fragment",
    ] {
        native.path_parameters.insert("id".into(), id.into());
        assert!(endpoint.route_url(&route(), &native).is_err(), "{id}");
    }
    let mut wrong = tenant();
    wrong.tenant = "../other".into();
    assert!(Endpoint::atlassian(&wrong).is_err());
    wrong = tenant();
    wrong.provider = "confluence".into();
    assert!(
        Endpoint::atlassian(&wrong)?
            .prefix
            .path()
            .starts_with("/ex/confluence/")
    );
    Ok(())
}

#[test]
fn continuations_preserve_filters_and_refuse_foreign_tenants_paths_and_duplicate_keys()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::atlassian(&tenant())?;
    let native = call();
    let next = endpoint.normalize_next(&route(), &native, "?cursor=opaque%2Fnext")?;
    assert_eq!(next.query["q"], "project = PAY");
    assert_eq!(next.query["cursor"], "opaque/next");
    for link in [
        "https://other.invalid/ex/jira/next".to_string(),
        format!("https://api.atlassian.com/ex/jira/{OTHER_CLOUD}/rest/api/3/issue/PAY-1?cursor=x"),
        format!("https://api.atlassian.com/ex/confluence/{CLOUD}/rest/api/3/issue/PAY-1?cursor=x"),
        "?cursor=x&cursor=y".into(),
        "?cursor=%FF".into(),
        "?cursor=%GG".into(),
        "?cursor=x&%63ursor=y".into(),
        "?access_token=private-sentinel".into(),
        "?q=project%3DOTHER".into(),
        "?cursor=x#fragment".into(),
        format!(
            "https://user:password@api.atlassian.com/ex/jira/{CLOUD}/rest/api/3/issue/PAY-1?cursor=x"
        ),
        "http://api.atlassian.com/".into(),
        "../OTHER?cursor=x".into(),
    ] {
        let error = endpoint
            .normalize_next(&route(), &native, &link)
            .err()
            .ok_or("Expected continuation refusal")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    let mut invalid = route();
    invalid.continuation_parameters.push("undeclared".into());
    assert!(invalid.validate().is_err());
    invalid = route();
    invalid.query_parameters.push("Authorization".into());
    assert!(invalid.validate().is_err());
    Ok(())
}

#[test]
fn jira_site_project_continuation_is_an_exact_constraint_without_changing_gateway_routing()
-> Result<(), Box<dyn std::error::Error>> {
    let site = JiraProjectSite::parse("https://craft-test.atlassian.net")?;
    let route = craft_provider_jira::projects::route();
    let call = NativeCall {
        repeated_query: std::collections::BTreeMap::new(),
        route: route.id.clone(),
        path_parameters: std::collections::BTreeMap::new(),
        query: std::collections::BTreeMap::from([
            ("startAt".into(), "0".into()),
            ("maxResults".into(), "50".into()),
            ("orderBy".into(), "key".into()),
            ("action".into(), "view".into()),
        ]),
        body: None,
    };
    let next = site.normalize(
        &route,
        &call,
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=50",
    )?;
    assert_eq!(next.query["startAt"], "50");
    assert_eq!(
        Endpoint::atlassian(&tenant())?
            .route_url(&route, &next)?
            .host_str(),
        Some("api.atlassian.com")
    );
    for link in [
        "https://other.atlassian.net/rest/api/3/project/search?startAt=50",
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=50&startAt=100",
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=%FF",
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=50&maxResults=100",
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=50&orderBy=name",
        "https://craft-test.atlassian.net/rest/api/3/project/search?startAt=50#fragment",
        "https://user:secret@craft-test.atlassian.net/rest/api/3/project/search?startAt=50",
        "https://craft-test.atlassian.net/rest/api/3/issue/search?startAt=50",
        "http://craft-test.atlassian.net/rest/api/3/project/search?startAt=50",
    ] {
        assert!(site.normalize(&route, &call, link).is_err(), "{link}");
    }
    for origin in [
        "https://craft-test.atlassian.net:443",
        "https://user@craft-test.atlassian.net",
        "https://craft-test.atlassian.net/path/..",
        "https://craft-test.atlassian.net/?x=y",
        "https://craft-test.atlassian.net#x",
        " https://craft-test.atlassian.net",
        "https://craft-test.evil.atlassian.net",
    ] {
        assert!(JiraProjectSite::parse(origin).is_err(), "{origin}");
    }
    assert!(
        site.normalize(&super::tests::route(), &call, "?cursor=x")
            .is_err()
    );
    Ok(())
}

#[test]
fn malformed_sessions_and_tls_policy_fail_without_accepting_a_bypass()
-> Result<(), Box<dyn std::error::Error>> {
    for token in [
        b"bad\r\nInjected: x".to_vec(),
        b"=prefix".to_vec(),
        b"a=b".to_vec(),
        vec![b'a'; 16_385],
        vec![0xff],
    ] {
        assert!(HostSession::new(tenant(), grant(), SecretBytes::new(token)?).is_err());
    }
    assert!(!format!("{:?}", session()?).contains("synthetic-bearer-sentinel"));
    assert!(HostSession::new(tenant(), grant(), SecretBytes::new(b"abc==".to_vec())?).is_ok());
    assert!(client_builder(TlsPolicy::AdditionalCaBundle(Vec::new())).is_err());
    assert!(client_builder(TlsPolicy::AdditionalCaBundle(b"not a certificate".to_vec())).is_err());
    Ok(())
}

#[test]
fn construction_refuses_wrong_session_grant_descriptor_and_unjournaled_mutations()
-> Result<(), Box<dyn std::error::Error>> {
    let (registry, operation) = prepared(EffectClass::Read)?;
    let wrong_tenant = TenantRef {
        tenant: OTHER_CLOUD.into(),
        ..tenant()
    };
    let wrong = HostSession::new(
        wrong_tenant,
        grant(),
        SecretBytes::new(b"synthetic-bearer-sentinel".to_vec())?,
    )?;
    assert_eq!(
        HttpContext::for_operation(
            &registry,
            &operation,
            &grant(),
            wrong,
            TlsPolicy::Platform,
            control(500)?
        )
        .err()
        .ok_or("Expected target refusal")?
        .code,
        ErrorCode::PolicyDenied
    );
    assert!(
        HttpContext::for_operation(
            &registry,
            &operation,
            &GrantRef {
                id: "other-grant".into()
            },
            session()?,
            TlsPolicy::Platform,
            control(500)?
        )
        .is_err()
    );
    let mut changed = Registry::default();
    changed.register_provider(&Provider {
        effect: EffectClass::Read,
        changed: true,
    })?;
    assert_eq!(
        HttpContext::for_operation(
            &changed,
            &operation,
            &grant(),
            session()?,
            TlsPolicy::Platform,
            control(500)?
        )
        .err()
        .ok_or("Expected descriptor refusal")?
        .code,
        ErrorCode::VersionConflict
    );
    let (registry, operation) = prepared(EffectClass::Write)?;
    assert_eq!(
        HttpContext::for_operation(
            &registry,
            &operation,
            &grant(),
            session()?,
            TlsPolicy::Platform,
            control(500)?
        )
        .err()
        .ok_or("Expected journal prerequisite")?
        .code,
        ErrorCode::UnsupportedCapability
    );
    Ok(())
}

use crate::tls_fixture::{Reply, TlsServer};

pub(super) fn context(
    server: &TlsServer,
    tls: TlsPolicy,
    control: InvocationControl,
) -> Result<HttpContext, Box<dyn std::error::Error>> {
    let (registry, operation) = prepared(EffectClass::Read)?;
    let mut context =
        HttpContext::for_operation(&registry, &operation, &grant(), session()?, tls, control)?;
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
    Ok(context)
}

#[test]
fn https_wire_uses_bound_bearer_and_preserves_bounded_json_and_rate_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(
        200,
        br#"[{"title":"international","ok":true}]"#,
        &[("atl-traceid", "trace-123"), ("Retry-After", "7")],
    ))?;
    let mut host = context(
        &server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control(2_000)?,
    )?;
    let response = host.invoke(&call())?;
    assert_eq!(response.status, 200);
    assert_eq!(response.body[0]["ok"], true);
    assert_eq!(response.request_id.as_deref(), Some("trace-123"));
    assert_eq!(response.retry_after, Some(RetryAfter::DelaySeconds(7)));
    let records = server.finish()?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "GET");
    assert!(
        records[0]
            .target
            .starts_with(&format!("/ex/jira/{CLOUD}/rest/api/3/issue/PAY-1?"))
    );
    assert_eq!(
        records[0].headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-bearer-sentinel")
    );
    assert_eq!(
        records[0].headers.get("accept").map(String::as_str),
        Some("application/json")
    );
    assert!(!records[0].headers.contains_key("referer"));
    assert!(!records[0].headers.contains_key("cookie"));
    assert_eq!(
        records[0]
            .headers
            .get("accept-encoding")
            .map(String::as_str),
        Some("identity")
    );
    assert!(records[0].body.is_empty());
    let date = HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT");
    assert_eq!(
        retry_after(Some(&date)),
        Some(RetryAfter::AtUnixMillis(1_445_412_480_000))
    );
    assert!(retry_after(Some(&HeaderValue::from_static("untrusted instructions"))).is_none());
    Ok(())
}

#[test]
fn preflight_and_operation_binding_fail_before_any_connection()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let mut host = context(
        &server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control(2_000)?,
    )?;
    let (registry, original) = prepared(EffectClass::Read)?;
    let mut changed =
        serde_json::from_value::<CanonicalRequest>(serde_json::to_value(original.request())?)?;
    changed.target.id = "PAY-2".into();
    let changed = registry.prepare(changed)?;
    assert_eq!(
        registry
            .execute(&changed, &mut host)
            .err()
            .ok_or("Expected binding refusal")?
            .code,
        ErrorCode::PolicyDenied
    );
    let mut invalid = call();
    invalid
        .query
        .insert("authorization".into(), "private-query-sentinel".into());
    assert!(host.invoke(&invalid).is_err());
    let token = CancellationToken::default();
    host.control = InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 2_000)?;
    token.cancel();
    let error = host.invoke(&call()).err().ok_or("Expected cancellation")?;
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(error.details.write_state, RequestState::NotSent);
    assert_eq!(server.connections(), 0);
    assert_eq!(server.request_count(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn invalid_certificate_or_hostname_never_receives_http_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    for wrong_hostname in [false, true] {
        let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
        let tls = if wrong_hostname {
            TlsPolicy::AdditionalCaBundle(server.root())
        } else {
            TlsPolicy::Platform
        };
        let mut host = context(&server, tls, control(2_000)?)?;
        if wrong_hostname {
            host.endpoint
                .prefix
                .set_host(Some("127.0.0.1"))
                .map_err(|_| "Invalid fixture host")?;
        }
        let error = host.invoke(&call()).err().ok_or("Expected TLS refusal")?;
        assert_eq!(error.code, ErrorCode::RemoteUnavailable);
        assert_eq!(server.connections(), 1);
        assert!(!serde_json::to_string(&error)?.contains("synthetic-bearer-sentinel"));
        assert!(server.finish()?.is_empty());
    }
    Ok(())
}

#[test]
fn redirects_are_not_followed_or_given_credentials() -> Result<(), Box<dyn std::error::Error>> {
    let sink = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let location = format!("https://localhost:{}/redirected", sink.address().port());
    let source = TlsServer::start(Reply::json(302, b"{}", &[("Location", &location)]))?;
    let mut host = context(
        &source,
        TlsPolicy::AdditionalCaBundle(source.root()),
        control(2_000)?,
    )?;
    let error = host
        .invoke(&call())
        .err()
        .ok_or("Expected redirect refusal")?;
    assert_eq!(error.code, ErrorCode::PolicyDenied);
    assert_eq!(error.details.write_state, RequestState::Failed);
    assert_eq!(source.finish()?.len(), 1);
    assert_eq!(sink.connections(), 0);
    assert!(sink.finish()?.is_empty());
    Ok(())
}

#[test]
fn malformed_oversized_and_chunked_bodies_are_rejected_without_remote_text()
-> Result<(), Box<dyn std::error::Error>> {
    let large = format!("\"{}\"", "x".repeat(600));
    let chunked = format!(
        "HTTP/1.1 200 Fixture\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{}\r\n0\r\n\r\n",
        large.len(),
        large
    );
    for reply in [
        Reply::json(200, br#"{"private-sentinel":1,"private-sentinel":2}"#, &[]),
        Reply::json(200, b"{invalid-private-sentinel", &[]),
        Reply::json(200, large.as_bytes(), &[]),
        Reply::Raw(chunked.into_bytes()),
        Reply::Raw(b"HTTP/1.1 200 Fixture\r\nContent-Type: text/html\r\nContent-Length: 16\r\nConnection: close\r\n\r\nprivate-sentinel".to_vec()),
        Reply::Disconnect,
    ] {
        let server = TlsServer::start(reply)?;
        let mut host = context(&server, TlsPolicy::AdditionalCaBundle(server.root()), control(2_000)?)?;
        let error = host.invoke(&call()).err().ok_or("Expected response refusal")?;
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
        assert_eq!(error.details.write_state, RequestState::Failed);
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn failed_statuses_return_only_status_advice_and_request_identity_without_retry()
-> Result<(), Box<dyn std::error::Error>> {
    for status in [401, 403, 429, 503] {
        let server = TlsServer::start(Reply::json(
            status,
            b"private-sentinel body must not escape",
            &[("Retry-After", "9"), ("x-request-id", "request-123")],
        ))?;
        let mut host = context(
            &server,
            TlsPolicy::AdditionalCaBundle(server.root()),
            control(2_000)?,
        )?;
        let response = host.invoke(&call())?;
        assert_eq!(response.status, status);
        assert!(response.body.is_null());
        assert_eq!(response.request_id.as_deref(), Some("request-123"));
        assert_eq!(response.retry_after, Some(RetryAfter::DelaySeconds(9)));
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn stalled_requests_observe_deadlines_and_cancellation_without_replay()
-> Result<(), Box<dyn std::error::Error>> {
    for cancel in [false, true] {
        let server = TlsServer::start(Reply::Stall)?;
        let token = CancellationToken::default();
        let invocation = InvocationControl::new(
            Arc::new(SystemClock::default()),
            token.clone(),
            if cancel { 5_000 } else { 1_000 },
        )?;
        let mut host = context(
            &server,
            TlsPolicy::AdditionalCaBundle(server.root()),
            invocation,
        )?;
        let trigger = if cancel {
            let received = server.request_counter();
            Some(std::thread::spawn(move || -> Result<(), &'static str> {
                let started = std::time::Instant::now();
                while received.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    if started.elapsed() >= Duration::from_secs(2) {
                        token.cancel();
                        return Err("No HTTP request observed before cancellation trigger");
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                token.cancel();
                Ok(())
            }))
        } else {
            None
        };
        let start = std::time::Instant::now();
        let error = host
            .invoke(&call())
            .err()
            .ok_or("Expected stalled-request termination")?;
        if let Some(trigger) = trigger {
            trigger
                .join()
                .map_err(|_| "Cancellation thread panicked")??;
        }
        assert_eq!(
            error.code,
            if cancel {
                ErrorCode::Cancelled
            } else {
                ErrorCode::DeadlineExceeded
            }
        );
        assert_eq!(error.details.write_state, RequestState::Failed);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn fixture_mutation_disconnect_preserves_unknown_effect_without_a_production_write_path()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::Disconnect)?;
    let mut host = context(
        &server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control(2_000)?,
    )?;
    host.effect = EffectClass::Write;
    host.routes[0].method = HttpMethod::Post;
    let mut call = call();
    call.body = Some(json!({"summary":"synthetic"}));
    let error = host
        .invoke(&call)
        .err()
        .ok_or("Expected uncertain response")?;
    assert_eq!(error.code, ErrorCode::WriteOutcomeUnknown);
    assert_eq!(error.details.write_state, RequestState::SentUnknown);
    let records = server.finish()?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "POST");
    assert_eq!(
        serde_json::from_slice::<Value>(&records[0].body)?,
        json!({"summary":"synthetic"})
    );
    Ok(())
}

#[test]
fn proxy_environment_does_not_redirect_requests() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CRAFT_TLS_PROXY_CHILD").is_some() {
        let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
        let mut host = context(
            &server,
            TlsPolicy::AdditionalCaBundle(server.root()),
            control(2_000)?,
        )?;
        assert_eq!(host.invoke(&call())?.status, 200);
        assert_eq!(server.finish()?.len(), 1);
        return Ok(());
    }
    let sink = TlsServer::start(Reply::Disconnect)?;
    let proxy = format!("http://127.0.0.1:{}", sink.address().port());
    let output = craft_test_support::process::run(
        &std::env::current_exe()?,
        &[
            "--exact",
            "transport::tests::proxy_environment_does_not_redirect_requests",
        ],
        None,
        craft_test_support::process::RunOptions {
            environment: vec![
                ("CRAFT_TLS_PROXY_CHILD".into(), Some("1".into())),
                ("HTTPS_PROXY".into(), Some(proxy.clone().into())),
                ("HTTP_PROXY".into(), Some(proxy.clone().into())),
                ("ALL_PROXY".into(), Some(proxy.clone().into())),
                ("https_proxy".into(), Some(proxy.clone().into())),
                ("http_proxy".into(), Some(proxy.clone().into())),
                ("all_proxy".into(), Some(proxy.into())),
                ("NO_PROXY".into(), Some("".into())),
                ("no_proxy".into(), Some("".into())),
            ],
            ..Default::default()
        },
    )?;
    assert!(
        output.status.success(),
        "Proxy-isolation child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(sink.connections(), 0);
    assert!(sink.finish()?.is_empty());
    Ok(())
}
