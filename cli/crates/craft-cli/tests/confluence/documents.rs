use super::*;
use craft_cli::planning_read::{self, DocumentGetArgs, PageFormat, PreparedDocumentRead};
use craft_workflows::plans::*;

#[test]
fn remote_document_reads_import_current_human_edits_and_retain_renewal_on_ambiguity() -> TestResult
{
    for mode in [
        "plan_storage",
        "plan_adf",
        "design",
        "unsupported",
        "wrong_space",
    ] {
        let setup = Setup::new()?;
        enable_reads(&setup)?;
        let destination = DocumentTarget {
            confluence_cloud_id: CLOUD.into(),
            space_id: if mode == "wrong_space" { "200" } else { "100" }.into(),
            parent_page_id: None,
        };
        let (command, document) = if mode == "design" {
            let mut design = Design::scaffold(
                "ledger-design".into(),
                "Old heading".into(),
                DesignKind::Hld,
                destination,
                true,
            )?;
            for section in &mut design.sections {
                section.body = RichText::Markdown(format!("Human revised {}.", section.title));
            }
            (planning_read::DESIGN_GET, design.render()?)
        } else {
            let mut plan = Plan::scaffold(
                "ledger-plan".into(),
                "Old heading".into(),
                PlanTarget {
                    confluence: destination,
                    jira_cloud_id: CLOUD.into(),
                    project_key: "PAY".into(),
                },
            )?;
            plan.context = RichText::Markdown("Human revised the shared context.".into());
            plan.review_state = ReviewState::Approved;
            plan.tasks.push(PlanTask {
                task_id: "contract".into(),
                title: "Human task title".into(),
                jira: JiraMapping {
                    project_key: "PAY".into(),
                    issue_type: "Task".into(),
                    existing_issue: None,
                },
                description: RichText::Markdown("Visible remote description.".into()),
                acceptance: vec![RichText::Markdown("Human changed the acceptance.".into())],
                repositories: vec!["api".into()],
                depends_on: vec![],
                designs: vec![],
                include_in_jira: true,
                estimate: None,
            });
            (planning_read::PLAN_GET, plan.render()?)
        };
        let adf = mode == "plan_adf";
        let representation = if adf { "atlas_doc_format" } else { "storage" };
        let mut body = if adf {
            serde_json::to_string(&document.to_adf()?)?
        } else {
            document.to_storage()?
        };
        if mode == "unsupported" {
            body.push_str("<ac:structured-macro ac:name=\"unsupported\" />");
        }
        let mut raw = page(101);
        raw["title"] = json!("Human page title");
        raw["body"] = json!({representation:{"representation":representation,"value":body}});
        let issuer = issuer(SCOPES)?;
        let product = TlsServer::start(Reply::Sequence(vec![identity(), identity(), reply(raw)?]))?;
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
        let registry = craft_cli::catalog::registry()?;
        let options = craft_cli::profiles::InvocationOptions {
            state_root: Some(&setup.root),
            profile: Some("factory"),
            request_id: "remote-document-request",
            max_bytes: 16384,
        };
        let selected = vec!["document".into(), "task_order".into()];
        let workflow = PreparedDocumentRead::prepare(
            &registry,
            command,
            DocumentGetArgs {
                page: craft_cli::confluence_reads::command::ConfluenceGetArgs {
                    cloud_id: Some(CLOUD.into()),
                    id: "101".into(),
                },
                format: if adf {
                    PageFormat::Adf
                } else {
                    PageFormat::Storage
                },
            },
            &selected,
            &options,
        )?;
        let native = craft_cli::confluence_reads::command::PreparedInvocation::prepare(
            workflow.native_args(),
            &workflow.native_fields(),
            &planning_read::native_options(&options),
            &setup.store,
            &control()?,
        )?
        .execute_cached(
            &setup.store,
            fixture.client(),
            &locked,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control()?,
        );
        let result = workflow.finish(native, &options, &control()?);
        assert_eq!(
            result.envelope.ok,
            !matches!(mode, "unsupported" | "wrong_space"),
            "{mode}: {:?}",
            result.envelope.error
        );
        let value = serde_json::to_value(&result.envelope)?;
        let schema = registry
            .projection(command, &selected)?
            .ok_or("projection")?
            .response_schema(&registry.descriptor(command)?.response_schema)?;
        assert!(
            jsonschema::validator_for(&schema)?.is_valid(&value),
            "{mode}: {value}"
        );
        let data = result.envelope.data.ok_or("data")?;
        assert_eq!(
            data["credential_cache_update"]["write_state"], "confirmed",
            "{mode}: {data}"
        );
        assert_eq!(data["review_verified"], false);
        assert_eq!(data["tickets_created"], 0);
        assert_eq!(data["source"]["revision"]["version"], 3);
        assert_eq!(data["source"]["page"]["id"], "101");
        if matches!(mode, "unsupported" | "wrong_space") {
            assert!(data["document"].is_null());
            assert_eq!(data["valid"], false);
        } else {
            assert_eq!(data["document"]["title"], "Human page title");
            assert!(
                data["source"]["document_digest"]
                    .as_str()
                    .is_some_and(|d| d.starts_with("sha256:"))
            );
            if mode != "design" {
                assert_eq!(data["document"]["review_state"], "approved");
                assert_eq!(data["task_order"], json!(["contract"]));
                assert!(
                    serde_json::to_string(&data["document"]["tasks"][0]["acceptance"])?
                        .contains("Human changed the acceptance")
                );
            }
        }
        let requests = product.finish()?;
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|r| r.method == "GET"));
        assert!(requests[2].target.contains("/wiki/api/v2/pages/101"));
        assert!(requests[2].target.contains("get-draft=false"));
        assert!(
            requests[2]
                .target
                .contains(&format!("body-format={representation}"))
        );
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        setup.no_secrets()?;
    }
    Ok(())
}
