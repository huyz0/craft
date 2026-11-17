//! Page display width. Confluence keeps it in two page content properties with the same value:
//! `content-appearance-published` decides what viewers see and `content-appearance-draft` what the
//! editor opens with. Setting only the published one lets the next editor publish revert it, so a
//! width write compiles one property write for each, published first.
use super::*;
use craft_core::contracts::{EffectClass, Envelope, SCHEMA_VERSION};
use craft_core::execution::{
    HttpMethod, NativeRoute, OperationContext, OperationHandler, RouteSegment,
};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const PAGE_WIDTH_SET: &str = "confluence.page.width.set";
/// Internal evidence read for the width host; it is not a public command.
pub const PAGE_WIDTH_OBSERVE: &str = "confluence.page.width.observe";
pub const PROPERTY_READ_ROUTE: &str = "confluence-page-property-read";
pub const PROPERTY_CREATE_ROUTE: &str = "confluence-page-property-create";
pub const PROPERTY_UPDATE_ROUTE: &str = "confluence-page-property-update";

/// The display widths the Confluence editor offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageWidth {
    /// The 760px reading column; the native default when no property exists.
    Fixed,
    /// Content up to 1800px wide.
    FullWidth,
    /// Edge to edge with no cap.
    Max,
}
impl PageWidth {
    pub fn native(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::FullWidth => "full-width",
            Self::Max => "max",
        }
    }
}

/// One of the two appearance properties. The property key is also the journal step name, so
/// each property write is claimed and recorded separately under one operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub enum WidthProperty {
    #[serde(rename = "content-appearance-published")]
    Published,
    #[serde(rename = "content-appearance-draft")]
    Draft,
}
impl WidthProperty {
    /// Send order: viewers' width first, then the editor's.
    pub const ORDER: [Self; 2] = [Self::Published, Self::Draft];
    pub fn key(self) -> &'static str {
        match self {
            Self::Published => "content-appearance-published",
            Self::Draft => "content-appearance-draft",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageWidthSetInput {
    pub space_id: String,
    pub page_id: String,
    pub width: PageWidth,
}
impl PageWidthSetInput {
    pub(super) fn validate(&self) -> Result<(), CraftError> {
        if !numeric(&self.space_id) || !numeric(&self.page_id) {
            return Err(invalid("/input", "expected_numeric_space_and_page"));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageWidthObserveInput {
    pub page_id: String,
    pub key: WidthProperty,
}

fn page_path() -> Vec<RouteSegment> {
    ["wiki", "api", "v2", "pages"]
        .map(|p| RouteSegment::Literal(p.into()))
        .into_iter()
        .collect()
}
pub(super) fn routes() -> [NativeRoute; 2] {
    let mut create = page_path();
    create.extend([
        RouteSegment::Parameter("id".into()),
        RouteSegment::Literal("properties".into()),
    ]);
    let mut update = page_path();
    update.extend([
        RouteSegment::Parameter("page-id".into()),
        RouteSegment::Literal("properties".into()),
        RouteSegment::Parameter("property-id".into()),
    ]);
    [
        NativeRoute {
            id: PROPERTY_CREATE_ROUTE.into(),
            method: HttpMethod::Post,
            path: create,
            query_parameters: vec![],
            continuation_parameters: vec![],
            repeated_query_parameters: vec![],
            response_max_bytes: 65_536,
        },
        NativeRoute {
            id: PROPERTY_UPDATE_ROUTE.into(),
            method: HttpMethod::Put,
            path: update,
            query_parameters: vec![],
            continuation_parameters: vec![],
            repeated_query_parameters: vec![],
            response_max_bytes: 65_536,
        },
    ]
}
fn read_route() -> NativeRoute {
    let mut path = page_path();
    path.extend([
        RouteSegment::Parameter("id".into()),
        RouteSegment::Literal("properties".into()),
    ]);
    NativeRoute {
        id: PROPERTY_READ_ROUTE.into(),
        method: HttpMethod::Get,
        path,
        query_parameters: vec!["key".into()],
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 65_536,
    }
}
/// The exact property lookup the host sends for one key.
pub fn width_read_call(page_id: &str, property: WidthProperty) -> NativeCall {
    NativeCall {
        route: PROPERTY_READ_ROUTE.into(),
        path_parameters: BTreeMap::from([("id".into(), page_id.into())]),
        query: BTreeMap::from([("key".into(), property.key().into())]),
        repeated_query: BTreeMap::new(),
        body: None,
    }
}

/// An existing property's identity and version; `None` means the key is absent.
struct Observed {
    id: String,
    version: u64,
}
/// A key-filtered list must hold at most one entry with exactly that key and no further page.
fn observe_property(
    response: &NativeResponse,
    property: WidthProperty,
) -> Result<Option<Observed>, CraftError> {
    let raw = &response.body;
    let results = raw
        .get("results")
        .and_then(Value::as_array)
        .filter(|_| response.status == 200)
        .ok_or_else(malformed)?;
    if raw.pointer("/_links/next").is_some_and(|v| !v.is_null()) || results.len() > 1 {
        return Err(malformed());
    }
    let Some(entry) = results.first() else {
        return Ok(None);
    };
    let id = string(entry, "id")?;
    let version = entry
        .pointer("/version/number")
        .and_then(Value::as_u64)
        .filter(|v| (1..i32::MAX as u64).contains(v))
        .ok_or_else(malformed)?;
    if string(entry, "key")? != property.key() || !numeric(id) {
        return Err(malformed());
    }
    Ok(Some(Observed {
        id: id.into(),
        version,
    }))
}

/// Requires the current page in the space and one fresh observation per property, then compiles
/// a create (absent key) or a next-version update for each, in send order. Each write sets the
/// full value, so a later operation that re-reads both properties converges without replaying an
/// earlier claimed request.
pub fn prepare_page_width_set(
    operation: &PreparedOperation,
    page: &NativeResponse,
    published: &NativeResponse,
    draft: &NativeResponse,
) -> Result<Vec<PreparedPageWrite>, CraftError> {
    validate_operation(operation, PAGE_WIDTH_SET)?;
    let input: PageWidthSetInput = input::parse(&operation.request().input)?;
    let raw = current(page)?;
    if string(raw, "id")? != input.page_id || string(raw, "spaceId")? != input.space_id {
        return Err(malformed());
    }
    let [create, update] = routes();
    let value = input.width.native();
    WidthProperty::ORDER
        .into_iter()
        .zip([published, draft])
        .map(|(property, response)| {
            let key = property.key();
            let (call, route, version) = match observe_property(response, property)? {
                None => (
                    NativeCall {
                        route: PROPERTY_CREATE_ROUTE.into(),
                        path_parameters: BTreeMap::from([("id".into(), input.page_id.clone())]),
                        query: BTreeMap::new(),
                        repeated_query: BTreeMap::new(),
                        body: Some(json!({"key":key,"value":value})),
                    },
                    &create,
                    // A created property's first version is assigned natively.
                    0,
                ),
                Some(existing) => {
                    let next = existing.version + 1;
                    (
                        NativeCall {
                            route: PROPERTY_UPDATE_ROUTE.into(),
                            path_parameters: BTreeMap::from([
                                ("page-id".into(), input.page_id.clone()),
                                ("property-id".into(), existing.id),
                            ]),
                            query: BTreeMap::new(),
                            repeated_query: BTreeMap::new(),
                            body: Some(json!({"key":key,"value":value,"version":{"number":next}})),
                        },
                        &update,
                        next as u32,
                    )
                }
            };
            route.validate_call(&call)?;
            Ok(PreparedPageWrite {
                kind: PageWriteKind::WidthSet,
                operation_digest: operation.digest().into(),
                site: operation.request().target.clone(),
                space_id: input.space_id.clone(),
                page_id: Some(input.page_id.clone()),
                parent_id: Some(key.into()),
                id: Some(input.page_id.clone()),
                title: value.into(),
                version,
                call,
            })
        })
        .collect()
}

impl PreparedPageWrite {
    /// The content property key this width write targets, which is also its journal step.
    pub fn width_property(&self) -> Option<&str> {
        (self.kind == PageWriteKind::WidthSet)
            .then_some(self.parent_id.as_deref())
            .flatten()
    }
    /// Confirmed only by the compiled key and value, and for an update the next version.
    pub(super) fn width_confirmed(&self, raw: &Value) -> bool {
        let version = raw.pointer("/version/number").and_then(Value::as_u64);
        raw.get("key").and_then(Value::as_str) == self.parent_id.as_deref()
            && raw.get("value").and_then(Value::as_str) == Some(&self.title)
            && match self.version {
                0 => version.is_some_and(|v| v >= 1),
                expected => version == Some(u64::from(expected)),
            }
    }
}

pub(super) fn register_observe(registry: &mut Registry) -> Result<(), CraftError> {
    registry.register_typed_handler::<PageWidthObserveInput>(
        OperationDescriptor {
            id: PAGE_WIDTH_OBSERVE.into(),
            provider: "confluence".into(),
            contract_version: SCHEMA_VERSION.into(),
            api_revision: "confluence-cloud-v2".into(),
            description: "Read one page appearance property as width-write evidence.".into(),
            effect: EffectClass::Read,
            input_schema: schema_for::<PageWidthObserveInput>()?,
            response_schema: json!({"type":"object"}),
            example: json!({"page_id":"101","key":"content-appearance-published"}),
            response_example: json!(Envelope::success(
                PAGE_WIDTH_OBSERVE,
                "example".into(),
                json!({})
            )),
            projection: None,
            scope_alternatives: vec![vec!["read:page:confluence".into()]],
            permissions: vec!["View the page.".into()],
            tenant_conditions: vec!["Only the width host reads this evidence.".into()],
            routes: vec![read_route()],
            retry_class: RetryClass::SafeRead,
            reconciliation: ReconciliationClass::NotApplicable,
        },
        Arc::new(ObserveHandler),
    )
}
struct ObserveHandler;
impl OperationHandler for ObserveHandler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        crate::registration::validate_target(&request.target)?;
        let input: PageWidthObserveInput = input::parse(&request.input)?;
        if !numeric(&input.page_id) {
            return Err(invalid("/page_id", "expected_numeric_page"));
        }
        Ok(())
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Width evidence is read only by the verified journaled host.",
        ))
    }
}
