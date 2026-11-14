use super::*;
use crate::documents::VisibleSection;

fn document(result: Result<Document, ContentError>) -> Result<Document, CraftError> {
    result.map_err(|error| content_error("/page", error))
}
fn literal(value: &str) -> Result<Document, CraftError> {
    document(compose::literal(value))
}
fn heading(level: u8, title: &str) -> Result<Document, CraftError> {
    document(compose::heading(level, title))
}
fn text(value: &Document) -> Result<String, CraftError> {
    value
        .plain_text()
        .map_err(|error| content_error("/page", error))
}
fn display(value: &impl Serialize) -> Result<String, CraftError> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Cannot encode document label."))
}
fn metadata(rows: Vec<(&str, String)>) -> Result<Document, CraftError> {
    document(compose::table(
        &["Field", "Value"],
        rows.into_iter()
            .map(|(key, value)| Ok(vec![literal(key)?, literal(&value)?]))
            .collect::<Result<_, CraftError>>()?,
    ))
}
fn fields(document: &Document, expected: &[&str]) -> Result<BTreeMap<String, String>, CraftError> {
    let values = document
        .field_table()
        .map_err(|error| content_error("/page", error))?;
    if values.len() != expected.len()
        || values
            .iter()
            .any(|field| !expected.contains(&field.name.as_str()))
    {
        return Err(CraftError::field(
            "/page",
            "unknown_or_missing_visible_field",
        ));
    }
    values
        .into_iter()
        .map(|field| Ok((field.name, text(&field.value)?)))
        .collect()
}
fn get(values: &BTreeMap<String, String>, key: &str) -> Result<String, CraftError> {
    values
        .get(key)
        .cloned()
        .ok_or_else(|| CraftError::field("/page", "missing_visible_field"))
}
fn optional(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
fn list(value: String) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value
            .split(',')
            .map(|value| value.trim().to_owned())
            .collect()
    }
}
fn target(values: &BTreeMap<String, String>) -> Result<DocumentTarget, CraftError> {
    Ok(DocumentTarget {
        confluence_cloud_id: get(values, "Confluence cloud ID")?,
        space_id: get(values, "Space ID")?,
        parent_page_id: optional(get(values, "Parent page ID")?),
    })
}
fn section(
    sections: &[VisibleSection],
    index: usize,
    level: u8,
    title: &str,
) -> Result<(), CraftError> {
    if !sections
        .get(index)
        .is_some_and(|section| section.level == level && section.heading == title)
    {
        return Err(CraftError::field(
            "/page",
            "missing_or_ambiguous_section_boundary",
        ));
    }
    Ok(())
}
fn collect(
    sections: &[VisibleSection],
    start: usize,
    stop: impl Fn(&VisibleSection) -> bool,
) -> Result<(Document, usize), CraftError> {
    let first = sections
        .get(start)
        .ok_or_else(|| CraftError::field("/page", "missing_section"))?;
    let mut parts = vec![first.body.clone()];
    let mut index = start + 1;
    while index < sections.len() && !stop(&sections[index]) {
        parts.push(document(sections[index].document())?);
        index += 1;
    }
    Ok((document(compose::join(&parts))?, index))
}

impl Plan {
    /// Renders ordinary visible sections; no hidden payload is needed for later import.
    pub fn render(&self) -> Result<Document, CraftError> {
        self.validate()?;
        let mut parts = vec![
            heading(1, &self.title)?,
            metadata(vec![
                ("Schema version", self.schema_version.clone()),
                ("Plan ID", self.plan_id.clone()),
                ("Review state", display(&self.review_state)?),
                (
                    "Confluence cloud ID",
                    self.target.confluence.confluence_cloud_id.clone(),
                ),
                ("Space ID", self.target.confluence.space_id.clone()),
                (
                    "Parent page ID",
                    self.target
                        .confluence
                        .parent_page_id
                        .clone()
                        .unwrap_or_default(),
                ),
                ("Jira cloud ID", self.target.jira_cloud_id.clone()),
                ("Default project", self.target.project_key.clone()),
            ])?,
            heading(2, "Context")?,
            self.context.document("/context")?,
            heading(2, "Design references")?,
        ];
        let rows = self
            .designs
            .iter()
            .map(|reference| {
                [
                    reference.design_id.clone(),
                    reference.cloud_id.clone(),
                    reference.page_id.clone(),
                    reference.version.to_string(),
                    reference.digest.clone(),
                ]
                .iter()
                .map(|value| literal(value))
                .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        parts.push(document(compose::table(
            &["Design ID", "Cloud ID", "Page ID", "Version", "Digest"],
            rows,
        ))?);
        for task in &self.tasks {
            parts.push(heading(2, &format!("Task {}", task.task_id))?);
            let mut rows = vec![
                ("Title", task.title.clone()),
                ("Jira project", task.jira.project_key.clone()),
                ("Issue type", task.jira.issue_type.clone()),
                ("Depends on", task.depends_on.join(", ")),
                ("Include in Jira", task.include_in_jira.to_string()),
                ("Repositories", task.repositories.join(", ")),
                ("Designs", task.designs.join(", ")),
                ("Estimate", task.estimate.clone().unwrap_or_default()),
            ];
            if let Some(existing) = &task.jira.existing_issue {
                rows.push(("Reuse existing issue ID", existing.issue_id.clone()));
                rows.push((
                    "Original create operation",
                    existing.origin_operation_id.clone(),
                ));
            }
            parts.push(metadata(rows)?);
            parts.push(heading(3, "Description")?);
            parts.push(task.description.document("/description")?);
            parts.push(heading(3, "Acceptance criteria")?);
            let criteria = task
                .acceptance
                .iter()
                .map(|value| value.document("/acceptance"))
                .collect::<Result<Vec<_>, _>>()?;
            parts.push(document(compose::bullets(&criteria))?);
        }
        document(compose::join(&parts))
    }

    /// Imports only the supplied visible document, never an earlier local draft or approval.
    pub fn from_document(document: &Document) -> Result<Self, CraftError> {
        let sections = document
            .sections()
            .map_err(|error| content_error("/page", error))?;
        let title = sections
            .first()
            .filter(|section| section.level == 1)
            .ok_or_else(|| CraftError::field("/page", "expected_plan_title"))?
            .heading
            .clone();
        let values = fields(
            &sections[0].body,
            &[
                "Schema version",
                "Plan ID",
                "Review state",
                "Confluence cloud ID",
                "Space ID",
                "Parent page ID",
                "Jira cloud ID",
                "Default project",
            ],
        )?;
        section(&sections, 1, 2, "Context")?;
        let (context, index) = collect(&sections, 1, |section| {
            section.level == 2
                && (section.heading == "Context"
                    || section.heading == "Design references"
                    || section.heading.starts_with("Task "))
        })?;
        section(&sections, index, 2, "Design references")?;
        let rows = compose::table_rows(&sections[index].body)
            .map_err(|error| content_error("/page", error))?;
        let expected = ["Design ID", "Cloud ID", "Page ID", "Version", "Digest"];
        if rows.is_empty() || rows[0].len() != expected.len() {
            return Err(CraftError::field(
                "/page",
                "expected_design_reference_table",
            ));
        }
        for (value, expected) in rows[0].iter().zip(expected) {
            if text(value)? != expected {
                return Err(CraftError::field(
                    "/page",
                    "expected_design_reference_header",
                ));
            }
        }
        let mut designs = Vec::new();
        for row in rows.into_iter().skip(1) {
            if row.len() != 5 {
                return Err(CraftError::field("/page", "ambiguous_design_reference"));
            }
            designs.push(DesignReference {
                design_id: text(&row[0])?,
                cloud_id: text(&row[1])?,
                page_id: text(&row[2])?,
                version: text(&row[3])?
                    .parse()
                    .map_err(|_| CraftError::field("/page", "expected_design_revision"))?,
                digest: text(&row[4])?,
            });
        }
        let mut tasks = Vec::new();
        let mut index = index + 1;
        while index < sections.len() {
            let task_id = if sections[index].level == 2 {
                sections[index].heading.strip_prefix("Task ")
            } else {
                None
            }
            .ok_or_else(|| CraftError::field("/page", "expected_task_section"))?
            .to_owned();
            let mut expected = vec![
                "Title",
                "Jira project",
                "Issue type",
                "Depends on",
                "Include in Jira",
                "Repositories",
                "Designs",
                "Estimate",
            ];
            let observed = sections[index]
                .body
                .field_table()
                .map_err(|error| content_error("/page", error))?;
            if observed.iter().any(|field| {
                matches!(
                    field.name.as_str(),
                    "Reuse existing issue ID" | "Original create operation"
                )
            }) {
                expected.extend(["Reuse existing issue ID", "Original create operation"]);
            }
            let task = fields(&sections[index].body, &expected)?;
            section(&sections, index + 1, 3, "Description")?;
            let (description, acceptance) = collect(&sections, index + 1, |section| {
                (section.level == 3
                    && matches!(
                        section.heading.as_str(),
                        "Description" | "Acceptance criteria"
                    ))
                    || (section.level == 2 && section.heading.starts_with("Task "))
            })?;
            section(&sections, acceptance, 3, "Acceptance criteria")?;
            let criteria = compose::bullet_items(&sections[acceptance].body)
                .map_err(|error| content_error("/page", error))?;
            tasks.push(PlanTask {
                task_id,
                title: get(&task, "Title")?,
                jira: JiraMapping {
                    project_key: get(&task, "Jira project")?,
                    issue_type: get(&task, "Issue type")?,
                    existing_issue: if expected.contains(&"Reuse existing issue ID") {
                        Some(ExistingIssue {
                            issue_id: get(&task, "Reuse existing issue ID")?,
                            origin_operation_id: get(&task, "Original create operation")?,
                        })
                    } else {
                        None
                    },
                },
                description: RichText::imported(&description)?,
                acceptance: criteria
                    .iter()
                    .map(RichText::imported)
                    .collect::<Result<_, _>>()?,
                repositories: list(get(&task, "Repositories")?),
                depends_on: list(get(&task, "Depends on")?),
                designs: list(get(&task, "Designs")?),
                include_in_jira: get(&task, "Include in Jira")?
                    .parse()
                    .map_err(|_| CraftError::field("/page", "expected_boolean_inclusion"))?,
                estimate: optional(get(&task, "Estimate")?),
            });
            index = acceptance + 1;
        }
        let plan = Self {
            schema_version: get(&values, "Schema version")?,
            plan_id: get(&values, "Plan ID")?,
            title,
            context: RichText::imported(&context)?,
            review_state: serde_json::from_value(json!(get(&values, "Review state")?))
                .map_err(|_| CraftError::field("/page", "unsupported_review_label"))?,
            target: PlanTarget {
                confluence: target(&values)?,
                jira_cloud_id: get(&values, "Jira cloud ID")?,
                project_key: get(&values, "Default project")?,
            },
            designs,
            tasks,
        };
        plan.validate()?;
        Ok(plan)
    }
}

impl Design {
    pub fn render(&self) -> Result<Document, CraftError> {
        self.validate()?;
        let mut parts = vec![
            heading(1, &self.title)?,
            metadata(vec![
                ("Schema version", self.schema_version.clone()),
                ("Design ID", self.design_id.clone()),
                ("Kind", display(&self.kind)?),
                (
                    "Confluence cloud ID",
                    self.target.confluence_cloud_id.clone(),
                ),
                ("Space ID", self.target.space_id.clone()),
                (
                    "Parent page ID",
                    self.target.parent_page_id.clone().unwrap_or_default(),
                ),
                ("Linked tasks", self.linked_tasks.join(", ")),
            ])?,
        ];
        for section in &self.sections {
            parts.push(heading(
                2,
                &format!("Section {}: {}", section.key, section.title),
            )?);
            parts.push(section.body.document("/body")?);
        }
        document(compose::join(&parts))
    }
    pub fn from_document(document: &Document) -> Result<Self, CraftError> {
        let sections = document
            .sections()
            .map_err(|error| content_error("/page", error))?;
        let title = sections
            .first()
            .filter(|section| section.level == 1)
            .ok_or_else(|| CraftError::field("/page", "expected_design_title"))?
            .heading
            .clone();
        let values = fields(
            &sections[0].body,
            &[
                "Schema version",
                "Design ID",
                "Kind",
                "Confluence cloud ID",
                "Space ID",
                "Parent page ID",
                "Linked tasks",
            ],
        )?;
        let mut imported = Vec::new();
        let mut index = 1;
        while index < sections.len() {
            let (key, title) = if sections[index].level == 2 {
                sections[index].heading.strip_prefix("Section ")
            } else {
                None
            }
            .and_then(|heading| heading.split_once(": "))
            .ok_or_else(|| CraftError::field("/page", "expected_design_section"))?;
            let (body, next) = collect(&sections, index, |section| {
                section.level == 2 && section.heading.starts_with("Section ")
            })?;
            imported.push(DesignSection {
                key: key.into(),
                title: title.into(),
                body: RichText::imported(&body)?,
            });
            index = next;
        }
        let design = Self {
            schema_version: get(&values, "Schema version")?,
            design_id: get(&values, "Design ID")?,
            title,
            kind: get(&values, "Kind")?.parse()?,
            target: target(&values)?,
            sections: imported,
            linked_tasks: list(get(&values, "Linked tasks")?),
        };
        design.validate()?;
        Ok(design)
    }
}
