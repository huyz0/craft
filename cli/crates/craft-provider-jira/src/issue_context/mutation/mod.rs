//! Typed issue writes compiled from private live metadata. No transport or replay authority.
pub mod actions;
mod fields;
mod input;
pub mod result;

use super::*;
use craft_core::contracts::Envelope;
use craft_core::presence::InputField;
use fields::QualifiedField;
pub use input::{IssueChanges, IssueEditInput, IssueTransitionInput, LabelDelta};

pub const EDIT_ISSUE: &str = "jira.issue.update";
pub const TRANSITION_ISSUE: &str = "jira.transition.apply";
pub const UPDATE_ROUTE: &str = "jira-issue-update";
pub const APPLY_ROUTE: &str = "jira-transition-apply";

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationData {
    pub target: ResourceRef,
    pub issue_before: IssueState,
    pub destination_status_id: Option<String>,
}

pub struct JiraIssueMutations;
impl ProviderModule for JiraIssueMutations {
    fn manifest(&self) -> ProviderManifest {
        JiraIssueContext.manifest()
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
            let transition = command == TRANSITION_ISSUE;
            let expected = json!({"id":"10000","key":"EX-1","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"});
            let mut example = json!({"issue":"10000","expected":expected,"changes":{"summary":"Reviewed summary"}});
            if transition {
                example["transition_id"] = json!("31");
                example["expected_status_id"] = json!("103");
            }
            let mut path: Vec<_> = ["rest", "api", "3", "issue"]
                .map(|value| RouteSegment::Literal(value.into()))
                .into();
            path.push(RouteSegment::Parameter("issueIdOrKey".into()));
            if transition {
                path.push(RouteSegment::Literal("transitions".into()));
            }
            let descriptor = OperationDescriptor {
                id: command.into(), provider:"jira".into(),contract_version:SCHEMA_VERSION.into(),api_revision:"jira-cloud-v3".into(),
                description:"Prepare a typed issue mutation against private live context; execution requires a verified journaled host.".into(),
                effect:EffectClass::Write,
                input_schema:if transition {schema_for::<IssueTransitionInput>()?} else {schema_for::<IssueEditInput>()?},
                response_schema:response_schema::<MutationData>()?,example,
                response_example:json!(Envelope::success(command,"example".into(),json!({
                    "target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"issue","id":"10000"},
                    "issue_before":expected,"destination_status_id":if transition {Some("103")} else {None}
                }))),
                projection:None,scope_alternatives:vec![vec!["write:jira-work".into()]],
                permissions:vec![if transition {"Browse projects, issue visibility and Transition issues; native workflow conditions apply."} else {"Browse projects, issue visibility and Edit issues."}.into()],
                tenant_conditions:vec!["Exact numeric issue, expected state and fresh qualified edit or transition fields; no bypass parameters.".into()],
                routes:vec![NativeRoute {
                    id:if transition {APPLY_ROUTE} else {UPDATE_ROUTE}.into(),
                    method:if transition {HttpMethod::Post} else {HttpMethod::Put},path,
                    query_parameters:if transition {vec![]} else {vec!["notifyUsers".into(),"returnIssue".into()]},
                    continuation_parameters:vec![],repeated_query_parameters:vec![],response_max_bytes:1_048_576,
                }],
                retry_class:RetryClass::ReconcileBeforeReplay,reconciliation:ReconciliationClass::KnownResource,
            };
            if transition {
                registry.register_typed_handler::<IssueTransitionInput>(
                    descriptor,
                    Arc::new(Handler),
                )?;
            } else {
                registry.register_typed_handler::<IssueEditInput>(descriptor, Arc::new(Handler))?;
            }
        }
        Ok(())
    }
}
struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_request(request)
    }
    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::UnsupportedCapability,
            "Issue mutations require fresh private context and a verified journaled host.",
        ))
    }
}
fn invalid(path: &str, reason: &str) -> CraftError {
    CraftError::field(path, reason)
}
pub fn validate_request(request: &CanonicalRequest) -> Result<(), CraftError> {
    if request.selection.cursor.is_some() {
        return Err(invalid("/cursor", "issue_mutation_cursor_unsupported"));
    }
    input::intent(request).map(|_| ())
}

/// The call is bound to its operation and principal; the host must recheck it at send.
pub struct FrozenIssueMutation {
    call: NativeCall,
    operation: String,
    principal: MetadataPrincipal,
    wall: u64,
    monotonic: u64,
    data: MutationData,
    action: Option<actions::ActionEvidence>,
}
impl FrozenIssueMutation {
    pub fn call(&self) -> &NativeCall {
        &self.call
    }
    pub fn data(&self) -> &MutationData {
        &self.data
    }
    /// Inert observed context, never a complete hierarchy or replay authorization.
    pub fn action(&self) -> Option<&actions::ActionEvidence> {
        self.action.as_ref()
    }
    pub fn verify(
        &self,
        operation: &PreparedOperation,
        principal: &MetadataPrincipal,
        clock: &dyn Clock,
    ) -> Result<(), CraftError> {
        if operation.digest() != self.operation || principal != &self.principal {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Frozen issue mutation differs from its operation or principal.",
            ));
        }
        check_age(self.wall, self.monotonic, clock)
    }
}
fn check_age(wall: u64, monotonic: u64, clock: &dyn Clock) -> Result<(), CraftError> {
    let now = clock.unix_millis();
    let elapsed = clock.monotonic_millis();
    if now < wall
        || elapsed < monotonic
        || now - wall >= FRESHNESS_MS
        || elapsed - monotonic >= FRESHNESS_MS
    {
        return Err(changed());
    }
    Ok(())
}

impl LiveIssueContext {
    /// Reads only omitted required current values when needed; never sends the returned mutation.
    pub fn prepare_mutation(
        &self,
        read: &PreparedOperation,
        write: &PreparedOperation,
        context: &mut dyn OperationContext,
        principal: &MetadataPrincipal,
        clock: &dyn Clock,
    ) -> Result<FrozenIssueMutation, CraftError> {
        self.verify(read, principal, clock)?;
        context.verify_operation(read)?;
        context.checkpoint()?;
        let intent = input::intent(write.request())?;
        if !matches!(write.effect(), EffectClass::Write)
            || write.request().target != self.observation.resolved_issue
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue mutation target differs from the resolved live context.",
            ));
        }
        if intent.expected != self.observation.state {
            return Err(changed());
        }
        let transition = intent.transition.as_ref();
        let metadata = if let Some((id, status)) = transition {
            if read.request().command != TRANSITIONS
                || !FieldPlan::compile(TRANSITIONS, &read.request().selection.fields)?
                    .transition_fields()
            {
                return Err(invalid("/metadata", "expected_expanded_transition_fields"));
            }
            let input: TransitionInput =
                serde_json::from_value(read.request().input.clone()).map_err(|_| malformed())?;
            let native = self
                .native
                .get("transitions")
                .and_then(Value::as_array)
                .ok_or_else(malformed)?;
            let found = native
                .iter()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(id))
                .ok_or_else(|| invalid("/transition_id", "transition_not_returned"))?;
            match found.get("isAvailable") {
                Some(Value::Bool(true)) => {}
                None if !input.include_unavailable => {}
                _ => return Err(invalid("/transition_id", "transition_not_available")),
            }
            if found.pointer("/to/id").and_then(Value::as_str) != Some(status) {
                return Err(invalid(
                    "/expected_status_id",
                    "transition_destination_mismatch",
                ));
            }
            if !found.get("hasScreen").is_some_and(Value::is_boolean) {
                return Err(malformed());
            }
            found
                .get("fields")
                .and_then(Value::as_object)
                .ok_or_else(malformed)?
        } else {
            if read.request().command != EDIT {
                return Err(invalid("/metadata", "expected_edit_metadata"));
            }
            self.native
                .get("fields")
                .and_then(Value::as_object)
                .ok_or_else(malformed)?
        };
        let target = &write.request().target;
        let values = intent.changes.values()?;
        let mut fields = serde_json::Map::new();
        for (id, value) in &values {
            let raw = metadata
                .get(id)
                .ok_or_else(|| invalid("/changes", "field_not_in_metadata"))?;
            let qualified = QualifiedField::compile(target, id, raw)?;
            fields.insert(id.clone(), qualified.encode(value)?);
        }
        let mut required = BTreeMap::new();
        if transition.is_some() {
            for (id, raw) in metadata {
                context.checkpoint()?;
                if raw
                    .get("required")
                    .and_then(Value::as_bool)
                    .ok_or_else(malformed)?
                    && !values.contains_key(id)
                {
                    let field = QualifiedField::compile(target, id, raw)?;
                    if !field.has_qualified_default() {
                        required.insert(id.clone(), field);
                    }
                }
            }
        }
        let mut update = serde_json::Map::new();
        if let InputField::Value(delta) = &intent.changes.label_delta {
            let raw = metadata
                .get("labels")
                .ok_or_else(|| invalid("/changes/label_delta", "labels_not_in_metadata"))?;
            let field = QualifiedField::compile(target, "labels", raw)?;
            if !delta.add.is_empty() {
                field.supports("add")?;
            }
            if !delta.remove.is_empty() {
                field.supports("remove")?;
            }
            if raw
                .get("required")
                .and_then(Value::as_bool)
                .ok_or_else(malformed)?
            {
                required.insert("labels".into(), field);
            }
            let operations: Vec<_> = delta
                .add
                .iter()
                .map(|v| json!({"add":v}))
                .chain(delta.remove.iter().map(|v| json!({"remove":v})))
                .collect();
            update.insert("labels".into(), json!(operations));
        }
        if required.len() > 64 {
            return Err(invalid("/changes", "too_many_required_current_fields"));
        }
        if !required.is_empty() {
            self.verify(read, principal, clock)?;
            let mut selected = "project,issuetype,status,updated".to_string();
            for id in required.keys() {
                selected.push(',');
                selected.push_str(id);
            }
            let current = invoke(
                context,
                NativeCall {
                    route: IDENTITY_ROUTE.into(),
                    path_parameters: BTreeMap::from([("issueIdOrKey".into(), target.id.clone())]),
                    query: BTreeMap::from([
                        ("fields".into(), selected),
                        ("fieldsByKeys".into(), "false".into()),
                        ("updateHistory".into(), "false".into()),
                        ("failFast".into(), "true".into()),
                    ]),
                    repeated_query: BTreeMap::new(),
                    body: None,
                },
            )?;
            if parse_state(&current, &target.id)? != intent.expected {
                return Err(changed());
            }
            for (id, field) in &required {
                let current = current
                    .get("fields")
                    .and_then(|fields| fields.get(id))
                    .ok_or_else(|| invalid("/changes", "required_current_value_not_returned"))?;
                if id == "labels"
                    && let InputField::Value(delta) = &intent.changes.label_delta
                {
                    let mut labels = field.current_labels(current)?;
                    for value in &delta.remove {
                        labels.remove(value);
                    }
                    labels.extend(delta.add.iter().cloned());
                    if !field.existing(&json!(labels)) {
                        return Err(invalid(
                            "/changes/label_delta",
                            "required_labels_would_be_empty_or_invalid",
                        ));
                    }
                } else if !field.existing(current) {
                    return Err(invalid(
                        "/changes",
                        "required_current_value_empty_or_unqualified",
                    ));
                }
            }
        }
        let mut body = json!({});
        if !fields.is_empty() {
            body["fields"] = Value::Object(fields);
        }
        if !update.is_empty() {
            body["update"] = Value::Object(update);
        }
        if let Some((id, _)) = transition {
            body["transition"] = json!({"id":id});
        }
        let frozen = FrozenIssueMutation {
            call: NativeCall {
                route: if transition.is_some() {
                    APPLY_ROUTE
                } else {
                    UPDATE_ROUTE
                }
                .into(),
                path_parameters: BTreeMap::from([("issueIdOrKey".into(), target.id.clone())]),
                query: if transition.is_some() {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([
                        ("notifyUsers".into(), "true".into()),
                        ("returnIssue".into(), "false".into()),
                    ])
                },
                repeated_query: BTreeMap::new(),
                body: Some(body),
            },
            operation: write.digest().into(),
            principal: principal.clone(),
            wall: self.wall,
            monotonic: self.monotonic,
            data: MutationData {
                target: target.clone(),
                issue_before: intent.expected,
                destination_status_id: transition.map(|(_, status)| status.clone()),
            },
            action: None,
        };
        context.checkpoint()?;
        self.verify(read, principal, clock)?;
        frozen.verify(write, principal, clock)?;
        Ok(frozen)
    }
}
