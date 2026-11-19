//! Request construction and answer interpretation for every Trello write, against documented
//! response shapes. Live verification is recorded separately.
use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::ErrorCode;
use craft_core::execution::{HttpMethod, NativeResponse, PreparedOperation};
use craft_core::registry::Registry;
use craft_core::storage::SubmissionState;
use craft_provider_trello::{TrelloContext, build, interpret, step, write_commands, write_entry};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MEMBER: &str = "5f1d2c3b4a5968778695a4b3";
const BOARD: &str = "5f1d2c3b4a5968778695a4b4";
const LIST: &str = "5f1d2c3b4a5968778695a4b5";
const CARD: &str = "5f1d2c3b4a5968778695a4b6";
const LABEL: &str = "5f1d2c3b4a5968778695a4b7";
const ITEM: &str = "5f1d2c3b4a5968778695a4b8";

fn request(command: &str, input: Value) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "trello".into(),
            deployment: "cloud".into(),
            tenant: MEMBER.into(),
            resource_type: "account".into(),
            id: MEMBER.into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}

fn prepared(command: &str, input: Value) -> Result<PreparedOperation, Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&TrelloContext)?;
    Ok(registry.prepare(request(command, input))?)
}

fn answer(status: u16, body: Value) -> NativeResponse {
    NativeResponse {
        status,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}

#[test]
fn every_write_declares_one_route_and_a_matching_journal_step() -> TestResult {
    let mut registry = Registry::default();
    registry.register_provider(&TrelloContext)?;
    for command in write_commands() {
        let descriptor = registry.descriptor(command)?;
        assert_eq!(descriptor.routes.len(), 1, "{command}");
        assert_eq!(Some(descriptor.routes[0].id.clone()), step(command));
        assert!(!matches!(descriptor.routes[0].method, HttpMethod::Get));
        assert!(descriptor.routes[0].query_parameters.is_empty());
        assert!(descriptor.routes[0].validate().is_ok());
    }
    // No delete of a board, list, card or comment is declared.
    assert!(write_commands().all(|c| !c.contains("delete")));
    assert!(write_commands().all(|c| {
        write_entry(c).is_some_and(|e| {
            !matches!(e.method, HttpMethod::Delete)
                || matches!(c, "trello.label.remove" | "trello.member.remove")
        })
    }));
    Ok(())
}

#[test]
fn card_create_sends_a_json_body_and_never_credentials() -> TestResult {
    let call = build(&request(
        "trello.card.create",
        json!({"list_id": LIST, "name": "Fix login", "description": "Steps", "label_ids": [LABEL],
               "member_ids": [MEMBER], "position": "top", "due": "2026-12-01T10:00:00.000Z"}),
    ))?;
    assert_eq!(call.route, "trello-card-create");
    assert!(call.path_parameters.is_empty() && call.query.is_empty());
    let body = call.body.ok_or("body")?;
    assert_eq!(body["idList"], LIST);
    assert_eq!(body["name"], "Fix login");
    assert_eq!(body["desc"], "Steps");
    assert_eq!(body["idLabels"], LABEL);
    assert_eq!(body["pos"], "top");
    let text = body.to_string();
    assert!(!text.contains("key") && !text.contains("token"));
    Ok(())
}

#[test]
fn path_parameters_and_bodies_follow_the_documented_routes() -> TestResult {
    let cases: [(&str, Value, &[(&str, &str)], Option<(&str, Value)>); 10] = [
        (
            "trello.card.update",
            json!({"id": CARD, "name": "N", "due_complete": true}),
            &[("id", CARD)],
            Some(("dueComplete", json!(true))),
        ),
        (
            "trello.card.move",
            json!({"id": CARD, "list_id": LIST, "board_id": BOARD}),
            &[("id", CARD)],
            Some(("idBoard", json!(BOARD))),
        ),
        (
            "trello.card.archive",
            json!({"id": CARD}),
            &[("id", CARD)],
            Some(("closed", json!(true))),
        ),
        (
            "trello.comment.add",
            json!({"card_id": CARD, "text": "hi"}),
            &[("id", CARD)],
            Some(("text", json!("hi"))),
        ),
        (
            "trello.comment.update",
            json!({"card_id": CARD, "comment_id": ITEM, "text": "hi"}),
            &[("id", CARD), ("id2", ITEM)],
            Some(("text", json!("hi"))),
        ),
        (
            "trello.label.add",
            json!({"card_id": CARD, "label_id": LABEL}),
            &[("id", CARD)],
            Some(("value", json!(LABEL))),
        ),
        (
            "trello.label.remove",
            json!({"card_id": CARD, "label_id": LABEL}),
            &[("id", CARD), ("id2", LABEL)],
            None,
        ),
        (
            "trello.member.remove",
            json!({"card_id": CARD, "member_id": MEMBER}),
            &[("id", CARD), ("id2", MEMBER)],
            None,
        ),
        (
            "trello.checkitem.set-state",
            json!({"card_id": CARD, "item_id": ITEM, "complete": false}),
            &[("id", CARD), ("id2", ITEM)],
            Some(("state", json!("incomplete"))),
        ),
        (
            "trello.attachment.add-url",
            json!({"card_id": CARD, "url": "https://example.com/spec", "name": "Spec"}),
            &[("id", CARD)],
            Some(("url", json!("https://example.com/spec"))),
        ),
    ];
    for (command, input, path, body) in cases {
        let call = build(&request(command, input))?;
        let want: std::collections::BTreeMap<String, String> = path
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        assert_eq!(call.path_parameters, want, "{command}");
        match body {
            Some((key, value)) => assert_eq!(call.body.ok_or(command)?[key], value, "{command}"),
            None => assert!(call.body.is_none(), "{command}"),
        }
    }
    Ok(())
}

#[test]
fn invalid_writes_are_refused_before_any_call() {
    let bad = [
        (
            "trello.card.create",
            json!({"list_id": "short", "name": "x"}),
        ),
        ("trello.card.create", json!({"list_id": LIST, "name": "  "})),
        (
            "trello.card.create",
            json!({"list_id": LIST, "name": "x", "due": "tomorrow"}),
        ),
        (
            "trello.card.create",
            json!({"list_id": LIST, "name": "x", "label_ids": [LABEL, LABEL]}),
        ),
        ("trello.card.update", json!({"id": CARD})),
        ("trello.card.update", json!({"id": CARD, "extra": 1})),
        (
            "trello.label.create",
            json!({"board_id": BOARD, "name": "x", "color": "chartreuse"}),
        ),
        (
            "trello.attachment.add-url",
            json!({"card_id": CARD, "url": "http://example.com"}),
        ),
        (
            "trello.attachment.add-url",
            json!({"card_id": CARD, "url": "https://exa mple.com"}),
        ),
        ("trello.comment.add", json!({"card_id": CARD, "text": ""})),
    ];
    for (command, input) in bad {
        assert!(
            prepared(command, input.clone()).is_err(),
            "{command} {input}"
        );
    }
}

#[test]
fn a_confirmed_create_names_the_new_resource_and_verifies_the_requested_fields() -> TestResult {
    let operation = prepared(
        "trello.card.create",
        json!({"list_id": LIST, "name": "Fix login"}),
    )?;
    let observation = interpret(
        &operation,
        &answer(
            200,
            json!({"id": CARD, "name": "Fix login", "idList": LIST, "idBoard": BOARD, "closed": false, "url": "https://trello.com/c/x", "extra": {"ignored": true}}),
        ),
    )?;
    assert_eq!(observation.state, SubmissionState::Confirmed);
    assert_eq!(observation.resources[0].resource_type, "card");
    assert_eq!(observation.resources[0].id, CARD);
    assert!(observation.desired_fields_verified && observation.error.is_none());
    let item = observation.item.ok_or("item")?;
    assert_eq!(item["list_id"], LIST);
    assert!(item.get("extra").is_none());
    Ok(())
}

#[test]
fn a_200_that_does_not_show_the_change_is_a_visible_partial_apply() -> TestResult {
    let operation = prepared(
        "trello.card.update",
        json!({"id": CARD, "name": "New name"}),
    )?;
    let observation = interpret(
        &operation,
        &answer(200, json!({"id": CARD, "name": "Old name"})),
    )?;
    assert_eq!(observation.state, SubmissionState::Confirmed);
    assert!(!observation.desired_fields_verified);
    assert_eq!(
        observation.error.ok_or("error")?.code,
        ErrorCode::PartialApply
    );
    Ok(())
}

#[test]
fn association_changes_confirm_the_card_and_check_the_returned_set() -> TestResult {
    let operation = prepared(
        "trello.label.add",
        json!({"card_id": CARD, "label_id": LABEL}),
    )?;
    let ok = interpret(&operation, &answer(200, json!([LABEL])))?;
    assert_eq!(ok.resources[0].id, CARD);
    assert!(ok.desired_fields_verified);
    let missing = interpret(&operation, &answer(200, json!([BOARD])))?;
    assert!(!missing.desired_fields_verified && missing.error.is_some());
    Ok(())
}

#[test]
fn refusals_are_failed_and_unclear_answers_are_unknown_and_never_retried() -> TestResult {
    let operation = prepared("trello.comment.add", json!({"card_id": CARD, "text": "hi"}))?;
    for (status, code) in [
        (400, ErrorCode::InputInvalid),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (404, ErrorCode::InputInvalid),
        (429, ErrorCode::RateLimited),
    ] {
        let observation = interpret(
            &operation,
            &answer(status, json!({"message": "provider-body-sentinel"})),
        )?;
        assert_eq!(observation.state, SubmissionState::Failed, "{status}");
        let error = observation.error.ok_or("error")?;
        assert_eq!(error.code, code);
        assert!(!error.message.contains("provider-body-sentinel"));
    }
    for answer in [
        answer(500, json!({})),
        answer(502, json!({})),
        answer(200, json!({"unexpected": true})),
    ] {
        let observation = interpret(&operation, &answer)?;
        assert_eq!(observation.state, SubmissionState::Unknown);
        assert_eq!(
            observation.error.ok_or("error")?.code,
            ErrorCode::WriteOutcomeUnknown
        );
    }
    Ok(())
}
