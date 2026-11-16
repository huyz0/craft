use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::execution::{NativeResponse, PreparedOperation};
use craft_core::registry::Registry;
use craft_core::storage::SubmissionState;
use craft_provider_confluence::pages::*;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn site() -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "site".into(),
        id: "00000000-1111-2222-3333-444444444444".into(),
    }
}
fn response(status: u16, body: Value) -> NativeResponse {
    NativeResponse {
        status,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
fn space() -> NativeResponse {
    response(
        200,
        json!({"id":"100","status":"current","homepageId":"101"}),
    )
}
fn page() -> NativeResponse {
    response(
        200,
        json!({"id":"101","spaceId":"100","status":"current","title":"Shared plan 文字","version":{"number":4},"parentId":"99","ownerId":"human","body":{"storage":{"representation":"storage","value":"<p>Human &amp; agent</p>"}}}),
    )
}
fn create_input() -> Value {
    json!({"space_id":"100","title":"Plan draft","placement":{"kind":"root"},"body":{"representation":"storage","value":"<p>Review together</p>"}})
}
fn registry() -> Result<Registry, craft_core::error::CraftError> {
    let mut r = Registry::default();
    r.register_provider(&ConfluencePages)?;
    Ok(r)
}
fn prepare(
    command: &str,
    input: Value,
) -> Result<PreparedOperation, craft_core::error::CraftError> {
    registry()?.prepare(CanonicalRequest {
        command: command.into(),
        target: site(),
        input,
        selection: Selection::default(),
        preconditions: None,
    })
}
fn update_input() -> Result<Value, craft_core::error::CraftError> {
    let observed = observe_page(&page(), PageRepresentation::Storage)?;
    Ok(
        json!({"id":"101","space_id":"100","expected":observed.revision()?,"editor_draft_policy":"native_reconciliation_may_overwrite","title":"Reviewed title"}),
    )
}

#[test]
fn create_places_a_normal_published_page_in_a_verified_space() -> TestResult {
    let write = prepare_create(&prepare(PAGE_CREATE, create_input())?, &space(), None)?;
    let call = write.call();
    assert_eq!(call.route, CREATE_ROUTE);
    assert_eq!(
        call.query.get("root-level").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        call.body,
        Some(
            json!({"spaceId":"100","status":"current","title":"Plan draft","body":{"representation":"storage","value":"<p>Review together</p>"}})
        )
    );
    let mut input = create_input();
    input["placement"] = json!({"kind":"homepage"});
    assert!(prepare_create(&prepare(PAGE_CREATE, input.clone())?, &space(), None).is_err());
    let parent = page();
    let write = prepare_create(&prepare(PAGE_CREATE, input)?, &space(), Some(&parent))?;
    assert_eq!(
        write.call().body.as_ref().and_then(|v| v.get("parentId")),
        Some(&json!("101"))
    );
    assert_eq!(write.call().query["root-level"], "false");
    Ok(())
}

#[test]
fn create_refuses_foreign_archived_or_changed_parent_evidence() -> TestResult {
    let mut input = create_input();
    input["placement"] = json!({"kind":"parent","id":"101"});
    let op = prepare(PAGE_CREATE, input)?;
    for (field, value) in [
        ("spaceId", json!("200")),
        ("id", json!("102")),
        ("status", json!("draft")),
    ] {
        let mut parent = page();
        parent.body[field] = value;
        assert!(prepare_create(&op, &space(), Some(&parent)).is_err());
    }
    for (field, value) in [("id", json!("200")), ("status", json!("archived"))] {
        let mut s = space();
        s.body[field] = value;
        assert!(prepare_create(&op, &s, Some(&page())).is_err());
    }
    assert!(prepare_create(&op, &space(), Some(&page())).is_ok());
    Ok(())
}

#[test]
fn update_preserves_omitted_rich_body_and_never_sends_owner_or_tree_changes() -> TestResult {
    let write = prepare_update(&prepare(PAGE_UPDATE, update_input()?)?, &page())?;
    assert_eq!(write.call().path_parameters["id"], "101");
    assert!(write.call().query.is_empty());
    assert_eq!(
        write.call().body,
        Some(
            json!({"id":"101","status":"current","title":"Reviewed title","body":{"representation":"storage","value":"<p>Human &amp; agent</p>"},"version":{"number":5}})
        )
    );
    let mut input = update_input()?;
    input.as_object_mut().ok_or("object")?.remove("title");
    input["body"] = json!({"representation":"atlas_doc_format","value":"{\"version\":1,\"type\":\"doc\",\"content\":[]}"});
    let write = prepare_update(&prepare(PAGE_UPDATE, input)?, &page())?;
    assert_eq!(
        write.call().body.as_ref().ok_or("body")?["title"],
        "Shared plan 文字"
    );
    assert_eq!(
        write.call().body.as_ref().ok_or("body")?["body"]["representation"],
        "atlas_doc_format"
    );
    Ok(())
}

#[test]
fn updates_stop_on_version_body_title_identity_or_lifecycle_drift() -> TestResult {
    let op = prepare(PAGE_UPDATE, update_input()?)?;
    for (pointer, value) in [
        ("/version/number", json!(5)),
        ("/title", json!("Human edited")),
        ("/body/storage/value", json!("<p>New human text</p>")),
        ("/spaceId", json!("200")),
        ("/id", json!("102")),
        ("/status", json!("trashed")),
    ] {
        let mut current = page();
        *current.body.pointer_mut(pointer).ok_or("pointer")? = value;
        assert!(prepare_update(&op, &current).is_err(), "{pointer}");
    }
    let mut current = page();
    current.body["subtype"] = json!("live");
    assert!(prepare_update(&op, &current).is_err());
    let mut input = update_input()?;
    input["expected"]["version"] = json!(i32::MAX);
    assert!(prepare(PAGE_UPDATE, input).is_err());
    Ok(())
}

#[test]
fn strict_inputs_reject_accidental_modes_nulls_noops_and_invalid_documents() -> TestResult {
    for (field, value) in [
        ("status", json!("draft")),
        ("ownerId", json!("agent")),
        ("private", json!(true)),
        ("subtype", json!("live")),
        ("space_id", json!("0100")),
        ("title", json!("  ")),
    ] {
        let mut input = create_input();
        input[field] = value;
        assert!(prepare(PAGE_CREATE, input).is_err(), "{field}");
    }
    for placement in [
        json!({"kind":"root","id":"101"}),
        json!({"kind":"parent","id":"0"}),
        json!({"kind":"parent"}),
    ] {
        let mut input = create_input();
        input["placement"] = placement;
        assert!(prepare(PAGE_CREATE, input).is_err());
    }
    for body in [
        json!({"representation":"wiki","value":"text"}),
        json!({"representation":"storage","value":""}),
        json!({"representation":"atlas_doc_format","value":"{\"type\":\"doc\",\"version\":1,\"version\":2,\"content\":[]}"}),
        json!({"representation":"atlas_doc_format","value":"{}"}),
    ] {
        let mut input = create_input();
        input["body"] = body;
        assert!(prepare(PAGE_CREATE, input).is_err());
    }
    for field in ["title", "body"] {
        let mut input = update_input()?;
        input[field] = Value::Null;
        assert!(prepare(PAGE_UPDATE, input).is_err());
    }
    let mut input = update_input()?;
    input.as_object_mut().ok_or("object")?.remove("title");
    assert!(prepare(PAGE_UPDATE, input).is_err());
    Ok(())
}

#[test]
fn native_success_refusal_and_unknown_evidence_never_imply_retry() -> TestResult {
    let write = prepare_create(&prepare(PAGE_CREATE, create_input())?, &space(), None)?;
    let success = json!({"id":"102","spaceId":"100","status":"current","title":"Plan draft","version":{"number":1},"_links":{"base":"https://untrusted.invalid"}});
    let observed = write.interpret(&response(200, success.clone()));
    assert_eq!(observed.state, SubmissionState::Confirmed);
    assert_eq!(observed.resources[0].id, "102");
    assert!(observed.error.is_none());
    for status in [201, 202, 204, 429, 500] {
        assert_eq!(
            write.interpret(&response(status, success.clone())).state,
            SubmissionState::Unknown
        );
    }
    for status in [400, 401, 404, 413] {
        assert_eq!(
            write
                .interpret(&response(
                    status,
                    json!({"message":"sensitive provider text"})
                ))
                .state,
            SubmissionState::Failed
        );
    }
    let mut incomplete = success;
    incomplete
        .as_object_mut()
        .ok_or("object")?
        .remove("version");
    let observed = write.interpret(&response(200, incomplete));
    assert_eq!(observed.state, SubmissionState::Unknown);
    assert_eq!(observed.resources[0].id, "102");
    let update = prepare_update(&prepare(PAGE_UPDATE, update_input()?)?, &page())?;
    let observed = update.interpret(&response(409, json!({})));
    assert_eq!(observed.state, SubmissionState::Failed);
    assert_eq!(
        observed.error.ok_or("conflict")?.code,
        craft_core::error::ErrorCode::VersionConflict
    );
    let mut updated = page();
    updated.body["title"] = json!("Reviewed title");
    updated.body["version"]["number"] = json!(5);
    assert_eq!(update.interpret(&updated).state, SubmissionState::Confirmed);
    updated.body["id"] = json!("999");
    let wrong = update.interpret(&updated);
    assert_eq!(wrong.state, SubmissionState::Unknown);
    assert!(wrong.resources.is_empty());
    Ok(())
}

#[test]
fn write_contracts_match_the_pinned_native_inventory() -> TestResult {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../openspec/changes/build-cloud-cli/provider-operations.json");
    let inventory: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let r = registry()?;
    for command in [PAGE_CREATE, PAGE_UPDATE] {
        let d = r.descriptor(command)?;
        r.validate(command, &d.example)?;
        assert!(jsonschema::validator_for(&d.response_schema)?.is_valid(&d.response_example));
        let native = inventory["commands"]
            .as_array()
            .ok_or("commands")?
            .iter()
            .find(|v| v["command"] == command)
            .ok_or("command")?;
        let route = &d.routes[0];
        let variant = native["variants"]
            .as_array()
            .ok_or("variants")?
            .iter()
            .find(|v| {
                v["route_shape"]["path"] == serde_json::to_value(&route.path).unwrap_or(Value::Null)
            })
            .ok_or("route")?;
        assert_eq!(
            variant["method"],
            if command == PAGE_CREATE {
                "POST"
            } else {
                "PUT"
            }
        );
        let scope = variant["oauth_scope_alternatives"]
            .as_array()
            .ok_or("scopes")?
            .iter()
            .find(|v| v["state"] == "Current")
            .ok_or("scope")?;
        assert_eq!(
            scope["scopes"],
            serde_json::to_value(&d.scope_alternatives[0])?
        );
    }
    Ok(())
}

#[test]
fn update_requires_explicit_editor_draft_policy_bound_into_digest_but_never_sent() -> TestResult {
    for policy in [
        None,
        Some(json!(null)),
        Some(json!("preserve")),
        Some(json!(true)),
    ] {
        let mut input = update_input()?;
        match policy {
            None => {
                input
                    .as_object_mut()
                    .ok_or("object")?
                    .remove("editor_draft_policy");
            }
            Some(value) => input["editor_draft_policy"] = value,
        }
        assert!(prepare(PAGE_UPDATE, input).is_err());
    }
    let op = prepare(PAGE_UPDATE, update_input()?)?;
    let write = prepare_update(&op, &page())?;
    let body = write.call().body.clone().ok_or("body")?;
    assert!(body.get("editor_draft_policy").is_none());
    assert!(!body.to_string().contains("native_reconciliation"));
    let mut other = update_input()?;
    other.as_object_mut().ok_or("object")?.remove("title");
    other["body"] = json!({"representation":"storage","value":"<p>x</p>"});
    assert_ne!(prepare(PAGE_UPDATE, other)?.digest(), op.digest());
    Ok(())
}

#[test]
fn trash_rechecks_revision_and_sends_one_delete_without_purge() -> TestResult {
    let observed = observe_page(&page(), PageRepresentation::Storage)?;
    let input = json!({"id":"101","space_id":"100","expected":observed.revision()?});
    let op = prepare(PAGE_TRASH, input.clone())?;
    let write = prepare_trash(&op, &page())?;
    assert_eq!(write.call().route, TRASH_ROUTE);
    assert!(write.call().body.is_none());
    assert!(write.call().query.is_empty());
    let done = write.interpret(&response(204, Value::Null));
    assert_eq!(done.state, SubmissionState::Confirmed);
    assert_eq!(done.resources.len(), 1);
    assert_eq!(done.resources[0].id, "101");
    for (status, state) in [
        (404, SubmissionState::Failed),
        (400, SubmissionState::Failed),
        (200, SubmissionState::Unknown),
        (500, SubmissionState::Unknown),
        (429, SubmissionState::Unknown),
    ] {
        let observed = write.interpret(&response(status, json!({})));
        assert_eq!(observed.state, state, "{status}");
        assert!(observed.resources.is_empty(), "{status}");
    }
    let mut changed = page();
    changed.body["version"]["number"] = json!(5);
    assert!(prepare_trash(&op, &changed).is_err());
    let mut stale = input;
    stale["expected"]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    assert!(prepare_trash(&prepare(PAGE_TRASH, stale)?, &page()).is_err());
    assert!(prepare_update(&op, &page()).is_err());
    Ok(())
}

fn native_comment(id: &str, version: u64) -> NativeResponse {
    response(
        200,
        json!({"id":id,"pageId":"101","status":"current","version":{"number":version,"authorId":"reviewer-one","createdAt":"2026-11-10T00:00:00.000Z"},"body":{"storage":{"representation":"storage","value":"<p>Looks good</p>"}}}),
    )
}
fn comment_revision(version: u64) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(json!(
        craft_provider_confluence::comments::observe_revision(
            &native_comment("201", version).body,
            "201",
            "101",
            "storage"
        )?
    ))
}

#[test]
fn comment_writes_bind_page_space_parent_and_reviewed_revision() -> TestResult {
    let body = json!({"representation":"storage","value":"<p>Agreed</p>"});
    let create = prepare(
        COMMENT_CREATE,
        json!({"space_id":"100","page_id":"101","body":body}),
    )?;
    let write = prepare_comment_create(&create, &page(), None)?;
    assert_eq!(write.call().route, COMMENT_CREATE_ROUTE);
    assert_eq!(write.call().body, Some(json!({"pageId":"101","body":body})));
    let created = write.interpret(&response(
        201,
        json!({"id":"301","pageId":"101","status":"current","version":{"number":1}}),
    ));
    assert_eq!(created.state, SubmissionState::Confirmed);
    assert_eq!(created.resources[0].resource_type, "comment");
    assert_eq!(created.resources[0].id, "301");
    let foreign = write.interpret(&response(
        201,
        json!({"id":"301","pageId":"999","status":"current","version":{"number":1}}),
    ));
    assert_eq!(foreign.state, SubmissionState::Unknown);
    assert!(foreign.resources.is_empty());
    let mut other_space = page();
    other_space.body["spaceId"] = json!("200");
    assert!(prepare_comment_create(&create, &other_space, None).is_err());

    let reply = prepare(
        COMMENT_CREATE,
        json!({"space_id":"100","page_id":"101","parent_comment_id":"201","body":body}),
    )?;
    assert!(prepare_comment_create(&reply, &page(), None).is_err());
    let write = prepare_comment_create(&reply, &page(), Some(&native_comment("201", 2)))?;
    assert_eq!(
        write.call().body,
        Some(json!({"parentCommentId":"201","body":body}))
    );
    let mut elsewhere = native_comment("201", 2);
    elsewhere.body["pageId"] = json!("102");
    assert!(prepare_comment_create(&reply, &page(), Some(&elsewhere)).is_err());

    let update = prepare(
        COMMENT_UPDATE,
        json!({"space_id":"100","page_id":"101","id":"201","expected":comment_revision(2)?,"body":body}),
    )?;
    let write = prepare_comment_update(&update, &page(), &native_comment("201", 2))?;
    assert_eq!(
        write.call().body,
        Some(json!({"version":{"number":3},"body":body}))
    );
    assert_eq!(
        write
            .interpret(&response(
                200,
                json!({"id":"201","pageId":"101","status":"current","version":{"number":3}})
            ))
            .state,
        SubmissionState::Confirmed
    );
    assert_eq!(
        write.interpret(&response(409, json!({}))).state,
        SubmissionState::Failed
    );
    assert!(prepare_comment_update(&update, &page(), &native_comment("201", 3)).is_err());

    let delete = prepare(
        COMMENT_DELETE,
        json!({"space_id":"100","page_id":"101","id":"201","expected":comment_revision(2)?}),
    )?;
    assert_eq!(
        delete.effect(),
        craft_core::contracts::EffectClass::Permanent
    );
    let write = prepare_comment_delete(&delete, &page(), &native_comment("201", 2))?;
    assert!(write.call().body.is_none());
    let deleted = write.interpret(&response(204, Value::Null));
    assert_eq!(deleted.state, SubmissionState::Confirmed);
    assert_eq!(deleted.resources[0].resource_type, "comment");
    assert_eq!(
        write.interpret(&response(200, json!({}))).state,
        SubmissionState::Unknown
    );
    assert!(prepare_comment_delete(&delete, &page(), &native_comment("201", 3)).is_err());
    for bad in [
        json!({"space_id":"100","page_id":"0101","body":body}),
        json!({"space_id":"100","page_id":"101","parent_comment_id":null,"body":body,"extra":1}),
        json!({"space_id":"100","page_id":"101","body":{"representation":"storage","value":" "}}),
    ] {
        assert!(prepare(COMMENT_CREATE, bad).is_err());
    }
    Ok(())
}

#[test]
fn attachment_trash_compiles_one_delete_bound_to_version_and_file() -> TestResult {
    let raw = json!({"id":"att301","pageId":"101","status":"current","title":"design.png","mediaType":"image/png","fileSize":2048,"fileId":"file-1","version":{"number":2}});
    let op = prepare(
        ATTACHMENT_TRASH,
        json!({"space_id":"100","page_id":"101","id":"att301","expected":{"version":2,"file_id":"file-1"}}),
    )?;
    assert_eq!(op.effect(), craft_core::contracts::EffectClass::Trash);
    let write = prepare_attachment_trash(&op, &page(), &response(200, raw.clone()))?;
    assert_eq!(write.call().route, ATTACHMENT_TRASH_ROUTE);
    assert!(write.call().query.is_empty());
    let done = write.interpret(&response(204, Value::Null));
    assert_eq!(done.state, SubmissionState::Confirmed);
    assert_eq!(done.resources[0].resource_type, "attachment");
    let mut moved = raw.clone();
    moved["pageId"] = json!("102");
    assert!(prepare_attachment_trash(&op, &page(), &response(200, moved)).is_err());
    let mut replaced = raw;
    replaced["fileId"] = json!("file-2");
    assert!(prepare_attachment_trash(&op, &page(), &response(200, replaced)).is_err());
    for bad in ["301", "att0301", "attx"] {
        assert!(
            prepare(
                ATTACHMENT_TRASH,
                json!({"space_id":"100","page_id":"101","id":bad,"expected":{"version":2,"file_id":"file-1"}}),
            )
            .is_err(),
            "{bad}"
        );
    }
    Ok(())
}

#[test]
fn space_create_requires_an_empty_exact_key_lookup_and_confirms_key_and_name() -> TestResult {
    let input =
        json!({"key":"CRAFT1","name":"Craft plans","visibility":"private","description":"Plans"});
    let op = prepare(SPACE_CREATE, input)?;
    let empty = response(200, json!({"results":[],"_links":{}}));
    let write = prepare_space_create(&op, &empty)?;
    assert_eq!(
        write.call().body,
        Some(
            json!({"name":"Craft plans","key":"CRAFT1","createPrivateSpace":true,"description":{"value":"Plans","representation":"plain"}})
        )
    );
    let done = write.interpret(&response(
        201,
        json!({"id":"300","key":"CRAFT1","name":"Craft plans"}),
    ));
    assert_eq!(done.state, SubmissionState::Confirmed);
    assert_eq!(done.resources[0].resource_type, "space");
    let renamed = write.interpret(&response(
        201,
        json!({"id":"300","key":"CRAFT1","name":"Other"}),
    ));
    assert_eq!(renamed.state, SubmissionState::Unknown);
    assert_eq!(renamed.resources.len(), 1);
    let other_key = write.interpret(&response(
        201,
        json!({"id":"300","key":"OTHER","name":"Craft plans"}),
    ));
    assert!(other_key.resources.is_empty());
    assert_eq!(
        write.interpret(&response(400, json!({}))).state,
        SubmissionState::Failed
    );
    assert_eq!(
        write.interpret(&response(200, json!({}))).state,
        SubmissionState::Unknown
    );
    let taken = response(200, json!({"results":[{"id":"9","key":"CRAFT1"}]}));
    assert!(prepare_space_create(&op, &taken).is_err());
    assert!(prepare_space_create(&op, &response(500, json!({}))).is_err());
    for bad in [
        json!({"key":"PLAN-X","name":"x","visibility":"private"}),
        json!({"key":"CRAFT1","name":"x"}),
        json!({"key":"CRAFT1","name":"x","visibility":"public"}),
        json!({"key":"CRAFT1","name":" ","visibility":"private"}),
    ] {
        assert!(prepare(SPACE_CREATE, bad).is_err());
    }
    Ok(())
}

fn folder(id: &str, status: &str, version: u64) -> Value {
    json!({"id":id,"type":"folder","status":status,"title":"Plans","spaceId":"100","parentId":"101","parentType":"page","position":1270,"version":{"number":version,"createdAt":"2026-01-01T00:00:00.000Z"},"createdAt":1794791026125u64})
}

#[test]
fn folder_and_whiteboard_create_bind_space_and_parent_then_confirm_type_title_and_parent()
-> TestResult {
    for (command, kind) in [(FOLDER_CREATE, "folder"), (WHITEBOARD_CREATE, "whiteboard")] {
        let root = prepare(
            command,
            json!({"space_id":"100","title":"Plans","placement":{"kind":"root"}}),
        )?;
        let write = prepare_content_create(&root, &space(), None)?;
        assert_eq!(
            write.call().body,
            Some(json!({"spaceId":"100","title":"Plans"}))
        );
        assert!(write.call().query.is_empty());
        let created = json!({"id":"401","type":kind,"status":"current","title":"Plans","spaceId":"100","parentId":"101","parentType":"page","version":{"number":1}});
        for status in [200, 201] {
            let done = write.interpret(&response(status, created.clone()));
            assert_eq!(done.state, SubmissionState::Confirmed, "{command} {status}");
            assert_eq!(done.resources[0].resource_type, kind);
            assert_eq!(done.resources[0].id, "401");
        }
        let mut other_type = created.clone();
        other_type["type"] = json!("page");
        let lost = write.interpret(&response(200, other_type));
        assert_eq!(lost.state, SubmissionState::Unknown);
        assert!(lost.resources.is_empty());
        let mut retitled = created.clone();
        retitled["title"] = json!("Other");
        let kept = write.interpret(&response(200, retitled));
        assert_eq!(kept.state, SubmissionState::Unknown);
        assert_eq!(kept.resources.len(), 1);
        let mut elsewhere = created.clone();
        elsewhere["spaceId"] = json!("999");
        assert!(
            write
                .interpret(&response(200, elsewhere))
                .resources
                .is_empty()
        );
        assert_eq!(
            write.interpret(&response(400, json!({}))).state,
            SubmissionState::Failed
        );
        assert_eq!(
            write.interpret(&response(500, json!({}))).state,
            SubmissionState::Unknown
        );
        // Root placement never takes parent evidence.
        assert!(prepare_content_create(&root, &space(), Some(&page())).is_err());

        let under_page = prepare(
            command,
            json!({"space_id":"100","title":"Plans","placement":{"kind":"page","id":"101"}}),
        )?;
        let write = prepare_content_create(&under_page, &space(), Some(&page()))?;
        assert_eq!(
            write.call().body,
            Some(json!({"spaceId":"100","title":"Plans","parentId":"101"}))
        );
        assert_eq!(
            write.interpret(&response(200, created.clone())).state,
            SubmissionState::Confirmed
        );
        let mut reparented = created.clone();
        reparented["parentId"] = json!("102");
        assert_eq!(
            write.interpret(&response(200, reparented)).state,
            SubmissionState::Unknown
        );
        assert!(prepare_content_create(&under_page, &space(), None).is_err());
        let mut foreign = page();
        foreign.body["spaceId"] = json!("200");
        assert!(prepare_content_create(&under_page, &space(), Some(&foreign)).is_err());

        let under_folder = prepare(
            command,
            json!({"space_id":"100","title":"Plans","placement":{"kind":"folder","id":"400"}}),
        )?;
        let parent = response(200, folder("400", "current", 3));
        assert!(prepare_content_create(&under_folder, &space(), Some(&parent)).is_ok());
        let trashed = response(200, folder("400", "trashed", 3));
        assert!(prepare_content_create(&under_folder, &space(), Some(&trashed)).is_err());
        // A page cannot stand in for a folder parent.
        assert!(prepare_content_create(&under_folder, &space(), Some(&page())).is_err());

        for bad in [
            json!({"space_id":"100","title":" ","placement":{"kind":"root"}}),
            json!({"space_id":"0100","title":"Plans","placement":{"kind":"root"}}),
            json!({"space_id":"100","title":"Plans"}),
            json!({"space_id":"100","title":"Plans","placement":{"kind":"page","id":"x"}}),
            json!({"space_id":"100","title":"Plans","placement":{"kind":"root"},"template":"kanban-board"}),
        ] {
            assert!(prepare(command, bad).is_err(), "{command}");
        }
    }
    Ok(())
}

#[test]
fn folder_and_whiteboard_trash_recheck_the_reviewed_version_and_send_one_delete() -> TestResult {
    for (command, kind) in [(FOLDER_TRASH, "folder"), (WHITEBOARD_TRASH, "whiteboard")] {
        let op = prepare(
            command,
            json!({"space_id":"100","id":"400","expected":{"version":3}}),
        )?;
        assert_eq!(op.effect(), craft_core::contracts::EffectClass::Trash);
        let mut raw = folder("400", "current", 3);
        raw["type"] = json!(kind);
        let write = prepare_content_trash(&op, &response(200, raw.clone()))?;
        assert!(write.call().query.is_empty());
        assert!(write.call().body.is_none());
        assert_eq!(
            write.call().path_parameters.get("id").map(String::as_str),
            Some("400")
        );
        let done = write.interpret(&response(204, Value::Null));
        assert_eq!(done.state, SubmissionState::Confirmed);
        assert_eq!(done.resources[0].resource_type, kind);
        assert_eq!(
            write.interpret(&response(404, json!({}))).state,
            SubmissionState::Failed
        );
        assert_eq!(
            write.interpret(&response(200, json!({}))).state,
            SubmissionState::Unknown
        );
        let mut newer = raw.clone();
        newer["version"] = json!({"number":4});
        let stale = prepare_content_trash(&op, &response(200, newer));
        assert!(matches!(stale, Err(e) if e.code == craft_core::error::ErrorCode::VersionConflict));
        let mut trashed = raw.clone();
        trashed["status"] = json!("trashed");
        assert!(prepare_content_trash(&op, &response(200, trashed)).is_err());
        let mut moved = raw.clone();
        moved["spaceId"] = json!("200");
        assert!(prepare_content_trash(&op, &response(200, moved)).is_err());
        let mut wrong_type = raw;
        wrong_type["type"] = json!(if kind == "folder" {
            "whiteboard"
        } else {
            "folder"
        });
        assert!(prepare_content_trash(&op, &response(200, wrong_type)).is_err());
        assert!(
            prepare(
                command,
                json!({"space_id":"100","id":"400","expected":{"version":0}})
            )
            .is_err()
        );
    }
    Ok(())
}
