use super::*;
use crate::confluence_pages::command::{PageCreateArgs, PageWriteArgs, PreparedInvocation};
use crate::issue_create::host::{CreatePolicy, PreparedCreate};
use crate::issue_links::{LinkEnvironment, PreparedLink};
use craft_provider_confluence::pages::{PageBody, PagePlacement};
use craft_runtime::confluence_write_policy::ConfluenceWriteOperation;
use craft_runtime::issue_mutation_policy::IssueMutationOperation;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
impl Run<'_> {
    pub(super) fn preflight_policies(&self) -> Result<(), CraftError> {
        let store = self.environment.store;
        let control = &self.environment.control;
        let result = store
            .confluence_write_policy(self.profile, &self.preview.space_id, control)?
            .ok_or_else(|| denied("Configure result-page creation before applying tickets."))?;
        if result.target.tenant_ref() != self.preview.source.tenant_ref() {
            return Err(denied("Result policy is outside the reviewed source site."));
        }
        store.verify_confluence_write_policy(&result, ConfluenceWriteOperation::Create, control)?;
        for task in &self.preview.tasks {
            let policy = store
                .create_policy(&task.profile, control)?
                .ok_or_else(|| denied("An approved task has no explicit create policy."))?;
            if policy.target != task.target {
                return Err(denied(
                    "A task create policy is outside its approved project.",
                ));
            }
            store.verify_create_policy(&policy, control)?;
        }
        for link in &self.preview.links {
            let outward = self.preview.task(&link.outward_task_id)?;
            let inward = self.preview.task(&link.inward_task_id)?;
            for target in [&outward.target, &inward.target] {
                let policy=store.issue_mutation_policy(&outward.profile,&target.id,control)?.ok_or_else(||denied("Dependency creation requires both endpoint allowances under the outward task profile."))?;
                if policy.target != *target {
                    return Err(denied(
                        "Dependency policy differs from the approved endpoint project.",
                    ));
                }
                store.verify_issue_mutation_policy(
                    &policy,
                    IssueMutationOperation::Link,
                    control,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn create_issue(
        &mut self,
        task: &ResolvedTask,
        spec: &StepSpec,
        batch: &verification::RevalidatedManifest<'_>,
    ) -> Result<(), CraftError> {
        batch.check()?;
        let operation_id = ledger::operation_id(&self.run_digest, &spec.key)?;
        self.data.current_operation_id = Some(operation_id.clone());
        let store = self.environment.store;
        let control = self.environment.control.clone();
        let policy = store
            .create_policy(&task.profile, &control)?
            .ok_or_else(|| denied("Approved issue create policy is missing."))?;
        if policy.target != task.target {
            return Err(denied(
                "Issue target differs from its current explicit create policy.",
            ));
        }
        store.verify_create_policy(&policy, &control)?;
        let prepared = PreparedCreate::prepare(
            CanonicalRequest {
                command: craft_provider_jira::created_issue::COMMAND.into(),
                target: task.target.clone(),
                input: task.input.clone(),
                selection: Selection {
                    max_bytes: 8_388_608,
                    ..Selection::default()
                },
                preconditions: None,
            },
            &operation_id,
            true,
            self.request_id,
            CreatePolicy {
                enabled: policy.enabled,
                target: task.target.clone(),
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
            },
        )?
        .constrain_reviewed_material(task.payload.clone(), task.metadata_digest.clone())?;
        self.save_intent(spec, prepared.binding())?;
        let (prepared, source) = match prepared.retain_source(store, self.journal, &control) {
            Ok(retained) => retained,
            Err(failure) => return self.capture(spec, failure.envelope),
        };
        let access = (|| {
            let locked = self.environment.credentials.lock(&policy.grant, &control)?;
            let client = self.environment.client.for_invocation(control.clone())?;
            Ok::<_, CraftError>((locked, client))
        })();
        let (locked, client) = match access {
            Ok(access) => access,
            Err(error) => {
                let result = prepared.reject_retained_source(error, source, self.journal, &control);
                return self.capture(spec, result.envelope);
            }
        };
        let result = prepared.execute_cached_with_policy_check(
            &client,
            &locked,
            self.journal,
            self.environment.tls.clone(),
            control.clone(),
            || {
                batch.check()?;
                store.verify_create_policy(&policy, &control)?;
                batch.check()
            },
        );
        drop(locked);
        self.capture(spec, crate::saved_requests::attach(result, source).envelope)
    }
    pub(super) fn create_link(
        &mut self,
        spec: &StepSpec,
        input: craft_provider_jira::issue_links::write::LinkInput,
        profile: &str,
        batch: &verification::RevalidatedManifest<'_>,
    ) -> Result<(), CraftError> {
        batch.check()?;
        let operation_id = ledger::operation_id(&self.run_digest, &spec.key)?;
        self.data.current_operation_id = Some(operation_id.clone());
        let prepared = PreparedLink::prepare(
            input,
            spec.target.clone(),
            profile,
            &operation_id,
            true,
            self.environment.store,
            &self.environment.control,
        )?;
        self.save_intent(spec, prepared.binding())?;
        let locked = self
            .environment
            .credentials
            .lock(prepared.grant(), &self.environment.control)?;
        let (data, error) = prepared.execute(
            LinkEnvironment {
                store: self.environment.store,
                client: self.environment.client,
                locked: &locked,
                journal: self.journal,
                tls: self.environment.tls.clone(),
                control: self.environment.control.clone(),
            },
            || batch.check(),
        );
        drop(locked);
        let command = craft_provider_jira::issue_links::write::CREATE;
        let envelope = match error {
            Some(error) => {
                Envelope::failure_with_data(command, self.request_id.into(), error, json!(data))
            }
            None => Envelope::success(command, self.request_id.into(), json!(data)),
        };
        self.capture(spec, envelope)
    }
    pub(super) fn publish_results(&mut self) -> Result<(), CraftError> {
        let mut body = format!(
            "<h1>Ticket results</h1><p>Run: {}. Approved manifest: {}.</p><table><tbody><tr><th>Task</th><th>Jira issue ID</th><th>Jira cloud ID</th></tr>",
            escape(&self.args.run_id),
            escape(&self.preview.manifest_digest)
        );
        for mapping in &self.data.known_issues {
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&mapping.task_id),
                escape(&mapping.issue.id),
                escape(&mapping.issue.tenant)
            ));
        }
        if !self.data.reused_issues.is_empty() {
            body.push_str("</tbody></table><h2>Explicitly reused issues</h2><table><tbody><tr><th>Task</th><th>Existing issue ID</th><th>Original operation</th><th>Original attribution</th></tr>");
            for reused in &self.data.reused_issues {
                body.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape(&reused.task_id),
                    escape(&reused.issue.id),
                    escape(&reused.origin.binding.operation_id),
                    if reused.origin_attribution_unresolved {
                        "Unresolved; reuse does not confirm the original create"
                    } else {
                        "Originally confirmed; no new create"
                    }
                ));
            }
        }
        body.push_str(&format!("</tbody></table><p>{} approved dependency relations were acknowledged. Duplicate relations are not attributed as newly created links.</p>",self.data.links_confirmed));
        let target = ResourceRef {
            resource_type: "site".into(),
            id: self.preview.source.tenant.clone(),
            ..self.preview.source.clone()
        };
        let spec = StepSpec {
            key: "results-page".into(),
            kind: Kind::ResultsPage,
            target: target.clone(),
            logical_digest: digest(
                &json!({"title":self.args.result_title,"parent":self.preview.source,"space_id":self.preview.space_id,"body":body}),
            )?,
            route: craft_provider_confluence::pages::CREATE_ROUTE.into(),
            expected_resources: vec![],
            origin_operation_id: None,
        };
        let observed = self.observe(&spec)?;
        if observed.report.complete {
            self.data.result_page = observed.report.resources.first().cloned();
        } else {
            if !observed.ready() {
                return Err(self.refuse_pending(observed));
            }
            let operation_id = ledger::operation_id(&self.run_digest, &spec.key)?;
            self.data.current_operation_id = Some(operation_id.clone());
            let options = InvocationOptions {
                state_root: Some(self.environment.store.path()),
                profile: Some(self.profile),
                request_id: self.request_id,
                max_bytes: 8_388_608,
            };
            let prepared = PreparedInvocation::prepare(
                PageWriteArgs::Create(PageCreateArgs {
                    operation_id,
                    apply: true,
                    cloud_id: Some(target.tenant),
                    space_id: Some(self.preview.space_id.clone()),
                    title: self.args.result_title.clone(),
                    placement: PagePlacement::Parent {
                        id: self.preview.source.id.clone(),
                    },
                    body: PageBody {
                        representation:
                            craft_provider_confluence::pages::PageRepresentation::Storage,
                        value: body,
                    },
                }),
                &[],
                &options,
                self.environment.store,
                &self.environment.control,
            )?;
            self.save_intent(&spec, prepared.binding())?;
            let locked = self
                .environment
                .credentials
                .lock(prepared.grant(), &self.environment.control)?;
            let client = self
                .environment
                .client
                .for_invocation(self.environment.control.clone())?;
            let result = prepared.execute_cached(
                self.environment.store,
                &client,
                &locked,
                self.journal,
                self.environment.tls.clone(),
                self.environment.control.clone(),
            );
            drop(locked);
            self.capture(&spec, result.envelope)?;
            self.data.result_page = self
                .reports
                .get(&spec.key)
                .and_then(|r| r.resources.first())
                .cloned();
        }
        let page = self
            .data
            .result_page
            .clone()
            .ok_or_else(|| invalid("confirmed_result_page_identity_missing"))?;
        let mut links = vec![];
        for link in &self.preview.links {
            links.push(json!({"prerequisite_task_id":link.prerequisite_task_id,"dependent_task_id":link.dependent_task_id,"outward_issue":self.mapping(&link.outward_task_id)?,"inward_issue":self.mapping(&link.inward_task_id)?,"link_type":link.link_type,"relation_confirmed":true,"new_link_created":null}));
        }
        let artifact_name = format!("ticket-results-{}.json", &self.run_digest[7..]);
        self.data.result_artifact_name = Some(artifact_name.clone());
        let mut results = json!({"format":"craft.ticket-results/v1","run_id":self.args.run_id,"manifest_digest":self.preview.manifest_digest,"preview":self.preview.receipt,"source":self.preview.source,"issues":self.data.known_issues,"links":links,"result_page":page});
        if !self.data.reused_issues.is_empty() {
            results["reused_issues"] = json!(self.data.reused_issues);
        }
        let result = ledger::ensure(
            self.artifacts,
            &artifact_name,
            &results,
            "application/vnd.craft.ticket-results+json",
            &self.preview.source,
        );
        match result {
            Ok(receipt) => self.data.result_artifact = Some(receipt),
            Err(error) => {
                self.data.artifact_error = Some(error.clone());
                self.data.artifact_write_state = error.details.write_state;
                return Err(error);
            }
        }
        Ok(())
    }
}
