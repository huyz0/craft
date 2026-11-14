//! Pure page publication preparation. The caller must supply verified fresh evidence and a journaled host.
mod input;
mod registration;
mod result;
use crate::models::{numeric, text};
use craft_core::contracts::{CanonicalRequest, EffectClass, ResourceRef};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, PreparedOperation};
use craft_core::input::canonical_digest;
use craft_core::presence::InputField;
pub use input::{
    EditorDraftPolicy, PageBody, PageCreateInput, PagePlacement, PageRepresentation, PageRevision,
    PageUpdateInput,
};
pub use registration::ConfluencePages;
pub use result::PageWriteObservation;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const PAGE_CREATE: &str = "confluence.page.create";
pub const PAGE_UPDATE: &str = "confluence.page.update";
pub const CREATE_ROUTE: &str = "confluence-page-create";
pub const UPDATE_ROUTE: &str = "confluence-page-update";

fn invalid(path: &str, rule: &str) -> CraftError {
    CraftError::field(path, rule)
}
fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence did not return the required current page or space evidence.",
    )
}
fn conflict() -> CraftError {
    CraftError::new(
        ErrorCode::VersionConflict,
        "The current published page differs from the expected revision. Read and review it before preparing another update.",
    )
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, CraftError> {
    value.get(key).and_then(Value::as_str).ok_or_else(malformed)
}
fn current(response: &NativeResponse) -> Result<&Value, CraftError> {
    if response.status != 200
        || response.body.get("status").and_then(Value::as_str) != Some("current")
        || response.body.get("subtype").is_some_and(|v| !v.is_null())
    {
        return Err(malformed());
    }
    Ok(&response.body)
}

/// Exact published page data. Obtaining this value does not establish service identity or freshness.
pub struct PageSnapshot {
    id: String,
    space_id: String,
    title: String,
    version: u32,
    body: PageBody,
}
impl PageSnapshot {
    pub fn revision(&self) -> Result<PageRevision, CraftError> {
        Ok(PageRevision {
            version: self.version,
            representation: self.body.representation,
            digest: canonical_digest(
                &json!({"id":self.id,"space_id":self.space_id,"title":self.title,"version":self.version,"body":self.body}),
            )?,
        })
    }
}

/// Reads one complete representation from a native published-page response without following links.
pub fn observe_page(
    response: &NativeResponse,
    representation: PageRepresentation,
) -> Result<PageSnapshot, CraftError> {
    let raw = current(response)?;
    let id = string(raw, "id")?;
    let space_id = string(raw, "spaceId")?;
    let title = string(raw, "title")?;
    let version = raw
        .pointer("/version/number")
        .and_then(Value::as_u64)
        .filter(|v| (1..=i32::MAX as u64).contains(v))
        .ok_or_else(malformed)?;
    let body = raw
        .get("body")
        .and_then(|b| b.get(representation.native()))
        .ok_or_else(malformed)?;
    if !numeric(id)
        || !numeric(space_id)
        || !text(title, 4096)
        || string(body, "representation")? != representation.native()
    {
        return Err(malformed());
    }
    let body = PageBody {
        representation,
        value: string(body, "value")?.into(),
    };
    body.validate().map_err(|_| malformed())?;
    Ok(PageSnapshot {
        id: id.into(),
        space_id: space_id.into(),
        title: title.into(),
        version: version as u32,
        body,
    })
}

/// A bound native payload, with no transport/replay authority. Keep the service grant lock and
/// original evidence freshness checks through the host's final claimed-send callback.
pub struct PreparedPageWrite {
    operation_digest: String,
    site: ResourceRef,
    space_id: String,
    id: Option<String>,
    title: String,
    version: u32,
    call: NativeCall,
}
impl PreparedPageWrite {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn operation_digest(&self) -> &str {
        &self.operation_digest
    }
    pub fn space_id(&self) -> &str {
        &self.space_id
    }
}
fn validate_operation(operation: &PreparedOperation, command: &str) -> Result<(), CraftError> {
    input::validate_request(operation.request())?;
    if operation.request().command != command || !matches!(operation.effect(), EffectClass::Write) {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "The prepared operation is not the requested page write.",
        ));
    }
    Ok(())
}

/// Checks explicit placement against observed space/parent data and compiles one normal page POST.
/// Evidence must have been fetched under the host's exact site, actor, grant and freshness policy.
pub fn prepare_create(
    operation: &PreparedOperation,
    space: &NativeResponse,
    parent: Option<&NativeResponse>,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, PAGE_CREATE)?;
    let input: PageCreateInput = input::parse(&operation.request().input)?;
    let s = current(space)?;
    if string(s, "id")? != input.space_id {
        return Err(malformed());
    }
    let parent_id = match &input.placement {
        PagePlacement::Root => {
            if parent.is_some() {
                return Err(invalid("/placement", "root_placement_has_no_parent"));
            }
            None
        }
        PagePlacement::Homepage => Some(string(s, "homepageId")?),
        PagePlacement::Parent { id } => Some(id.as_str()),
    };
    if let Some(id) = parent_id {
        let p = current(parent.ok_or_else(malformed)?)?;
        if !numeric(id) || string(p, "id")? != id || string(p, "spaceId")? != input.space_id {
            return Err(malformed());
        }
    }
    let mut body =
        json!({"spaceId":input.space_id,"status":"current","title":input.title,"body":input.body});
    if let Some(id) = parent_id {
        body["parentId"] = json!(id);
    }
    let call = NativeCall {
        route: CREATE_ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::from([("root-level".into(), parent_id.is_none().to_string())]),
        repeated_query: BTreeMap::new(),
        body: Some(body),
    };
    registration::route(false).validate_call(&call)?;
    Ok(PreparedPageWrite {
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        id: None,
        title: input.title,
        version: 1,
        call,
    })
}

/// Compares both version and exact content fingerprint, preserving omitted title/body values.
/// Published CAS cannot establish safety around other users' unpublished editor drafts.
pub fn prepare_update(
    operation: &PreparedOperation,
    page: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, PAGE_UPDATE)?;
    let input: PageUpdateInput = input::parse(&operation.request().input)?;
    let observed = observe_page(page, input.expected.representation)?;
    if observed.id != input.id
        || observed.space_id != input.space_id
        || observed.version != input.expected.version
        || observed.revision()?.digest != input.expected.digest
    {
        return Err(conflict());
    }
    let title = match input.title {
        InputField::Value(value) => value,
        _ => observed.title,
    };
    let body = match input.body {
        InputField::Value(value) => value,
        _ => observed.body,
    };
    let version = observed
        .version
        .checked_add(1)
        .filter(|v| *v <= i32::MAX as u32)
        .ok_or_else(conflict)?;
    let call = NativeCall {
        route: UPDATE_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), input.id.clone())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: Some(
            json!({"id":input.id,"status":"current","title":title,"body":body,"version":{"number":version}}),
        ),
    };
    registration::route(true).validate_call(&call)?;
    Ok(PreparedPageWrite {
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        id: Some(input.id),
        title,
        version,
        call,
    })
}
