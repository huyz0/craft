use super::*;

fn target() -> PlanTarget {
    PlanTarget {
        confluence: DocumentTarget {
            confluence_cloud_id: "00000000-1111-2222-3333-444444444444".into(),
            space_id: "1".into(),
            parent_page_id: Some("2".into()),
        },
        jira_cloud_id: "00000000-1111-2222-3333-444444444444".into(),
        project_key: "PAY".into(),
    }
}
pub(crate) fn plan() -> Result<Plan, CraftError> {
    let mut plan = Plan::scaffold(
        "invoice-replay".into(),
        "Prevent invoice replay".into(),
        target(),
    )?;
    plan.context = RichText::Markdown(
        "Repeated delivery must preserve one ledger entry.\n\n### Background\n日本語 🦀".into(),
    );
    plan.designs.push(DesignReference {
        design_id: "invoice-contract".into(),
        cloud_id: target().jira_cloud_id,
        page_id: "3".into(),
        version: 2,
        digest: format!("sha256:{}", "a".repeat(64)),
    });
    for (key, title, depends) in [
        ("event-identity", "Define event identity", Vec::new()),
        (
            "reject-replay",
            "Reject repeated events",
            vec!["event-identity".into()],
        ),
    ] {
        plan.tasks.push(PlanTask {
            task_id: key.into(),
            title: title.into(),
            jira: JiraMapping {
                project_key: "PAY".into(),
                issue_type: "Task".into(),
                existing_issue: None,
            },
            description: RichText::Markdown(
                "Keep the **contract** explicit.\n\n```rust\nlet once = \"日本語 ]]>\";\n```"
                    .into(),
            ),
            acceptance: vec![RichText::Markdown(
                "Producer and consumer use [the contract](https://example.com/design).".into(),
            )],
            repositories: vec!["billing-api".into(), "finance-ledger".into()],
            depends_on: depends,
            designs: vec!["invoice-contract".into()],
            include_in_jira: true,
            estimate: Some("2 days".into()),
        });
    }
    Ok(plan)
}

#[test]
fn plan_round_trips_visible_metadata_rich_content_and_graphs()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = plan()?;
    assert_eq!(plan.validate()?, vec!["event-identity", "reject-replay"]);
    let digest = plan.digest()?;
    let page = plan.render()?;
    for document in [
        Document::from_storage(&page.to_storage()?)?,
        Document::from_adf(&serde_json::to_vec(&page.to_adf()?)?)?,
    ] {
        let imported = Plan::from_document(&document)?;
        assert_eq!(imported.canonical_value()?, plan.canonical_value()?);
        assert_eq!(imported.digest()?, digest);
    }
    Ok(())
}

#[test]
fn reviewed_reuse_is_visible_round_trips_and_refuses_partial_or_duplicate_decisions()
-> Result<(), Box<dyn std::error::Error>> {
    let original = plan()?;
    let old = original.render()?.to_storage()?;
    assert!(!old.contains("Reuse existing issue"));
    assert!(
        serde_json::to_value(&original.tasks[0].jira)?
            .get("existing_issue")
            .is_none()
    );
    let mut reused = original;
    reused.tasks[0].jira.existing_issue = Some(ExistingIssue {
        issue_id: "11001".into(),
        origin_operation_id: "earlier-create".into(),
    });
    reused.validate()?;
    let body = reused.render()?.to_storage()?;
    assert!(body.contains("Reuse existing issue ID") && body.contains("Original create operation"));
    let imported = Plan::from_document(&Document::from_storage(&body)?)?;
    assert_eq!(imported.canonical_value()?, reused.canonical_value()?);
    let partial = body.replace("Original create operation", "Unexpected origin");
    assert!(Plan::from_document(&Document::from_storage(&partial)?).is_err());
    reused.tasks[1].jira.existing_issue = reused.tasks[0].jira.existing_issue.clone();
    assert!(reused.validate().is_err());
    reused.tasks[1].jira.existing_issue = None;
    reused.tasks[0]
        .jira
        .existing_issue
        .as_mut()
        .ok_or("reuse")?
        .issue_id = "011001".into();
    assert!(reused.validate().is_err());
    Ok(())
}

#[test]
fn human_title_criteria_estimate_and_exclusion_edits_are_authoritative()
-> Result<(), Box<dyn std::error::Error>> {
    let original = plan()?;
    let storage = original
        .render()?
        .to_storage()?
        .replace("Define event identity", "Human title &amp; scope")
        .replace("Producer and consumer use", "Human reviewer requires")
        .replace("2 days", "3 days");
    let imported = Plan::from_document(&Document::from_storage(&storage)?)?;
    assert_eq!(imported.tasks[0].title, "Human title & scope");
    assert_eq!(imported.tasks[0].estimate.as_deref(), Some("3 days"));
    assert_ne!(imported.digest()?, original.digest()?);
    assert!(
        serde_json::to_string(&imported.tasks[0].acceptance)?.contains("Human reviewer requires")
    );
    let storage = storage.replace(
        "<td>Include in Jira</td><td>true</td>",
        "<td>Include in Jira</td><td>false</td>",
    );
    let excluded = Plan::from_document(&Document::from_storage(&storage)?)?;
    assert!(excluded.tasks.iter().all(|task| !task.include_in_jira));
    Ok(())
}

#[test]
fn invalid_graphs_and_ambiguous_visible_boundaries_fail_without_repair()
-> Result<(), Box<dyn std::error::Error>> {
    let original = plan()?;
    for mode in [
        "duplicate",
        "missing",
        "cycle",
        "excluded",
        "unknown-design",
        "reserved",
    ] {
        let mut plan = original.clone();
        match mode {
            "duplicate" => plan.tasks[1].task_id = plan.tasks[0].task_id.clone(),
            "missing" => plan.tasks[1].depends_on = vec!["missing-work".into()],
            "cycle" => plan.tasks[0].depends_on = vec!["reject-replay".into()],
            "excluded" => plan.tasks[0].include_in_jira = false,
            "unknown-design" => plan.tasks[0].designs = vec!["missing-design".into()],
            _ => {
                plan.tasks[0].description =
                    RichText::Markdown("### Acceptance criteria\nInjected boundary".into())
            }
        };
        let error = plan.validate().err().ok_or("Invalid graph accepted")?;
        assert_eq!(error.code, ErrorCode::InputInvalid);
    }
    let storage = original.render()?.to_storage()?;
    for edited in [
        storage.replace("<h3>Acceptance criteria</h3>", "<h3>Unknown section</h3>"),
        storage.replace("<td>Issue type</td>", "<td>Unknown field</td>"),
    ] {
        assert!(Plan::from_document(&Document::from_storage(&edited)?).is_err());
    }
    Ok(())
}

#[test]
fn design_scaffolds_are_incomplete_until_their_sections_are_authored()
-> Result<(), Box<dyn std::error::Error>> {
    for compact in [false, true] {
        let mut design = Design::scaffold(
            "invoice-design".into(),
            "Invoice design".into(),
            DesignKind::Hld,
            target().confluence,
            compact,
        )?;
        assert_eq!(design.sections.len(), if compact { 4 } else { 13 });
        assert!(design.validate().is_err());
        for section in &mut design.sections {
            section.body = RichText::Markdown(format!(
                "Explain **{}**.\n\n### Detail\nKeep decisions visible.",
                section.key
            ));
        }
        design.linked_tasks = vec!["event-identity".into()];
        let page = design.render()?;
        for document in [
            Document::from_storage(&page.to_storage()?)?,
            Document::from_adf(&serde_json::to_vec(&page.to_adf()?)?)?,
        ] {
            let imported = Design::from_document(&document)?;
            assert_eq!(imported.digest()?, design.digest()?);
        }
    }
    assert!(
        Plan::scaffold("empty-draft".into(), "Draft".into(), target())?
            .validate()
            .is_err()
    );
    Ok(())
}

#[test]
fn schemas_and_closed_json_refuse_unknown_input_and_preserve_display_state()
-> Result<(), Box<dyn std::error::Error>> {
    let mut plan = plan()?;
    plan.review_state = ReviewState::Approved;
    let value = serde_json::to_value(&plan)?;
    let schema = craft_core::registry::schema_for::<Plan>()?;
    assert!(jsonschema::validator_for(&schema)?.is_valid(&value));
    let mut invalid = value.clone();
    invalid["tasks"][0]["unknown"] = json!(true);
    assert!(Plan::from_json(&serde_json::to_vec(&invalid)?).is_err());
    let raw = serde_json::to_string(&value)?.replacen(
        "\"schema_version\":\"1.0\"",
        "\"schema_version\":\"1.0\",\"schema_version\":\"1.0\"",
        1,
    );
    assert!(Plan::from_json(raw.as_bytes()).is_err());
    assert!(matches!(
        Plan::from_document(&plan.render()?)?.review_state,
        ReviewState::Approved
    ));
    Ok(())
}
