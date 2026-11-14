use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contracts::Selection;
use crate::error::{CraftError, ErrorCode};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionMode {
    Local,
    Server,
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionContract {
    pub item_pointer: String,
    pub always_returned: Vec<String>,
    pub default_selected: Vec<String>,
    pub mode: ProjectionMode,
}

#[derive(Default)]
struct FieldTree {
    complete: bool,
    children: BTreeMap<String, FieldTree>,
}

impl FieldTree {
    fn insert(&mut self, parts: &[&str]) {
        if let Some((head, tail)) = parts.split_first() {
            self.children
                .entry((*head).into())
                .or_default()
                .insert(tail);
        } else {
            self.complete = true;
        }
    }
}

pub struct CompiledProjection {
    item_pointer: String,
    tree: FieldTree,
}

fn unsupported() -> CraftError {
    CraftError::new(ErrorCode::Internal, "Unsupported projection schema shape.")
}

fn unknown_field() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::FieldUnknown,
        "Field is not selectable for this command.",
    );
    error.next_action = "Inspect schema --command <command> --response.".into();
    error
}

fn resolved(node: &Value, root: &Value, depth: usize) -> Result<Value, CraftError> {
    if depth > 32 {
        return Err(unsupported());
    }
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let path = reference.strip_prefix('#').ok_or_else(unsupported)?;
        let mut target = resolved(root.pointer(path).ok_or_else(unsupported)?, root, depth + 1)?;
        for (key, value) in node.as_object().ok_or_else(unsupported)? {
            if key == "$ref" {
                continue;
            }
            if !matches!(
                key.as_str(),
                "title" | "description" | "default" | "deprecated" | "readOnly" | "writeOnly"
            ) {
                return Err(unsupported());
            }
            target
                .as_object_mut()
                .ok_or_else(unsupported)?
                .insert(key.clone(), value.clone());
        }
        return Ok(target);
    }
    Ok(node.clone())
}

fn nullable_branch(node: &Value) -> Result<Option<(usize, &Value)>, CraftError> {
    if node.get("oneOf").is_some() || node.get("allOf").is_some() {
        return Err(unsupported());
    }
    if let Some(branches) = node.get("anyOf").and_then(Value::as_array) {
        let mut nonnull = branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| branch.get("type").and_then(Value::as_str) != Some("null"));
        let branch = nonnull.next().ok_or_else(unsupported)?;
        if nonnull.next().is_some() {
            return Err(unsupported());
        }
        return Ok(Some(branch));
    }
    Ok(None)
}

fn opaque_union(node: &Value) -> bool {
    node.get("oneOf").and_then(Value::as_array).is_some()
        || node
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|branches| {
                branches
                    .iter()
                    .filter(|branch| branch.get("type").and_then(Value::as_str) != Some("null"))
                    .count()
                    > 1
            })
}

fn child_schema(
    node: &Value,
    field: &str,
    root: &Value,
    depth: usize,
) -> Result<Value, CraftError> {
    if depth > 32 {
        return Err(unsupported());
    }
    let node = resolved(node, root, depth)?;
    if opaque_union(&node) {
        return Err(unknown_field());
    }
    if let Some((_, branch)) = nullable_branch(&node)? {
        return child_schema(branch, field, root, depth + 1);
    }
    if let Some(items) = node.get("items") {
        return child_schema(items, field, root, depth + 1);
    }
    node.get("properties")
        .and_then(|properties| properties.get(field))
        .cloned()
        .ok_or_else(unknown_field)
}

fn container_tokens(pointer: &str) -> Result<Vec<&str>, CraftError> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if pointer.len() > 512 || !pointer.starts_with('/') {
        return Err(unsupported());
    }
    let tokens: Vec<_> = pointer[1..].split('/').collect();
    if tokens.len() > 8
        || tokens.iter().any(|token| {
            token.is_empty()
                || !token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
    {
        return Err(unsupported());
    }
    Ok(tokens)
}

impl ProjectionContract {
    pub fn selectable_fields(&self, response_schema: &Value) -> Result<Vec<String>, CraftError> {
        let mut item = response_schema
            .pointer("/properties/data")
            .ok_or_else(unsupported)?
            .clone();
        for token in container_tokens(&self.item_pointer)? {
            item = child_schema(&item, token, response_schema, 0)?;
        }
        let mut fields = Vec::new();
        collect_fields(&item, response_schema, "", 0, &mut fields)?;
        Ok(fields)
    }

    /// Validates paths locally before a provider can fetch bodies or expand relationships.
    pub fn compile(
        &self,
        response_schema: &Value,
        requested: &[String],
    ) -> Result<CompiledProjection, CraftError> {
        let selection = Selection {
            fields: requested.to_vec(),
            ..Selection::default()
        };
        selection.validate()?;
        let mut seen = BTreeSet::new();
        if requested.iter().any(|field| !seen.insert(field)) {
            return Err(CraftError::field("/fields", "duplicate_projection_path"));
        }
        let tokens = container_tokens(&self.item_pointer)?;
        let mut item = response_schema
            .pointer("/properties/data")
            .ok_or_else(unsupported)?
            .clone();
        for token in tokens {
            item = child_schema(&item, token, response_schema, 0)?;
        }
        let chosen = if requested.is_empty() {
            &self.default_selected
        } else {
            requested
        };
        let mut tree = FieldTree::default();
        for field in chosen.iter().chain(&self.always_returned) {
            Selection {
                fields: vec![field.clone()],
                ..Selection::default()
            }
            .validate()?;
            let parts: Vec<_> = field.split('.').collect();
            let mut node = item.clone();
            for part in &parts {
                node = child_schema(&node, part, response_schema, 0)?;
            }
            tree.insert(&parts);
        }
        if tree.children.is_empty() {
            return Err(unsupported());
        }
        Ok(CompiledProjection {
            item_pointer: self.item_pointer.clone(),
            tree,
        })
    }
}

fn collect_fields(
    node: &Value,
    root: &Value,
    prefix: &str,
    depth: usize,
    fields: &mut Vec<String>,
) -> Result<(), CraftError> {
    if depth > 32 {
        return Err(unsupported());
    }
    let node = resolved(node, root, 0)?;
    // A heterogeneous value is selectable whole; traversing one variant would misstate its schema.
    if opaque_union(&node) {
        return Ok(());
    }
    if let Some((_, branch)) = nullable_branch(&node)? {
        return collect_fields(branch, root, prefix, depth + 1, fields);
    }
    if let Some(items) = node.get("items") {
        return collect_fields(items, root, prefix, depth + 1, fields);
    }
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (field, schema) in properties {
            let path = if prefix.is_empty() {
                field.clone()
            } else {
                format!("{prefix}.{field}")
            };
            if (Selection {
                fields: vec![path.clone()],
                ..Selection::default()
            })
            .validate()
            .is_err()
            {
                continue;
            }
            fields.push(path.clone());
            if fields.len() > 512 {
                return Err(unsupported());
            }
            collect_fields(schema, root, &path, depth + 1, fields)?;
        }
    }
    Ok(())
}

fn project_value(node: &Value, tree: &FieldTree) -> Result<Value, CraftError> {
    if tree.complete || node.is_null() {
        return Ok(node.clone());
    }
    match node {
        Value::Array(items) => items
            .iter()
            .map(|item| project_value(item, tree))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(object) => {
            let mut selected = serde_json::Map::new();
            for (field, subtree) in &tree.children {
                if let Some(value) = object.get(field) {
                    selected.insert(field.clone(), project_value(value, subtree)?);
                }
            }
            Ok(Value::Object(selected))
        }
        _ => Err(unsupported()),
    }
}

fn project_schema(
    node: &Value,
    tokens: &[&str],
    tree: &FieldTree,
    root: &Value,
    depth: usize,
) -> Result<Value, CraftError> {
    if depth > 32 {
        return Err(unsupported());
    }
    if tokens.is_empty() && tree.complete {
        return Ok(node.clone());
    }
    let mut node = resolved(node, root, depth)?;
    if let Some((index, branch)) = nullable_branch(&node)? {
        let projected = project_schema(branch, tokens, tree, root, depth + 1)?;
        node["anyOf"][index] = projected;
        return Ok(node);
    }
    if let Some(items) = node.get("items") {
        let projected = project_schema(items, tokens, tree, root, depth + 1)?;
        node["items"] = projected;
        return Ok(node);
    }
    let properties = node
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .ok_or_else(unsupported)?;
    if let Some((head, tail)) = tokens.split_first() {
        let selected = project_schema(
            properties.get(*head).ok_or_else(unsupported)?,
            tail,
            tree,
            root,
            depth + 1,
        )?;
        properties.insert((*head).into(), selected);
        return Ok(node);
    }
    properties.retain(|field, _| tree.children.contains_key(field));
    for (field, subtree) in &tree.children {
        let child = properties.get(field).ok_or_else(unsupported)?;
        let selected = project_schema(child, &[], subtree, root, depth + 1)?;
        properties.insert(field.clone(), selected);
    }
    if let Some(required) = node.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|field| {
            field
                .as_str()
                .is_some_and(|field| tree.children.contains_key(field))
        });
    }
    node["additionalProperties"] = Value::Bool(false);
    Ok(node)
}

impl CompiledProjection {
    /// Projects data only; mandatory envelope, paging and failure evidence are handled outside it.
    pub fn data(&self, data: &Value) -> Result<Value, CraftError> {
        if self.item_pointer.is_empty() {
            return project_value(data, &self.tree);
        }
        let mut output = data.clone();
        if let Some(items) = output.pointer_mut(&self.item_pointer) {
            *items = project_value(items, &self.tree)?;
        }
        Ok(output)
    }

    pub fn response_schema(&self, source: &Value) -> Result<Value, CraftError> {
        let tokens = container_tokens(&self.item_pointer)?;
        let mut output = source.clone();
        output["properties"]["data"] = project_schema(
            source.pointer("/properties/data").ok_or_else(unsupported)?,
            &tokens,
            &self.tree,
            source,
            0,
        )?;
        Ok(output)
    }
}
