//! Private native preparation binds comment identity, original author and project evidence.
mod input;
#[cfg(test)]
mod tests;
use super::*;
use crate::create_metadata::MetadataPrincipal;
use craft_core::contracts::RequestState;
use craft_core::execution::NativeResponse;
use craft_core::storage::{Clock, SubmissionState};
use input::Intent;
pub use input::{CreateInput, DeleteInput, Ownership, UpdateInput, VisibilityChange};
use std::cell::Cell;

pub const CREATE: &str = "jira.comment.create";
pub const UPDATE: &str = "jira.comment.update";
pub const DELETE: &str = "jira.comment.delete";
pub const CREATE_CONTEXT: &str = "jira.comment.create-context";
pub const UPDATE_CONTEXT: &str = "jira.comment.update-context";
pub const DELETE_CONTEXT: &str = "jira.comment.delete-context";
pub const CREATE_ROUTE: &str = "jira-comment-create";
pub const UPDATE_ROUTE: &str = "jira-comment-update";
pub const DELETE_ROUTE: &str = "jira-comment-delete";
const ISSUE: &str = "comment-issue-project";
const PERMISSIONS: &str = "comment-mutation-permissions";

fn write_command(command: &str) -> Option<&'static str> {
    match command {
        CREATE | CREATE_CONTEXT => Some(CREATE),
        UPDATE | UPDATE_CONTEXT => Some(UPDATE),
        DELETE | DELETE_CONTEXT => Some(DELETE),
        _ => None,
    }
}
pub fn context_command(command: &str) -> Result<&'static str, CraftError> {
    match command {
        CREATE => Ok(CREATE_CONTEXT),
        UPDATE => Ok(UPDATE_CONTEXT),
        DELETE => Ok(DELETE_CONTEXT),
        _ => Err(CraftError::field("/command", "expected_comment_mutation")),
    }
}
pub fn validate(request: &CanonicalRequest) -> Result<(), CraftError> {
    input::intent(request).map(|_| ())
}
fn route(id: &str, method: HttpMethod, parts: &[&str], query: &[&str]) -> NativeRoute {
    NativeRoute {
        id: id.into(),
        method,
        path: parts
            .iter()
            .map(|s| {
                if matches!(*s, "issueIdOrKey" | "id") {
                    RouteSegment::Parameter((*s).into())
                } else {
                    RouteSegment::Literal((*s).into())
                }
            })
            .collect(),
        query_parameters: query.iter().map(|s| (*s).into()).collect(),
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: RESPONSE_MAX as u32,
    }
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
            "Comment mutations require private preparation and the journaled host.",
        ))
    }
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [
        CREATE,
        UPDATE,
        DELETE,
        CREATE_CONTEXT,
        UPDATE_CONTEXT,
        DELETE_CONTEXT,
    ] {
        let native = write_command(command).ok_or_else(malformed)?;
        let context = command != native;
        let mut example = json!({"issue_id":"10000"});
        if native == CREATE {
            example["body"] = json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Review complete."}]}]});
            example["visibility"] = json!({"mode":"project"});
        } else {
            example["comment_id"] = json!("10001");
            example["expected_revision"] = json!(format!("sha256:{}", "0".repeat(64)));
            example["ownership"] = json!("own");
            if native == UPDATE {
                example["visibility"] = json!({"mode":"project"});
            }
        }
        let mut path = vec!["rest", "api", "3", "issue", "issueIdOrKey", "comment"];
        if native != CREATE {
            path.push("id");
        }
        let routes = if context {
            vec![
                route(
                    ISSUE,
                    HttpMethod::Get,
                    &["rest", "api", "3", "issue", "issueIdOrKey"],
                    &["fields", "updateHistory", "failFast"],
                ),
                route(
                    PERMISSIONS,
                    HttpMethod::Get,
                    &["rest", "api", "3", "mypermissions"],
                    &["issueId", "permissions"],
                ),
                route(
                    GET_ROUTE,
                    HttpMethod::Get,
                    &["rest", "api", "3", "issue", "issueIdOrKey", "comment", "id"],
                    &[],
                ),
            ]
        } else {
            vec![route(
                match native {
                    CREATE => CREATE_ROUTE,
                    UPDATE => UPDATE_ROUTE,
                    _ => DELETE_ROUTE,
                },
                match native {
                    CREATE => HttpMethod::Post,
                    UPDATE => HttpMethod::Put,
                    _ => HttpMethod::Delete,
                },
                &path,
                &[],
            )]
        };
        let descriptor = OperationDescriptor {
            id:command.into(), provider:"jira".into(), contract_version:SCHEMA_VERSION.into(),
            api_revision:"jira-cloud-v3-comment-mutation-v1".into(),
            description:if context {"Qualify exact comment/project, original author, expected fingerprint and native permissions."} else {"Apply one exact comment mutation through the journaled host; no automatic retry."}.into(),
            effect:if context {EffectClass::Read} else if native == DELETE {EffectClass::Permanent} else {EffectClass::Write},
            input_schema:match native { CREATE => schema_for::<CreateInput>()?, UPDATE => schema_for::<UpdateInput>()?, _ => schema_for::<DeleteInput>()? },
            response_schema:json!({"type":"object"}), example, response_example:json!({}), projection:None,
            scope_alternatives:vec![vec![if context {"read:jira-work"} else {"write:jira-work"}.into()]],
            permissions:vec!["Browse projects, issue security and native Add/Edit own/all/Delete own/all comments; visibility membership remains native.".into()],
            tenant_conditions:vec!["Fingerprint rechecks are non-atomic; inherited child visibility and native app access rules may refuse the write.".into()],
            routes, retry_class:if context {RetryClass::SafeRead} else {RetryClass::ReconcileBeforeReplay},
            reconciliation:if context {ReconciliationClass::NotApplicable} else {ReconciliationClass::KnownResource},
        };
        match native {
            CREATE => {
                registry.register_typed_handler::<CreateInput>(descriptor, Arc::new(Handler))?
            }
            UPDATE => {
                registry.register_typed_handler::<UpdateInput>(descriptor, Arc::new(Handler))?
            }
            _ => registry.register_typed_handler::<DeleteInput>(descriptor, Arc::new(Handler))?,
        }
    }
    Ok(())
}
struct Freshness {
    clock: Arc<dyn Clock>,
    wall: u64,
    mono: u64,
    last_wall: Cell<u64>,
    last_mono: Cell<u64>,
}
impl Freshness {
    fn new(clock: Arc<dyn Clock>) -> Self {
        let wall = clock.unix_millis();
        let mono = clock.monotonic_millis();
        Self {
            clock,
            wall,
            mono,
            last_wall: Cell::new(wall),
            last_mono: Cell::new(mono),
        }
    }
    fn check(&self) -> Result<(), CraftError> {
        let wall = self.clock.unix_millis();
        let mono = self.clock.monotonic_millis();
        if wall < self.last_wall.get()
            || mono < self.last_mono.get()
            || wall.saturating_sub(self.wall) >= 30_000
            || mono.saturating_sub(self.mono) >= 30_000
        {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Comment preparation expired or its clock moved backward.",
            ));
        }
        self.last_wall.set(wall);
        self.last_mono.set(mono);
        Ok(())
    }
}
pub struct FrozenComment {
    call: NativeCall,
    digest: String,
    principal: MetadataPrincipal,
    freshness: Freshness,
    before: Option<CommentRecord>,
}
impl FrozenComment {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn before(&self) -> Option<&CommentRecord> {
        self.before.as_ref()
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
    ) -> Result<(), CraftError> {
        if !matches!(
            operation.request().command.as_str(),
            CREATE | UPDATE | DELETE
        ) || self.digest != operation.digest()
            || &self.principal != principal
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Frozen comment differs from the exact operation or authenticated principal.",
            ));
        }
        self.freshness.check()
    }
}
fn call(
    route: &str,
    issue: Option<&str>,
    comment: Option<&str>,
    query: BTreeMap<String, String>,
) -> NativeCall {
    let mut params = BTreeMap::new();
    if let Some(issue) = issue {
        params.insert("issueIdOrKey".into(), issue.into());
    }
    if let Some(comment) = comment {
        params.insert("id".into(), comment.into());
    }
    NativeCall {
        route: route.into(),
        path_parameters: params,
        query,
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
fn read(
    context: &mut dyn OperationContext,
    call: &NativeCall,
    freshness: &Freshness,
) -> Result<Value, CraftError> {
    context.checkpoint()?;
    freshness.check()?;
    let response = context.invoke(call)?;
    context.checkpoint()?;
    freshness.check()?;
    if response.status != 200 {
        return Err(CraftError::new(
            match response.status {
                401 => ErrorCode::AuthRequired,
                403 => ErrorCode::PermissionDenied,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::RemoteUnavailable,
            },
            "Comment preparation could not establish native evidence; no retry was performed.",
        ));
    }
    if !response.body.is_object()
        || serde_json::to_vec(&response.body)
            .map_err(|_| malformed())?
            .len()
            > RESPONSE_MAX
        || ["errors", "errorMessages", "warningMessages"]
            .iter()
            .any(|k| response.body.get(k).is_some())
    {
        return Err(malformed());
    }
    Ok(response.body)
}
fn project(
    context: &mut dyn OperationContext,
    issue: &str,
    target: &ResourceRef,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let raw = read(
        context,
        &call(
            ISSUE,
            Some(issue),
            None,
            BTreeMap::from([
                ("fields".into(), "project".into()),
                ("updateHistory".into(), "false".into()),
                ("failFast".into(), "true".into()),
            ]),
        ),
        freshness,
    )?;
    if raw["id"] != issue || raw["fields"]["project"]["id"] != target.id {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Comment issue or current project differs from the explicit target.",
        ));
    }
    Ok(())
}
fn comment(
    context: &mut dyn OperationContext,
    intent: &Intent,
    principal: &MetadataPrincipal,
    freshness: &Freshness,
) -> Result<Option<CommentRecord>, CraftError> {
    let Some((id, revision, ownership)) = intent.comment() else {
        return Ok(None);
    };
    let raw = read(
        context,
        &call(GET_ROUTE, Some(intent.issue()), Some(id), BTreeMap::new()),
        freshness,
    )?;
    let record = parse_record(&raw, intent.issue(), true)?;
    if record.id != id || record.revision.as_deref() != Some(revision) {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Current complete comment evidence differs from its expected fingerprint.",
        ));
    }
    if ownership == Ownership::Own && record.author_account_id.as_deref() != Some(&principal.actor)
    {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Own-comment intent requires the authenticated actor to be the original author.",
        ));
    }
    Ok(Some(record))
}
fn permissions(
    context: &mut dyn OperationContext,
    intent: &Intent,
    freshness: &Freshness,
) -> Result<(), CraftError> {
    let required = match intent {
        Intent::Create(_) => vec!["BROWSE_PROJECTS", "ADD_COMMENTS"],
        Intent::Update(_) => vec!["BROWSE_PROJECTS", "EDIT_OWN_COMMENTS", "EDIT_ALL_COMMENTS"],
        Intent::Delete(_) => vec![
            "BROWSE_PROJECTS",
            "DELETE_OWN_COMMENTS",
            "DELETE_ALL_COMMENTS",
        ],
    };
    let raw = read(
        context,
        &call(
            PERMISSIONS,
            None,
            None,
            BTreeMap::from([
                ("issueId".into(), intent.issue().into()),
                ("permissions".into(), required.join(",")),
            ]),
        ),
        freshness,
    )?;
    let flags = required
        .iter()
        .map(|key| {
            let item = &raw["permissions"][*key];
            if item["key"] != *key {
                return Err(malformed());
            }
            item["havePermission"].as_bool().ok_or_else(malformed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let allowed = flags[0]
        && match intent {
            Intent::Create(_) => flags[1],
            _ => {
                flags[2]
                    || intent
                        .comment()
                        .is_some_and(|(_, _, own)| own == Ownership::Own)
                        && flags[1]
            }
        };
    if !allowed {
        return Err(CraftError::new(
            ErrorCode::PermissionDenied,
            "Issue-scoped native comment permission is missing or insufficient for own/any intent.",
        ));
    }
    Ok(())
}
pub fn prepare(
    read_op: &PreparedOperation,
    write_op: &PreparedOperation,
    context: &mut dyn OperationContext,
    principal: MetadataPrincipal,
    clock: Arc<dyn Clock>,
) -> Result<FrozenComment, CraftError> {
    let command = write_op.request().command.as_str();
    if read_op.request().command != context_command(command)?
        || !matches!(read_op.effect(), EffectClass::Read)
        || !matches!(
            (command, write_op.effect()),
            (CREATE | UPDATE, EffectClass::Write) | (DELETE, EffectClass::Permanent)
        )
        || read_op.request().input != write_op.request().input
        || read_op.request().target != write_op.request().target
    {
        return Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Expected matching sealed comment context and mutation operations.",
        ));
    }
    let intent = input::intent(write_op.request())?;
    principal.grant.validate()?;
    craft_core::contracts::validate_identity("/actor", &principal.actor)?;
    context.verify_operation(read_op)?;
    let freshness = Freshness::new(clock);
    project(
        context,
        intent.issue(),
        &write_op.request().target,
        &freshness,
    )?;
    let before = comment(context, &intent, &principal, &freshness)?;
    permissions(context, &intent, &freshness)?;
    comment(context, &intent, &principal, &freshness)?;
    project(
        context,
        intent.issue(),
        &write_op.request().target,
        &freshness,
    )?;
    context.checkpoint()?;
    freshness.check()?;
    let mut call = call(
        match command {
            CREATE => CREATE_ROUTE,
            UPDATE => UPDATE_ROUTE,
            _ => DELETE_ROUTE,
        },
        Some(intent.issue()),
        intent.comment().map(|v| v.0),
        BTreeMap::new(),
    );
    call.body = intent.payload()?;
    Ok(FrozenComment {
        call,
        digest: write_op.digest().into(),
        principal,
        freshness,
        before,
    })
}

pub struct CommentObservation {
    pub state: SubmissionState,
    pub resources: Vec<ResourceRef>,
    pub comment: Option<CommentRecord>,
    pub acknowledged: bool,
    pub desired_fields_verified: bool,
    pub error: Option<CraftError>,
}
fn failure(state: SubmissionState, code: ErrorCode, message: &str) -> CommentObservation {
    let mut error = CraftError::new(code, message);
    error.details.write_state = if state == SubmissionState::Failed {
        RequestState::Failed
    } else {
        RequestState::SentUnknown
    };
    CommentObservation {
        state,
        resources: vec![],
        comment: None,
        acknowledged: false,
        desired_fields_verified: false,
        error: Some(error),
    }
}
pub fn interpret(
    operation: &PreparedOperation,
    response: &NativeResponse,
) -> Result<CommentObservation, CraftError> {
    let intent = input::intent(operation.request())?;
    let command = operation.request().command.as_str();
    if !matches!(command, CREATE | UPDATE | DELETE) {
        return Err(CraftError::field("/command", "expected_comment_mutation"));
    }
    let success = match command {
        CREATE => 201,
        UPDATE => 200,
        _ => 204,
    };
    if response.status != success {
        let refused = matches!(response.status, 400 | 401 | 404)
            || command == CREATE && response.status == 413
            || command == DELETE && response.status == 405;
        return Ok(failure(
            if refused {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            },
            match response.status {
                400 => ErrorCode::InputInvalid,
                413 if command == CREATE => ErrorCode::InputInvalid,
                401 => ErrorCode::AuthRequired,
                404 => ErrorCode::RemoteUnavailable,
                405 if command == DELETE => ErrorCode::UnsupportedCapability,
                429 => ErrorCode::RateLimited,
                _ => ErrorCode::WriteOutcomeUnknown,
            },
            "Comment mutation did not return its documented acknowledgment; retain the receipt and do not retry.",
        ));
    }
    let id = if let Some((id, _, _)) = intent.comment() {
        Some(id.to_owned())
    } else {
        response.body["id"]
            .as_str()
            .filter(|s| super::numeric(s))
            .map(str::to_owned)
    };
    let mut observation = CommentObservation {
        state: if id.is_some() {
            SubmissionState::Confirmed
        } else {
            SubmissionState::Unknown
        },
        resources: id
            .as_ref()
            .map(|id| {
                vec![ResourceRef {
                    resource_type: "comment".into(),
                    id: id.clone(),
                    ..operation.request().target.clone()
                }]
            })
            .unwrap_or_default(),
        comment: None,
        acknowledged: true,
        desired_fields_verified: command == DELETE,
        error: None,
    };
    if command != DELETE {
        let parsed = parse_record(&response.body, intent.issue(), true)
            .ok()
            .filter(|r| Some(&r.id) == id.as_ref());
        if let Some(record) = parsed {
            observation.desired_fields_verified = intent.desired(&record)?;
            observation.comment = Some(record);
        }
        if !observation.desired_fields_verified || id.is_none() {
            let mut error = CraftError::new(
                ErrorCode::WriteOutcomeUnknown,
                "Native acknowledgment was observed but complete desired comment fields could not be verified; retained identity does not authorize replay.",
            );
            error.details.write_state = if observation.state == SubmissionState::Confirmed {
                RequestState::Confirmed
            } else {
                RequestState::SentUnknown
            };
            observation.error = Some(error);
        }
    }
    Ok(observation)
}
