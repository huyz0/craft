use super::*;
use craft_core::contracts::{CanonicalRequest, Selection};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::presence::InputField;
use craft_provider_jira::create_metadata::{self, MetadataPrincipal};

type GroupKey = (String, String, String, Option<String>);
struct Guard<'a> {
    native: &'a mut dyn OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
    details: &'a mut BTreeMap<String, Value>,
}
impl OperationContext for Guard<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(op)
    }
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let response = self.native.invoke(call)?;
        crate::issue_create::material::observe_fields(call, &response, self.details)?;
        self.checkpoint()?;
        Ok(response)
    }
}
impl LiveBindingHost<'_> {
    pub(super) fn qualify_tasks(
        &mut self,
        plan: &Plan,
        mut remaining: usize,
    ) -> Result<Vec<ResolvedTask>, CraftError> {
        let mut groups: BTreeMap<GroupKey, Vec<usize>> = BTreeMap::new();
        for (index, task) in self.request.tasks.iter().enumerate() {
            let parent = match &task.input.parent_issue_id {
                InputField::Value(id) => Some(id.clone()),
                _ => None,
            };
            groups
                .entry((
                    task.profile.clone(),
                    task.project_id.clone(),
                    task.input.issue_type_id.clone(),
                    parent,
                ))
                .or_default()
                .push(index);
        }
        let mut result = vec![];
        for (key, tasks) in groups {
            result.extend(self.qualify_group(plan, &key, &tasks, &mut remaining)?);
        }
        result.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(result)
    }
    fn qualify_group(
        &mut self,
        plan: &Plan,
        key: &GroupKey,
        tasks: &[usize],
        remaining: &mut usize,
    ) -> Result<Vec<ResolvedTask>, CraftError> {
        let policy = self.capture_jira(&key.0)?;
        self.check_all()?;
        self.reserve_open()?;
        let registry = crate::issue_create::host::registry()?;
        let reuse_registry = if tasks.iter().any(|index| {
            plan.tasks.iter().any(|task| {
                task.task_id == self.request.tasks[*index].task_id
                    && task.jira.existing_issue.is_some()
            })
        }) {
            let mut reads = craft_core::registry::Registry::default();
            reads.register_provider(&craft_provider_jira::issue_reads::JiraIssues)?;
            Some(reads)
        } else {
            None
        };
        let prepare = |command: &str, input: Value| {
            registry.prepare(CanonicalRequest {
                command: command.into(),
                target: policy.target.clone(),
                input,
                selection: Selection::default(),
                preconditions: None,
            })
        };
        let operation = prepare(
            create_metadata::COMMAND,
            json!({"issue_type_id":key.2,"parent_issue_id":key.3,"page_size":50}),
        )?;
        let mut accounts = BTreeMap::new();
        let links = if tasks.iter().any(|index| {
            plan.tasks.iter().any(|t| {
                t.task_id == self.request.tasks[*index].task_id && !t.depends_on.is_empty()
            })
        }) {
            Some(prepare(craft_provider_jira::issue_links::TYPES, json!({}))?)
        } else {
            None
        };
        for index in tasks {
            if let InputField::Value(id) = &self.request.tasks[*index].input.assignee_account_id {
                accounts.insert(
                    id.clone(),
                    prepare(create_metadata::ASSIGNEE_COMMAND, json!({"account_id":id}))?,
                );
            }
        }
        let locked = self.credentials.lock(&policy.grant, &self.control)?;
        let client = self.client.for_invocation(self.control.clone())?;
        let access = match client.open_service_access(&locked, &policy.target.tenant_ref()) {
            Ok(access) => access,
            Err(failure) => return Err(self.record_auth_failure(policy.grant.clone(), failure)),
        };
        if let Some(r) = access.cache_update() {
            self.receipts.push(CredentialCacheUpdate {
                grant: policy.grant.clone(),
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            });
        }
        let principal = MetadataPrincipal {
            grant: access.grant().clone(),
            actor: access.identity().actor().into(),
        };
        if access.tenant() != &policy.target.tenant_ref()
            || access.grant() != &policy.grant
            || access.identity().actor() != policy.expected_actor
        {
            return Err(denied(
                "Native Jira metadata access differs from its captured create-preparation authority.",
            ));
        }
        for read in std::iter::once(&operation)
            .chain(accounts.values())
            .chain(links.iter())
        {
            access.check_operation(&registry, read)?;
        }
        let check = || {
            self.control.check()?;
            access.check_operation(&registry, &operation)?;
            self.check_review()?;
            self.store.verify_create_policy(&policy, &self.control)?;
            self.control.check()
        };
        check()?;
        let mut native = access.read_context(
            &registry,
            &operation,
            self.tls.clone(),
            self.control.clone(),
        )?;
        let mut details = BTreeMap::new();
        let context = {
            let mut guard = Guard {
                native: &mut native,
                check: &check,
                details: &mut details,
            };
            create_metadata::read_context(
                &operation,
                &mut guard,
                principal.clone(),
                client.clock().clone(),
            )?
        };
        let mut assignees = BTreeMap::new();
        let link_type = if let Some(op) = &links {
            let mut native =
                access.read_context(&registry, op, self.tls.clone(), self.control.clone())?;
            let mut unused = BTreeMap::new();
            let mut guard = Guard {
                native: &mut native,
                check: &check,
                details: &mut unused,
            };
            let types = craft_provider_jira::issue_links::read_types(op, &mut guard)?;
            Some(
                types
                    .items
                    .into_iter()
                    .find(|t| Some(&t.id) == self.request.dependency_link_type_id.as_ref())
                    .ok_or_else(|| invalid("selected_dependency_link_type_not_available"))?,
            )
        } else {
            None
        };
        for (id, op) in &accounts {
            let mut native =
                access.read_context(&registry, op, self.tls.clone(), self.control.clone())?;
            let mut unused = BTreeMap::new();
            let mut guard = Guard {
                native: &mut native,
                check: &check,
                details: &mut unused,
            };
            assignees.insert(
                id.clone(),
                create_metadata::read_assignable(
                    op,
                    &mut guard,
                    principal.clone(),
                    client.clock().clone(),
                )?,
            );
        }
        let summary = context.summary();
        let metadata = crate::issue_create::material::snapshot(&context, &details);
        let metadata_digest = digest(&metadata)?;
        let mut result = vec![];
        for index in tasks {
            check()?;
            let proposal = &self.request.tasks[*index];
            let task = plan
                .tasks
                .iter()
                .find(|t| t.task_id == proposal.task_id)
                .ok_or_else(|| invalid("task_not_in_live_plan"))?;
            if summary.project_key != task.jira.project_key
                || summary.issue_type_name != task.jira.issue_type
            {
                return Err(invalid(
                    "native_project_key_or_issue_type_name_differs_from_plan",
                ));
            }
            let input = serde_json::to_value(&proposal.input)
                .map_err(|_| invalid("invalid_create_proposal"))?;
            let create = prepare(craft_provider_jira::created_issue::COMMAND, input.clone())?;
            let assignee = match &proposal.input.assignee_account_id {
                InputField::Value(id) => assignees.get(id),
                _ => None,
            };
            let frozen = context.prepare_create_with_assignee(&create, &principal, assignee)?;
            frozen.verify(&create, &principal)?;
            let payload = frozen
                .call()
                .body
                .clone()
                .ok_or_else(|| invalid("missing_frozen_create_payload"))?;
            for field in &summary.fields {
                if field.has_default
                    && payload["fields"].get(&field.id).is_none()
                    && metadata["field_details"][&field.id]["default_value"].is_null()
                {
                    return Err(CraftError::field(
                        format!("/tasks/{}/input/{}", proposal.task_id, field.id),
                        "omitted_field_default_not_reported_by_provider",
                    ));
                }
            }
            let resolved = ResolvedTask {
                task_id: proposal.task_id.clone(),
                profile: proposal.profile.clone(),
                target: policy.target.clone(),
                input,
                payload,
                metadata: metadata.clone(),
                metadata_digest: metadata_digest.clone(),
                existing_issue: task.jira.existing_issue.clone(),
                existing_issue_observation: if let Some(existing) = &task.jira.existing_issue {
                    let registry = reuse_registry
                        .as_ref()
                        .ok_or_else(|| invalid("missing_reuse_read_registry"))?;
                    let mut target = policy.target.clone();
                    target.resource_type = "site".into();
                    target.id = target.tenant.clone();
                    let operation = registry.prepare(CanonicalRequest {
                        command: craft_provider_jira::issue_reads::GET.into(),
                        target,
                        input: json!({"issue": existing.issue_id}),
                        selection: Selection {
                            fields: vec!["id".into(), "project.id".into(), "issue_type.id".into()],
                            ..Selection::default()
                        },
                        preconditions: None,
                    })?;
                    let fields = craft_provider_jira::issue_reads::FieldPlan::compile(
                        craft_provider_jira::issue_reads::GET,
                        &operation.request().selection.fields,
                    )?;
                    let mut native = access.read_context(
                        registry,
                        &operation,
                        self.tls.clone(),
                        self.control.clone(),
                    )?;
                    let mut unused = BTreeMap::new();
                    let mut guard = Guard {
                        native: &mut native,
                        check: &check,
                        details: &mut unused,
                    };
                    let (observed, _) = craft_provider_jira::issue_reads::read_get(
                        &operation, &mut guard, &fields,
                    )?;
                    if observed["id"] != existing.issue_id
                        || observed["project"]["id"] != policy.target.id
                        || observed["issue_type"]["id"] != proposal.input.issue_type_id
                    {
                        return Err(invalid("reused_issue_identity_project_or_type_mismatch"));
                    }
                    Some(observed)
                } else {
                    None
                },
                dependency_link_type: if task.depends_on.is_empty() {
                    None
                } else {
                    Some(DependencyType {
                        link_type: link_type
                            .clone()
                            .ok_or_else(|| invalid("missing_native_dependency_link_type"))?,
                        dependent_direction: self
                            .request
                            .dependency_link_direction
                            .ok_or_else(|| invalid("missing_dependency_direction"))?,
                    })
                },
            };
            *remaining = remaining
                .checked_sub(encoded_size(&resolved)?)
                .ok_or_else(|| invalid("review_snapshot_byte_limit"))?;
            result.push(resolved);
        }
        check()?;
        self.check_all()?;
        Ok(result)
    }
}
