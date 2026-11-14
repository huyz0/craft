use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_confluence::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn footer_comment_get_binds_current_page_author_and_body_fingerprint() -> TestResult {
    let command = craft_provider_confluence::comments::COMMENT_GET;
    let original = json!({"id":"201","pageId":"101","status":"current","version":{"number":2,"authorId":"reviewer-one","createdAt":"2026-11-10T00:00:00.000Z"},"body":{"storage":{"representation":"storage","value":"<p>Visible review text</p>"}},"_links":{"base":"https://untrusted.invalid"}});
    let operation = prepare(
        command,
        json!({"id":"201","page_id":"101"}),
        &["revision", "body_storage", "parent_comment_id"],
    )?;
    let mut c = context(original.clone());
    let first = read_get(&operation, &mut c)?;
    assert_eq!(first["version_author_id"], "reviewer-one");
    assert_eq!(first["page_id"], "101");
    assert_eq!(first["revision"]["version"], 2);
    assert_eq!(first["parent_comment_id"], Value::Null);
    assert_eq!(
        c.calls[0].query,
        BTreeMap::from([
            ("body-format".into(), "storage".into()),
            ("include-version".into(), "true".into())
        ])
    );
    assert_eq!(c.calls[0].path_parameters["comment-id"], "201");
    for mode in ["edited_body", "edited_author", "edited_version"] {
        let mut raw = original.clone();
        match mode {
            "edited_body" => raw["body"]["storage"]["value"] = json!("<p>Changed</p>"),
            "edited_author" => raw["version"]["authorId"] = json!("worker-agent"),
            _ => raw["version"]["number"] = json!(3),
        }
        let next = read_get(&operation, &mut context(raw))?;
        assert_ne!(
            next["revision"]["digest"], first["revision"]["digest"],
            "{mode}"
        );
    }
    for mode in [
        "wrong_id",
        "wrong_page",
        "historical",
        "missing_author",
        "missing_timestamp",
        "missing_version",
        "alternate_container",
        "missing_body",
    ] {
        let mut raw = original.clone();
        match mode {
            "wrong_id" => raw["id"] = json!("999"),
            "wrong_page" => raw["pageId"] = json!("999"),
            "historical" => raw["status"] = json!("historical"),
            "missing_author" => raw["version"]["authorId"] = Value::Null,
            "missing_timestamp" => raw["version"]["createdAt"] = Value::Null,
            "missing_version" => raw["version"]["number"] = json!(0),
            "alternate_container" => raw["blogPostId"] = json!("101"),
            _ => raw["body"] = Value::Null,
        }
        assert!(read_get(&operation, &mut context(raw)).is_err(), "{mode}");
    }
    let op = prepare(command, json!({"id":"201","page_id":"101"}), &[])?;
    let metadata = read_get(&op, &mut context(original))?;
    assert!(metadata.get("body_storage").is_none());
    assert!(metadata.get("revision").is_none());
    assert_eq!(metadata["body_state"], "not_requested");
    assert!(
        prepare(
            command,
            json!({"id":"201","page_id":"101"}),
            &["body_storage", "body_atlas_doc_format"]
        )
        .is_err()
    );
    assert!(prepare(command, json!({"id":"201"}), &[]).is_err());
    Ok(())
}

#[test]
fn revision_selection_fetches_exact_current_content_without_returning_unselected_body() -> TestResult
{
    let op = prepare(PAGE_GET, json!({"id":"101"}), &["revision"])?;
    let mut c = context(page());
    let item = read_get(&op, &mut c)?;
    assert!(item.get("body_storage").is_none());
    assert_eq!(item["body_state"], "not_requested");
    assert_eq!(item["revision"]["version"], 4);
    assert_eq!(item["revision"]["representation"], "storage");
    assert_eq!(c.calls[0].query["body-format"], "storage");
    let expected = craft_provider_confluence::pages::observe_page(
        &NativeResponse {
            status: 200,
            body: page(),
            request_id: None,
            retry_after: None,
            link_headers: None,
        },
        craft_provider_confluence::pages::PageRepresentation::Storage,
    )?
    .revision()?;
    assert_eq!(item["revision"], serde_json::to_value(expected)?);
    let mut raw = page();
    raw.as_object_mut().ok_or("page")?.remove("body");
    assert!(read_get(&op, &mut context(raw)).is_err());
    assert!(prepare(PAGE_LIST, json!({}), &["revision"]).is_err());
    assert!(prepare(PAGE_SEARCH, json!({"cql":"type = page"}), &["revision"]).is_err());
    Ok(())
}

#[test]
fn implemented_routes_and_scopes_match_the_independently_pinned_provider_inventory() -> TestResult {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../openspec/changes/build-cloud-cli/provider-operations.json");
    let matrix: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let registry = registry()?;
    for command in [SPACE_GET, SPACE_LIST, PAGE_GET, PAGE_LIST, PAGE_SEARCH] {
        let descriptor = registry.descriptor(command)?;
        let native = matrix["commands"]
            .as_array()
            .ok_or("commands")?
            .iter()
            .find(|c| c["command"] == command)
            .ok_or("command")?;
        for route in &descriptor.routes {
            let variant = native["variants"]
                .as_array()
                .ok_or("variants")?
                .iter()
                .find(|v| {
                    v["route_shape"]["path"]
                        == serde_json::to_value(&route.path).unwrap_or(Value::Null)
                })
                .ok_or("native route")?;
            assert_eq!(variant["method"], "GET");
            for name in &route.query_parameters {
                assert!(
                    variant["route_shape"]["query_parameters"]
                        .as_array()
                        .ok_or("query")?
                        .contains(&json!(name))
                );
            }
            let scopes = variant["oauth_scope_alternatives"]
                .as_array()
                .ok_or("scopes")?
                .iter()
                .find(|s| s["state"] == "Current")
                .ok_or("current scopes")?;
            assert_eq!(
                serde_json::to_value(&descriptor.scope_alternatives[0])?,
                scopes["scopes"]
            );
        }
    }
    Ok(())
}
fn site() -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "site".into(),
        id: "00000000-1111-2222-3333-444444444444".into(),
    }
}
fn request(command: &str, input: Value, fields: &[&str]) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: site(),
        input,
        selection: Selection {
            fields: fields.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        },
        preconditions: None,
    }
}
fn registry() -> Result<Registry, CraftError> {
    let mut r = Registry::default();
    r.register_provider(&ConfluenceContext)?;
    Ok(r)
}
fn prepare(command: &str, input: Value, fields: &[&str]) -> Result<PreparedOperation, CraftError> {
    registry()?.prepare(request(command, input, fields))
}
fn page() -> Value {
    json!({"id":"101","spaceId":"100","title":"Plan 文字","status":"current","version":{"number":4},"body":{"storage":{"representation":"storage","value":"<p>Reviewed &amp; visible</p>"}},"ownerId":"ignored-user","_links":{"webui":"https://untrusted.invalid"}})
}
fn space() -> Value {
    json!({"id":"100","key":"ENG","name":"Engineering","type":"global","status":"current"})
}
struct Context {
    raw: Value,
    status: u16,
    calls: Vec<NativeCall>,
    next: Option<NativePosition>,
    changed: bool,
}
fn context(raw: Value) -> Context {
    Context {
        raw,
        status: 200,
        calls: vec![],
        next: None,
        changed: false,
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        Ok(NativeResponse {
            body: self.raw.clone(),
            status: self.status,
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
    fn normalize_next(&self, call: &NativeCall, _link: &str) -> Result<NativeCall, CraftError> {
        let mut next = call.clone();
        for (k, v) in self
            .next
            .as_ref()
            .ok_or_else(|| CraftError::new(ErrorCode::PolicyDenied, "Untrusted continuation."))?
            .0
            .iter()
        {
            next.query.insert(k.clone(), v.clone());
        }
        if self.changed {
            next.query.insert("limit".into(), "999".into());
        }
        Ok(next)
    }
}
#[test]
fn descriptors_and_inputs_cover_each_native_read_without_public_registration() -> TestResult {
    let r = registry()?;
    for command in [SPACE_GET, SPACE_LIST, PAGE_GET, PAGE_LIST, PAGE_SEARCH] {
        let d = r.descriptor(command)?;
        r.validate(command, &d.example)?;
        assert!(jsonschema::validator_for(&d.response_schema)?.is_valid(&d.response_example));
        assert!(
            d.routes
                .iter()
                .all(|r| matches!(r.method, craft_core::execution::HttpMethod::Get))
        );
        assert!(!d.scope_alternatives.is_empty());
    }
    for (command, input) in [
        (PAGE_GET, json!({"id":"101"})),
        (SPACE_GET, json!({"id":"100"})),
        (SPACE_LIST, json!({})),
        (PAGE_LIST, json!({})),
        (PAGE_SEARCH, json!({"cql":"type = page"})),
    ] {
        let mut bad = request(command, input.clone(), &[]);
        bad.target.provider = "jira".into();
        assert!(r.prepare(bad).is_err());
        let mut bad = request(command, input, &[]);
        bad.input["unexpected"] = json!(true);
        assert!(r.prepare(bad).is_err());
    }
    for id in ["0", "01", "-1", "9223372036854775808", "ENG", "../101"] {
        assert!(prepare(PAGE_GET, json!({"id":id}), &[]).is_err());
    }
    assert!(
        prepare(
            PAGE_GET,
            json!({"id":"101"}),
            &["body_storage", "body_atlas_doc_format"]
        )
        .is_err()
    );
    assert!(prepare(PAGE_SEARCH, json!({"cql":" "}), &[]).is_err());
    assert!(prepare(PAGE_LIST, json!({"page_size":251}), &[]).is_err());
    Ok(())
}
#[test]
fn page_get_selects_body_representation_and_preserves_content_without_metadata_leaks() -> TestResult
{
    let op = prepare(
        PAGE_GET,
        json!({"id":"101"}),
        &["title", "body_storage", "version"],
    )?;
    let mut c = context(page());
    let item = read_get(&op, &mut c)?;
    assert_eq!(item["body_storage"], "<p>Reviewed &amp; visible</p>");
    assert_eq!(item["version"], 4);
    assert_eq!(item["body_state"], "returned");
    assert!(item.get("ownerId").is_none());
    assert!(item.get("_links").is_none());
    assert_eq!(c.calls.len(), 1);
    assert_eq!(c.calls[0].query["body-format"], "storage");
    assert_eq!(c.calls[0].query["get-draft"], "false");
    assert_eq!(c.calls[0].query["status"], "current");
    let op = prepare(PAGE_GET, json!({"id":"101"}), &[])?;
    let mut c = context(page());
    let item = read_get(&op, &mut c)?;
    assert_eq!(item["body_state"], "not_requested");
    assert!(item.get("body_storage").is_none());
    assert!(!c.calls[0].query.contains_key("body-format"));
    let op = prepare(PAGE_GET, json!({"id":"101"}), &["body_storage"])?;
    let mut raw = page();
    raw.as_object_mut().ok_or("page")?.remove("body");
    let item = read_get(&op, &mut context(raw))?;
    assert_eq!(item["body_state"], "not_returned");
    assert_eq!(item["fields_not_returned"], json!(["body_storage"]));
    let mut raw = page();
    raw["body"]["storage"]["representation"] = json!("view");
    assert!(read_get(&op, &mut context(raw)).is_err());
    let mut raw = page();
    raw["id"] = json!("102");
    assert!(read_get(&op, &mut context(raw)).is_err());
    Ok(())
}
#[test]
fn space_identity_is_never_inferred_from_key_or_title() -> TestResult {
    let op = prepare(SPACE_GET, json!({"id":"100"}), &[])?;
    let item = read_get(&op, &mut context(space()))?;
    assert_eq!(item["id"], "100");
    assert_eq!(item["key"], "ENG");
    let op = prepare(SPACE_LIST, json!({"key":"ENG","page_size":25}), &[])?;
    let mut c = context(json!({"results":[space()],"_links":{}}));
    let p = read_page(&op, &mut c, None)?;
    assert_eq!(p.items[0]["key"], "ENG");
    assert_eq!(c.calls[0].query["keys"], "ENG");
    let mut wrong = space();
    wrong["key"] = json!("OTHER");
    assert!(
        read_page(
            &op,
            &mut context(json!({"results":[wrong],"_links":{}})),
            None
        )
        .is_err()
    );
    assert!(prepare(SPACE_GET, json!({"id":"ENG"}), &[]).is_err());
    Ok(())
}
#[test]
fn list_cursors_preserve_route_filters_and_allow_short_or_empty_continuing_pages() -> TestResult {
    let op = prepare(
        PAGE_LIST,
        json!({"space_id":"100","title":"Plan 文字","page_size":25}),
        &[],
    )?;
    let mut c = context(
        json!({"results":[],"_links":{"next":"/wiki/api/v2/spaces/100/pages?cursor=opaque"}}),
    );
    c.next = Some(NativePosition(BTreeMap::from([(
        "cursor".into(),
        "opaque".into(),
    )])));
    let p = read_page(&op, &mut c, None)?;
    assert!(p.items.is_empty());
    assert!(p.next.is_some());
    assert_eq!(c.calls[0].path_parameters["id"], "100");
    let p2 = read_page(
        &op,
        &mut context(json!({"results":[page()],"_links":{}})),
        p.next.as_ref(),
    )?;
    assert_eq!(p2.items.len(), 1);
    assert!(p2.next.is_none());
    c.changed = true;
    assert!(read_page(&op, &mut c, None).is_err());
    let mut c = context(json!({"results":[page(),page()],"_links":{}}));
    assert!(read_page(&op, &mut c, None).is_err());
    let mut wrong = page();
    wrong["spaceId"] = json!("999");
    assert!(
        read_page(
            &op,
            &mut context(json!({"results":[wrong],"_links":{}})),
            None
        )
        .is_err()
    );
    let mut c = context(json!({"results":[],"_links":{"next":"https://untrusted.invalid/"}}));
    assert!(read_page(&op, &mut c, None).is_err());
    Ok(())
}
#[test]
fn cql_search_expands_only_requested_context_and_never_drops_unsupported_results() -> TestResult {
    let op = prepare(
        PAGE_SEARCH,
        json!({"cql":"type = page AND space = ENG","page_size":25}),
        &["title", "space_id", "version", "body_storage"],
    )?;
    let raw = json!({"results":[{"content":{"id":"101","type":"page","status":"current","title":"Plan","space":{"id":"100","key":"ENG"},"version":{"number":4},"body":{"storage":{"representation":"storage","value":"<p>draft plan</p>"}}},"excerpt":"ignored highlight"}],"size":1,"start":0,"limit":25,"_links":{}});
    let mut c = context(raw.clone());
    let p = read_page(&op, &mut c, None)?;
    assert_eq!(p.items[0]["space_id"], "100");
    assert_eq!(p.items[0]["body_state"], "returned");
    assert_eq!(c.calls[0].query["cql"], "type = page AND space = ENG");
    assert_eq!(c.calls[0].query["excerpt"], "none");
    assert_eq!(
        c.calls[0].query["expand"],
        "content.body.storage,content.space,content.version"
    );
    let mut wrong = raw;
    wrong["results"][0]["content"]["type"] = json!("blogpost");
    assert!(read_page(&op, &mut context(wrong), None).is_err());
    Ok(())
}
#[test]
fn remote_errors_are_sanitized_and_never_retried_or_treated_as_absence() -> TestResult {
    let op = prepare(PAGE_GET, json!({"id":"101"}), &[])?;
    for status in [400, 401, 403, 404, 429, 500] {
        let mut c = context(json!({"private":"provider-secret"}));
        c.status = status;
        let error = read_get(&op, &mut c).err().ok_or("error")?;
        assert!(!serde_json::to_string(&error)?.contains("provider-secret"));
        assert_eq!(c.calls.len(), 1);
    }
    Ok(())
}

#[test]
fn search_clamps_native_limit_without_changing_bound_filters_and_requires_advancing_offsets()
-> TestResult {
    let op = prepare(
        PAGE_SEARCH,
        json!({"cql":"type = page","page_size":100}),
        &["id"],
    )?;
    let mut c = context(
        json!({"results":[],"size":0,"start":0,"limit":25,"_links":{"next":"/wiki/rest/api/search?start=25&limit=25"}}),
    );
    c.next = Some(NativePosition(BTreeMap::from([
        ("start".into(), "25".into()),
        ("limit".into(), "25".into()),
    ])));
    let p = read_page(&op, &mut c, None)?;
    assert_eq!(
        p.next.ok_or("next")?.0,
        BTreeMap::from([("start".into(), "25".into())])
    );
    assert_eq!(c.calls[0].query["limit"], "100");
    Ok(())
}
/// A new site's default space has type `onboarding`, alongside a personal space,
/// with null description/icon and only `_links.base` at the top level. This is the
/// shape a real site returned for the space list.
#[test]
fn live_shaped_space_list_accepts_onboarding_and_personal_spaces() -> TestResult {
    let op = prepare(SPACE_LIST, json!({"page_size":25}), &["name", "space_type"])?;
    let personal = json!({"id":"98315","key":"~712020abcdef","name":"Example Person","type":"personal","status":"current","authorId":"712020:abc","createdAt":"2026-11-01T00:00:00.000Z","homepageId":"98400","spaceOwnerId":"712020:abc","currentActiveAlias":"~712020abcdef","description":null,"icon":null,"_links":{"webui":"/spaces/~712020abcdef"}});
    let mut onboarding = personal.clone();
    onboarding["id"] = json!("98316");
    onboarding["key"] = json!("SD");
    onboarding["currentActiveAlias"] = json!("SD");
    onboarding["type"] = json!("onboarding");
    let mut c = context(
        json!({"results":[personal, onboarding],"_links":{"base":"https://untrusted.invalid/wiki"}}),
    );
    let p = read_page(&op, &mut c, None)?;
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.items[1]["space_type"], "onboarding");
    assert!(p.next.is_none());
    Ok(())
}
