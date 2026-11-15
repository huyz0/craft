//! Complete page attachment bytes. The native route always redirects to Atlassian's media service;
//! the streaming host follows exactly that one redirect without Craft credentials.
use crate::attachments::valid_attachment_id;
use crate::models::numeric;
use crate::registration::validate_target;
use craft_core::contracts::{
    CanonicalRequest, CredentialCacheUpdate, EffectClass, Envelope, RequestState, ResourceRef,
    SCHEMA_VERSION,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeRoute, OperationContext, OperationHandler, PreparedOperation,
    RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_core::storage::ArtifactReceipt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const DOWNLOAD: &str = "confluence.attachment.download";
const ROUTE: &str = "confluence-attachment-download";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadInput {
    #[schemars(regex(pattern = "^att[1-9][0-9]*$"))]
    pub id: String,
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub page_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadData {
    pub target: ResourceRef,
    pub attachment_id: String,
    pub page_id: String,
    pub artifact_name: String,
    pub artifact: Option<ArtifactReceipt>,
    pub artifact_write_state: RequestState,
    pub exported: Option<ArtifactReceipt>,
    pub export_write_state: RequestState,
    pub received_bytes: u64,
    pub response_media_type: Option<String>,
    /// True once the credential-free request to the media service was sent.
    pub followed_media_redirect: bool,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    validate_target(&request.target)?;
    let input: DownloadInput = serde_json::from_value(request.input.clone())
        .map_err(|_| CraftError::field("/input", "expected_attachment_and_page_ids"))?;
    if request.command != DOWNLOAD
        || !valid_attachment_id(&input.id)
        || !numeric(&input.page_id)
        || request.preconditions.is_some()
        || request.selection.cursor.is_some()
        || !request.selection.fields.is_empty()
    {
        return Err(CraftError::field(
            "/request",
            "expected_page_attachment_download_without_cursor_projection_or_preconditions",
        ));
    }
    Ok(())
}
pub fn call(operation: &PreparedOperation) -> Result<NativeCall, CraftError> {
    validate(operation.request())?;
    let input: DownloadInput = serde_json::from_value(operation.request().input.clone())
        .map_err(|_| CraftError::field("/input", "expected_attachment_and_page_ids"))?;
    Ok(NativeCall {
        route: ROUTE.into(),
        path_parameters: BTreeMap::from([
            ("id".into(), input.page_id),
            ("attachmentId".into(), input.id),
        ]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    })
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate(request)
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Attachment bytes require the verified streaming host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "site".into(),
        id: "00000000-1111-2222-3333-444444444444".into(),
    };
    let data = DownloadData {
        target,
        attachment_id: "att301".into(),
        page_id: "101".into(),
        artifact_name: "attachment.bin".into(),
        artifact: None,
        artifact_write_state: RequestState::NotSent,
        exported: None,
        export_write_state: RequestState::NotSent,
        received_bytes: 0,
        response_media_type: None,
        followed_media_redirect: false,
        credential_cache_update: None,
    };
    let path = ["wiki", "rest", "api", "content"]
        .into_iter()
        .map(|v| RouteSegment::Literal(v.into()))
        .chain([
            RouteSegment::Parameter("id".into()),
            RouteSegment::Literal("child".into()),
            RouteSegment::Literal("attachment".into()),
            RouteSegment::Parameter("attachmentId".into()),
            RouteSegment::Literal("download".into()),
        ])
        .collect();
    registry.register_typed_handler::<DownloadInput>(
        OperationDescriptor {
            id: DOWNLOAD.into(),
            provider: "confluence".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "confluence-cloud-v1-attachment-download".into(),
            description: "Stream page attachment bytes to an artifact and guarded output; follows one credential-free media redirect.".into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<DownloadInput>()?,
            response_schema: response_schema::<DownloadData>()?,
            example: json!({"id":"att301","page_id":"101"}),
            response_example: json!(Envelope::success(DOWNLOAD, "example".into(), json!(data))),
            projection: None,
            scope_alternatives: vec![
                vec!["readonly:content.attachment:confluence".into()],
                vec!["read:attachment:confluence".into()],
            ],
            permissions: vec!["View the page that holds the attachment.".into()],
            tenant_conditions: vec!["Explicit download allowance required. One redirect, only to https://api.media.atlassian.com, is followed without Craft credentials. Local artifact and export receipts have independent dispositions.".into()],
            routes: vec![NativeRoute {
                id: ROUTE.into(),
                method: HttpMethod::Get,
                path,
                query_parameters: vec![],
                continuation_parameters: vec![],
                repeated_query_parameters: vec![],
                response_max_bytes: 512,
            }],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        },
        Arc::new(Handler),
    )
}
