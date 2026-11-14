use super::*;
use craft_provider_confluence::comments::COMMENT_GET;

#[test]
fn comment_collections_bind_native_headers_parent_proof_and_independent_allowances() -> TestResult {
    use craft_provider_confluence::comments::paging::{COMMENT_LIST, COMMENT_REPLIES};
    use craft_runtime::confluence_read_policy::{
        ConfluenceReadOperation as Op, ConfluenceReadPolicyUpdate,
    };
    for mode in [
        "header",
        "body",
        "terminal",
        "conflict",
        "replies",
        "wrong_parent",
        "legacy",
    ] {
        let setup = Setup::new()?;
        let is_reply = matches!(mode, "replies" | "wrong_parent");
        let command = if is_reply {
            COMMENT_REPLIES
        } else {
            COMMENT_LIST
        };
        setup.store.set_confluence_read_policy(
            "factory",
            ConfluenceReadPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                operations: vec![if mode == "legacy" {
                    Op::CommentGet
                } else if is_reply {
                    Op::CommentReplies
                } else {
                    Op::CommentList
                }],
                enabled: true,
                expected_revision: None,
            },
            true,
            &control()?,
            |_| Ok(()),
        )?;
        let raw = |id: &str| json!({"id":id,"pageId":"101","status":"current","version":{"number":1,"authorId":"reviewer-account","createdAt":"2026-11-10T00:00:00.000Z"}});
        let link = "/wiki/api/v2/pages/101/footer-comments?limit=25&sort=created-date&status=current&cursor=opaque%2Fnext";
        let mut replies = vec![identity()];
        if mode != "legacy" {
            replies.push(identity());
            if is_reply {
                replies.push(reply(raw("201"))?);
                let mut child = raw("202");
                child.as_object_mut().ok_or("child")?.remove("pageId");
                child["parentCommentId"] =
                    json!(if mode == "wrong_parent" { "999" } else { "201" });
                replies.push(reply(json!({"results":[child],"_links":{}}))?);
                if mode == "replies" {
                    replies.push(reply(raw("201"))?);
                }
            } else {
                let body = json!({"results":[raw("201")],"_links":if mode=="body"{json!({"next":link})}else if mode=="conflict"{json!({"next":link.replace("opaque%2Fnext","different")})}else{json!({})}});
                replies.push(Reply::json(
                    200,
                    &serde_json::to_vec(&body)?,
                    &if matches!(mode, "header" | "conflict") {
                        vec![("Link", format!("<{link}>; rel=\"next\""))]
                    } else {
                        vec![]
                    }
                    .iter()
                    .map(|(k, v)| (*k, v.as_str()))
                    .collect::<Vec<_>>(),
                ));
                if mode == "header" {
                    replies.push(identity());
                    replies.push(reply(json!({"results":[raw("202")],"_links":{}}))?);
                }
            }
        }
        let issuer = issuer("read:confluence-user read:comment:confluence")?;
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3600))?;
        let input = if is_reply {
            json!({"page_id":"101","parent_comment_id":"201"})
        } else {
            json!({"page_id":"101"})
        };
        let invocation = candidate(&setup, command, input.clone(), &[], None, 1, 16384);
        if mode == "legacy" {
            assert_eq!(
                invocation.err().ok_or("policy refusal")?.code,
                ErrorCode::PolicyDenied
            );
        } else {
            let result = invocation?.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            assert_eq!(
                result.envelope.ok,
                !matches!(mode, "conflict" | "wrong_parent"),
                "{mode}: {:?}",
                result.envelope.error
            );
            assert_eq!(
                result.envelope.data.as_ref().ok_or("data")?["credential_cache_update"]["write_state"],
                "confirmed"
            );
            if result.envelope.ok {
                assert_eq!(
                    result.envelope.data.as_ref().ok_or("data")?["items"][0]["page_id"],
                    "101"
                );
                let paging = result.envelope.page.ok_or("page")?;
                assert_eq!(paging.has_more, matches!(mode, "header" | "body"));
                if mode == "header" {
                    let cursor = paging.next_cursor.ok_or("cursor")?;
                    let next = candidate(&setup, command, input, &[], Some(&cursor), 1, 16384)?
                        .execute_cached(
                            &setup.store,
                            fixture.client(),
                            &locked,
                            TlsPolicy::AdditionalCaBundle(product.root()),
                            control()?,
                        );
                    assert!(next.envelope.ok, "{:?}", next.envelope.error);
                    assert_eq!(next.envelope.data.ok_or("data")?["items"][0]["id"], "202");
                    assert!(!next.envelope.page.ok_or("page")?.has_more);
                }
            }
        }
        let requests = product.finish()?;
        assert_eq!(
            requests.len(),
            match mode {
                "legacy" => 1,
                "header" | "replies" => 5,
                "wrong_parent" => 4,
                _ => 3,
            },
            "{mode}"
        );
        if mode == "header" {
            assert!(requests[4].target.contains("cursor=opaque%2Fnext"));
        }
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert_eq!(issuer.finish()?.len(), if mode == "legacy" { 1 } else { 2 });
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}

#[test]
fn page_bound_comment_reads_use_explicit_policy_and_preserve_renewal_on_refusal() -> TestResult {
    for mode in [
        "body",
        "revision",
        "adf",
        "wrong_page",
        "missing_author",
        "old_policy",
        "scope",
    ] {
        let setup = Setup::new()?;
        if mode == "old_policy" {
            enable_reads(&setup)?;
        } else {
            setup.store.set_confluence_read_policy(
                "factory",
                craft_runtime::confluence_read_policy::ConfluenceReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    operations: vec![
                        craft_runtime::confluence_read_policy::ConfluenceReadOperation::CommentGet,
                    ],
                    enabled: true,
                    expected_revision: None,
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
        }
        let mut raw = json!({"id":"201","pageId":"101","status":"current","version":{"number":2,"authorId":"reviewer-account","createdAt":"2026-11-10T00:00:00.000Z"},"body":{"storage":{"representation":"storage","value":"<p>Visible review text</p>"},"atlas_doc_format":{"representation":"atlas_doc_format","value":"{\"version\":1,\"type\":\"doc\",\"content\":[]}"}}});
        if mode == "wrong_page" {
            raw["pageId"] = json!("999");
        }
        if mode == "missing_author" {
            raw["version"]["authorId"] = Value::Null;
        }
        let scopes = if mode == "scope" {
            "read:confluence-user read:page:confluence"
        } else {
            "read:confluence-user read:comment:confluence"
        };
        let issuer = issuer(scopes)?;
        let mut replies = vec![identity()];
        if mode != "old_policy" {
            replies.push(identity());
            if mode != "scope" {
                replies.push(reply(raw)?);
            }
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3600))?;
        let fields = match mode {
            "revision" => vec!["revision"],
            "adf" => vec!["body_atlas_doc_format", "revision"],
            _ => vec!["body_storage", "revision"],
        };
        let options = craft_cli::profiles::InvocationOptions {
            state_root: Some(&setup.root),
            profile: Some("factory"),
            request_id: "comment-request",
            max_bytes: 16384,
        };
        let args = craft_cli::confluence_reads::command::ConfluenceReadArgs::CommentGet(
            craft_cli::confluence_reads::command::ConfluenceCommentGetArgs {
                cloud_id: Some(CLOUD.into()),
                id: "201".into(),
                page_id: "101".into(),
            },
        );
        let invocation = craft_cli::confluence_reads::command::PreparedInvocation::prepare(
            args,
            &fields.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            &options,
            &setup.store,
            &control()?,
        );
        if mode == "old_policy" {
            assert_eq!(
                invocation.err().ok_or("policy refusal")?.code,
                ErrorCode::PolicyDenied
            );
            assert_eq!(product.finish()?.len(), 1);
            assert_eq!(issuer.finish()?.len(), 1);
        } else {
            let result = invocation?.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            let success = matches!(mode, "body" | "revision" | "adf");
            assert_eq!(
                result.envelope.ok, success,
                "{mode}: {:?}",
                result.envelope.error
            );
            let registry = craft_cli::catalog::registry()?;
            let projection = registry
                .projection(
                    COMMENT_GET,
                    &fields.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
                )?
                .ok_or("projection")?;
            let schema =
                projection.response_schema(&registry.descriptor(COMMENT_GET)?.response_schema)?;
            assert!(
                jsonschema::validator_for(&schema)?
                    .is_valid(&serde_json::to_value(&result.envelope)?),
                "{mode}"
            );
            let data = result.envelope.data.ok_or("data")?;
            assert_eq!(
                data["credential_cache_update"]["write_state"], "confirmed",
                "{mode}"
            );
            if success {
                assert_eq!(data["item"]["page_id"], "101");
                assert_eq!(data["item"]["version_author_id"], "reviewer-account");
                assert_eq!(data["item"]["revision"]["version"], 2);
                if mode == "revision" {
                    assert!(data["item"].get("body_storage").is_none());
                }
            } else {
                assert!(data["item"].is_null());
            }
            let requests = product.finish()?;
            assert_eq!(requests.len(), if mode == "scope" { 2 } else { 3 });
            assert!(requests.iter().all(|r| r.method == "GET"));
            if mode != "scope" {
                assert!(
                    requests[2]
                        .target
                        .contains("/wiki/api/v2/footer-comments/201")
                );
                assert!(requests[2].target.contains("include-version=true"));
                assert!(!requests[2].target.contains("?version="));
            }
            assert_eq!(issuer.finish()?.len(), 2);
        }
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
