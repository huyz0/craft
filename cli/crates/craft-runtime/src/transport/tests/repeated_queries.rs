use super::*;

fn repeated_route() -> NativeRoute {
    let mut declared = route();
    declared.repeated_query_parameters = vec!["id".into(), "type".into(), "projectIds".into()];
    declared
        .query_parameters
        .extend(declared.repeated_query_parameters.clone());
    declared
}

fn repeated_call() -> NativeCall {
    NativeCall {
        repeated_query: BTreeMap::from([
            (
                "id".into(),
                vec![
                    "customfield_1".into(),
                    "équipe, plus+ &x=y".into(),
                    "".into(),
                    "customfield_1".into(),
                ],
            ),
            ("type".into(), vec!["custom".into(), "system".into()]),
            ("projectIds".into(), vec!["00123".into(), "456".into()]),
        ]),
        ..call()
    }
}

struct RepeatedHandler;

impl OperationHandler for RepeatedHandler {
    fn validate(&self, _: &CanonicalRequest) -> Result<(), CraftError> {
        Ok(())
    }

    fn execute(
        &self,
        _: &PreparedOperation,
        context: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Ok(context.invoke(&repeated_call())?.body)
    }
}

struct RepeatedProvider;

impl ProviderModule for RepeatedProvider {
    fn manifest(&self) -> ProviderManifest {
        Provider {
            effect: EffectClass::Read,
            changed: false,
        }
        .manifest()
    }

    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        let (base, _) = prepared(EffectClass::Read)?;
        let mut descriptor = base.descriptor("jira.fixture")?.clone();
        descriptor.routes = vec![repeated_route()];
        registry.register_typed_handler::<Empty>(descriptor, Arc::new(RepeatedHandler))
    }
}

fn repeated_context(
    server: &TlsServer,
) -> Result<(Registry, PreparedOperation, HttpContext), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&RepeatedProvider)?;
    let (_, original) = prepared(EffectClass::Read)?;
    let operation = registry.prepare(original.request().clone())?;
    let mut host = HttpContext::for_operation(
        &registry,
        &operation,
        &grant(),
        session()?,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control(5_000)?,
    )?;
    host.endpoint
        .prefix
        .set_host(Some("localhost"))
        .map_err(|_| "Invalid fixture host")?;
    host.endpoint
        .prefix
        .set_port(Some(server.address().port()))
        .map_err(|_| "Invalid fixture port")?;
    Ok((registry, operation, host))
}

#[test]
fn repeated_get_filters_survive_tls_encoding_and_omitted_or_exact_continuations()
-> Result<(), Box<dyn std::error::Error>> {
    let exact = "?cursor=third%2Fpage&%69d=customfield_1&id=%C3%A9quipe%2C+plus%2B+%26x%3Dy&id=&id=customfield_1&type=custom&type=system&projectIds=00123&projectIds=456";
    let server = TlsServer::start(Reply::Sequence(vec![
        Reply::json(200, br#"{"next":"?cursor=second%2Fpage"}"#, &[]),
        Reply::json(200, &serde_json::to_vec(&json!({"next": exact}))?, &[]),
        Reply::json(200, br#"{"done":true}"#, &[]),
    ]))?;
    let (registry, operation, mut host) = repeated_context(&server)?;
    let first = registry.execute(&operation, &mut host)?;
    let second = host.normalize_next(
        &repeated_call(),
        first["next"].as_str().ok_or("Missing next link")?,
    )?;
    let response = host.invoke(&second)?;
    let third = host.normalize_next(
        &second,
        response.body["next"].as_str().ok_or("Missing next link")?,
    )?;
    assert_eq!(host.invoke(&third)?.body["done"], true);
    assert_eq!(second.repeated_query, repeated_call().repeated_query);
    assert_eq!(third.repeated_query, repeated_call().repeated_query);
    assert_eq!(second.query["cursor"], "second/page");
    assert_eq!(third.query["cursor"], "third/page");
    let records = server.finish()?;
    assert_eq!(records.len(), 3);
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record.method, "GET");
        assert_eq!(
            record.headers.get("authorization").map(String::as_str),
            Some("Bearer synthetic-bearer-sentinel")
        );
        assert!(record.body.is_empty());
        let url = url::Url::parse(&format!("https://localhost{}", record.target))?;
        assert_eq!(
            url.path(),
            format!("/ex/jira/{CLOUD}/rest/api/3/issue/PAY-1")
        );
        let pairs: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        for (name, values) in repeated_call().repeated_query {
            assert_eq!(
                pairs
                    .iter()
                    .filter(|(key, _)| key == &name)
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>(),
                values.iter().collect::<Vec<_>>()
            );
        }
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "q")
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["project = PAY"]
        );
        assert_eq!(
            pairs.iter().filter(|(key, _)| key == "cursor").count(),
            usize::from(index != 0)
        );
        assert!(record.target.contains("id=%C3%A9quipe%2C+plus%2B+%26x%3Dy"));
        assert!(!record.target.contains("id=customfield_1%2C"));
    }
    Ok(())
}

#[test]
fn repeated_continuations_refuse_filter_drift_and_malformed_or_foreign_links_without_connections()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (_, _, host) = repeated_context(&server)?;
    for link in [
        "?cursor=x&id=customfield_1",
        "?cursor=x&id=customfield_1&id=&id=%C3%A9quipe%2C+plus%2B+%26x%3Dy&id=customfield_1",
        "?cursor=x&projectIds=123&projectIds=456",
        "?cursor=x&type=custom&type=system&type=custom",
        "?cursor=x&q=project+%3D+PAY&q=project+%3D+PAY",
        "?cursor=x&%63ursor=y",
        "?cursor=x&id=%FF",
        "?cursor=x&id=%GG",
        "?cursor=x&id=%",
        "?cursor=x&%FF=custom",
        "?cursor=x&authorization=private-query-sentinel",
        "?cursor=x#fragment",
        "https://foreign.invalid/rest/api/3/issue/PAY-1?cursor=x",
        "../PAY-2?cursor=x",
    ] {
        let error = host
            .normalize_next(&repeated_call(), link)
            .err()
            .ok_or("Expected next-link refusal")?;
        assert_eq!(error.code, ErrorCode::PolicyDenied, "{link}");
        assert!(!serde_json::to_string(&error)?.contains("private-query-sentinel"));
    }
    let mut absent = repeated_call();
    absent.repeated_query.remove("type");
    assert!(
        host.normalize_next(&absent, "?cursor=x&type=custom")
            .is_err()
    );
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn invalid_repeated_calls_and_encoded_url_overflow_fail_before_connections()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (_, _, mut host) = repeated_context(&server)?;
    let mut invalid = Vec::new();
    let mut scalar = repeated_call();
    scalar.repeated_query.remove("id");
    scalar.query.insert("id".into(), "customfield_1".into());
    invalid.push(scalar);
    let mut collision = repeated_call();
    collision.query.insert("id".into(), "customfield_1".into());
    invalid.push(collision);
    for (name, values) in [
        ("id", vec![]),
        ("id", vec!["x".into(); 101]),
        ("id", vec!["é".repeat(8_193)]),
        ("undeclared", vec!["x".into()]),
        ("cursor", vec!["x".into()]),
        // The value fits the byte bound but percent encoding exceeds the complete URL bound.
        ("id", vec!["é".repeat(8_192)]),
    ] {
        let mut call = repeated_call();
        call.repeated_query.insert(name.into(), values);
        invalid.push(call);
    }
    for call in invalid {
        let error = host.invoke(&call).err().ok_or("Expected query refusal")?;
        assert_eq!(error.details.write_state, RequestState::NotSent);
    }
    let many = format!("?{}", vec!["id=x"; 101].join("&"));
    assert!(host.normalize_next(&repeated_call(), &many).is_err());
    let too_many = format!("?{}", vec!["id=x"; 257].join("&"));
    assert!(host.normalize_next(&repeated_call(), &too_many).is_err());
    assert_eq!(server.connections(), 0);
    assert_eq!(server.request_count(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn jira_site_parser_cannot_extend_scalar_project_search_to_repeated_filters()
-> Result<(), Box<dyn std::error::Error>> {
    let site = JiraProjectSite::parse("https://craft-test.atlassian.net")?;
    let mut route = craft_provider_jira::projects::route();
    route.repeated_query_parameters.push("query".into());
    route.validate()?;
    let call = NativeCall {
        route: route.id.clone(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::from([("query".into(), vec!["PAY".into()])]),
        body: None,
    };
    assert!(site.normalize(&route, &call, "?startAt=50").is_err());
    Ok(())
}

#[test]
fn repeated_continuation_pair_and_rebuilt_url_bounds_apply_before_connections()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(200, b"{}", &[]))?;
    let (_, _, host) = repeated_context(&server)?;
    let mut original = repeated_call();
    original.repeated_query = BTreeMap::from([
        ("id".into(), vec!["x".into(); 100]),
        ("type".into(), vec!["x".into(); 100]),
        ("projectIds".into(), vec!["x".into(); 54]),
    ]);
    let mut link = url::form_urlencoded::Serializer::new(String::new());
    link.append_pair("q", &original.query["q"]);
    link.append_pair("cursor", "next");
    for (name, values) in &original.repeated_query {
        for value in values {
            link.append_pair(name, value);
        }
    }
    let exact = format!("?{}", link.finish());
    let next = host.normalize_next(&original, &exact)?;
    assert_eq!(next.repeated_query, original.repeated_query);
    assert_eq!(next.query["cursor"], "next");
    assert!(
        host.normalize_next(&original, &format!("{exact}&projectIds=x"))
            .is_err()
    );

    original.repeated_query = BTreeMap::from([("id".into(), vec!["a".repeat(16_384)])]);
    let link = format!("?cursor={}", "%26".repeat(6_000));
    let error = host
        .normalize_next(&original, &link)
        .err()
        .ok_or("Expected rebuilt URL refusal")?;
    assert_eq!(error.code, ErrorCode::InputInvalid);
    assert_eq!(error.details.issues[0].rule, "native_url_limit_exceeded");
    assert_eq!(error.details.write_state, RequestState::NotSent);
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}
