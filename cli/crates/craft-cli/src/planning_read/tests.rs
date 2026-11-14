use super::*;
use craft_core::control::{CancellationToken, InvocationControl};
use craft_workflows::plans::*;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(craft_test_support::state::ManualClock::new(1000)),
        CancellationToken::default(),
        60_000,
    )
}
fn options(bytes: usize) -> InvocationOptions<'static> {
    InvocationOptions {
        state_root: None,
        profile: None,
        request_id: "remote-document-request",
        max_bytes: bytes,
    }
}
fn args() -> DocumentGetArgs {
    DocumentGetArgs {
        page: ConfluenceGetArgs {
            cloud_id: Some(CLOUD.into()),
            id: "101".into(),
        },
        format: PageFormat::Storage,
    }
}
fn plan() -> Result<Plan, CraftError> {
    let mut p = Plan::scaffold(
        "invoice-plan".into(),
        "Original heading".into(),
        PlanTarget {
            confluence: DocumentTarget {
                confluence_cloud_id: CLOUD.into(),
                space_id: "100".into(),
                parent_page_id: None,
            },
            jira_cloud_id: CLOUD.into(),
            project_key: "PAY".into(),
        },
    )?;
    p.context = RichText::Markdown("Human changes drive the next preview.".into());
    p.review_state = ReviewState::Approved;
    p.tasks.push(PlanTask {
        task_id: "contract".into(),
        title: "Human task title".into(),
        jira: JiraMapping {
            project_key: "PAY".into(),
            issue_type: "Task".into(),
            existing_issue: None,
        },
        description: RichText::Markdown("Remote body, not an original file.".into()),
        acceptance: vec![RichText::Markdown("Human revised the acceptance.".into())],
        repositories: vec!["api".into()],
        depends_on: vec![],
        designs: vec![],
        include_in_jira: true,
        estimate: None,
    });
    Ok(p)
}
fn native(body: &str) -> CommandResult {
    CommandResult {
        envelope: Envelope::success(
            PAGE_GET,
            "remote-document-request".into(),
            json!({"target":{"provider":"confluence","deployment":"cloud","tenant":CLOUD,"resource_type":"site","id":CLOUD},"item":{"id":"101","space_id":"100","title":"Human page title","status":"current","body_storage":body,"revision":{"version":7,"digest":format!("sha256:{}","a".repeat(64)),"representation":"storage"}},"observed_at":1000,"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000001"},"write_state":"confirmed","cleanup_pending":true}}),
        ),
        output_policy: crate::output::OutputPolicy::PreserveEvidence,
    }
}
fn fields(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).into()).collect()
}
fn valid_schema(
    registry: &Registry,
    command: &str,
    fields: &[String],
    envelope: &Envelope,
) -> Result<(), Box<dyn std::error::Error>> {
    let projection = registry.projection(command, fields)?.ok_or("projection")?;
    let schema = projection.response_schema(&registry.descriptor(command)?.response_schema)?;
    let value = serde_json::to_value(envelope)?;
    assert!(
        jsonschema::validator_for(&schema)?.is_valid(&value),
        "{value}"
    );
    Ok(())
}
#[test]
fn remote_import_preserves_human_edits_and_native_title_without_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = crate::catalog::registry()?;
    let select = fields(&[
        "document.tasks.acceptance",
        "document.title",
        "document.review_state",
        "task_order",
    ]);
    let body = plan()?.render()?.to_storage()?;
    let result =
        PreparedDocumentRead::prepare(&registry, PLAN_GET, args(), &select, &options(16384))?
            .finish(native(&body), &options(16384), &control()?);
    assert!(result.envelope.ok, "{:?}", result.envelope.error);
    let data = result.envelope.data.as_ref().ok_or("data")?;
    assert_eq!(data["document"]["title"], "Human page title");
    assert_eq!(data["document"]["review_state"], "approved");
    assert_eq!(data["review_verified"], false);
    assert_eq!(data["tickets_created"], 0);
    assert_eq!(data["task_order"], json!(["contract"]));
    assert!(!data["document"]["tasks"][0]["acceptance"].is_null());
    assert!(data["document"]["tasks"][0].get("description").is_none());
    assert_eq!(data["source"]["revision"]["version"], 7);
    let mut expected = plan()?;
    expected.title = "Human page title".into();
    assert_eq!(data["source"]["document_digest"], expected.digest()?);
    valid_schema(&registry, PLAN_GET, &select, &result.envelope)?;
    Ok(())
}
#[test]
fn remote_import_errors_budget_and_cancellation_retain_cache_and_never_return_partial_document()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = crate::catalog::registry()?;
    for mode in [
        "unsupported",
        "ambiguous",
        "target",
        "identity",
        "budget",
        "cancelled",
        "native_error",
    ] {
        let mut p = plan()?;
        if mode == "target" {
            p.target.confluence.space_id = "200".into();
        }
        if mode == "budget" {
            p.tasks[0].description = RichText::Markdown("Human text ".repeat(2000));
        }
        let mut body = p.render()?.to_storage()?;
        if mode == "unsupported" {
            body.push_str("<ac:structured-macro ac:name=\"unknown\" />");
        }
        if mode == "ambiguous" {
            body = body.replace("<p>contract</p>", "<p>ambiguous</p>");
            body.push_str("<h2>Unknown boundary</h2>");
        }
        let mut native = native(&body);
        if mode == "identity" {
            native.envelope.data.as_mut().ok_or("data")?["item"]["id"] = json!("999");
        }
        if mode == "native_error" {
            native.envelope.ok = false;
            native.envelope.error = Some(CraftError::new(
                ErrorCode::RateLimited,
                "Native rate advice.",
            ));
        }
        let select = fields(&["document"]);
        let opts = options(if mode == "budget" { 4096 } else { 16384 });
        let cancellation = CancellationToken::default();
        let c = InvocationControl::new(
            Arc::new(craft_test_support::state::ManualClock::new(1000)),
            cancellation.clone(),
            60_000,
        )?;
        if mode == "cancelled" {
            cancellation.cancel();
        }
        let result = PreparedDocumentRead::prepare(&registry, PLAN_GET, args(), &select, &opts)?
            .finish(native, &opts, &c);
        assert!(!result.envelope.ok, "{mode}");
        let data = result.envelope.data.as_ref().ok_or("data")?;
        assert_eq!(data["valid"], false, "{mode}");
        assert!(data["document"].is_null(), "{mode}");
        assert_eq!(
            data["credential_cache_update"]["cleanup_pending"], true,
            "{mode}"
        );
        assert!(fits(&result.envelope, opts.max_bytes), "{mode}");
        valid_schema(&registry, PLAN_GET, &select, &result.envelope)?;
    }
    Ok(())
}
