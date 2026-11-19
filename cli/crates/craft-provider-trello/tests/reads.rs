//! Provider-level mock fidelity: the shapes below follow Trello's documented REST responses.
//! Live verification against the service is recorded separately.
use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_trello::{TrelloContext, read_get, read_page};
use serde_json::{Value, json};
use std::collections::BTreeMap;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MEMBER: &str = "5f1d2c3b4a5968778695a4b3";
const BOARD: &str = "5f1d2c3b4a5968778695a4b4";
const LIST: &str = "5f1d2c3b4a5968778695a4b5";
const CARD: &str = "5f1d2c3b4a5968778695a4b6";

struct Mock {
    status: u16,
    body: Value,
    calls: Vec<NativeCall>,
}
impl Mock {
    fn ok(body: Value) -> Self {
        Self {
            status: 200,
            body,
            calls: Vec::new(),
        }
    }
}
impl OperationContext for Mock {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
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
    fn normalize_next(&self, _: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        Err(CraftError::new(ErrorCode::Internal, "no links"))
    }
}

fn target() -> ResourceRef {
    ResourceRef {
        provider: "trello".into(),
        deployment: "cloud".into(),
        tenant: MEMBER.into(),
        resource_type: "account".into(),
        id: MEMBER.into(),
    }
}

fn prepare(command: &str, input: Value) -> Result<(Registry, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&TrelloContext)?;
    let operation = registry.prepare(CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((registry, operation))
}

fn position(name: &str, value: &str) -> NativePosition {
    NativePosition(BTreeMap::from([(name.into(), value.into())]))
}

fn card(id: &str) -> Value {
    json!({
        "id": id, "name": "Fix login", "desc": "details", "closed": false,
        "due": "2026-12-01T00:00:00.000Z", "dueComplete": false,
        "idList": LIST, "idBoard": BOARD, "idLabels": [BOARD], "idMembers": [MEMBER],
        "url": "https://trello.com/c/abc", "shortLink": "abc",
        "dateLastActivity": "2026-11-18T00:00:00.000Z", "pos": 65535.0,
        "badges": {"comments": 2, "attachments": 1, "checkItems": 4, "checkItemsChecked": 1, "votes": 0}
    })
}

#[test]
fn card_get_maps_native_names_and_sends_one_fixed_route() -> TestResult {
    let (_, operation) = prepare("trello.card.get", json!({"id": CARD}))?;
    let mut context = Mock::ok(card(CARD));
    let item = read_get(&operation, &mut context)?;
    assert_eq!(item["id"], CARD);
    assert_eq!(item["list_id"], LIST);
    assert_eq!(item["board_id"], BOARD);
    assert_eq!(item["due_complete"], false);
    assert_eq!(item["badges"]["checklist_items_checked"], 1);
    assert_eq!(context.calls.len(), 1);
    assert_eq!(context.calls[0].route, "trello-card-get");
    assert_eq!(context.calls[0].path_parameters["id"], CARD);
    assert!(context.calls[0].query["fields"].contains("idList"));
    Ok(())
}

#[test]
fn a_response_for_a_different_id_is_malformed_not_trusted() -> TestResult {
    let (_, operation) = prepare("trello.card.get", json!({"id": CARD}))?;
    let mut context = Mock::ok(card(LIST));
    let error = read_get(&operation, &mut context)
        .err()
        .ok_or("expected a mismatch")?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    Ok(())
}

#[test]
fn collection_reads_slice_by_offset_and_offer_a_continuation() -> TestResult {
    let lists: Vec<Value> = (0..5)
        .map(|n| json!({"id": format!("5f1d2c3b4a5968778695a4c{n}"), "name": format!("L{n}"), "closed": false, "pos": n, "idBoard": BOARD}))
        .collect();
    let (_, operation) = prepare(
        "trello.list.list",
        json!({"board_id": BOARD, "page_size": 2}),
    )?;
    let mut context = Mock::ok(json!(lists));
    let first = read_page(&operation, &mut context, None)?;
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next, Some(position("offset", "2")));
    let second = read_page(&operation, &mut context, first.next.as_ref())?;
    assert_eq!(second.items[0]["name"], "L2");
    assert_eq!(second.next, Some(position("offset", "4")));
    let last = read_page(&operation, &mut context, second.next.as_ref())?;
    assert_eq!(last.items.len(), 1);
    assert!(last.next.is_none());
    assert_eq!(context.calls[0].route, "trello-list-list");
    Ok(())
}

#[test]
fn comments_page_by_before_id_and_stop_on_a_short_page() -> TestResult {
    let action = |n: u8| {
        json!({"id": format!("5f1d2c3b4a5968778695a4d{n}"), "idMemberCreator": MEMBER, "date": "2026-11-18T00:00:00.000Z",
               "data": {"text": format!("note {n}"), "card": {"id": CARD}}, "memberCreator": {"username": "agent"}})
    };
    let (_, operation) = prepare(
        "trello.comment.list",
        json!({"card_id": CARD, "page_size": 2}),
    )?;
    let mut context = Mock::ok(json!([action(1), action(2)]));
    let page = read_page(&operation, &mut context, None)?;
    assert_eq!(page.items[0]["text"], "note 1");
    assert_eq!(page.items[0]["author_username"], "agent");
    assert_eq!(
        page.next,
        Some(position("before", "5f1d2c3b4a5968778695a4d2"))
    );
    assert_eq!(context.calls[0].query["limit"], "2");
    assert_eq!(context.calls[0].query["filter"], "commentCard");
    let mut context = Mock::ok(json!([action(3)]));
    let page = read_page(&operation, &mut context, page.next.as_ref())?;
    assert!(page.next.is_none());
    assert_eq!(context.calls[0].query["before"], "5f1d2c3b4a5968778695a4d2");
    Ok(())
}

#[test]
fn search_pages_and_scopes_to_a_board_without_widening_the_query() -> TestResult {
    let (_, operation) = prepare(
        "trello.card.search",
        json!({"query": "login", "board_id": BOARD, "page_size": 1}),
    )?;
    let mut context = Mock::ok(json!({"cards": [card(CARD)]}));
    let page = read_page(&operation, &mut context, None)?;
    assert_eq!(page.next, Some(position("page", "1")));
    let call = &context.calls[0];
    assert_eq!(call.query["modelTypes"], "cards");
    assert_eq!(call.query["idBoards"], BOARD);
    assert_eq!(call.query["cards_page"], "0");
    Ok(())
}

#[test]
fn checklists_report_item_state() -> TestResult {
    let (_, operation) = prepare("trello.checklist.list", json!({"card_id": CARD}))?;
    let mut context = Mock::ok(json!([{
        "id": LIST, "name": "Steps", "idCard": CARD,
        "checkItems": [
            {"id": BOARD, "name": "Write test", "state": "complete", "pos": 1},
            {"id": MEMBER, "name": "Ship", "state": "incomplete", "pos": 2}
        ]
    }]));
    let page = read_page(&operation, &mut context, None)?;
    assert_eq!(page.items[0]["items"][0]["complete"], true);
    assert_eq!(page.items[0]["items"][1]["complete"], false);
    Ok(())
}

#[test]
fn statuses_map_to_stable_codes_without_retry_or_body_text() -> TestResult {
    for (status, code) in [
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (429, ErrorCode::RateLimited),
        (404, ErrorCode::RemoteUnavailable),
        (500, ErrorCode::RemoteUnavailable),
    ] {
        let (_, operation) = prepare("trello.board.get", json!({"id": BOARD}))?;
        let mut context = Mock {
            status,
            body: json!({"message": "provider-body-sentinel"}),
            calls: Vec::new(),
        };
        let error = read_get(&operation, &mut context)
            .err()
            .ok_or("expected a refusal")?;
        assert_eq!(error.code, code, "{status}");
        assert!(!error.message.contains("provider-body-sentinel"));
        assert_eq!(context.calls.len(), 1);
    }
    Ok(())
}

#[test]
fn bad_inputs_and_malformed_shapes_are_refused() -> TestResult {
    assert!(prepare("trello.card.get", json!({"id": "NOT-AN-ID"})).is_err());
    assert!(prepare("trello.card.get", json!({"id": CARD, "extra": 1})).is_err());
    assert!(prepare("trello.card.list", json!({"list_id": LIST, "page_size": 0})).is_err());
    assert!(prepare("trello.card.search", json!({"query": "  "})).is_err());
    let (_, operation) = prepare("trello.board.list", json!({}))?;
    // An object where Trello documents an array.
    let mut context = Mock::ok(json!({"boards": []}));
    assert!(read_page(&operation, &mut context, None).is_err());
    // A position that is not valid for the command's paging style.
    let mut context = Mock::ok(json!([]));
    assert!(read_page(&operation, &mut context, Some(&position("before", BOARD))).is_err());
    // Another provider's target is refused before any call.
    let mut registry = Registry::default();
    registry.register_provider(&TrelloContext)?;
    let mut other = target();
    other.provider = "jira".into();
    assert!(
        registry
            .prepare(CanonicalRequest {
                command: "trello.board.list".into(),
                target: other,
                input: json!({}),
                selection: Selection::default(),
                preconditions: None,
            })
            .is_err()
    );
    Ok(())
}
