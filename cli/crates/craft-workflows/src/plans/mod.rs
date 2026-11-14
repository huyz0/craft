//! Local plan/design contracts. Displayed review state never grants approval authority.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, parse_object};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::documents::compose;
use crate::documents::{ContentError, Document};

mod page;
#[cfg(test)]
pub(crate) mod tests;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "format",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum StructuredBody {
    Markdown(String),
    Storage(String),
    Adf(Value),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RichText {
    Markdown(String),
    Structured(StructuredBody),
}

pub(super) fn content_error(path: &str, error: ContentError) -> CraftError {
    let mut failure = CraftError::field(path, error.reason);
    failure.message = match error.path {
        Some(source) => format!("Document content is invalid at JSON path {source}."),
        None => format!("Document content is invalid at byte {}.", error.offset),
    };
    failure
}

impl RichText {
    pub fn document(&self, path: &str) -> Result<Document, CraftError> {
        let size = serde_json::to_vec(self)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode document body."))?
            .len();
        if size > 65_536 {
            return Err(CraftError::field(path, "body_byte_limit"));
        }
        match self {
            Self::Markdown(source) | Self::Structured(StructuredBody::Markdown(source)) => {
                Document::from_markdown(source)
            }
            Self::Structured(StructuredBody::Storage(source)) => Document::from_storage(source),
            Self::Structured(StructuredBody::Adf(value)) => Document::from_adf(
                &serde_json::to_vec(value)
                    .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode ADF body."))?,
            ),
        }
        .map_err(|error| content_error(path, error))
    }
    pub(super) fn imported(document: &Document) -> Result<Self, CraftError> {
        Ok(Self::Structured(StructuredBody::Adf(
            document
                .to_adf()
                .map_err(|error| content_error("/body", error))?,
        )))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentTarget {
    #[schemars(length(min = 36, max = 36))]
    pub confluence_cloud_id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,31}$"))]
    pub space_id: String,
    pub parent_page_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanTarget {
    pub confluence: DocumentTarget,
    #[schemars(length(min = 36, max = 36))]
    pub jira_cloud_id: String,
    #[schemars(length(min = 1, max = 64))]
    pub project_key: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Proposed,
    InReview,
    ChangesRequested,
    Approved,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DesignKind {
    Hld,
    Detailed,
    Decision,
}

impl std::str::FromStr for DesignKind {
    type Err = CraftError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "hld" => Ok(Self::Hld),
            "detailed" => Ok(Self::Detailed),
            "decision" => Ok(Self::Decision),
            _ => Err(CraftError::field("/kind", "unsupported_design_kind")),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignReference {
    #[schemars(length(min = 1, max = 64))]
    pub design_id: String,
    pub cloud_id: String,
    pub page_id: String,
    pub version: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JiraMapping {
    pub project_key: String,
    #[schemars(length(min = 1, max = 128))]
    pub issue_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_issue: Option<ExistingIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExistingIssue {
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub issue_id: String,
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._:-]+$"))]
    pub origin_operation_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanTask {
    #[schemars(length(min = 1, max = 64))]
    pub task_id: String,
    #[schemars(length(min = 1, max = 240))]
    pub title: String,
    pub jira: JiraMapping,
    pub description: RichText,
    #[schemars(length(max = 64))]
    pub acceptance: Vec<RichText>,
    #[schemars(length(max = 32))]
    pub repositories: Vec<String>,
    #[schemars(length(max = 256))]
    pub depends_on: Vec<String>,
    #[schemars(length(max = 64))]
    pub designs: Vec<String>,
    pub include_in_jira: bool,
    pub estimate: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: String,
    #[schemars(length(min = 1, max = 64))]
    pub plan_id: String,
    #[schemars(length(min = 1, max = 240))]
    pub title: String,
    pub context: RichText,
    pub review_state: ReviewState,
    pub target: PlanTarget,
    #[schemars(length(max = 64))]
    pub designs: Vec<DesignReference>,
    #[schemars(length(max = 256))]
    pub tasks: Vec<PlanTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignSection {
    pub key: String,
    pub title: String,
    pub body: RichText,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Design {
    pub schema_version: String,
    pub design_id: String,
    pub title: String,
    pub kind: DesignKind,
    pub target: DocumentTarget,
    #[schemars(length(max = 32))]
    pub sections: Vec<DesignSection>,
    #[schemars(length(max = 256))]
    pub linked_tasks: Vec<String>,
}

fn label(path: &str, value: &str, maximum: usize) -> Result<(), CraftError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(CraftError::field(path, "expected_bounded_single_line_text"));
    }
    Ok(())
}
fn identity(path: &str, value: &str) -> Result<(), CraftError> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(CraftError::field(path, "expected_semantic_identity"));
    }
    Ok(())
}
fn uuid(path: &str, value: &str) -> Result<(), CraftError> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(CraftError::field(path, "expected_cloud_uuid"));
    }
    Ok(())
}
fn numeric(path: &str, value: &str) -> Result<(), CraftError> {
    if value.is_empty()
        || value.len() > 32
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(CraftError::field(path, "expected_positive_numeric_id"));
    }
    Ok(())
}
fn project(path: &str, value: &str) -> Result<(), CraftError> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes()[0].is_ascii_alphabetic()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(CraftError::field(path, "expected_project_key"));
    }
    Ok(())
}
fn unique(path: &str, values: &[String], maximum: usize, semantic: bool) -> Result<(), CraftError> {
    if values.len() > maximum {
        return Err(CraftError::field(path, "list_item_limit"));
    }
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        let path = format!("{path}/{index}");
        if semantic {
            identity(&path, value)?;
        } else {
            label(&path, value, 512)?;
            if value.contains(',') {
                return Err(CraftError::field(&path, "comma_not_supported_in_locator"));
            }
        }
        if !seen.insert(value) {
            return Err(CraftError::field(&path, "duplicate_value"));
        }
    }
    Ok(())
}
fn body(
    path: &str,
    value: &RichText,
    reserved: impl Fn(u8, &str) -> bool,
) -> Result<Document, CraftError> {
    let document = value.document(path)?;
    if !compose::has_text(&document) {
        return Err(CraftError::field(path, "expected_nonempty_rich_text"));
    }
    for (level, title) in
        compose::headings(&document).map_err(|error| content_error(path, error))?
    {
        if reserved(level, &title) {
            return Err(CraftError::field(path, "reserved_section_heading"));
        }
    }
    Ok(document)
}
fn schema_version(value: &str) -> Result<(), CraftError> {
    if value != "1.0" {
        return Err(CraftError::field(
            "/schema_version",
            "unsupported_document_version",
        ));
    }
    Ok(())
}

impl DocumentTarget {
    pub fn validate(&self, path: &str) -> Result<(), CraftError> {
        uuid(
            &format!("{path}/confluence_cloud_id"),
            &self.confluence_cloud_id,
        )?;
        numeric(&format!("{path}/space_id"), &self.space_id)?;
        if let Some(parent) = &self.parent_page_id {
            numeric(&format!("{path}/parent_page_id"), parent)?;
        }
        Ok(())
    }
}
impl PlanTarget {
    pub fn validate(&self) -> Result<(), CraftError> {
        self.confluence.validate("/target/confluence")?;
        uuid("/target/jira_cloud_id", &self.jira_cloud_id)?;
        project("/target/project_key", &self.project_key)
    }
}

impl Plan {
    pub fn scaffold(
        plan_id: String,
        title: String,
        target: PlanTarget,
    ) -> Result<Self, CraftError> {
        identity("/plan_id", &plan_id)?;
        label("/title", &title, 240)?;
        target.validate()?;
        Ok(Self {
            schema_version: "1.0".into(),
            plan_id,
            title,
            context: RichText::Markdown(String::new()),
            review_state: ReviewState::Proposed,
            target,
            designs: Vec::new(),
            tasks: Vec::new(),
        })
    }
    pub fn from_json(source: &[u8]) -> Result<Self, CraftError> {
        let value = parse_object(source)?;
        let plan: Self = serde_json::from_value(value)
            .map_err(|_| CraftError::field("/plan", "invalid_plan_shape"))?;
        plan.validate()?;
        Ok(plan)
    }
    /// Validates the complete visible graph; its order is deterministic, not a worker lease.
    pub fn validate(&self) -> Result<Vec<String>, CraftError> {
        schema_version(&self.schema_version)?;
        identity("/plan_id", &self.plan_id)?;
        label("/title", &self.title, 240)?;
        self.target.validate()?;
        body("/context", &self.context, |level, title| {
            level == 2
                && (title == "Context"
                    || title == "Design references"
                    || title.starts_with("Task "))
        })?;
        if self.tasks.is_empty() || self.tasks.len() > 256 {
            return Err(CraftError::field("/tasks", "expected_bounded_task_graph"));
        }
        if self.designs.len() > 64 {
            return Err(CraftError::field("/designs", "list_item_limit"));
        }
        let mut designs = BTreeSet::new();
        for (index, design) in self.designs.iter().enumerate() {
            let path = format!("/designs/{index}");
            identity(&format!("{path}/design_id"), &design.design_id)?;
            if !designs.insert(&design.design_id) {
                return Err(CraftError::field(&path, "duplicate_design_identity"));
            }
            uuid(&format!("{path}/cloud_id"), &design.cloud_id)?;
            numeric(&format!("{path}/page_id"), &design.page_id)?;
            if design.version == 0 {
                return Err(CraftError::field(&path, "expected_design_revision"));
            }
            if !design.digest.strip_prefix("sha256:").is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }) {
                return Err(CraftError::field(&path, "expected_design_digest"));
            }
        }
        let mut indices = BTreeMap::new();
        let mut reused_issues = BTreeSet::new();
        let mut reused_origins = BTreeSet::new();
        for (index, task) in self.tasks.iter().enumerate() {
            let path = format!("/tasks/{index}");
            identity(&format!("{path}/task_id"), &task.task_id)?;
            if indices.insert(&task.task_id, index).is_some() {
                return Err(CraftError::field(
                    format!("{path}/task_id"),
                    "duplicate_task_identity",
                ));
            }
            label(&format!("{path}/title"), &task.title, 240)?;
            project(&format!("{path}/jira/project_key"), &task.jira.project_key)?;
            if let Some(existing) = &task.jira.existing_issue {
                if !reused_issues.insert((&task.jira.project_key, &existing.issue_id))
                    || !reused_origins.insert(&existing.origin_operation_id)
                {
                    return Err(CraftError::field(
                        format!("{path}/jira/existing_issue"),
                        "duplicate_reuse_issue_or_origin",
                    ));
                }
                if !task.include_in_jira
                    || !existing
                        .issue_id
                        .parse::<i64>()
                        .is_ok_and(|id| id > 0 && id.to_string() == existing.issue_id)
                    || existing.origin_operation_id.is_empty()
                    || existing.origin_operation_id.len() > 128
                    || !existing
                        .origin_operation_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
                {
                    return Err(CraftError::field(
                        format!("{path}/jira/existing_issue"),
                        "expected_included_exact_issue_and_origin_operation",
                    ));
                }
            }
            label(
                &format!("{path}/jira/issue_type"),
                &task.jira.issue_type,
                128,
            )?;
            body(
                &format!("{path}/description"),
                &task.description,
                |level, title| {
                    (level == 3 && matches!(title, "Description" | "Acceptance criteria"))
                        || (level == 2 && title.starts_with("Task "))
                },
            )?;
            if task.acceptance.is_empty() || task.acceptance.len() > 64 {
                return Err(CraftError::field(
                    format!("{path}/acceptance"),
                    "expected_bounded_acceptance",
                ));
            }
            for (criterion, value) in task.acceptance.iter().enumerate() {
                let path = format!("{path}/acceptance/{criterion}");
                let document = body(&path, value, |_, _| false)?;
                if !compose::paragraph(&document) {
                    return Err(CraftError::field(&path, "expected_paragraph_criterion"));
                }
            }
            unique(
                &format!("{path}/repositories"),
                &task.repositories,
                32,
                false,
            )?;
            if task.repositories.is_empty() {
                return Err(CraftError::field(
                    format!("{path}/repositories"),
                    "expected_repository_scope",
                ));
            }
            unique(&format!("{path}/depends_on"), &task.depends_on, 256, true)?;
            unique(&format!("{path}/designs"), &task.designs, 64, true)?;
            if task.designs.iter().any(|name| !designs.contains(name)) {
                return Err(CraftError::field(
                    format!("{path}/designs"),
                    "unknown_design_reference",
                ));
            }
            if let Some(estimate) = &task.estimate {
                label(&format!("{path}/estimate"), estimate, 128)?;
            }
        }
        let mut indegree = vec![0usize; self.tasks.len()];
        let mut next = vec![Vec::new(); self.tasks.len()];
        for (index, task) in self.tasks.iter().enumerate() {
            for (dependency, name) in task.depends_on.iter().enumerate() {
                let source = *indices.get(name).ok_or_else(|| {
                    CraftError::field(
                        format!("/tasks/{index}/depends_on/{dependency}"),
                        "missing_task_reference",
                    )
                })?;
                if task.include_in_jira && !self.tasks[source].include_in_jira {
                    return Err(CraftError::field(
                        format!("/tasks/{index}/depends_on/{dependency}"),
                        "included_task_depends_on_excluded_work",
                    ));
                }
                indegree[index] += 1;
                next[source].push(index);
            }
        }
        let mut ready: VecDeque<_> = indegree
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(index))
            .collect();
        let mut order = Vec::new();
        while let Some(index) = ready.pop_front() {
            order.push(self.tasks[index].task_id.clone());
            for target in &next[index] {
                indegree[*target] -= 1;
                if indegree[*target] == 0 {
                    ready.push_back(*target);
                }
            }
        }
        if order.len() != self.tasks.len() {
            return Err(CraftError::field("/tasks", "cyclic_task_dependencies"));
        }
        Ok(order)
    }
    pub fn canonical_value(&self) -> Result<Value, CraftError> {
        self.validate()?;
        let mut value = serde_json::to_value(self)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode plan."))?;
        value["context"] = self
            .context
            .document("/context")?
            .to_adf()
            .map_err(|error| content_error("/context", error))?;
        for (index, task) in self.tasks.iter().enumerate() {
            value["tasks"][index]["description"] = task
                .description
                .document("/description")?
                .to_adf()
                .map_err(|error| content_error("/description", error))?;
            value["tasks"][index]["acceptance"] = Value::Array(
                task.acceptance
                    .iter()
                    .map(|value| {
                        value
                            .document("/acceptance")?
                            .to_adf()
                            .map_err(|error| content_error("/acceptance", error))
                    })
                    .collect::<Result<_, _>>()?,
            );
        }
        Ok(json!({"canonicalization":"plan-content-v1","document":value}))
    }
    pub fn digest(&self) -> Result<String, CraftError> {
        canonical_digest(&self.canonical_value()?)
    }
}

impl Design {
    pub fn scaffold(
        design_id: String,
        title: String,
        kind: DesignKind,
        target: DocumentTarget,
        compact: bool,
    ) -> Result<Self, CraftError> {
        identity("/design_id", &design_id)?;
        label("/title", &title, 240)?;
        target.validate("/target")?;
        let full = [
            ("context", "Context and problem"),
            ("goals", "Goals"),
            ("non-goals", "Non-goals"),
            ("current-system", "Current system"),
            ("alternatives", "Alternatives"),
            ("decision", "Decision"),
            ("high-level-design", "High-level design"),
            ("interfaces-and-data", "Interfaces and data"),
            ("failure-handling", "Failure handling"),
            ("security", "Security"),
            ("testing", "Testing"),
            ("rollout-and-recovery", "Rollout and recovery"),
            ("open-questions", "Open questions"),
        ];
        let sections = full
            .into_iter()
            .filter(|(key, _)| {
                !compact || ["context", "decision", "interfaces-and-data", "testing"].contains(key)
            })
            .map(|(key, title)| DesignSection {
                key: key.into(),
                title: title.into(),
                body: RichText::Markdown(String::new()),
            })
            .collect();
        Ok(Self {
            schema_version: "1.0".into(),
            design_id,
            title,
            kind,
            target,
            sections,
            linked_tasks: Vec::new(),
        })
    }
    pub fn from_json(source: &[u8]) -> Result<Self, CraftError> {
        let value = parse_object(source)?;
        let design: Self = serde_json::from_value(value)
            .map_err(|_| CraftError::field("/design", "invalid_design_shape"))?;
        design.validate()?;
        Ok(design)
    }
    pub fn validate(&self) -> Result<(), CraftError> {
        schema_version(&self.schema_version)?;
        identity("/design_id", &self.design_id)?;
        label("/title", &self.title, 240)?;
        self.target.validate("/target")?;
        if self.sections.is_empty() || self.sections.len() > 32 {
            return Err(CraftError::field(
                "/sections",
                "expected_bounded_design_sections",
            ));
        }
        unique("/linked_tasks", &self.linked_tasks, 256, true)?;
        let mut seen = BTreeSet::new();
        for (index, section) in self.sections.iter().enumerate() {
            let path = format!("/sections/{index}");
            identity(&format!("{path}/key"), &section.key)?;
            label(&format!("{path}/title"), &section.title, 240)?;
            if !seen.insert(&section.key) {
                return Err(CraftError::field(&path, "duplicate_section_identity"));
            }
            body(&format!("{path}/body"), &section.body, |level, title| {
                level == 2 && title.starts_with("Section ")
            })?;
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String, CraftError> {
        self.validate()?;
        let mut value = serde_json::to_value(self)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode design."))?;
        for (index, section) in self.sections.iter().enumerate() {
            value["sections"][index]["body"] = section
                .body
                .document("/body")?
                .to_adf()
                .map_err(|error| content_error("/body", error))?;
        }
        canonical_digest(&json!({"canonicalization":"design-content-v1","document":value}))
    }
}
