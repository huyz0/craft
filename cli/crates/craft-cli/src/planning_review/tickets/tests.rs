use super::*;
use craft_workflows::plans::{DocumentTarget, JiraMapping, PlanTarget, PlanTask, RichText};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const CLOUD: &str = "00000000-1111-2222-3333-444444444444";
fn target(provider: &str, kind: &str, id: &str) -> ResourceRef {
    ResourceRef {
        provider: provider.into(),
        deployment: "cloud".into(),
        tenant: CLOUD.into(),
        resource_type: kind.into(),
        id: id.into(),
    }
}
fn snapshot() -> Result<BindingSnapshot, CraftError> {
    let mut plan = Plan::scaffold(
        "delivery".into(),
        "Delivery".into(),
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
    plan.context = RichText::Markdown("Shared work.".into());
    for (id, dependencies, included) in [
        ("release", vec!["api", "ui"], true),
        ("ui", vec!["contract"], true),
        ("contract", vec![], true),
        ("api", vec!["contract"], true),
        ("notes", vec![], false),
    ] {
        plan.tasks.push(PlanTask {
            task_id: id.into(),
            title: id.into(),
            jira: JiraMapping {
                project_key: "PAY".into(),
                issue_type: "Task".into(),
                existing_issue: None,
            },
            description: RichText::Markdown("Reviewed description.".into()),
            acceptance: vec![RichText::Markdown("Works.".into())],
            repositories: vec!["api".into()],
            depends_on: dependencies.into_iter().map(String::from).collect(),
            designs: vec![],
            include_in_jira: included,
            estimate: None,
        });
    }
    let mut tasks = Vec::new();
    for task in plan.tasks.iter().filter(|t| t.include_in_jira) {
        tasks.push(ResolvedTask {
            existing_issue: None, existing_issue_observation: None,
            task_id:task.task_id.clone(),profile:"jira-test".into(),target:target("jira","project","200"),input:json!({"issue_type_id":"10001","summary":task.title}),payload:json!({"fields":{"project":{"id":"200"},"issuetype":{"id":"10001"},"summary":task.title}}),metadata:json!({}),metadata_digest:digest(&json!({}))?,
            dependency_link_type:(!task.depends_on.is_empty()).then(||DependencyType {link_type:LinkType {id:"10000".into(),name:"Blocks".into(),inward:"is blocked by".into(),outward:"blocks".into()},dependent_direction:DependencyDirection::Inward}),
        });
    }
    let binding = ReviewBinding::from_plan(
        &plan,
        target("confluence", "page", "101"),
        1,
        Representation::Storage,
        digest(&"body")?,
        BTreeMap::from([("PAY".into(), target("jira", "project", "200"))]),
        digest(&json!({}))?,
    )?;
    let statement = ReviewStatement::new(Decision::Approve, binding.clone())?;
    let source = serde_json::from_value(json!({"page":binding.page,"space_id":"100","title":plan.title,"revision":{"version":1,"digest":binding.page_digest,"representation":"storage"},"observed_at":1000,"document_digest":plan.digest()?})).map_err(|_|invalid("test_source"))?;
    Ok(BindingSnapshot {
        binding,
        statement,
        plan: DocumentSnapshot {
            source,
            body_storage: plan
                .render()?
                .to_storage()
                .map_err(|_| invalid("test_body"))?,
            document: plan,
        },
        designs: vec![],
        tasks,
        reviewer_policy_digest: digest(&"policy")?,
    })
}
#[test]
fn diamond_dependencies_form_complete_deterministic_batches_and_link_endpoints() -> TestResult {
    let manifest = compile(snapshot()?)?;
    assert_eq!(
        manifest.batches,
        vec![vec!["contract"], vec!["api", "ui"], vec!["release"]]
    );
    assert_eq!(manifest.links.len(), 4);
    for link in &manifest.links {
        assert_eq!(link.inward_task_id, link.prerequisite_task_id);
        assert_eq!(link.outward_task_id, link.dependent_task_id);
        assert_eq!(link.dependent_label, "is blocked by");
    }
    let summary = manifest.summary()?;
    assert_eq!(
        (
            summary.task_count,
            summary.batch_count,
            summary.dependency_count
        ),
        (4, 3, 4)
    );
    let mut reordered = snapshot()?;
    reordered.tasks.reverse();
    reordered.plan.source.observed_at += 500;
    assert_eq!(
        compile(reordered)?.summary()?.manifest_digest,
        summary.manifest_digest
    );
    let mut changed = snapshot()?;
    changed.tasks[0].payload["fields"]["summary"] = json!("Changed");
    assert_ne!(
        compile(changed)?.summary()?.manifest_digest,
        summary.manifest_digest
    );
    Ok(())
}
#[test]
fn incomplete_duplicate_and_cyclic_manifest_graphs_refuse() -> TestResult {
    let mut missing = snapshot()?;
    missing.tasks.pop();
    assert!(compile(missing).is_err());
    let mut duplicate = snapshot()?;
    duplicate.tasks[0].task_id = duplicate.tasks[1].task_id.clone();
    assert!(compile(duplicate).is_err());
    let mut cycle = snapshot()?;
    cycle.plan.document.tasks[2]
        .depends_on
        .push("release".into());
    assert!(compile(cycle).is_err());
    let mut no_type = snapshot()?;
    no_type.tasks[0].dependency_link_type = None;
    assert!(compile(no_type).is_err());
    Ok(())
}
