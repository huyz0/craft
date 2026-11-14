//! Content-state badge writes on a current published page. Only a state the space currently
//! offers can be selected; free-form name/color creation is not exposed. Badges never approve.
use super::*;
use crate::content_state::{observe_available, observe_state};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONTENT_STATE_SET: &str = "confluence.content-state.set";
pub const CONTENT_STATE_CLEAR: &str = "confluence.content-state.clear";
pub const CONTENT_STATE_SET_ROUTE: &str = "confluence-content-state-set";
pub const CONTENT_STATE_CLEAR_ROUTE: &str = "confluence-content-state-clear";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentStateSetInput {
    pub space_id: String,
    pub page_id: String,
    /// An ID from `confluence.content-state.available` for this page.
    pub state_id: u64,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentStateClearInput {
    pub space_id: String,
    pub page_id: String,
}
impl ContentStateSetInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || !numeric(&self.page_id) || self.state_id > i64::MAX as u64 {
            return Err(invalid("/input", "expected_numeric_space_page_and_state"));
        }
        Ok(())
    }
}
impl ContentStateClearInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || !numeric(&self.page_id) {
            return Err(invalid("/input", "expected_numeric_space_and_page"));
        }
        Ok(())
    }
}
fn check_page(page: &NativeResponse, page_id: &str, space_id: &str) -> Result<(), CraftError> {
    let raw = current(page)?;
    if string(raw, "id")? != page_id || string(raw, "spaceId")? != space_id {
        return Err(malformed());
    }
    Ok(())
}
fn call(kind: PageWriteKind, page_id: &str, body: Option<Value>) -> Result<NativeCall, CraftError> {
    let call = NativeCall {
        route: kind.route().into(),
        path_parameters: BTreeMap::from([("id".into(), page_id.into())]),
        // The lifecycle selector is mandatory: only the published page's badge is changed.
        query: BTreeMap::from([("status".into(), "current".into())]),
        repeated_query: BTreeMap::new(),
        body,
    };
    registration::route(kind).validate_call(&call)?;
    Ok(call)
}

/// Requires the selected state to be offered now for this page, then compiles one PUT by ID.
pub fn prepare_content_state_set(
    operation: &PreparedOperation,
    page: &NativeResponse,
    available: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, CONTENT_STATE_SET)?;
    let input: ContentStateSetInput = input::parse(&operation.request().input)?;
    check_page(page, &input.page_id, &input.space_id)?;
    if available.status != 200 {
        return Err(malformed());
    }
    let (space, custom) = observe_available(&available.body)?;
    if !space.iter().chain(&custom).any(|s| s.id == input.state_id) {
        return Err(invalid(
            "/state_id",
            "state_not_offered_by_current_space_settings",
        ));
    }
    let call = call(
        PageWriteKind::ContentStateSet,
        &input.page_id,
        Some(json!({"id":input.state_id})),
    )?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::ContentStateSet,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: Some(input.page_id.clone()),
        parent_id: None,
        id: Some(input.page_id),
        title: input.state_id.to_string(),
        version: 0,
        call,
    })
}

/// Compiles one DELETE of the published page's badge.
pub fn prepare_content_state_clear(
    operation: &PreparedOperation,
    page: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, CONTENT_STATE_CLEAR)?;
    let input: ContentStateClearInput = input::parse(&operation.request().input)?;
    check_page(page, &input.page_id, &input.space_id)?;
    let call = call(PageWriteKind::ContentStateClear, &input.page_id, None)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::ContentStateClear,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: input.space_id,
        page_id: Some(input.page_id.clone()),
        parent_id: None,
        id: Some(input.page_id),
        title: String::new(),
        version: 0,
        call,
    })
}

impl PreparedPageWrite {
    /// The response must show exactly the selected badge, or no badge after clearing.
    pub(super) fn content_state_confirmed(&self, raw: &Value) -> bool {
        match (self.kind, observe_state(raw)) {
            (PageWriteKind::ContentStateSet, Ok(Some(state))) => state.id.to_string() == self.title,
            (PageWriteKind::ContentStateClear, Ok(None)) => true,
            _ => false,
        }
    }
}
