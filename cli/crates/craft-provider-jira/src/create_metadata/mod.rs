//! Bounded create-context reads. Remote metadata remains data, never routing authority.

mod accounts;
mod fields;
mod reads;

use std::collections::BTreeMap;
use std::sync::Arc;

use craft_core::contracts::{GrantRef, ResourceRef};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, OperationContext, PreparedOperation};
use craft_core::input::canonical_digest;
use craft_core::metadata::FieldMapContract;
use craft_core::storage::Clock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use accounts::{
    AssigneeContext, AssigneeSummary, COMMAND as ASSIGNEE_COMMAND, read_assignable,
};
pub use reads::JiraMetadata;
pub const COMMAND: &str = "jira.metadata.create";
const FRESHNESS_MS: u64 = 60_000;
const MAX_ITEMS: usize = 2_000;
const MAX_PAGES: usize = 40;
const MAX_BYTES: usize = 8 * 1_048_576;

#[derive(Clone, PartialEq, Eq)]
pub struct MetadataPrincipal {
    pub grant: GrantRef,
    pub actor: String,
}

impl MetadataPrincipal {
    fn validate(&self) -> Result<(), CraftError> {
        self.grant.validate()?;
        craft_core::contracts::validate_identity("/actor", &self.actor)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetadataRequest {
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue_type_id: String,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u16,
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub parent_issue_id: Option<String>,
}
fn page_size() -> u16 {
    50
}

impl MetadataRequest {
    fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.issue_type_id) || !(1..=100).contains(&self.page_size) {
            return Err(CraftError::field(
                "/issue_type_id",
                "invalid_issue_type_or_metadata_page_size",
            ));
        }
        if self.parent_issue_id.as_ref().is_some_and(|id| !numeric(id)) {
            return Err(CraftError::field(
                "/parent_issue_id",
                "expected_numeric_parent_issue_id",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldInfo {
    pub id: String,
    pub name: String,
    pub required: bool,
    pub has_default: bool,
    pub settable: bool,
    pub schema: Option<Value>,
    pub unsupported_reason: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetadataSummary {
    pub target: ResourceRef,
    pub project_key: String,
    pub issue_type_id: String,
    pub issue_type_name: String,
    pub subtask: bool,
    pub parent: Option<ResourceRef>,
    pub observed_at: u64,
    pub fields: Vec<FieldInfo>,
    #[schemars(extend("const" = true))]
    pub complete: bool,
    #[schemars(extend("const" = false))]
    pub mutation_authorized: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    #[schemars(length(min = 1, max = 512), regex(pattern = "^[0-9]+$"))]
    pub issue_type_id: String,
    pub summary: String,
    #[serde(
        default,
        skip_serializing_if = "craft_core::presence::InputField::is_omitted"
    )]
    pub description: craft_core::presence::InputField<String>,
    #[serde(
        default,
        skip_serializing_if = "craft_core::presence::InputField::is_omitted"
    )]
    pub labels: craft_core::presence::InputField<Vec<String>>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, Value>,
    #[serde(
        default,
        skip_serializing_if = "craft_core::presence::InputField::is_omitted"
    )]
    pub parent_issue_id: craft_core::presence::InputField<String>,
    #[serde(
        default,
        skip_serializing_if = "craft_core::presence::InputField::is_omitted"
    )]
    pub assignee_account_id: craft_core::presence::InputField<String>,
}

impl CreateInput {
    /// Checks input that needs no remote metadata, before the host opens credentials.
    pub fn validate_basic(&self) -> Result<(), CraftError> {
        use craft_core::presence::InputField;
        if !numeric(&self.issue_type_id) {
            return Err(CraftError::field(
                "/issue_type_id",
                "expected_numeric_issue_type_id",
            ));
        }
        if self.summary.trim().is_empty() || self.summary.chars().count() > 255 {
            return Err(CraftError::field(
                "/summary",
                "expected_nonempty_summary_up_to_255_characters",
            ));
        }
        if matches!(&self.labels,InputField::Value(labels) if labels.len()>100 || labels.iter().any(|label| label.is_empty() || label.len()>255 || label.chars().any(|ch| ch.is_control() || ch.is_whitespace())))
        {
            return Err(CraftError::field("/labels", "invalid_bounded_labels"));
        }
        match &self.parent_issue_id {
            InputField::Omitted => {}
            InputField::Value(id) if numeric(id) => {}
            _ => {
                return Err(CraftError::field(
                    "/parent_issue_id",
                    "expected_numeric_parent_issue_id",
                ));
            }
        }
        match &self.assignee_account_id {
            InputField::Omitted => {}
            InputField::Null => {
                return Err(CraftError::new(
                    ErrorCode::UnsupportedCapability,
                    "Explicit unassignment requires qualified project policy.",
                ));
            }
            InputField::Value(id) if accounts::valid_account(id) => {}
            _ => {
                return Err(CraftError::field(
                    "/assignee_account_id",
                    "expected_exact_assignable_account_id",
                ));
            }
        }
        if self
            .custom_fields
            .keys()
            .any(|id| !id.strip_prefix("customfield_").is_some_and(numeric))
        {
            return Err(CraftError::field(
                "/custom_fields",
                "expected_native_custom_field_id",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Freshness {
    clock: Arc<dyn Clock>,
    wall: u64,
    monotonic: u64,
}

impl Freshness {
    fn check(&self) -> Result<(), CraftError> {
        let wall = self.clock.unix_millis().checked_sub(self.wall);
        let mono = self.clock.monotonic_millis().checked_sub(self.monotonic);
        if wall.is_none_or(|age| age >= FRESHNESS_MS) || mono.is_none_or(|age| age >= FRESHNESS_MS)
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Create metadata is stale or its clock moved backward; fetch it again.",
            ));
        }
        Ok(())
    }
}

pub struct CreateContext {
    summary: MetadataSummary,
    principal: MetadataPrincipal,
    freshness: Freshness,
    contract: FieldMapContract,
    context: ResourceRef,
    textarea: Vec<String>,
}

pub struct FrozenCreate {
    call: NativeCall,
    operation_digest: String,
    principal: MetadataPrincipal,
    freshness: Freshness,
    assignee_freshness: Option<Freshness>,
}

impl FrozenCreate {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
    ) -> Result<(), CraftError> {
        if self.operation_digest != operation.digest() || &self.principal != principal {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Frozen create no longer matches its operation or principal.",
            ));
        }
        self.freshness.check()?;
        if let Some(freshness) = &self.assignee_freshness {
            freshness.check()?;
        }
        Ok(())
    }
}

impl CreateContext {
    pub fn summary(&self) -> &MetadataSummary {
        &self.summary
    }

    pub fn prepare_create(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
    ) -> Result<FrozenCreate, CraftError> {
        self.prepare_create_with_assignee(operation, principal, None)
    }

    pub fn prepare_create_with_assignee(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
        assignee: Option<&AssigneeContext>,
    ) -> Result<FrozenCreate, CraftError> {
        if operation.request().command != crate::created_issue::COMMAND
            || !matches!(
                operation.effect(),
                craft_core::contracts::EffectClass::Write
            )
            || operation.request().target != self.summary.target
            || principal != &self.principal
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Create does not match the metadata target and principal.",
            ));
        }
        self.freshness.check()?;
        if self.summary.subtask && self.summary.parent.is_none() {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "Subtask creation requires qualified parent-project evidence.",
            ));
        }
        let input: CreateInput = serde_json::from_value(operation.request().input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_typed_create_input"))?;
        input.validate_basic()?;
        if input.issue_type_id != self.summary.issue_type_id {
            return Err(CraftError::field(
                "/issue_type_id",
                "issue_type_differs_from_qualified_metadata",
            ));
        }
        let mut values = serde_json::Map::new();
        values.insert("summary".into(), json!(input.summary));
        values.insert("project".into(), json!({"id":self.summary.target.id}));
        values.insert("issuetype".into(), json!({"id":self.summary.issue_type_id}));
        match &input.parent_issue_id {
            craft_core::presence::InputField::Value(id)
                if self.summary.subtask
                    && self
                        .summary
                        .parent
                        .as_ref()
                        .is_some_and(|parent| &parent.id == id) =>
            {
                values.insert("parent".into(), json!({"id":id}));
            }
            craft_core::presence::InputField::Omitted if !self.summary.subtask => {}
            _ => {
                return Err(CraftError::field(
                    "/parent_issue_id",
                    "parent_requires_matching_qualified_subtask_context",
                ));
            }
        }
        let assignee_freshness = match &input.assignee_account_id {
            craft_core::presence::InputField::Omitted => None,
            craft_core::presence::InputField::Null => {
                return Err(CraftError::new(
                    ErrorCode::UnsupportedCapability,
                    "Explicit unassignment requires qualified project policy.",
                ));
            }
            craft_core::presence::InputField::Value(id) => {
                let proof = assignee.ok_or_else(|| {
                    CraftError::field(
                        "/assignee_account_id",
                        "assignable_account_evidence_required",
                    )
                })?;
                proof.verify(&self.summary.target, principal, id)?;
                values.insert("assignee".into(), json!({"accountId":id}));
                Some(proof.freshness.clone())
            }
        };
        if !input.description.is_omitted() {
            values.insert("description".into(), json!(input.description));
        }
        if !input.labels.is_omitted() {
            values.insert("labels".into(), json!(input.labels));
        }
        for (id, value) in input.custom_fields {
            values.insert(id, value);
        }
        for field in &self.summary.fields {
            if values.contains_key(&field.id) && field.schema.is_none() {
                return Err(CraftError::field(
                    field_path(&field.id),
                    "field_schema_or_set_operation_unsupported",
                ));
            }
            if field.required && !field.has_default && !values.contains_key(&field.id) {
                return Err(CraftError::field(
                    field_path(&field.id),
                    "required_field_has_no_reported_default",
                ));
            }
        }
        self.contract
            .validate(&self.context, &Value::Object(values.clone()))
            .map_err(|mut error| {
                for issue in &mut error.details.issues {
                    if let Some(path) = issue.path.strip_prefix("/custom_fields/")
                        && let Some(name) = path.split('/').next()
                        && !name.starts_with("customfield_")
                    {
                        issue.path = format!(
                            "{}{}",
                            field_path(name),
                            path.strip_prefix(name).unwrap_or_default()
                        );
                    }
                }
                error
            })?;
        for id in &self.textarea {
            if let Some(Value::String(text)) = values.get(id) {
                values.insert(id.clone(), adf(text));
            }
        }
        let call = NativeCall {
            repeated_query: std::collections::BTreeMap::new(),
            route: crate::created_issue::ROUTE.into(),
            path_parameters: BTreeMap::new(),
            query: BTreeMap::new(),
            body: Some(json!({"fields":values})),
        };
        let body = serde_json::to_vec(&call.body).map_err(|_| malformed())?;
        if body.len() > craft_core::input::MAX_INPUT_BYTES {
            return Err(CraftError::field(
                "/input",
                "create_body_byte_limit_exceeded",
            ));
        }
        Ok(FrozenCreate {
            call,
            operation_digest: operation.digest().into(),
            principal: principal.clone(),
            freshness: self.freshness.clone(),
            assignee_freshness,
        })
    }
}

fn field_path(id: &str) -> String {
    if id.starts_with("customfield_") {
        format!("/custom_fields/{id}")
    } else {
        format!(
            "/{}",
            match id {
                "parent" => "parent_issue_id",
                "assignee" => "assignee_account_id",
                _ => id,
            }
        )
    }
}
fn numeric(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit())
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Create metadata is incomplete, inconsistent, malformed or exceeds its bound.",
    )
}
fn adf(text: &str) -> Value {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![json!({"type":"text","text":text})]
    };
    json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":content}]})
}

/// The host supplies its verified principal; this reader does not authenticate that assertion.
pub fn read_context(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<CreateContext, CraftError> {
    if operation.request().command != COMMAND
        || !matches!(operation.effect(), craft_core::contracts::EffectClass::Read)
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected a prepared create-metadata read.",
        ));
    }
    crate::created_issue::validate_target(&operation.request().target)?;
    let input: MetadataRequest = serde_json::from_value(operation.request().input.clone())
        .map_err(|_| CraftError::field("/input", "invalid_metadata_request"))?;
    input.validate()?;
    principal.validate()?;
    context.verify_operation(operation)?;
    context.checkpoint()?;
    let freshness = Freshness {
        wall: clock.unix_millis(),
        monotonic: clock.monotonic_millis(),
        clock,
    };
    let summary = reads::fetch(operation, context, &input, freshness.wall)?;
    freshness.check()?;
    let context_ref = ResourceRef {
        resource_type: "create_metadata".into(),
        id: canonical_digest(
            &json!({"target":summary.target,"issue_type":summary.issue_type_id,"parent":summary.parent}),
        )?,
        ..summary.target.clone()
    };
    let schemas = summary
        .fields
        .iter()
        .filter_map(|field| {
            field
                .schema
                .clone()
                .map(|schema| (field.id.clone(), schema))
        })
        .collect();
    let contract = FieldMapContract::new(context_ref.clone(), schemas)?;
    let textarea = summary
        .fields
        .iter()
        .filter(|field| fields::is_textarea(field))
        .map(|field| field.id.clone())
        .collect();
    Ok(CreateContext {
        summary,
        principal,
        freshness,
        contract,
        context: context_ref,
        textarea,
    })
}
