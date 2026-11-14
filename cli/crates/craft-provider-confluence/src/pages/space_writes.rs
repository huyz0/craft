//! Space creation through the v2 route, which Atlassian offers on tenants with role-based access
//! control. The key must be absent in a fresh exact-key lookup; visibility is always explicit.
//! There is no fallback to the legacy v1 route, so an uncertain v2 create is never replayed there.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SPACE_CREATE: &str = "confluence.space.create";
pub const SPACE_CREATE_ROUTE: &str = "confluence-space-create";

/// `default_roles` applies the site's default space roles; `private` restricts the space to its
/// creator. There is no implicit default.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpaceVisibility {
    DefaultRoles,
    Private,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceCreateInput {
    #[schemars(length(min = 1, max = 255), regex(pattern = "^[A-Za-z0-9]+$"))]
    pub key: String,
    pub name: String,
    pub visibility: SpaceVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
impl SpaceCreateInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if self.key.is_empty()
            || self.key.len() > 255
            || !self.key.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(invalid("/key", "expected_alphanumeric_space_key"));
        }
        if !text(&self.name, 200) {
            return Err(invalid("/name", "expected_nonempty_space_name"));
        }
        if self
            .description
            .as_ref()
            .is_some_and(|d| d.trim().is_empty() || d.len() > 4096 || d.contains('\0'))
        {
            return Err(invalid(
                "/description",
                "expected_bounded_plain_description",
            ));
        }
        Ok(())
    }
}

/// The exact-key space lookup must have completed with no results; any match, ambiguity or
/// continuation stops before the POST.
pub fn prepare_space_create(
    operation: &PreparedOperation,
    lookup: &NativeResponse,
) -> Result<PreparedPageWrite, CraftError> {
    validate_operation(operation, SPACE_CREATE)?;
    let input: SpaceCreateInput = input::parse(&operation.request().input)?;
    let results = lookup
        .body
        .get("results")
        .and_then(Value::as_array)
        .filter(|_| lookup.status == 200)
        .ok_or_else(malformed)?;
    if !results.is_empty() {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "A space with this key already exists or is visible; no space was created.",
        ));
    }
    let mut body = json!({
        "name": input.name,
        "key": input.key,
        "createPrivateSpace": input.visibility == SpaceVisibility::Private,
    });
    if let Some(description) = &input.description {
        body["description"] = json!({"value": description, "representation": "plain"});
    }
    let call = NativeCall {
        route: SPACE_CREATE_ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: Some(body),
    };
    registration::route(PageWriteKind::SpaceCreate).validate_call(&call)?;
    Ok(PreparedPageWrite {
        kind: PageWriteKind::SpaceCreate,
        operation_digest: operation.digest().into(),
        site: operation.request().target.clone(),
        space_id: String::new(),
        page_id: None,
        parent_id: Some(input.key),
        id: None,
        title: input.name,
        version: 0,
        call,
    })
}
