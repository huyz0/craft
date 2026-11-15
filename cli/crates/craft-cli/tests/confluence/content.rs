//! Folder and whiteboard reads and writes through the composed host and mock TLS servers.
use super::*;
use craft_provider_confluence::pages;
use craft_runtime::journal::NativeJournal;

const KINDS: [(&str, &str); 2] = [("folder", "folders"), ("whiteboard", "whiteboards")];

fn native_item(kind: &str, id: &str, version: u64) -> Value {
    json!({"id":id,"type":kind,"status":"current","title":"Plans","spaceId":"100","parentId":null,"parentType":null,"position":1,"authorId":"private-sentinel","ownerId":"private-sentinel","createdAt":"2026-01-01T00:00:00.000Z","version":{"number":version},"_links":{"base":"https://untrusted.invalid"}})
}
fn options(setup: &Setup) -> craft_cli::profiles::InvocationOptions<'_> {
    craft_cli::profiles::InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "content-request",
        max_bytes: 16384,
    }
}
fn commands(kind: &str) -> (&'static str, &'static str, &'static str) {
    if kind == "folder" {
        (
            craft_provider_confluence::content::FOLDER_GET,
            pages::FOLDER_CREATE,
            pages::FOLDER_TRASH,
        )
    } else {
        (
            craft_provider_confluence::content::WHITEBOARD_GET,
            pages::WHITEBOARD_CREATE,
            pages::WHITEBOARD_TRASH,
        )
    }
}

#[test]
fn folder_and_whiteboard_get_check_type_and_omit_private_fields() -> TestResult {
    use craft_cli::confluence_reads::command::{
        ConfluenceGetArgs, ConfluenceReadArgs, PreparedInvocation,
    };
    use craft_runtime::confluence_read_policy::{
        ConfluenceReadOperation as Op, ConfluenceReadPolicyUpdate,
    };
    for (kind, collection) in KINDS {
        for mode in ["get", "wrong_type", "denied", "no_allowance"] {
            let setup = Setup::new()?;
            let allowed = if mode == "no_allowance" {
                vec![Op::PageGet]
            } else {
                vec![Op::FolderGet, Op::WhiteboardGet]
            };
            setup.store.set_confluence_read_policy(
                "factory",
                ConfluenceReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    operations: allowed,
                    enabled: true,
                    expected_revision: None,
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
            let mut raw = native_item(kind, "401", 2);
            if mode == "wrong_type" {
                raw["type"] = json!("page");
            }
            let reply_item = if mode == "denied" {
                Reply::json(403, b"{}", &[])
            } else {
                reply(raw)?
            };
            let issuer =
                issuer("read:confluence-user read:folder:confluence read:whiteboard:confluence")?;
            let product =
                TlsServer::start(Reply::Sequence(vec![identity(), identity(), reply_item]))?;
            let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
            let locked = fixture.lock(&setup.grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let args = ConfluenceGetArgs {
                cloud_id: Some(CLOUD.into()),
                id: "401".into(),
            };
            let args = if kind == "folder" {
                ConfluenceReadArgs::FolderGet(args)
            } else {
                ConfluenceReadArgs::WhiteboardGet(args)
            };
            let prepared =
                PreparedInvocation::prepare(args, &[], &options(&setup), &setup.store, &control()?);
            if mode == "no_allowance" {
                assert!(prepared.is_err(), "{kind}");
                drop(locked);
                continue;
            }
            let result = prepared?.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            assert_eq!(
                result.envelope.ok,
                mode == "get",
                "{kind} {mode}: {:?}",
                result.envelope.error
            );
            let registry = craft_cli::catalog::registry()?;
            assert!(
                jsonschema::validator_for(&registry.descriptor(commands(kind).0)?.response_schema)?
                    .is_valid(&serde_json::to_value(&result.envelope)?),
                "{kind} {mode}"
            );
            let requests = product.finish()?;
            let read = requests.last().ok_or("request")?;
            assert_eq!(read.method, "GET");
            assert!(
                read.target
                    .ends_with(&format!("/wiki/api/v2/{collection}/401")),
                "{}",
                read.target
            );
            if mode == "get" {
                let data = result.envelope.data.clone().ok_or("data")?;
                assert_eq!(data["item"]["id"], "401");
                assert_eq!(data["item"]["space_id"], "100");
                assert_eq!(data["item"]["version"], 2);
                assert!(data["item"]["parent_id"].is_null());
                let text = data.to_string();
                assert!(!text.contains("private-sentinel") && !text.contains("untrusted.invalid"));
            }
            drop(locked);
            setup.no_secrets()?;
        }
    }
    Ok(())
}

fn enable_content_writes(setup: &Setup, all: bool) -> Result<(), CraftError> {
    use craft_runtime::confluence_write_policy::{
        ConfluenceWriteOperation as Op, ConfluenceWritePolicyUpdate,
    };
    setup.store.set_confluence_write_policy(
        "factory",
        ConfluenceWritePolicyUpdate {
            target: ResourceRef {
                resource_type: "space".into(),
                id: "100".into(),
                ..target()
            },
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
            operations: if all {
                vec![
                    Op::FolderCreate,
                    Op::FolderTrash,
                    Op::WhiteboardCreate,
                    Op::WhiteboardTrash,
                ]
            } else {
                vec![Op::Create, Op::Trash]
            },
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}

#[test]
fn folder_and_whiteboard_writes_read_space_parent_and_revision_then_send_once() -> TestResult {
    use craft_cli::confluence_pages::command::{PageWriteArgs, PreparedInvocation};
    for (kind, collection) in KINDS {
        let (_, create, trash) = commands(kind);
        for mode in [
            "create_root",
            "create_in_folder",
            "create_lost",
            "trash",
            "trash_stale",
            "unscoped",
            "no_allowance",
        ] {
            let setup = Setup::new()?;
            enable_content_writes(&setup, mode != "no_allowance")?;
            let scopes = if mode == "unscoped" {
                "read:confluence-user read:space:confluence read:folder:confluence".to_string()
            } else {
                "read:confluence-user read:space:confluence read:page:confluence read:folder:confluence write:folder:confluence delete:folder:confluence read:whiteboard:confluence write:whiteboard:confluence delete:whiteboard:confluence".to_string()
            };
            let issuer = issuer(&scopes)?;
            let creating = mode.starts_with("create");
            let mut replies = vec![identity(), identity()];
            let mut created = native_item(kind, "402", 1);
            if creating && mode != "unscoped" {
                replies.push(reply(space())?);
                if mode == "create_in_folder" {
                    replies.push(reply(native_item("folder", "400", 5))?);
                    created["parentId"] = json!("400");
                    created["parentType"] = json!("folder");
                }
                replies.push(if mode == "create_lost" {
                    Reply::Disconnect
                } else {
                    reply(created)?
                });
            }
            if mode == "trash" || mode == "trash_stale" {
                replies.push(reply(native_item(
                    kind,
                    "401",
                    if mode == "trash_stale" { 3 } else { 2 },
                ))?);
                if mode == "trash" {
                    replies.push(Reply::json(204, b"", &[]));
                }
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
            let locked = fixture.lock(&setup.grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let journal = NativeJournal::open(&setup.store, &control()?)?;
            let (command, mut input) = if creating || mode == "unscoped" || mode == "no_allowance" {
                (
                    create,
                    json!({"space_id":"100","title":"Plans","placement":if mode == "create_in_folder" {json!({"kind":"folder","id":"400"})} else {json!({"kind":"root"})}}),
                )
            } else {
                (
                    trash,
                    json!({"space_id":"100","id":"401","expected":{"version":2}}),
                )
            };
            input["operation_id"] = json!("content-write");
            input["apply"] = json!(true);
            let prepared = PreparedInvocation::prepare(
                PageWriteArgs::decode(command, input)?,
                &[],
                &options(&setup),
                &setup.store,
                &control()?,
            );
            if mode == "no_allowance" {
                assert_eq!(
                    prepared.err().ok_or("expected denial")?.code,
                    ErrorCode::PolicyDenied
                );
                drop(locked);
                continue;
            }
            let result = prepared?.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                &journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            let data = result.envelope.data.clone().ok_or("data")?;
            assert_eq!(data["replay_authorized"], false, "{kind} {mode}");
            let requests = product.finish()?;
            let sends: Vec<_> = requests
                .iter()
                .filter(|r| r.method == "POST" || r.method == "DELETE")
                .filter(|r| r.target.contains("/wiki/api/v2/"))
                .collect();
            match mode {
                "create_root" | "create_in_folder" => {
                    assert!(
                        result.envelope.ok,
                        "{kind} {mode}: {:?}",
                        result.envelope.error
                    );
                    assert_eq!(data["receipt"]["request_state"], "confirmed");
                    let resource = &data["receipt"]["confirmed_resources"][0];
                    assert_eq!(resource["resource_type"], kind);
                    assert_eq!(resource["id"], "402");
                    assert_eq!(sends.len(), 1);
                    assert!(
                        sends[0]
                            .target
                            .ends_with(&format!("/wiki/api/v2/{collection}"))
                    );
                    let body: Value = serde_json::from_slice(&sends[0].body)?;
                    let expected = if mode == "create_in_folder" {
                        json!({"spaceId":"100","title":"Plans","parentId":"400"})
                    } else {
                        json!({"spaceId":"100","title":"Plans"})
                    };
                    assert_eq!(body, expected);
                    if mode == "create_in_folder" {
                        assert!(
                            requests.iter().any(|r| r.method == "GET"
                                && r.target.ends_with("/wiki/api/v2/folders/400"))
                        );
                    }
                }
                "create_lost" => {
                    assert_eq!(data["receipt"]["request_state"], "sent_unknown");
                    assert_eq!(data["reconciliation_required"], true);
                    assert_eq!(sends.len(), 1);
                }
                "trash" => {
                    assert!(
                        result.envelope.ok,
                        "{kind} {mode}: {:?}",
                        result.envelope.error
                    );
                    assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "401");
                    assert_eq!(
                        data["receipt"]["confirmed_resources"][0]["resource_type"],
                        kind
                    );
                    assert_eq!(sends.len(), 1);
                    assert_eq!(sends[0].method, "DELETE");
                    assert!(
                        sends[0]
                            .target
                            .ends_with(&format!("/wiki/api/v2/{collection}/401"))
                    );
                    assert!(!sends[0].target.contains("purge"));
                }
                "trash_stale" => {
                    assert_eq!(
                        result.envelope.error.ok_or("err")?.code,
                        ErrorCode::VersionConflict
                    );
                    assert_eq!(data["receipt"]["request_state"], "not_sent");
                    assert!(sends.is_empty());
                }
                _ => {
                    assert!(!result.envelope.ok);
                    assert_eq!(data["receipt"]["request_state"], "not_sent");
                    assert!(sends.is_empty());
                }
            }
            drop(locked);
            setup.no_secrets()?;
        }
    }
    Ok(())
}
