use crate::{models::*, registration::*};
use craft_core::error::{CraftError, ErrorCode};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub(crate) fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Confluence response did not establish valid selected data, identity or continuation.",
    )
}
pub(crate) struct FieldPlan {
    selected: BTreeSet<String>,
    pub body: Option<&'static str>,
    pub description: Option<&'static str>,
}
impl FieldPlan {
    pub fn compile(command: &str, fields: &[String]) -> Result<Self, CraftError> {
        let contract = projection(command);
        contract.compile(&response_schema_for(command)?, fields)?;
        let mut selected: BTreeSet<_> = if fields.is_empty() {
            contract.default_selected
        } else {
            fields.to_vec()
        }
        .into_iter()
        .collect();
        selected.extend(contract.always_returned);
        if selected.contains("revision") && command != PAGE_GET {
            return Err(CraftError::field(
                "/fields",
                "revision_requires_current_page_get",
            ));
        }
        let body = match (
            selected.contains("body_storage"),
            selected.contains("body_atlas_doc_format"),
        ) {
            (true, false) => Some("storage"),
            (false, true) => Some("atlas_doc_format"),
            (false, false) if selected.contains("revision") => Some("storage"),
            (false, false) => None,
            _ => {
                return Err(CraftError::field(
                    "/fields",
                    "select_one_primary_body_representation",
                ));
            }
        };
        let description = match (
            selected.contains("description_plain"),
            selected.contains("description_view"),
        ) {
            (true, false) => Some("plain"),
            (false, true) => Some("view"),
            (false, false) => None,
            _ => {
                return Err(CraftError::field(
                    "/fields",
                    "select_one_space_description_representation",
                ));
            }
        };
        Ok(Self {
            selected,
            body,
            description,
        })
    }
    pub fn wants(&self, key: &str) -> bool {
        self.selected.contains(key)
    }
    pub fn expansions(&self) -> String {
        let mut names = BTreeSet::new();
        if self.wants("space_id") {
            names.insert("content.space".to_owned());
        }
        if self.wants("version") {
            names.insert("content.version".to_owned());
        }
        if let Some(body) = self.body {
            names.insert(format!("content.body.{body}"));
        }
        names.into_iter().collect::<Vec<_>>().join(",")
    }
    pub fn normalize(&self, command: &str, raw: &Value) -> Result<Value, CraftError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|v| numeric(v))
            .ok_or_else(malformed)?;
        let mut item = Map::from_iter([("id".into(), json!(id))]);
        let mut missing = Vec::new();
        if is_space(command) {
            let key = raw
                .get("key")
                .and_then(Value::as_str)
                .filter(|v| text(v, 256))
                .ok_or_else(malformed)?;
            item.insert("key".into(), json!(key));
        }
        for field in &self.selected {
            let pointer = match field.as_str() {
                "space_id" => {
                    if command == PAGE_SEARCH {
                        "/space/id"
                    } else {
                        "/spaceId"
                    }
                }
                "title" => "/title",
                "status" => "/status",
                "version" => "/version/number",
                "parent_id" => "/parentId",
                "name" => "/name",
                "space_type" => "/type",
                _ => continue,
            };
            let Some(value) = raw.pointer(pointer) else {
                missing.push(field.clone());
                continue;
            };
            let value = match field.as_str() {
                "version" => {
                    let number = value
                        .as_u64()
                        .filter(|v| *v > 0 && *v <= i32::MAX as u64)
                        .ok_or_else(malformed)?;
                    json!(number)
                }
                "parent_id" if value.is_null() => Value::Null,
                "space_id" if command == PAGE_SEARCH && value.is_i64() => {
                    let number = value.as_i64().filter(|v| *v > 0).ok_or_else(malformed)?;
                    json!(number.to_string())
                }
                _ => {
                    let value = value.as_str().ok_or_else(malformed)?;
                    let valid = match field.as_str() {
                        "space_id" | "parent_id" => numeric(value),
                        "status" => matches!(
                            value,
                            "current" | "draft" | "archived" | "historical" | "trashed" | "deleted"
                        ),
                        "space_type" => matches!(
                            value,
                            // "onboarding" is the type of the space new sites are created with.
                            "global"
                                | "personal"
                                | "collaboration"
                                | "knowledge_base"
                                | "onboarding"
                        ),
                        _ => text(value, 4096),
                    };
                    if !valid {
                        return Err(malformed());
                    }
                    json!(value)
                }
            };
            item.insert(field.clone(), value);
        }
        let body_field = self
            .body
            .map(|b| {
                if b == "storage" {
                    "body_storage"
                } else {
                    "body_atlas_doc_format"
                }
            })
            .filter(|field| self.wants(field));
        let mut body_returned = false;
        for (container, representation, field) in [
            ("body", self.body, body_field),
            (
                "description",
                self.description,
                self.description.map(|d| {
                    if d == "plain" {
                        "description_plain"
                    } else {
                        "description_view"
                    }
                }),
            ),
        ] {
            if let (Some(representation), Some(field)) = (representation, field) {
                match raw.get(container).and_then(|b| b.get(representation)) {
                    None | Some(Value::Null) => missing.push(field.into()),
                    Some(value) => {
                        if value.get("representation").and_then(Value::as_str)
                            != Some(representation)
                        {
                            return Err(malformed());
                        }
                        let text = value
                            .get("value")
                            .and_then(Value::as_str)
                            .filter(|s| s.len() <= 1_048_576)
                            .ok_or_else(malformed)?;
                        item.insert(field.into(), json!(text));
                        if container == "body" {
                            body_returned = true;
                        }
                    }
                }
            }
        }
        if !is_space(command) {
            item.insert(
                "body_state".into(),
                json!(if body_field.is_none() {
                    "not_requested"
                } else if body_returned {
                    "returned"
                } else {
                    "not_returned"
                }),
            );
        }
        if self.wants("revision") {
            let representation = if self.body == Some("atlas_doc_format") {
                crate::pages::PageRepresentation::AtlasDocFormat
            } else {
                crate::pages::PageRepresentation::Storage
            };
            let response = craft_core::execution::NativeResponse {
                status: 200,
                body: raw.clone(),
                request_id: None,
                retry_after: None,
                link_headers: None,
            };
            let revision = crate::pages::observe_page(&response, representation)?.revision()?;
            item.insert("revision".into(), json!(revision));
        }
        missing.sort();
        item.insert("fields_not_returned".into(), json!(missing));
        let item = Value::Object(item);
        if is_space(command) {
            serde_json::from_value::<SpaceSummary>(item.clone()).map_err(|_| malformed())?;
        } else {
            serde_json::from_value::<PageSummary>(item.clone()).map_err(|_| malformed())?;
        }
        Ok(item)
    }
}
