use super::*;

pub struct JiraIssueActions;
impl ProviderModule for JiraIssueActions {
    fn manifest(&self) -> ProviderManifest {
        JiraIssueContext.manifest()
    }
    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for command in [ASSIGN_ISSUE, UNASSIGN_ISSUE, DELETE_ISSUE] {
            for read in [false, true] {
                let descriptor = descriptor(command, read)?;
                match command {
                    ASSIGN_ISSUE => registry.register_typed_handler::<IssueAssignmentInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?,
                    UNASSIGN_ISSUE => registry.register_typed_handler::<IssueUnassignInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?,
                    _ => registry.register_typed_handler::<IssueDeleteInput>(
                        descriptor,
                        Arc::new(Handler),
                    )?,
                }
            }
        }
        Ok(())
    }
}
fn descriptor(command: &str, read: bool) -> Result<OperationDescriptor, CraftError> {
    let deletion = command == DELETE_ISSUE;
    let id = if read {
        metadata_command(command)?
    } else {
        command
    };
    let expected = json!({"id":"10000","key":"EX-1","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"});
    let mut example = json!({"issue":"10000","expected":expected});
    if deletion {
        example["subtasks"] = json!({"mode":"refuse"});
    } else {
        example["expected_assignee"] = json!({"kind":"unassigned"});
    }
    if command == ASSIGN_ISSUE {
        example["account_id"] = json!("account-one");
    }
    let mut path: Vec<_> = ["rest", "api", "3", "issue"]
        .map(|v| RouteSegment::Literal(v.into()))
        .into();
    path.push(RouteSegment::Parameter("issueIdOrKey".into()));
    if !read && !deletion {
        path.push(RouteSegment::Literal("assignee".into()));
    }
    let mut routes = vec![NativeRoute {
        id: if read {
            IDENTITY_ROUTE
        } else if deletion {
            DELETE_ROUTE
        } else {
            ASSIGN_ROUTE
        }
        .into(),
        method: if read {
            HttpMethod::Get
        } else if deletion {
            HttpMethod::Delete
        } else {
            HttpMethod::Put
        },
        path,
        query_parameters: if read {
            ["fields", "fieldsByKeys", "updateHistory", "failFast"]
                .map(String::from)
                .into()
        } else if deletion {
            vec!["deleteSubtasks".into()]
        } else {
            vec![]
        },
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 1_048_576,
    }];
    let mut scopes = vec![
        if read {
            "read:jira-work"
        } else {
            "write:jira-work"
        }
        .into(),
    ];
    if read && command == ASSIGN_ISSUE {
        scopes.push("read:jira-user".into());
        routes.push(NativeRoute {
            id: ACCOUNT_ROUTE.into(),
            method: HttpMethod::Get,
            path: ["rest", "api", "3", "user", "assignable", "search"]
                .map(|v| RouteSegment::Literal(v.into()))
                .into(),
            query_parameters: ["issueId", "accountId", "startAt", "maxResults"]
                .map(String::from)
                .into(),
            continuation_parameters: vec![],
            repeated_query_parameters: vec![],
            response_max_bytes: 65_536,
        });
    }
    let evidence = if deletion {
        json!({"kind":"deletion","observed_subtasks":[],"delete_all_subtasks":false,
            "includes_unobserved_subtasks":false,"hierarchy_complete":false})
    } else {
        let intended = if command == ASSIGN_ISSUE {
            json!({"kind":"account","account_id":"account-one"})
        } else {
            json!({"kind":"unassigned"})
        };
        json!({"kind":"assignment","before":{"kind":"unassigned"},"intended":intended})
    };
    let response_example = json!(Envelope::success(
        id,
        "example".into(),
        json!({
            "target":{"provider":"jira","deployment":"cloud",
                "tenant":"00000000-1111-2222-3333-444444444444","resource_type":"issue","id":"10000"},
            "issue_before":expected,"evidence":evidence,"atomic_snapshot":false,"mutation_authorized":false
        })
    ));
    Ok(OperationDescriptor {
        id: id.into(),
        provider: "jira".into(),
        contract_version: SCHEMA_VERSION.into(),
        api_revision: "jira-cloud-v3".into(),
        description: "Prepare an issue-specific assignment or explicit hierarchy deletion; verified journaled execution is separate.".into(),
        effect: if read { EffectClass::Read } else if deletion { EffectClass::Permanent } else { EffectClass::Write },
        input_schema: match command {
            ASSIGN_ISSUE => schema_for::<IssueAssignmentInput>()?,
            UNASSIGN_ISSUE => schema_for::<IssueUnassignInput>()?,
            _ => schema_for::<IssueDeleteInput>()?,
        },
        response_schema: response_schema::<ActionData>()?,
        example,
        response_example,
        projection: None,
        scope_alternatives: vec![scopes],
        permissions: vec![if deletion {
            "Browse projects, issue visibility and Delete issues."
        } else {
            "Browse projects, issue visibility and Assign issues; native account/unassignment configuration applies."
        }.into()],
        tenant_conditions: vec!["Exact numeric issue, project, principal and expected state; no server CAS or complete hierarchy guarantee.".into()],
        routes,
        retry_class: if read { RetryClass::SafeRead } else { RetryClass::ReconcileBeforeReplay },
        reconciliation: if read { ReconciliationClass::NotApplicable } else { ReconciliationClass::KnownResource },
    })
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
            "Issue actions require private live preparation and verified journaled execution.",
        ))
    }
}
