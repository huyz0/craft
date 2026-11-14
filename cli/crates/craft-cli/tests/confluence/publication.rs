use super::*;
use craft_cli::confluence_pages::{PageWritePolicy, PreparedPagePublication};
use craft_core::storage::OperationJournal;
use craft_provider_confluence::pages::{self, PAGE_CREATE, PAGE_UPDATE, PageRepresentation};
use craft_runtime::journal::NativeJournal;
const WRITE_SCOPES: &str =
    "read:confluence-user read:space:confluence read:page:confluence write:page:confluence";

struct PolicyClock {
    base: ManualClock,
    root: PathBuf,
    count: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
    claimed: bool,
    keep_enabled: bool,
    armed: std::sync::atomic::AtomicBool,
    changed: std::sync::atomic::AtomicBool,
}
impl Clock for PolicyClock {
    fn unix_millis(&self) -> u64 {
        if self.armed.load(Ordering::SeqCst)
            && !self.changed.load(Ordering::SeqCst)
            && self.count.load(Ordering::SeqCst) >= self.threshold
        {
            let result = (|| -> Result<bool, CraftError> {
                let c = control()?;
                let store = ProfileStore::open(&self.root, &c)?;
                if self.claimed {
                    let journal = NativeJournal::open(&store, &c).map_err(|_| {
                        CraftError::new(ErrorCode::Internal, "Fixture journal failed.")
                    })?;
                    if !journal
                        .get("publish-plan")
                        .map_err(|_| {
                            CraftError::new(ErrorCode::Internal, "Fixture journal failed.")
                        })?
                        .is_some_and(|r| {
                            r.steps.values().any(|s| {
                                s.state == craft_core::storage::SubmissionState::SendStarted
                            })
                        })
                    {
                        return Ok(false);
                    }
                }
                use craft_runtime::confluence_write_policy::{
                    ConfluenceWriteOperation as Op, ConfluenceWritePolicyUpdate,
                };
                store.set_confluence_write_policy(
                    "factory",
                    ConfluenceWritePolicyUpdate {
                        target: ResourceRef {
                            resource_type: "space".into(),
                            id: "100".into(),
                            ..target()
                        },
                        expected_actor: "actor-one".into(),
                        enabled: self.keep_enabled,
                        operations: vec![Op::Create, Op::Update],
                        expected_revision: Some(1),
                    },
                    true,
                    &c,
                    |_| Ok(()),
                )?;
                Ok(true)
            })();
            if matches!(result, Ok(true)) {
                self.changed.store(true, Ordering::SeqCst);
            }
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn captured_page_write_policy_stops_revocation_and_allowed_revision_change_before_send()
-> TestResult {
    for mode in [
        "before_auth",
        "after_identity",
        "after_read",
        "changed_allowed",
        "claimed",
    ] {
        let setup = Setup::new()?;
        enable_writes(&setup, None, true)?;
        let issuer = issuer(WRITE_SCOPES)?;
        let mut replies = vec![identity()];
        if mode != "before_auth" {
            replies.push(identity());
        }
        if matches!(mode, "after_read" | "changed_allowed" | "claimed") {
            replies.push(reply(space())?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(PolicyClock {
            base: ManualClock::new(1000),
            root: setup.root.clone(),
            count: product.request_counter(),
            threshold: if mode == "after_identity" { 2 } else { 3 },
            claimed: mode == "claimed",
            keep_enabled: mode == "changed_allowed",
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
        let invocation = candidate(&setup, PAGE_CREATE, create_input(json!({"kind":"root"})))?;
        if mode == "before_auth" {
            enable_writes(&setup, Some(1), false)?;
        } else {
            clock.armed.store(true, Ordering::SeqCst);
        }
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        let result = invocation.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            &journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(!result.envelope.ok, "{mode}");
        assert_eq!(
            result.envelope.error.ok_or("error")?.code,
            if mode == "changed_allowed" {
                ErrorCode::VersionConflict
            } else {
                ErrorCode::PolicyDenied
            }
        );
        assert_eq!(
            result.envelope.data.ok_or("data")?["receipt"]["request_state"],
            if mode == "claimed" {
                "sent_unknown"
            } else {
                "not_sent"
            }
        );
        assert_eq!(journal.get("publish-plan")?.is_some(), mode == "claimed");
        if mode != "before_auth" {
            assert!(clock.changed.load(Ordering::SeqCst), "{mode}");
        }
        let requests = product.finish()?;
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert_eq!(
            requests.len(),
            if mode == "before_auth" {
                1
            } else if mode == "after_identity" {
                2
            } else {
                3
            }
        );
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}

fn create_input(placement: Value) -> Value {
    json!({"space_id":"100","title":"Plan draft","placement":placement,"body":{"representation":"storage","value":"<p>Discuss together 文字</p>"}})
}

struct PublicationSource(PathBuf);
impl Drop for PublicationSource {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn complete_plan_and_design_publication_send_only_shared_pages_and_retain_unknowns() -> TestResult {
    use craft_cli::planning_publication::{self, DocumentPublicationArgs};
    use craft_workflows::plans::*;
    for (kind, format, lost) in [
        ("plan", "json", false),
        ("plan", "storage", false),
        ("plan", "adf", true),
        ("design", "json", false),
    ] {
        let setup = Setup::new()?;
        enable_writes(&setup, None, true)?;
        let document_target = DocumentTarget {
            confluence_cloud_id: CLOUD.into(),
            space_id: "100".into(),
            parent_page_id: None,
        };
        let (command, bytes, expected) = if kind == "plan" {
            let mut plan = Plan::scaffold(
                "ledger-plan".into(),
                "Plan draft".into(),
                PlanTarget {
                    confluence: document_target,
                    jira_cloud_id: CLOUD.into(),
                    project_key: "PAY".into(),
                },
            )?;
            plan.context = RichText::Markdown("Preserve **human context** 文字.".into());
            for (id, dependencies) in [
                ("contract", vec![]),
                ("implementation", vec!["contract".into()]),
            ] {
                plan.tasks.push(PlanTask {
                    task_id: id.into(),
                    title: format!("Define {id}"),
                    jira: JiraMapping {
                        project_key: "PAY".into(),
                        issue_type: "Task".into(),
                        existing_issue: None,
                    },
                    description: RichText::Markdown("Keep the visible task details.".into()),
                    acceptance: vec![RichText::Markdown(
                        "Human can review the complete graph.".into(),
                    )],
                    repositories: vec!["api".into(), "worker".into()],
                    depends_on: dependencies,
                    designs: vec![],
                    include_in_jira: true,
                    estimate: None,
                });
            }
            let document = plan.render()?;
            let bytes = match format {
                "storage" => document.to_storage()?.into_bytes(),
                "adf" => serde_json::to_vec(&document.to_adf()?)?,
                _ => serde_json::to_vec(&plan)?,
            };
            (planning_publication::PLAN_CREATE, bytes, plan.digest()?)
        } else {
            let mut design = Design::scaffold(
                "ledger-design".into(),
                "Plan draft".into(),
                DesignKind::Hld,
                document_target,
                true,
            )?;
            for section in &mut design.sections {
                section.body =
                    RichText::Markdown(format!("Explain {} across api and worker.", section.title));
            }
            (
                planning_publication::DESIGN_CREATE,
                serde_json::to_vec(&design)?,
                design.digest()?,
            )
        };
        let source = PublicationSource(std::env::current_dir()?.join(format!(
                ".craft-publication-{}-{}.json",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            )));
        use std::io::Write;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&source.0)?
            .write_all(&bytes)?;
        let options = craft_cli::profiles::InvocationOptions {
            state_root: Some(&setup.root),
            profile: Some("factory"),
            request_id: "publication-request",
            max_bytes: 16384,
        };
        let args = planning_publication::prepare(
            command,
            DocumentPublicationArgs {
                source: craft_cli::args::DocumentFileArgs {
                    file: source.0.to_str().ok_or("source path")?.into(),
                    source_format: match format {
                        "storage" => craft_cli::args::DocumentSourceFormat::Storage,
                        "adf" => craft_cli::args::DocumentSourceFormat::Adf,
                        _ => craft_cli::args::DocumentSourceFormat::Json,
                    },
                },
                operation_id: "publish-plan".into(),
                apply: true,
            },
            &[],
            &options,
            &control()?,
        )?;
        let prepared = craft_cli::confluence_pages::command::PreparedInvocation::prepare(
            args,
            &[],
            &options,
            &setup.store,
            &control()?,
        )?;
        let issuer = issuer(WRITE_SCOPES)?;
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(space())?,
            if lost {
                Reply::Disconnect
            } else {
                reply(
                    json!({"id":"102","spaceId":"100","title":"Plan draft","status":"current","version":{"number":1}}),
                )?
            },
        ]))?;
        let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        let result = prepared.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            &journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert_eq!(result.envelope.ok, !lost, "{:?}", result.envelope.error);
        let data = result.envelope.data.ok_or("receipt")?;
        assert_eq!(
            data["receipt"]["request_state"],
            if lost { "sent_unknown" } else { "confirmed" }
        );
        assert_eq!(data["replay_authorized"], false);
        let binding = journal.get("publish-plan")?.ok_or("journal")?.binding;
        assert_eq!(binding.target, target());
        assert_eq!(binding.actor, "actor-one");
        assert_eq!(binding.grant, setup.grant);
        let requests = product.finish()?;
        assert_eq!(requests.len(), 4);
        assert!(
            requests
                .iter()
                .all(|r| !r.target.contains("/rest/api/3/issue"))
        );
        let sent = requests.last().ok_or("page write")?;
        assert_eq!(sent.method, "POST");
        assert!(sent.target.contains("/wiki/api/v2/pages"));
        let body: Value = serde_json::from_slice(&sent.body)?;
        assert_eq!(body["status"], "current");
        assert_eq!(body["spaceId"], "100");
        let document = craft_workflows::documents::Document::from_storage(
            body["body"]["value"].as_str().ok_or("visible content")?,
        )?;
        if kind == "plan" {
            let imported = Plan::from_document(&document)?;
            assert_eq!(imported.digest()?, expected);
            assert_eq!(imported.tasks[1].depends_on, vec!["contract"]);
            assert!(matches!(imported.review_state, ReviewState::Proposed));
        } else {
            assert_eq!(Design::from_document(&document)?.digest()?, expected);
        }
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
fn current_page() -> Value {
    page(101)
}
fn update_input() -> Result<Value, CraftError> {
    let observed = pages::observe_page(&native(current_page()), PageRepresentation::Storage)?;
    Ok(
        json!({"id":"101","space_id":"100","expected":observed.revision()?,"editor_draft_policy":"native_reconciliation_may_overwrite","title":"Plan draft"}),
    )
}
fn native(body: Value) -> craft_core::execution::NativeResponse {
    craft_core::execution::NativeResponse {
        status: 200,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
fn prepared_write(
    grant: &GrantRef,
    command: &str,
    input: Value,
) -> Result<PreparedPagePublication, CraftError> {
    PreparedPagePublication::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input,
            selection: Selection::default(),
            preconditions: None,
        },
        "publish-plan",
        true,
        "publication-request",
        PageWritePolicy {
            target: target(),
            space_id: "100".into(),
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            allow_create: true,
            allow_update: true,
        },
    )
}

#[test]
fn actual_page_post_and_put_preserve_content_and_record_without_replay() -> TestResult {
    for mode in ["root", "homepage", "parent", "title", "body"] {
        let setup = Setup::new()?;
        enable_writes(&setup, None, true)?;
        let issuer = issuer(WRITE_SCOPES)?;
        let update = matches!(mode, "title" | "body");
        let command = if update { PAGE_UPDATE } else { PAGE_CREATE };
        let mut input = if update {
            update_input()?
        } else {
            create_input(if mode == "parent" {
                json!({"kind":"parent","id":"101"})
            } else {
                json!({"kind":mode})
            })
        };
        if mode == "body" {
            input.as_object_mut().ok_or("input")?.remove("title");
            input["body"] = json!({"representation":"storage","value":"<p>Reviewed body</p>"});
        }
        let mut success = json!({"id":if update {"101"} else {"102"},"spaceId":"100","title":if mode=="body" {"Plan 101 文字"} else {"Plan draft"},"status":"current","version":{"number":if update {4} else {1}}});
        success["_links"] = json!({"base":"https://untrusted.invalid"});
        let mut replies = vec![identity(), identity()];
        if update {
            replies.push(reply(current_page())?);
        } else {
            let mut s = space();
            s["homepageId"] = json!("101");
            replies.push(reply(s)?);
            if mode != "root" {
                replies.push(reply(current_page())?);
            }
        }
        replies.push(reply(success)?);
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
        let result = candidate(&setup, command, input.clone())?.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            &journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert!(result.envelope.ok, "{mode}: {:?}", result.envelope.error);
        assert!(
            jsonschema::validator_for(&craft_cli::confluence_pages::schema()?)?
                .is_valid(&serde_json::to_value(&result.envelope)?)
        );
        let data = result.envelope.data.ok_or("data")?;
        assert_eq!(data["receipt"]["request_state"], "confirmed");
        assert_eq!(data["persistence"], "recorded");
        assert_eq!(data["replay_authorized"], false);
        let count = product.request_count();
        let repeated = candidate(&setup, command, input)?.execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            &journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        assert_eq!(
            repeated.envelope.error.ok_or("replay")?.code,
            ErrorCode::OperationIdReused
        );
        assert_eq!(
            repeated.envelope.data.ok_or("receipt")?["receipt"]["request_state"],
            "confirmed"
        );
        assert_eq!(product.request_count(), count);
        let requests = product.finish()?;
        let sent = requests.last().ok_or("write")?;
        assert_eq!(sent.method, if update { "PUT" } else { "POST" });
        let body: Value = serde_json::from_slice(&sent.body)?;
        assert_eq!(body["status"], "current");
        for key in ["ownerId", "subtype", "private", "embedded"] {
            assert!(body.get(key).is_none());
        }
        if update {
            assert_eq!(body["version"]["number"], 4);
            assert!(body.get("spaceId").is_none());
            assert!(body.get("parentId").is_none());
            assert_eq!(
                body["body"]["value"],
                if mode == "body" {
                    "<p>Reviewed body</p>"
                } else {
                    "<p>Plan</p>"
                }
            );
            assert!(requests[2].target.contains("get-draft=false"));
            assert!(requests[2].target.contains("body-format=storage"));
        } else {
            assert_eq!(body["spaceId"], "100");
        }
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}

#[test]
fn lost_refused_rate_and_incomplete_success_keep_native_and_cache_evidence() -> TestResult {
    for mode in ["lost", "rate", "refused", "undocumented", "incomplete"] {
        let setup = Setup::new()?;
        let issuer = issuer(WRITE_SCOPES)?;
        let status = match mode {
            "rate" => 429,
            "refused" => 400,
            "undocumented" => 201,
            _ => 200,
        };
        let write_reply = if mode == "lost" {
            Reply::Disconnect
        } else {
            Reply::json(
                status,
                br#"{"id":"102","spaceId":"100","status":"current","title":"Plan draft"}"#,
                &[("Retry-After", "7")],
            )
        };
        let product = TlsServer::start(Reply::Sequence(vec![
            identity(),
            identity(),
            reply(space())?,
            write_reply,
        ]))?;
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
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        let input = create_input(json!({"kind":"root"}));
        let result = prepared_write(&setup.grant, PAGE_CREATE, input.clone())?
            .execute_cached_with_policy_check(
                fixture.client(),
                &locked,
                &journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
                || Ok(()),
            );
        assert!(!result.envelope.ok, "{mode}");
        let data = result.envelope.data.ok_or("data")?;
        assert_eq!(
            data["receipt"]["request_state"],
            if mode == "refused" {
                "failed"
            } else {
                "sent_unknown"
            }
        );
        assert_eq!(data["persistence"], "recorded");
        assert_eq!(data["credential_cache_update"]["write_state"], "confirmed");
        assert_eq!(data["replay_authorized"], false);
        if mode == "incomplete" {
            assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "102");
        }
        if mode == "rate" {
            assert_eq!(data["retry_after"]["value"], 7);
        }
        let count = product.request_count();
        let repeated = prepared_write(&setup.grant, PAGE_CREATE, input)?
            .execute_cached_with_policy_check(
                fixture.client(),
                &locked,
                &journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
                || Ok(()),
            );
        assert_eq!(
            repeated.envelope.error.ok_or("replay")?.code,
            ErrorCode::OperationIdReused
        );
        assert_eq!(product.request_count(), count);
        assert_eq!(product.finish()?.len(), 4);
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}

#[test]
fn policy_scope_revision_and_claimed_refusal_prevent_the_native_write() -> TestResult {
    for mode in [
        "before_auth",
        "scope",
        "after_identity",
        "after_read",
        "changed_page",
        "claimed",
        "expired",
    ] {
        let setup = Setup::new()?;
        let issuer = issuer(if mode == "scope" {
            "read:confluence-user"
        } else {
            WRITE_SCOPES
        })?;
        let mut replies = vec![identity()];
        if mode != "before_auth" {
            replies.push(identity());
        }
        if matches!(mode, "after_read" | "claimed" | "expired") {
            replies.push(reply(space())?);
        }
        if mode == "changed_page" {
            let mut p = current_page();
            p["version"]["number"] = json!(4);
            replies.push(reply(p)?);
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
        let journal = NativeJournal::open(&setup.store, &control()?)?;
        let count = product.request_counter();
        let (command, input) = if mode == "changed_page" {
            (PAGE_UPDATE, update_input()?)
        } else {
            (PAGE_CREATE, create_input(json!({"kind":"root"})))
        };
        let expired = std::sync::atomic::AtomicBool::new(false);
        let result = prepared_write(&setup.grant, command, input)?
            .execute_cached_with_policy_check(
                fixture.client(),
                &locked,
                &journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control()?,
                || {
                    let n = count.load(Ordering::SeqCst);
                    if mode == "expired" && n >= 3 && !expired.swap(true, Ordering::SeqCst) {
                        clock.advance(Duration::from_secs(30)).map_err(|_| {
                            CraftError::new(ErrorCode::Internal, "Clock fixture failed.")
                        })?;
                    }
                    let claimed = mode == "claimed"
                        && journal
                            .get("publish-plan")
                            .map_err(|_| {
                                CraftError::new(ErrorCode::Internal, "Journal fixture failed.")
                            })?
                            .is_some_and(|r| {
                                r.steps.values().any(|s| {
                                    s.state != craft_core::storage::SubmissionState::Prepared
                                })
                            });
                    if mode == "before_auth"
                        || (mode == "after_identity" && n >= 2)
                        || (mode == "after_read" && n >= 3)
                        || claimed
                    {
                        return Err(CraftError::new(
                            ErrorCode::PolicyDenied,
                            "Publication authority revoked.",
                        ));
                    }
                    Ok(())
                },
            );
        assert!(!result.envelope.ok, "{mode}");
        let data = result.envelope.data.ok_or("data")?;
        assert_eq!(
            data["receipt"]["request_state"],
            if mode == "claimed" {
                "sent_unknown"
            } else {
                "not_sent"
            }
        );
        let saved = journal.get("publish-plan")?;
        assert_eq!(saved.is_some(), mode == "claimed");
        let requests = product.finish()?;
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert_eq!(
            requests.len(),
            if mode == "before_auth" {
                1
            } else if matches!(mode, "scope" | "after_identity") {
                2
            } else {
                3
            }
        );
        assert_eq!(issuer.finish()?.len(), 1);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}

fn enable_writes(setup: &Setup, revision: Option<u64>, enabled: bool) -> Result<(), CraftError> {
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
            enabled,
            expected_revision: revision,
            operations: vec![Op::Create, Op::Update],
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn candidate(
    setup: &Setup,
    command: &str,
    mut input: Value,
) -> Result<craft_cli::confluence_pages::command::PreparedInvocation, CraftError> {
    use craft_cli::confluence_pages::command::{PageWriteArgs, PreparedInvocation};
    input["operation_id"] = json!("publish-plan");
    input["apply"] = json!(true);
    if command == PAGE_UPDATE {
        let mut changes = serde_json::Map::new();
        for key in ["title", "body"] {
            if let Some(v) = input.as_object_mut().and_then(|m| m.remove(key)) {
                changes.insert(key.into(), v);
            }
        }
        input["changes"] = Value::Object(changes);
    }
    PreparedInvocation::prepare(
        PageWriteArgs::decode(command, input)?,
        &[],
        &craft_cli::profiles::InvocationOptions {
            state_root: Some(&setup.root),
            profile: Some("factory"),
            request_id: "publication-request",
            max_bytes: 16384,
        },
        &setup.store,
        &control()?,
    )
}
