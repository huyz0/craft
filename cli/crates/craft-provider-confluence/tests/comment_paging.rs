use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_confluence::comments::paging::{COMMENT_LIST, COMMENT_REPLIES};
use craft_provider_confluence::{ConfluenceContext, read_page};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn registry() -> Result<Registry, CraftError> {
    let mut r = Registry::default();
    r.register_provider(&ConfluenceContext)?;
    Ok(r)
}
fn request(command: &str, fields: &[&str]) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "confluence".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        },
        input: if command == COMMENT_LIST {
            json!({"page_id":"101"})
        } else {
            json!({"page_id":"101","parent_comment_id":"201"})
        },
        selection: Selection {
            fields: fields.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        },
        preconditions: None,
    }
}
fn comment(id: &str) -> Value {
    json!({"id":id,"pageId":"101","status":"current","version":{"number":1,"authorId":"reviewer-one","createdAt":"2026-11-10T00:00:00.000Z"},"body":{"storage":{"representation":"storage","value":"<p>Review</p>"}}})
}
fn response(body: Value, links: Option<Vec<String>>) -> NativeResponse {
    NativeResponse {
        status: 200,
        body,
        request_id: None,
        retry_after: None,
        link_headers: links,
    }
}
struct Context {
    responses: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Missing fixture response."))
    }
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        if !link.starts_with('?') {
            return Err(CraftError::new(ErrorCode::PolicyDenied, "Untrusted link."));
        }
        let mut normalized = call.clone();
        normalized.query.clear();
        for pair in link[1..].split('&') {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| CraftError::field("/link", "invalid"))?;
            normalized.query.insert(k.into(), v.into());
        }
        Ok(normalized)
    }
}
fn context(responses: Vec<NativeResponse>) -> Context {
    Context {
        responses: responses.into(),
        calls: vec![],
    }
}
const NEXT: &str =
    "/wiki/api/v2/pages/101/footer-comments?limit=25&sort=created-date&status=current&cursor=next";
#[test]
fn current_roots_use_both_continuation_locations_and_preserve_projection() -> TestResult {
    let r = registry()?;
    let op = r.prepare(request(COMMENT_LIST, &["revision"]))?;
    let next = format!("{NEXT}&body-format=storage");
    for mode in ["terminal", "header", "body", "both"] {
        let body = json!({"results":[comment("201")],"_links":if matches!(mode,"body"|"both"){json!({"next":next})}else{json!({})}});
        let links = if matches!(mode, "header" | "both") {
            vec![format!("<{next}>; rel=\"next\"")]
        } else {
            vec![]
        };
        let mut c = context(vec![response(body, Some(links))]);
        let page = read_page(&op, &mut c, None)?;
        assert_eq!(page.next.is_some(), mode != "terminal");
        assert_eq!(page.items[0]["page_id"], "101");
        assert_eq!(page.items[0]["parent_comment_id"], Value::Null);
        assert!(page.items[0].get("body_storage").is_none());
        assert!(page.items[0]["revision"]["digest"].is_string());
        assert_eq!(c.calls[0].query["sort"], "created-date");
        assert_eq!(c.calls[0].query["body-format"], "storage");
    }
    Ok(())
}
#[test]
fn unknown_malformed_conflicting_and_unbound_continuations_cannot_end_inventory() -> TestResult {
    let r = registry()?;
    let op = r.prepare(request(COMMENT_LIST, &[]))?;
    for mode in [
        "unknown",
        "empty_header",
        "bad_header",
        "empty_relation",
        "whitespace_relation",
        "invalid_relation",
        "duplicate_next",
        "foreign",
        "wrong_path",
        "changed_filter",
        "conflict",
        "bad_body",
        "stalled",
    ] {
        let mut link = NEXT.to_owned();
        match mode {
            "foreign" => link = "https://foreign.invalid/?cursor=next".into(),
            "wrong_path" => link = NEXT.replace("101", "999"),
            "changed_filter" => link = NEXT.replace("current", "historical"),
            _ => {}
        }
        let mut body = json!({"results":[],"_links":{}});
        let header = match mode {
            "unknown" => None,
            "empty_header" => Some(vec![String::new()]),
            "bad_header" => Some(vec!["not a Link".into()]),
            "empty_relation" => Some(vec![format!("<{NEXT}>; rel=\"\"")]),
            "whitespace_relation" => Some(vec![format!("<{NEXT}>; rel=\"   \"")]),
            "invalid_relation" => Some(vec![format!("<{NEXT}>; rel=\"next!\"")]),
            "duplicate_next" => Some(vec![format!("<{NEXT}>; rel=next, <{NEXT}>; rel=next")]),
            _ => Some(vec![format!("<{link}>; rel=next")]),
        };
        if mode == "conflict" {
            body["_links"]["next"] = json!(NEXT.replace("cursor=next", "cursor=other"));
        }
        if mode == "bad_body" {
            body["_links"] = Value::Null;
        }
        let mut c = context(vec![response(body, header)]);
        let position = NativePosition(BTreeMap::from([("cursor".into(), "next".into())]));
        assert!(
            read_page(
                &op,
                &mut c,
                if mode == "stalled" {
                    Some(&position)
                } else {
                    None
                }
            )
            .is_err(),
            "{mode}"
        );
        assert_eq!(c.calls.len(), 1);
    }
    Ok(())
}
#[test]
fn reply_page_inherits_only_bracketed_current_parent_provenance() -> TestResult {
    let r = registry()?;
    let op = r.prepare(request(COMMENT_REPLIES, &["body_storage", "revision"]))?;
    for mode in [
        "valid",
        "wrong_parent",
        "wrong_page",
        "parent_moved",
        "parent_edited",
        "self_reply",
        "duplicate",
        "noncurrent",
        "alternate_container",
    ] {
        let mut child = comment("202");
        child.as_object_mut().ok_or("child")?.remove("pageId");
        child["parentCommentId"] = json!("201");
        let mut final_parent = comment("201");
        match mode {
            "wrong_parent" => child["parentCommentId"] = json!("999"),
            "wrong_page" => child["pageId"] = json!("999"),
            "parent_moved" => final_parent["pageId"] = json!("999"),
            "parent_edited" => final_parent["version"]["number"] = json!(2),
            "self_reply" => child["id"] = json!("201"),
            "noncurrent" => child["status"] = json!("historical"),
            "alternate_container" => child["blogPostId"] = json!("101"),
            _ => {}
        }
        let values = if mode == "duplicate" {
            vec![child.clone(), child]
        } else {
            vec![child]
        };
        let mut c = context(vec![
            response(comment("201"), None),
            response(json!({"results":values,"_links":{}}), Some(vec![])),
            response(final_parent, None),
        ]);
        let page = read_page(&op, &mut c, None);
        assert_eq!(page.is_ok(), mode == "valid", "{mode}");
        if let Ok(page) = page {
            assert_eq!(page.items[0]["page_id"], "101");
            assert_eq!(page.items[0]["parent_comment_id"], "201");
            assert!(page.next.is_none());
            assert_eq!(c.calls.len(), 3);
        }
    }
    Ok(())
}
#[test]
fn collection_schemas_reject_bad_selectors_and_match_examples() -> TestResult {
    let r = registry()?;
    for command in [COMMENT_LIST, COMMENT_REPLIES] {
        let d = r.descriptor(command)?;
        assert!(
            jsonschema::validator_for(&d.response_schema)?.is_valid(&d.response_example),
            "{command}"
        );
        let mut req = request(command, &[]);
        req.input["page_size"] = json!(101);
        assert!(r.prepare(req).is_err());
        let mut req = request(command, &[]);
        req.input["page_id"] = json!("01");
        assert!(r.prepare(req).is_err());
        assert!(
            r.prepare(request(command, &["body_storage", "body_atlas_doc_format"]))
                .is_err()
        );
    }
    Ok(())
}
