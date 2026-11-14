use std::path::Path;
use std::process::Output;

use craft_test_support::process::{RunOptions, run};
use craft_workflows::plans::*;
use serde_json::{Value, json};

fn invoke(args: &[&str], source: Option<&[u8]>) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(run(
        Path::new(env!("CARGO_BIN_EXE_craft")),
        args,
        source,
        RunOptions {
            timeout: std::time::Duration::from_secs(30),
            ..Default::default()
        },
    )?)
}
fn body(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn target() -> PlanTarget {
    PlanTarget {
        confluence: DocumentTarget {
            confluence_cloud_id: "00000000-1111-2222-3333-444444444444".into(),
            space_id: "1".into(),
            parent_page_id: None,
        },
        jira_cloud_id: "00000000-1111-2222-3333-444444444444".into(),
        project_key: "PAY".into(),
    }
}
fn plan() -> Result<Plan, Box<dyn std::error::Error>> {
    let mut plan = Plan::scaffold("invoice-replay".into(), "Invoice replay".into(), target())?;
    plan.context = RichText::Markdown("Prevent repeated ledger entries.".into());
    plan.review_state = ReviewState::Approved;
    plan.tasks.push(PlanTask {
        task_id: "event-identity".into(),
        title: "Define identity".into(),
        jira: JiraMapping {
            project_key: "PAY".into(),
            issue_type: "Task".into(),
            existing_issue: None,
        },
        description: RichText::Markdown(
            "Keep the **identity** stable.\n\n```rust\nlet value = \"🦀 ]]>\";\n```".into(),
        ),
        acceptance: vec![RichText::Markdown("Producer and consumer agree.".into())],
        repositories: vec!["billing-api".into()],
        depends_on: Vec::new(),
        designs: Vec::new(),
        include_in_jira: true,
        estimate: None,
    });
    Ok(plan)
}

fn authority(data: &Value) {
    assert_eq!(data["review_verified"], false);
    assert_eq!(data["live_conditions_checked"], false);
    assert_eq!(data["tickets_created"], 0);
}

#[test]
fn remote_document_input_and_output_controls_refuse_before_state()
-> Result<(), Box<dyn std::error::Error>> {
    for command in ["plan", "design"] {
        for (extra, expected) in [
            (vec!["--id", "bad-id"], "INPUT_INVALID"),
            (vec!["--id", "101", "--fields", "unknown"], "FIELD_UNKNOWN"),
            (
                vec!["--id", "101", "--max-bytes", "512"],
                "OUTPUT_BUDGET_TOO_SMALL",
            ),
        ] {
            let mut args = vec![
                command,
                "page-get",
                "--state-root",
                "unreachable-remote-document-state",
                "--profile",
                "factory",
            ];
            args.extend(extra);
            let result = body(&invoke(&args, None)?)?;
            assert_eq!(result["error"]["code"], expected, "{result}");
            assert!(!Path::new("unreachable-remote-document-state").exists());
        }
    }
    Ok(())
}

#[test]
fn publication_refuses_nonproposed_invalid_and_double_stdin_before_state()
-> Result<(), Box<dyn std::error::Error>> {
    let mut document = plan()?;
    for (state, apply, maximum, expected) in [
        (ReviewState::Approved, true, "16384", "INPUT_INVALID"),
        (ReviewState::InReview, true, "16384", "INPUT_INVALID"),
        (
            ReviewState::ChangesRequested,
            true,
            "16384",
            "INPUT_INVALID",
        ),
        (ReviewState::Proposed, false, "16384", "APPLY_REQUIRED"),
        (
            ReviewState::Proposed,
            true,
            "512",
            "OUTPUT_BUDGET_TOO_SMALL",
        ),
    ] {
        document.review_state = state;
        let mut args = vec![
            "plan",
            "page-create",
            "--file",
            "-",
            "--operation-id",
            "publish-plan",
            "--state-root",
            "unreachable-publication-state",
            "--max-bytes",
            maximum,
        ];
        if apply {
            args.push("--apply");
        }
        let output = invoke(&args, Some(&serde_json::to_vec(&document)?))?;
        let result = body(&output)?;
        assert!(!output.status.success(), "{result}");
        assert_eq!(result["command"], "plan.page-create");
        assert_eq!(result["error"]["code"], expected, "{result}");
        assert!(!Path::new("unreachable-publication-state").exists());
    }
    let output = invoke(
        &["plan", "page-create", "--input", "-"],
        Some(br#"{"file":"-","operation_id":"publish-plan","apply":true}"#),
    )?;
    assert_eq!(
        body(&output)?["error"]["details"]["issues"][0]["rule"],
        "stdin_can_only_be_consumed_once"
    );
    let output = invoke(
        &[
            "plan",
            "page-create",
            "--file",
            "-",
            "--operation-id",
            "publish-plan",
            "--apply",
        ],
        Some(b"{}"),
    )?;
    assert_eq!(body(&output)?["error"]["code"], "INPUT_INVALID");
    Ok(())
}

#[test]
fn planning_scaffolds_are_typed_incomplete_and_have_no_implicit_targets()
-> Result<(), Box<dyn std::error::Error>> {
    for (command, seed) in [
        (
            "plan",
            json!({"plan_id":"invoice-replay","title":"Invoice replay",
                "confluence_cloud_id":target().confluence.confluence_cloud_id,
                "space_id":"1","jira_cloud_id":target().jira_cloud_id,"project_key":"PAY"}),
        ),
        (
            "design",
            json!({"design_id":"invoice-design","title":"Invoice design",
                "confluence_cloud_id":target().confluence.confluence_cloud_id,
                "space_id":"1","compact":true}),
        ),
    ] {
        let output = invoke(
            &[command, "scaffold", "--input", "-"],
            Some(&serde_json::to_vec(&seed)?),
        )?;
        let result = body(&output)?;
        assert!(output.status.success(), "{result}");
        authority(&result["data"]);
        assert_eq!(result["data"]["ready"], false);
        let document = &result["data"]["document"];
        assert_eq!(document["schema_version"], "1.0");
        let output = invoke(
            &[command, "validate", "--file", "-"],
            Some(&serde_json::to_vec(document)?),
        )?;
        assert!(!output.status.success());
        assert_eq!(body(&output)?["error"]["code"], "INPUT_INVALID");
    }
    let output = invoke(
        &[
            "plan",
            "scaffold",
            "--plan-id",
            "example",
            "--title",
            "Example",
        ],
        None,
    )?;
    assert!(!output.status.success());
    assert_eq!(
        body(&output)?["error"]["details"]["issues"][0]["path"],
        "/confluence_cloud_id"
    );
    Ok(())
}

#[test]
fn scaffold_flags_and_json_agree_and_targets_are_required_in_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let cloud = "00000000-1111-2222-3333-444444444444";
    for command in ["plan", "design"] {
        let mut seed = json!({"title":"Invoice work","confluence_cloud_id":cloud,
            "space_id":"1","parent_page_id":"2"});
        let mut flags = vec![
            command,
            "scaffold",
            "--title",
            "Invoice work",
            "--confluence-cloud-id",
            cloud,
            "--space-id",
            "1",
            "--parent-page-id",
            "2",
        ];
        if command == "plan" {
            seed["plan_id"] = json!("invoice-replay");
            seed["jira_cloud_id"] = json!(cloud);
            seed["project_key"] = json!("PAY");
            flags.extend([
                "--plan-id",
                "invoice-replay",
                "--jira-cloud-id",
                cloud,
                "--project-key",
                "PAY",
            ]);
        } else {
            seed["design_id"] = json!("invoice-design");
            seed["kind"] = json!("detailed");
            seed["compact"] = json!(true);
            flags.extend([
                "--design-id",
                "invoice-design",
                "--kind",
                "detailed",
                "--compact",
            ]);
        }
        let flagged = invoke(&flags, None)?;
        let structured = invoke(
            &[command, "scaffold", "--input", "-"],
            Some(&serde_json::to_vec(&seed)?),
        )?;
        assert!(flagged.status.success(), "{}", body(&flagged)?);
        assert!(structured.status.success(), "{}", body(&structured)?);
        assert_eq!(body(&flagged)?["data"], body(&structured)?["data"]);
        let operation = format!("{command}.scaffold");
        let schema_result = invoke(
            &[
                "schema",
                "--command",
                &operation,
                "--view",
                "full",
                "--max-bytes",
                "1048576",
            ],
            None,
        )?;
        let schema_result = body(&schema_result)?;
        let schema = jsonschema::validator_for(&schema_result["data"]["schema"])?;
        assert!(schema.is_valid(&seed));
        let required = if command == "plan" {
            vec![
                "confluence_cloud_id",
                "space_id",
                "jira_cloud_id",
                "project_key",
            ]
        } else {
            vec!["confluence_cloud_id", "space_id"]
        };
        for field in required {
            let mut missing = seed.clone();
            missing.as_object_mut().ok_or("Missing seed")?.remove(field);
            assert!(!schema.is_valid(&missing), "Optional target field: {field}");
            let output = invoke(
                &[command, "scaffold", "--input", "-"],
                Some(&serde_json::to_vec(&missing)?),
            )?;
            assert!(!output.status.success());
            assert_eq!(body(&output)?["error"]["code"], "INPUT_INVALID");
        }
        let mut malformed = seed.clone();
        malformed["confluence_cloud_id"] = json!("z".repeat(36));
        let output = invoke(
            &[command, "scaffold", "--input", "-"],
            Some(&serde_json::to_vec(&malformed)?),
        )?;
        assert!(!output.status.success());
        let error = body(&output)?;
        assert_eq!(
            error["error"]["details"]["issues"][0]["path"],
            "/confluence_cloud_id"
        );
        assert_eq!(
            error["error"]["details"]["issues"][0]["rule"],
            "expected_cloud_uuid"
        );
        seed["unknown"] = json!(true);
        let output = invoke(
            &[command, "scaffold", "--input", "-"],
            Some(&serde_json::to_vec(&seed)?),
        )?;
        assert!(!output.status.success());
        assert_eq!(body(&output)?["error"]["code"], "INPUT_INVALID");
    }
    Ok(())
}

#[test]
fn public_plan_render_import_and_selected_validation_keep_edits_without_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = plan()?;
    let source = serde_json::to_vec(&plan)?;
    let output = invoke(
        &["plan", "render", "--file", "-", "--fields", "storage"],
        Some(&source),
    )?;
    let result = body(&output)?;
    assert!(output.status.success(), "{result}");
    authority(&result["data"]);
    let edited = result["data"]["storage"]
        .as_str()
        .ok_or("Missing storage")?
        .replace("Define identity", "Human-edited title");
    let output = invoke(
        &[
            "plan",
            "validate",
            "--file",
            "-",
            "--source-format",
            "storage",
            "--fields",
            "document.tasks.task_id,document.tasks.title",
        ],
        Some(edited.as_bytes()),
    )?;
    let imported = body(&output)?;
    assert!(output.status.success(), "{imported}");
    authority(&imported["data"]);
    assert_eq!(
        imported["data"]["document"]["tasks"][0]["title"],
        "Human-edited title"
    );
    assert!(
        imported["data"]["document"]["tasks"][0]
            .get("description")
            .is_none()
    );
    let output = invoke(
        &[
            "plan", "render", "--file", "-", "--format", "adf", "--fields", "adf",
        ],
        Some(&source),
    )?;
    let adf = body(&output)?;
    assert!(output.status.success(), "{adf}");
    authority(&adf["data"]);
    let output = invoke(
        &["plan", "validate", "--file", "-", "--source-format", "adf"],
        Some(&serde_json::to_vec(&adf["data"]["adf"])?),
    )?;
    let validated = body(&output)?;
    assert!(output.status.success(), "{validated}");
    authority(&validated["data"]);
    assert_eq!(validated["data"]["digest"], plan.digest()?);
    assert!(validated["data"].get("document").is_none());
    Ok(())
}

#[test]
fn design_render_validation_and_response_schemas_agree() -> Result<(), Box<dyn std::error::Error>> {
    let mut design = Design::scaffold(
        "invoice-design".into(),
        "Invoice design".into(),
        DesignKind::Detailed,
        target().confluence,
        true,
    )?;
    for section in &mut design.sections {
        section.body = RichText::Markdown(format!("Explain {}.", section.title));
    }
    let source = serde_json::to_vec(&design)?;
    for action in ["validate", "render"] {
        let output = invoke(&["design", action, "--file", "-"], Some(&source))?;
        let result = body(&output)?;
        assert!(output.status.success(), "{result}");
        authority(&result["data"]);
        let schema = body(&invoke(
            &[
                "schema",
                "--command",
                &format!("design.{action}"),
                "--response",
                "--view",
                "full",
                "--max-bytes",
                "1048576",
            ],
            None,
        )?)?;
        assert!(jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(&result));
    }
    Ok(())
}

#[test]
fn invalid_plans_stdin_conflicts_context_and_projection_fail_locally()
-> Result<(), Box<dyn std::error::Error>> {
    let mut invalid = plan()?;
    invalid.tasks[0].depends_on = vec!["missing-work".into()];
    let output = invoke(
        &["plan", "validate", "--file", "-"],
        Some(&serde_json::to_vec(&invalid)?),
    )?;
    assert_eq!(body(&output)?["error"]["code"], "INPUT_INVALID");
    for command in ["plan", "design"] {
        for action in ["validate", "render"] {
            let output = invoke(&[command, action, "--input", "-"], Some(br#"{"file":"-"}"#))?;
            assert_eq!(
                body(&output)?["error"]["details"]["issues"][0]["rule"],
                "stdin_can_only_be_consumed_once"
            );
        }
    }
    let output = invoke(
        &[
            "plan",
            "validate",
            "--file",
            "-",
            "--state-root",
            "not-applicable",
        ],
        Some(&serde_json::to_vec(&plan()?)?),
    )?;
    assert_eq!(
        body(&output)?["error"]["details"]["issues"][0]["rule"],
        "state_and_profile_options_not_applicable"
    );
    let output = invoke(
        &[
            "plan",
            "validate",
            "--file",
            "-",
            "--fields",
            "document.context.value",
        ],
        Some(&serde_json::to_vec(&plan()?)?),
    )?;
    assert_eq!(body(&output)?["error"]["code"], "FIELD_UNKNOWN");
    Ok(())
}
