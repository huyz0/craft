use crate::models::*;
use crate::table::{Filter, Kind, Param, entry, route};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::{NativePage, NativePosition};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn malformed() -> CraftError {
    CraftError::new(
        ErrorCode::RemoteUnavailable,
        "Bitbucket returned a response that does not match its documented shape.",
    )
}

fn invoke(context: &mut dyn OperationContext, call: &NativeCall) -> Result<Value, CraftError> {
    context.checkpoint()?;
    let NativeResponse { status, body, .. } = context.invoke(call)?;
    if status != 200 {
        return Err(CraftError::new(
            match status {
                400 | 422 => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            if status == 404 {
                "Bitbucket resource is absent or not visible; absence was not established."
            } else {
                "Bitbucket read failed; no further automatic retry is made."
            },
        ));
    }
    Ok(body)
}

/// Follows a chain of object keys; any non-object link ends the walk.
fn at<'a>(raw: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(raw, |value, key| value.get(*key))
        .filter(|value| !value.is_null())
}
fn text(raw: &Value, path: &[&str]) -> Value {
    at(raw, path)
        .and_then(Value::as_str)
        .map_or(Value::Null, |value| json!(value))
}
fn flag(raw: &Value, path: &[&str]) -> Value {
    at(raw, path)
        .and_then(Value::as_bool)
        .map_or(Value::Null, |value| json!(value))
}
fn count(raw: &Value, path: &[&str]) -> Value {
    at(raw, path)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .map_or(Value::Null, |value| json!(value))
}
fn big(raw: &Value, path: &[&str]) -> Value {
    at(raw, path)
        .and_then(Value::as_u64)
        .map_or(Value::Null, |value| json!(value))
}
fn uuid_of(raw: &Value) -> Result<String, CraftError> {
    raw.get("uuid")
        .and_then(Value::as_str)
        .and_then(bare_uuid)
        .ok_or_else(malformed)
}
fn numeric_id(raw: &Value) -> Result<u64, CraftError> {
    raw.get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id >= 1)
        .ok_or_else(malformed)
}
fn names(raw: &Value, key: &str) -> Value {
    at(raw, &[key])
        .and_then(Value::as_array)
        .map_or(Value::Null, |values| {
            json!(
                values
                    .iter()
                    .filter_map(|v| v.get("display_name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
            )
        })
}

/// Bitbucket spells the outcome in one of two places depending on whether the run finished.
fn outcome(raw: &Value) -> Value {
    let result = text(raw, &["state", "result", "name"]);
    if result.is_null() {
        text(raw, &["state", "stage", "name"])
    } else {
        result
    }
}

/// Maps one native Bitbucket object to craft's stable, bounded summary shape.
fn normalize(command: &str, raw: &Value) -> Result<Value, CraftError> {
    if !raw.is_object() {
        return Err(malformed());
    }
    let mut item = Map::new();
    let mut put = |key: &str, value: Value| {
        item.insert(key.into(), value);
    };
    match command {
        "bitbucket.repository.list" | "bitbucket.repository.get" => {
            let id = raw
                .get("slug")
                .and_then(Value::as_str)
                .filter(|slug| slug_ok(slug))
                .ok_or_else(malformed)?;
            put("id", json!(id));
            put(
                "uuid",
                raw.get("uuid")
                    .and_then(Value::as_str)
                    .and_then(bare_uuid)
                    .map_or(Value::Null, |uuid| json!(uuid)),
            );
            put("name", text(raw, &["name"]));
            put("full_name", text(raw, &["full_name"]));
            put("description", text(raw, &["description"]));
            put("is_private", flag(raw, &["is_private"]));
            put("language", text(raw, &["language"]));
            put("main_branch", text(raw, &["mainbranch", "name"]));
            put("project_key", text(raw, &["project", "key"]));
            put("created_on", text(raw, &["created_on"]));
            put("updated_on", text(raw, &["updated_on"]));
            put("url", text(raw, &["links", "html", "href"]));
        }
        "bitbucket.branch.list" => {
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| {
                    !name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control)
                })
                .ok_or_else(malformed)?;
            put("id", json!(name));
            put("commit", text(raw, &["target", "hash"]));
            put("date", text(raw, &["target", "date"]));
            put(
                "author",
                match text(raw, &["target", "author", "user", "display_name"]) {
                    Value::Null => text(raw, &["target", "author", "raw"]),
                    name => name,
                },
            );
            put("message", text(raw, &["target", "message"]));
        }
        "bitbucket.commit.list" => {
            let hash = raw
                .get("hash")
                .and_then(Value::as_str)
                .filter(|hash| {
                    (7..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit())
                })
                .ok_or_else(malformed)?;
            put("id", json!(hash));
            put("message", text(raw, &["message"]));
            put("date", text(raw, &["date"]));
            put(
                "author",
                match text(raw, &["author", "user", "display_name"]) {
                    Value::Null => text(raw, &["author", "raw"]),
                    name => name,
                },
            );
            put(
                "parents",
                at(raw, &["parents"])
                    .and_then(Value::as_array)
                    .map_or(Value::Null, |parents| {
                        json!(
                            parents
                                .iter()
                                .filter_map(|p| p.get("hash").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                        )
                    }),
            );
            put("url", text(raw, &["links", "html", "href"]));
        }
        "bitbucket.pullrequest.list" | "bitbucket.pullrequest.get" => {
            put("id", json!(numeric_id(raw)?));
            put("title", text(raw, &["title"]));
            put("description", text(raw, &["description"]));
            put("state", text(raw, &["state"]));
            put("author", text(raw, &["author", "display_name"]));
            put("source_branch", text(raw, &["source", "branch", "name"]));
            put(
                "source_repository",
                text(raw, &["source", "repository", "full_name"]),
            );
            put(
                "destination_branch",
                text(raw, &["destination", "branch", "name"]),
            );
            put("created_on", text(raw, &["created_on"]));
            put("updated_on", text(raw, &["updated_on"]));
            put("comment_count", count(raw, &["comment_count"]));
            put("task_count", count(raw, &["task_count"]));
            put("close_source_branch", flag(raw, &["close_source_branch"]));
            put("merge_commit", text(raw, &["merge_commit", "hash"]));
            put("reviewers", names(raw, "reviewers"));
            put(
                "participants",
                at(raw, &["participants"]).and_then(Value::as_array).map_or(
                    Value::Null,
                    |participants| {
                        json!(
                            participants
                                .iter()
                                .map(|p| json!({
                                    "display_name": text(p, &["user", "display_name"]),
                                    "role": text(p, &["role"]),
                                    "approved": flag(p, &["approved"]),
                                    "state": text(p, &["state"]),
                                }))
                                .collect::<Vec<_>>()
                        )
                    },
                ),
            );
            put("url", text(raw, &["links", "html", "href"]));
        }
        "bitbucket.pullrequest.diffstat" => {
            let path = text(raw, &["new", "path"]);
            let id = match &path {
                Value::Null => text(raw, &["old", "path"]),
                path => path.clone(),
            };
            if id.is_null() {
                return Err(malformed());
            }
            put("id", id);
            put("status", text(raw, &["status"]));
            put("lines_added", count(raw, &["lines_added"]));
            put("lines_removed", count(raw, &["lines_removed"]));
            put("old_path", text(raw, &["old", "path"]));
            put("new_path", path);
        }
        "bitbucket.comment.list" => {
            put("id", json!(numeric_id(raw)?));
            put("text", text(raw, &["content", "raw"]));
            put("author", text(raw, &["user", "display_name"]));
            put("created_on", text(raw, &["created_on"]));
            put("updated_on", text(raw, &["updated_on"]));
            put("deleted", flag(raw, &["deleted"]));
            put("parent_id", big(raw, &["parent", "id"]));
            put(
                "inline",
                at(raw, &["inline"]).map_or(Value::Null, |inline| {
                    json!({
                        "path": text(inline, &["path"]),
                        "from_line": count(inline, &["from"]),
                        "to_line": count(inline, &["to"]),
                    })
                }),
            );
        }
        "bitbucket.pipeline.list" | "bitbucket.pipeline.get" => {
            put("id", json!(uuid_of(raw)?));
            put("build_number", big(raw, &["build_number"]));
            put("state", text(raw, &["state", "name"]));
            put("result", outcome(raw));
            put("ref_name", text(raw, &["target", "ref_name"]));
            put("ref_type", text(raw, &["target", "ref_type"]));
            put("commit", text(raw, &["target", "commit", "hash"]));
            put("creator", text(raw, &["creator", "display_name"]));
            put("trigger", text(raw, &["trigger", "name"]));
            put("created_on", text(raw, &["created_on"]));
            put("completed_on", text(raw, &["completed_on"]));
            put("duration_seconds", big(raw, &["duration_in_seconds"]));
        }
        "bitbucket.step.list" => {
            put("id", json!(uuid_of(raw)?));
            put("name", text(raw, &["name"]));
            put("state", text(raw, &["state", "name"]));
            put("result", outcome(raw));
            put("started_on", text(raw, &["started_on"]));
            put("completed_on", text(raw, &["completed_on"]));
            put("duration_seconds", big(raw, &["duration_in_seconds"]));
        }
        _ => return Err(CraftError::field("/command", "unknown_bitbucket_read")),
    }
    Ok(Value::Object(item))
}

fn slug_ok(value: &str) -> bool {
    slug(value)
}

fn invalid() -> CraftError {
    CraftError::field("/input", "invalid_bitbucket_read_input")
}

fn page_of(input: &Value) -> Result<u16, CraftError> {
    let page = input
        .get("page_size")
        .and_then(Value::as_u64)
        .map_or(Ok(25), |value| u16::try_from(value).map_err(|_| invalid()))?;
    page_size_field(page)?;
    Ok(page)
}

/// The optional filter's native query value, validated and rewritten for Bitbucket.
fn filter_value(filter: &Filter, input: &Value) -> Result<Option<String>, CraftError> {
    let Some(raw) = input.get(filter.field) else {
        return Ok((filter.field == "state").then(|| "OPEN".into()));
    };
    if raw.is_null() {
        return Ok((filter.field == "state").then(|| "OPEN".into()));
    }
    let value = raw.as_str().ok_or_else(invalid)?;
    Ok(Some(match filter.field {
        "name_contains" => {
            name_filter_field(value)?;
            format!("name ~ \"{value}\"")
        }
        "branch" => {
            branch_field("/branch", value)?;
            value.to_owned()
        }
        "state" => {
            state_field(value)?;
            value.to_owned()
        }
        _ => return Err(invalid()),
    }))
}

fn page_token(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

fn validate_position(position: &NativePosition) -> Result<(), CraftError> {
    position.validate()?;
    let keys: Vec<_> = position.0.keys().map(String::as_str).collect();
    if keys == ["page"] && page_token(&position.0["page"]) {
        Ok(())
    } else {
        Err(CraftError::field("/cursor", "invalid_bitbucket_position"))
    }
}

fn call(
    operation: &PreparedOperation,
    position: Option<&NativePosition>,
) -> Result<(NativeCall, u16), CraftError> {
    let request = operation.request();
    let entry = entry(&request.command)
        .ok_or_else(|| CraftError::field("/command", "unknown_bitbucket_read"))?;
    crate::registration::validate_target(&request.target)?;
    let input = &request.input;
    let mut path_parameters =
        BTreeMap::from([("workspace".to_owned(), request.target.tenant.clone())]);
    for param in entry.params {
        let value = match param {
            Param::Repo => {
                let repo = input
                    .get("repo")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                repo_field(repo)?;
                repo.to_owned()
            }
            Param::PullRequest => {
                let id = input
                    .get("pull_request_id")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?;
                pull_request_field(id)?;
                id.to_string()
            }
            Param::Pipeline => {
                let uuid = input
                    .get("pipeline_uuid")
                    .and_then(Value::as_str)
                    .and_then(bare_uuid)
                    .ok_or_else(|| CraftError::field("/pipeline_uuid", "expected_pipeline_uuid"))?;
                format!("{{{uuid}}}")
            }
        };
        path_parameters.insert(param.name().to_owned(), value);
    }
    let mut query: BTreeMap<String, String> = entry
        .query
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect();
    for filter in entry.filters {
        if let Some(value) = filter_value(filter, input)? {
            query.insert(filter.query.into(), value);
        }
    }
    let page = match entry.kind {
        Kind::Get => 25,
        Kind::Paged => {
            let page = page_of(input)?;
            query.insert("pagelen".into(), page.to_string());
            if let Some(position) = position {
                validate_position(position)?;
                query.insert("page".into(), position.0["page"].clone());
            }
            page
        }
    };
    Ok((
        NativeCall {
            route: route(entry).id,
            path_parameters,
            query,
            repeated_query: BTreeMap::new(),
            body: None,
        },
        page,
    ))
}

/// The identity a Get response must carry for the requested resource.
fn expected_identity(command: &str, call: &NativeCall) -> Option<Value> {
    let parameter = |name: &str| call.path_parameters.get(name).cloned();
    match command {
        "bitbucket.repository.get" => parameter("repo").map(Value::from),
        "bitbucket.pullrequest.get" => parameter("pull_request_id")
            .and_then(|id| id.parse::<u64>().ok())
            .map(Value::from),
        "bitbucket.pipeline.get" => parameter("pipeline_uuid")
            .and_then(|uuid| bare_uuid(&uuid))
            .map(Value::from),
        _ => None,
    }
}

/// Exact selected resource read. It never resolves names or follows links.
pub fn read_get(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
) -> Result<Value, CraftError> {
    context.verify_operation(operation)?;
    let command = &operation.request().command;
    if !crate::table::is_get(command) {
        return Err(CraftError::field("/command", "expected_bitbucket_get"));
    }
    let (call, _) = call(operation, None)?;
    let raw = invoke(context, &call)?;
    let item = normalize(command, &raw)?;
    if expected_identity(command, &call).as_ref() != Some(&item["id"]) {
        return Err(malformed());
    }
    Ok(item)
}

/// Fetches one native page. The shared host owns cursor persistence and access checks. The next
/// position is the `page` token of Bitbucket's own `next` link, accepted only after the transport
/// confirms the link stays on the same host, route and filters.
pub fn read_page(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    context.verify_operation(operation)?;
    let command = &operation.request().command;
    if crate::table::kind_of(command) != Some(Kind::Paged) {
        return Err(CraftError::field("/command", "expected_bitbucket_list"));
    }
    let (call, page) = call(operation, position)?;
    let body = invoke(context, &call)?;
    let values = body
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    if values.len() > usize::from(page) {
        return Err(malformed());
    }
    let next = match body.get("next") {
        None | Some(Value::Null) => None,
        Some(link) => {
            let link = link.as_str().ok_or_else(malformed)?;
            let followed = context.normalize_next(&call, link)?;
            let token = followed
                .query
                .get("page")
                .filter(|token| page_token(token))
                .ok_or_else(malformed)?;
            if call.query.get("page") == Some(token) {
                return Err(malformed());
            }
            Some(NativePosition(BTreeMap::from([(
                "page".into(),
                token.clone(),
            )])))
        }
    };
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for raw in values {
        context.checkpoint()?;
        let item = normalize(command, raw)?;
        if !seen.insert(item["id"].to_string()) {
            return Err(malformed());
        }
        items.push(item);
    }
    Ok(NativePage {
        context: None,
        items,
        next,
    })
}
