//! Current footer-comment observations. A version author is not proof of human approval.
use crate::models::{GetData, numeric};
use crate::registration::validate_target;
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::input::canonical_digest;
use craft_core::presence::InputField;
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
pub const COMMENT_GET: &str = "confluence.comment.get";
const ROUTE: &str = "confluence-footer-comment-get";
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentGetInput {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]*$"))]
    pub page_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRevision {
    #[schemars(range(min = 1, max = 2147483647))]
    pub version: u32,
    #[schemars(regex(pattern = "^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
    #[schemars(regex(pattern = "^(storage|atlas_doc_format)$"))]
    pub representation: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentSummary {
    pub id: String,
    pub page_id: String,
    pub status: String,
    pub version: u32,
    pub version_author_id: String,
    pub version_created_at: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub parent_comment_id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub body_storage: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub body_atlas_doc_format: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub revision: InputField<CommentRevision>,
    pub body_state: String,
    pub fields_not_returned: Vec<String>,
}
pub fn validate_input(input: &CommentGetInput) -> Result<(), CraftError> {
    if !numeric(&input.id) || !numeric(&input.page_id) {
        return Err(CraftError::field(
            "/input",
            "expected_exact_comment_and_page_ids",
        ));
    }
    Ok(())
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Comment response did not establish its current page, version or selected content.",
    )
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str, CraftError> {
    v[k].as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .ok_or_else(malformed)
}
pub(crate) fn projection() -> ProjectionContract {
    let metadata = [
        "id",
        "page_id",
        "status",
        "version",
        "version_author_id",
        "version_created_at",
        "body_state",
        "fields_not_returned",
    ]
    .map(String::from)
    .to_vec();
    ProjectionContract {
        item_pointer: "/item".into(),
        always_returned: metadata.clone(),
        default_selected: metadata,
        mode: ProjectionMode::Mixed,
    }
}
fn selected(fields: &[String]) -> Result<(BTreeSet<String>, Option<&'static str>), CraftError> {
    projection().compile(&response_schema::<GetData<CommentSummary>>()?, fields)?;
    let mut selected: BTreeSet<_> = fields.iter().cloned().collect();
    selected.extend(projection().always_returned);
    let body = match (
        selected.contains("body_storage"),
        selected.contains("body_atlas_doc_format"),
    ) {
        (true, true) => {
            return Err(CraftError::field(
                "/fields",
                "select_one_primary_body_representation",
            ));
        }
        (_, true) => Some("atlas_doc_format"),
        (true, false) => Some("storage"),
        _ if selected.contains("revision") => Some("storage"),
        _ => None,
    };
    Ok((selected, body))
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "site".into(),
        id: "00000000-1111-2222-3333-444444444444".into(),
    };
    registry.register_typed_handler::<CommentGetInput>(OperationDescriptor {
        id:COMMENT_GET.into(),provider:"confluence".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"confluence-cloud-v2".into(),description:"Read one current page-bound footer comment; version authors and text do not grant approval.".into(),effect:EffectClass::Read,
        input_schema:schema_for::<CommentGetInput>()?,response_schema:response_schema::<GetData<CommentSummary>>()?,example:json!({"id":"201","page_id":"101"}),
        response_example:json!(Envelope::success(COMMENT_GET,"example".into(),json!({"target":target,"item":{"id":"201","page_id":"101","status":"current","version":1,"version_author_id":"reviewer-account","version_created_at":"2026-11-10T00:00:00.000Z","body_state":"not_requested","fields_not_returned":[]},"observed_at":1000,"credential_cache_update":null}))),
        projection:Some(projection()),scope_alternatives:vec![vec!["read:comment:confluence".into()]],permissions:vec!["View the page container and its space".into()],tenant_conditions:vec!["Exact current page association; no human eligibility or review authority is inferred".into()],
        routes:vec![NativeRoute {id:ROUTE.into(),method:HttpMethod::Get,path:["wiki","api","v2","footer-comments"].map(|v|RouteSegment::Literal(v.into())).into_iter().chain([RouteSegment::Parameter("comment-id".into())]).collect(),query_parameters:vec!["body-format".into(),"include-version".into()],repeated_query_parameters:vec![],continuation_parameters:vec![],response_max_bytes:1_048_576}],retry_class:RetryClass::SafeRead,reconciliation:ReconciliationClass::NotApplicable,
    },Arc::new(Handler))?;
    paging::register(registry)
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        let input: CommentGetInput = serde_json::from_value(request.input.clone())
            .map_err(|_| CraftError::field("/input", "invalid_comment_read"))?;
        validate_input(&input)?;
        if request.preconditions.is_some()
            || request.selection.cursor.is_some()
            || request.selection.limit != craft_core::contracts::Selection::default().limit
        {
            return Err(CraftError::field(
                "/selection",
                "comment_get_has_no_paging_or_preconditions",
            ));
        }
        selected(&request.selection.fields)?;
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Comment reads require the verified host and saved policy.",
        ))
    }
}
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    let request = operation.request();
    if request.command != COMMENT_GET {
        return Err(CraftError::field("/command", "expected_comment_get"));
    }
    let input: CommentGetInput =
        serde_json::from_value(request.input.clone()).map_err(|_| malformed())?;
    validate_input(&input)?;
    let (fields, representation) = selected(&request.selection.fields)?;
    let mut query = BTreeMap::from([("include-version".into(), "true".into())]);
    if let Some(body) = representation {
        query.insert("body-format".into(), body.into());
    }
    context.checkpoint()?;
    let response = context.invoke(&NativeCall {
        route: ROUTE.into(),
        path_parameters: BTreeMap::from([("comment-id".into(), input.id.clone())]),
        query,
        repeated_query: BTreeMap::new(),
        body: None,
    })?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Comment is unavailable or unreadable; no absence, approval or retry was established.",
        ));
    }
    let encoded = serde_json::to_vec(&response.body).map_err(|_| malformed())?;
    let bounded = craft_core::input::parse_json(&encoded, 1_048_576).map_err(|_| malformed())?;
    let item = normalize(&bounded, &input, &fields, representation)?;
    context.checkpoint()?;
    Ok(item)
}
/// Fingerprint of one current footer comment in the selected representation, computed exactly as
/// `confluence.comment.get` reports `revision`.
pub fn observe_revision(
    raw: &Value,
    id: &str,
    page_id: &str,
    representation: &str,
) -> Result<CommentRevision, CraftError> {
    if !matches!(representation, "storage" | "atlas_doc_format") {
        return Err(malformed());
    }
    let input = CommentGetInput {
        id: id.into(),
        page_id: page_id.into(),
    };
    let fields = BTreeSet::from(["revision".to_string()]);
    let item = normalize(raw, &input, &fields, Some(representation))?;
    serde_json::from_value(item["revision"].clone()).map_err(|_| malformed())
}
fn normalize(
    raw: &Value,
    input: &CommentGetInput,
    fields: &BTreeSet<String>,
    representation: Option<&str>,
) -> Result<Value, CraftError> {
    if string(raw, "id")? != input.id
        || string(raw, "pageId")? != input.page_id
        || raw["status"] != "current"
        || ["blogPostId", "attachmentId", "customContentId"]
            .iter()
            .any(|k| raw.get(k).is_some_and(|v| !v.is_null()))
    {
        return Err(malformed());
    }
    let version = raw["version"]["number"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= i32::MAX as u64)
        .ok_or_else(malformed)?;
    let author = string(&raw["version"], "authorId")?;
    craft_core::contracts::validate_identity("/comment/version_author_id", author)
        .map_err(|_| malformed())?;
    let created = string(&raw["version"], "createdAt")?;
    let mut item = json!({"id":input.id,"page_id":input.page_id,"status":"current","version":version,"version_author_id":author,"version_created_at":created,"body_state":"not_requested","fields_not_returned":[]});
    let parent = match raw.get("parentCommentId") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str().filter(|v| numeric(v)).ok_or_else(malformed)?),
    };
    if fields.contains("parent_comment_id") {
        item["parent_comment_id"] = json!(parent);
    }
    if let Some(representation) = representation {
        let body = &raw["body"][representation];
        if body["representation"] != representation {
            return Err(malformed());
        }
        let value = body["value"]
            .as_str()
            .filter(|v| v.len() <= 1_048_576 && !v.contains('\0'))
            .ok_or_else(malformed)?;
        let field = if representation == "storage" {
            "body_storage"
        } else {
            "body_atlas_doc_format"
        };
        if fields.contains(field) {
            item[field] = json!(value);
            item["body_state"] = json!("returned");
        }
        if fields.contains("revision") {
            item["revision"] = json!(CommentRevision {
                version: version as u32,
                representation: representation.into(),
                digest: canonical_digest(
                    &json!({"id":input.id,"page_id":input.page_id,"parent_comment_id":parent,"status":"current","version":version,"version_author_id":author,"version_created_at":created,"representation":representation,"body":value})
                )?
            });
        }
    }
    Ok(item)
}

pub mod paging;
