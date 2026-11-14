use super::*;
use craft_core::contracts::Selection;
use craft_core::execution::NativeResponse;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn metadata() -> Value {
    json!({"id":101,"filename":"設計.txt","size":23,"mimeType":"text/plain","created":"2026-11-11T00:00:00.000+0000","author":{"accountId":"actor-one","displayName":"private-sentinel","emailAddress":"private-sentinel"},"self":"https://invalid.example/private-sentinel","content":"https://invalid.example/private-sentinel","thumbnail":"https://invalid.example/private-sentinel","properties":{"secret":"private-sentinel"}})
}
#[test]
fn metadata_preserves_scalar_identity_without_urls_or_author_presentation() -> TestResult {
    let record = parse_record(&metadata())?;
    assert_eq!(record.id, "101");
    assert_eq!(record.filename.as_deref(), Some("設計.txt"));
    assert_eq!(record.size_bytes, Some(23));
    assert!(!serde_json::to_string(&record)?.contains("private-sentinel"));
    let mut alternate = metadata();
    alternate["id"] = json!("101");
    alternate["content"] = json!("https://another.invalid");
    assert_eq!(
        record.metadata_revision,
        parse_record(&alternate)?.metadata_revision
    );
    alternate["size"] = json!(24);
    assert_ne!(
        record.metadata_revision,
        parse_record(&alternate)?.metadata_revision
    );
    let unknown = parse_record(&json!({"id":"101"}))?;
    assert!(
        unknown.filename.is_none()
            && unknown.size_bytes.is_none()
            && unknown.author_account_id.is_none()
    );
    Ok(())
}
#[test]
fn malformed_metadata_never_gets_coerced_or_truncated() {
    for (key, value) in [
        ("id", json!(0)),
        ("id", json!(1.0)),
        ("id", json!("0101")),
        ("size", json!(-1)),
        ("size", json!(1.0)),
        ("size", json!(u64::MAX)),
        ("mimeType", json!(false)),
        ("filename", json!("x".repeat(4097))),
        ("filename", json!("bad\nname")),
        ("warningMessages", json!([])),
    ] {
        let mut raw = metadata();
        raw[key] = value;
        assert!(parse_record(&raw).is_err(), "{key} {raw}");
    }
}
#[test]
fn unknown_settings_are_distinct_from_disabled_or_zero_limits() -> TestResult {
    let unknown = parse_settings(&json!({}))?;
    assert!(unknown.enabled.is_none() && unknown.upload_limit_bytes.is_none());
    let disabled = parse_settings(&json!({"enabled":false,"uploadLimit":0}))?;
    assert_eq!(disabled.enabled, Some(false));
    assert_eq!(disabled.upload_limit_bytes, Some(0));
    for raw in [
        json!({"enabled":"false"}),
        json!({"uploadLimit":-1}),
        json!({"uploadLimit":1.0}),
        json!({"errors":{}}),
    ] {
        assert!(parse_settings(&raw).is_err());
    }
    Ok(())
}
fn operation(command: &str, input: Value) -> Result<PreparedOperation, CraftError> {
    let mut registry = Registry::default();
    register(&mut registry)?;
    registry.prepare(CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    })
}
struct Port {
    status: u16,
    body: Value,
    calls: Vec<NativeCall>,
}
impl OperationContext for Port {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        Ok(NativeResponse {
            status: self.status,
            body: self.body.clone(),
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
}
#[test]
fn list_reads_only_the_exact_issue_attachment_field_and_has_no_native_continuation() -> TestResult {
    let op = operation(LIST, json!({"issue_id":"100"}))?;
    let mut port = Port {
        status: 200,
        body: json!({"id":"100","fields":{"attachment":[metadata()]}}),
        calls: vec![],
    };
    let page = read_page(&op, &mut port, None)?;
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_none());
    assert_eq!(
        page.context,
        Some(json!({"source_issue_id":"100","coverage":"visible_issue_attachment_field"}))
    );
    assert_eq!(port.calls.len(), 1);
    assert_eq!(port.calls[0].path_parameters["issueIdOrKey"], "100");
    assert_eq!(
        port.calls[0].query,
        BTreeMap::from([
            ("fields".into(), "attachment".into()),
            ("updateHistory".into(), "false".into()),
            ("failFast".into(), "true".into())
        ])
    );
    assert!(read_page(&op, &mut port, Some(&NativePosition(BTreeMap::new()))).is_err());
    assert_eq!(port.calls.len(), 1);
    Ok(())
}
#[test]
fn duplicate_foreign_missing_or_overlimit_issue_evidence_fails_without_partial_items() -> TestResult
{
    let op = operation(LIST, json!({"issue_id":"100"}))?;
    for raw in [
        json!({"id":"999","fields":{"attachment":[]}}),
        json!({"id":"100","fields":{}}),
        json!({"id":"100","fields":{"attachment":null}}),
        json!({"id":"100","fields":{"attachment":[metadata(),metadata()]}}),
        json!({"id":"100","fields":{"attachment":vec![metadata();1001]}}),
        json!({"id":"100","fields":{"attachment":[]},"nextPage":"untrusted"}),
    ] {
        let mut port = Port {
            status: 200,
            body: raw,
            calls: vec![],
        };
        assert!(read_page(&op, &mut port, None).is_err());
        assert_eq!(port.calls.len(), 1);
    }
    Ok(())
}
#[test]
fn exact_get_settings_and_denials_use_one_compiled_route_without_retry() -> TestResult {
    for (command, input, raw) in [
        (GET, json!({"id":"101"}), metadata()),
        (
            SETTINGS,
            json!({}),
            json!({"enabled":true,"uploadLimit":10485760}),
        ),
    ] {
        let op = operation(command, input)?;
        let mut port = Port {
            status: 200,
            body: raw,
            calls: vec![],
        };
        let page = read_page(&op, &mut port, None)?;
        assert_eq!(page.items.len(), 1);
        assert!(page.context.is_none() && page.next.is_none());
        assert_eq!(port.calls.len(), 1);
        if command == GET {
            port.body["id"] = json!(999);
            assert!(read_page(&op, &mut port, None).is_err());
        }
        for status in [401, 403, 404, 429, 500] {
            let mut port = Port {
                status,
                body: json!({"private":"private-sentinel"}),
                calls: vec![],
            };
            let error = read_page(&op, &mut port, None)
                .err()
                .ok_or("expected denial")?;
            assert_eq!(port.calls.len(), 1);
            assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
        }
    }
    Ok(())
}
