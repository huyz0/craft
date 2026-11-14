use super::*;
use craft_cli::confluence_reads::inventory::PreparedInventory;
use craft_runtime::confluence_read_policy::{
    ConfluenceReadOperation as Op, ConfluenceReadPolicyUpdate,
};
use craft_workflows::review::Coverage;
fn review_page() -> ResourceRef {
    ResourceRef {
        resource_type: "page".into(),
        id: "101".into(),
        ..target()
    }
}
fn enable_inventory(setup: &Setup) -> Result<(), CraftError> {
    setup.store.set_confluence_read_policy(
        "factory",
        ConfluenceReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![Op::CommentList, Op::CommentReplies],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn node(id: &str, parent: Option<&str>) -> Value {
    json!({"id":id,"pageId":"101","parentCommentId":parent,"status":"current","version":{"number":1,"authorId":"reviewer-account","createdAt":"2026-11-10T00:00:00.000Z"},"body":{"storage":{"representation":"storage","value":"<p>Visible review</p>"}}})
}
fn children(
    replies: &mut Vec<Reply>,
    parent: Value,
    mut items: Vec<Value>,
    next: Option<&str>,
) -> TestResult {
    for child in &mut items {
        child.as_object_mut().ok_or("child")?.remove("pageId");
    }
    replies.push(reply(parent.clone())?);
    replies.push(reply(json!({"results":items,"_links":next.map(|s|json!({"next":s})).unwrap_or_else(||json!({}))}))?);
    replies.push(reply(parent)?);
    Ok(())
}
#[test]
fn native_inventory_exhausts_root_reply_and_nested_leaf_pages_without_partial_success() -> TestResult
{
    for mode in ["complete", "empty", "gap", "duplicate", "cursor_cycle"] {
        let setup = Setup::new()?;
        enable_inventory(&setup)?;
        let mut replies = vec![identity(), identity()];
        if mode == "complete" {
            let link = "/wiki/api/v2/pages/101/footer-comments?limit=25&sort=created-date&status=current&body-format=storage&cursor=opaque%2Froot";
            replies.push(Reply::json(
                200,
                &serde_json::to_vec(&json!({"results":[node("201",None)],"_links":{}}))?,
                &[("Link", &format!("<{link}>; rel=next"))],
            ));
            replies.push(reply(json!({"results":[node("202",None)],"_links":{}}))?);
            children(
                &mut replies,
                node("201", None),
                vec![node("203", Some("201"))],
                Some(
                    "/wiki/api/v2/footer-comments/201/children?limit=25&sort=created-date&body-format=storage&cursor=opaque%2Freply",
                ),
            )?;
            children(
                &mut replies,
                node("201", None),
                vec![node("204", Some("201"))],
                None,
            )?;
            children(&mut replies, node("202", None), vec![], None)?;
            children(
                &mut replies,
                node("203", Some("201")),
                vec![node("205", Some("203"))],
                None,
            )?;
            children(&mut replies, node("204", Some("201")), vec![], None)?;
            children(&mut replies, node("205", Some("203")), vec![], None)?;
        } else if mode == "empty" {
            replies.push(reply(json!({"results":[],"_links":{}}))?);
        } else if mode == "cursor_cycle" {
            for cursor in ["a", "b", "a"] {
                replies.push(reply(json!({"results":[],"_links":{"next":format!("/wiki/api/v2/pages/101/footer-comments?limit=25&sort=created-date&status=current&body-format=storage&cursor={cursor}")}}))?);
            }
        } else {
            replies.push(reply(json!({"results":if mode=="gap"{vec![node("201",None)]}else{vec![node("201",None),node("202",None)]},"_links":{}}))?);
            if mode == "gap" {
                replies.push(reply(node("201", None))?);
                replies.push(Reply::json(403, b"{}", &[]));
            } else {
                children(
                    &mut replies,
                    node("201", None),
                    vec![node("202", Some("201"))],
                    None,
                )?;
            }
        }
        let expected_requests = replies.len();
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let issuer = issuer("read:confluence-user read:comment:confluence")?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let prepared = PreparedInventory::prepare(
            review_page(),
            "factory",
            "review-inventory",
            &setup.store,
            &control()?,
        )?;
        let locked = fixture.lock(prepared.grant(), &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3600))?;
        let result = prepared.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert_eq!(result.execution_actor.as_deref(), Some("actor-one"));
        assert_eq!(
            result
                .credential_cache_update
                .ok_or("cache receipt")?
                .write_state,
            craft_core::contracts::RequestState::Confirmed
        );
        if matches!(mode, "complete" | "empty") {
            let inventory = result.inventory?;
            assert!(matches!(inventory.coverage, Coverage::Complete));
            assert_eq!(
                inventory.comments.len(),
                if mode == "complete" { 5 } else { 0 }
            );
            if mode == "complete" {
                assert_eq!(
                    inventory
                        .comments
                        .iter()
                        .map(|c| c.id.as_str())
                        .collect::<Vec<_>>(),
                    vec!["201", "202", "203", "204", "205"]
                );
            }
            assert!(inventory.comments.iter().all(|c| c.page == review_page()));
        } else {
            assert!(result.inventory.is_err(), "{mode}");
        }
        let requests = product.finish()?;
        assert_eq!(requests.len(), expected_requests, "{mode}");
        if mode == "complete" {
            assert!(
                requests
                    .iter()
                    .any(|r| r.target.contains("cursor=opaque%2Froot"))
            );
            assert!(
                requests
                    .iter()
                    .any(|r| r.target.contains("cursor=opaque%2Freply"))
            );
        }
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
#[test]
fn inventory_requires_both_operations_and_rechecks_policy_after_identity_and_native_reads()
-> TestResult {
    for mode in [
        "missing_replies",
        "before_auth",
        "after_identity",
        "after_root",
        "scope",
    ] {
        let setup = Setup::new()?;
        if mode == "missing_replies" {
            setup.store.set_confluence_read_policy(
                "factory",
                ConfluenceReadPolicyUpdate {
                    target: target(),
                    expected_actor: "actor-one".into(),
                    operations: vec![Op::CommentList],
                    enabled: true,
                    expected_revision: None,
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
        } else {
            enable_inventory(&setup)?;
        }
        let mut replies = vec![identity()];
        if matches!(mode, "after_identity" | "after_root" | "scope") {
            replies.push(identity());
        }
        if mode == "after_root" {
            replies.push(reply(json!({"results":[],"_links":{}}))?);
        }
        let expected_requests = replies.len();
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let issuer = issuer(if mode == "scope" {
            "read:confluence-user read:page:confluence"
        } else {
            "read:confluence-user read:comment:confluence"
        })?;
        let clock = Arc::new(CapturedPolicyClock {
            base: ManualClock::new(1000),
            root: setup.root.clone(),
            counter: product.request_counter(),
            threshold: if mode == "after_identity" { 2 } else { 3 },
            delay: 0,
            hits: std::sync::atomic::AtomicUsize::new(0),
            armed: std::sync::atomic::AtomicBool::new(false),
            changed: std::sync::atomic::AtomicBool::new(false),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.base.advance(Duration::from_secs(3600))?;
        let prepared = PreparedInventory::prepare(
            review_page(),
            "factory",
            "inventory-policy",
            &setup.store,
            &control()?,
        );
        if mode == "missing_replies" {
            assert_eq!(
                prepared.err().ok_or("allowance refusal")?.code,
                ErrorCode::PolicyDenied
            );
        } else {
            if mode == "before_auth" {
                setup.store.set_confluence_read_policy(
                    "factory",
                    ConfluenceReadPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        operations: vec![Op::CommentList, Op::CommentReplies],
                        enabled: false,
                        expected_revision: Some(1),
                    },
                    true,
                    &control()?,
                    |_| Ok(()),
                )?;
            }
            if matches!(mode, "after_identity" | "after_root") {
                clock.armed.store(true, Ordering::SeqCst);
            }
            let result = prepared?.execute_cached(
                &setup.store,
                fixture.client(),
                &locked,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
            );
            assert!(result.inventory.is_err(), "{mode}");
            assert_eq!(
                result.credential_cache_update.is_some(),
                mode != "before_auth",
                "{mode}"
            );
            if let Some(cache) = result.credential_cache_update {
                assert_eq!(
                    cache.write_state,
                    craft_core::contracts::RequestState::Confirmed
                );
            }
            if matches!(mode, "after_identity" | "after_root") {
                assert!(clock.changed.load(Ordering::SeqCst));
            }
        }
        assert_eq!(product.finish()?.len(), expected_requests, "{mode}");
        assert_eq!(
            issuer.finish()?.len(),
            if matches!(mode, "missing_replies" | "before_auth") {
                1
            } else {
                2
            }
        );
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
