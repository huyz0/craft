//! Reviewed batches compose sealed native hosts; immutable records never restore approval authority.
pub mod command;
mod ledger;
mod native;
pub mod recovery;
mod reuse;
pub use reuse::ReuseEvidence;
#[cfg(test)]
mod tests;

use super::*;
use crate::profiles::InvocationOptions;
use craft_core::contracts::{CanonicalRequest, Envelope, Selection};
use craft_core::storage::{
    ArtifactReceipt, ArtifactStore, JournalRecord, OperationJournal, StoreError,
};
use craft_runtime::journal::NativeJournal;
use ledger::{Kind, Observed, StepReport, StepSpec};

pub const CREATE: &str = "plan.tickets-create";
pub const RESUME: &str = "plan.tickets-resume";
const OUTPUT: usize = 16_777_216;

fn local_error(error: StoreError) -> CraftError {
    let mut failure = CraftError::new(
        match error {
            StoreError::Conflict => ErrorCode::VersionConflict,
            StoreError::OutcomeUnknown => ErrorCode::WriteOutcomeUnknown,
            StoreError::Cancelled => ErrorCode::Cancelled,
            StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            _ => ErrorCode::Internal,
        },
        "Ticket ledger publication or inspection failed; preserve the run and native operation identities before recovery.",
    );
    if error == StoreError::OutcomeUnknown {
        failure.details.write_state = RequestState::SentUnknown;
    }
    failure.next_action="Keep the same run ID, preview and artifact names. Inspect local records before tickets-resume; do not replace the run or replay an unknown native operation.".into();
    failure
}

#[derive(Clone, Copy, clap::ValueEnum, Serialize, Deserialize, JsonSchema)]
#[value(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Consistency {
    RevalidateEachBatch,
}
fn required_consistency<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Consistency>, D::Error> {
    Consistency::deserialize(d).map(Some)
}
#[derive(clap::Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub run_id: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))]
    pub preview_artifact: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 255))]
    pub result_title: String,
    #[arg(long, value_enum)]
    #[serde(deserialize_with = "required_consistency")]
    #[schemars(required, with = "Consistency")]
    pub consistency: Option<Consistency>,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
}
impl ApplyArgs {
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        validate_profile_name(
            options
                .profile
                .ok_or_else(|| invalid("explicit_ticket_profile_required"))?,
        )?;
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        if !identifier(&self.run_id)
            || self.preview_artifact.len() > 128
            || self.preview_artifact.is_empty()
            || self.preview_artifact == "."
            || self.preview_artifact == ".."
            || !self
                .preview_artifact
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || self.result_title.trim().is_empty()
            || self.result_title.chars().count() > 255
            || self.result_title.chars().any(char::is_control)
            || self.consistency.is_none()
            || options.request_id.len() > 80
            || !fields.is_empty()
        {
            return Err(invalid("invalid_ticket_run_or_incomplete_output"));
        }
        if options.max_bytes != OUTPUT {
            return Err(CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Use --max-bytes 16777216 to reserve complete exceptional ticket and artifact evidence; successful output stays compact.",
            ));
        }
        if !self.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Ticket application requires explicit apply.",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueMapping {
    pub task_id: String,
    pub issue: ResourceRef,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionData {
    pub run_id: String,
    pub run_digest: Option<String>,
    pub run_intent: Option<ArtifactReceipt>,
    pub run_intent_name: Option<String>,
    pub known_issues: Vec<IssueMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reused_issues: Vec<ReuseEvidence>,
    pub links_confirmed: u32,
    pub resources_complete: bool,
    pub complete: bool,
    pub result_page: Option<ResourceRef>,
    pub result_artifact: Option<ArtifactReceipt>,
    pub result_artifact_name: Option<String>,
    pub progress_artifact: Option<ArtifactReceipt>,
    pub progress_artifact_name: Option<String>,
    pub artifact_write_state: RequestState,
    pub artifact_error: Option<CraftError>,
    pub credential_cache_updates: Vec<CredentialCacheUpdate>,
    pub current_operation_id: Option<String>,
    pub current_attempt: Option<Envelope>,
    pub unresolved_record: Option<JournalRecord>,
    pub stop_error: Option<CraftError>,
}
fn empty(args: &ApplyArgs) -> ExecutionData {
    ExecutionData {
        run_id: args.run_id.clone(),
        run_digest: None,
        run_intent: None,
        run_intent_name: None,
        known_issues: vec![],
        reused_issues: vec![],
        links_confirmed: 0,
        resources_complete: false,
        complete: false,
        result_page: None,
        result_artifact: None,
        result_artifact_name: None,
        progress_artifact: None,
        progress_artifact_name: None,
        artifact_write_state: RequestState::NotSent,
        artifact_error: None,
        credential_cache_updates: vec![],
        current_operation_id: None,
        current_attempt: None,
        unresolved_record: None,
        stop_error: None,
    }
}
fn current_receipt_state(data: &ExecutionData) -> Option<RequestState> {
    let operation = data.current_operation_id.as_ref()?;
    let envelope = data.current_attempt.as_ref()?;
    if !matches!(
        envelope.command.as_str(),
        "jira.issue.create" | "jira.link.create" | "confluence.page.create"
    ) {
        return None;
    }
    let receipt = &envelope.data.as_ref()?["receipt"];
    if receipt["operation_id"] != *operation {
        return None;
    }
    serde_json::from_value(receipt["request_state"].clone()).ok()
}
fn has_uncertainty(
    data: &ExecutionData,
    reports: &BTreeMap<String, StepReport>,
    failure: &CraftError,
) -> bool {
    let current = data.current_operation_id.as_deref();
    failure.details.write_state == RequestState::SentUnknown
        || reports
            .values()
            .any(|r| r.state == RequestState::SentUnknown)
        || data.current_attempt.as_ref().is_some_and(|e| {
            current.is_some()
                && matches!(
                    e.command.as_str(),
                    "jira.issue.create" | "jira.link.create" | "confluence.page.create"
                )
                && e.data.as_ref().is_some_and(|d| {
                    d["receipt"]["operation_id"].as_str() == current
                        && (d["receipt"]["request_state"] == "sent_unknown"
                            || e.error.as_ref().is_some_and(|error| {
                                error.details.write_state == RequestState::SentUnknown
                            }))
                })
        })
        || data.unresolved_record.as_ref().is_some_and(|r| {
            Some(r.binding.operation_id.as_str()) == current
                && r.steps.values().any(|s| s.state.requires_reconciliation())
        })
        || data
            .reused_issues
            .iter()
            .any(|e| e.origin_attribution_unresolved)
}
struct Preview {
    receipt: ArtifactReceipt,
    review_id: String,
    manifest_digest: String,
    source: ResourceRef,
    space_id: String,
    tasks: Vec<ResolvedTask>,
    batches: Vec<Vec<String>>,
    links: Vec<tickets::DependencyLink>,
}
impl Preview {
    fn load(name: &str, artifacts: &dyn ArtifactStore) -> Result<Self, CraftError> {
        let blob = artifacts
            .read(name, SNAPSHOT_BYTES as u64)
            .map_err(local_error)?
            .ok_or_else(|| invalid("ticket_preview_not_found"))?;
        let value = craft_core::input::parse_document_json(&blob.bytes, SNAPSHOT_BYTES)?;
        let manifest = &value["manifest"];
        let source = blob
            .receipt
            .source
            .clone()
            .ok_or_else(|| invalid("ticket_preview_has_no_source"))?;
        let review_id = value["review_id"]
            .as_str()
            .filter(|v| identifier(v))
            .ok_or_else(|| invalid("invalid_original_review_id"))?
            .into();
        let tasks: Vec<ResolvedTask> = serde_json::from_value(manifest["context"]["tasks"].clone())
            .map_err(|_| invalid("invalid_preview_tasks"))?;
        let batches: Vec<Vec<String>> = serde_json::from_value(manifest["batches"].clone())
            .map_err(|_| invalid("invalid_preview_batches"))?;
        let links: Vec<tickets::DependencyLink> = serde_json::from_value(manifest["links"].clone())
            .map_err(|_| invalid("invalid_preview_links"))?;
        let binding = manifest["binding_digest"]
            .as_str()
            .ok_or_else(|| invalid("missing_manifest_binding"))?;
        let raw = manifest["context"]["tasks"]
            .as_array()
            .ok_or_else(|| invalid("invalid_preview_tasks"))?;
        let summary =
            tickets::material_summary("craft.ticket-manifest/v1", binding, &batches, &links, raw)?;
        let space_id = manifest["context"]["plan"]["source"]["space_id"]
            .as_str()
            .filter(|v| numeric(v))
            .ok_or_else(|| invalid("missing_preview_space"))?
            .into();
        let ids: BTreeSet<_> = tasks.iter().map(|t| t.task_id.as_str()).collect();
        let flattened: Vec<_> = batches.iter().flatten().map(String::as_str).collect();
        if value["format"] != "craft.ticket-preview/v1"
            || manifest["format"] != "craft.ticket-manifest/v1"
            || blob.receipt.media_type != "application/vnd.craft.ticket-preview+json"
            || source.provider != "confluence"
            || source.resource_type != "page"
            || json!(summary) != value["ticket_preview"]
            || tasks.is_empty()
            || tasks.len() > 256
            || ids.len() != tasks.len()
            || flattened.len() != ids.len()
            || flattened.iter().copied().collect::<BTreeSet<_>>() != ids
            || manifest["context"]["binding"]["page"] != json!(source)
        {
            return Err(invalid("ticket_preview_is_inconsistent"));
        }
        Ok(Self {
            receipt: blob.receipt,
            review_id,
            manifest_digest: summary.manifest_digest,
            source,
            space_id,
            tasks,
            batches,
            links,
        })
    }
    fn task(&self, id: &str) -> Result<&ResolvedTask, CraftError> {
        self.tasks
            .iter()
            .find(|t| t.task_id == id)
            .ok_or_else(|| invalid("missing_manifest_task"))
    }
}
struct Run<'a> {
    args: ApplyArgs,
    profile: &'a str,
    request_id: &'a str,
    environment: BindingEnvironment<'a>,
    artifacts: &'a dyn ArtifactStore,
    journal: &'a NativeJournal,
    preview: Preview,
    run_digest: String,
    data: ExecutionData,
    reports: BTreeMap<String, StepReport>,
    reuse_descriptor_digest: Option<String>,
}
impl<'a> Run<'a> {
    fn environment(&self) -> BindingEnvironment<'a> {
        BindingEnvironment {
            store: self.environment.store,
            credentials: self.environment.credentials,
            client: self.environment.client,
            tls: self.environment.tls.clone(),
            control: self.environment.control.clone(),
        }
    }
    fn issue_spec(task: &ResolvedTask) -> Result<StepSpec, CraftError> {
        if let Some(existing) = &task.existing_issue {
            if task
                .existing_issue_observation
                .as_ref()
                .is_none_or(|observed| {
                    observed["id"] != existing.issue_id
                        || observed["project"]["id"] != task.target.id
                        || observed["issue_type"]["id"] != task.input["issue_type_id"]
                })
            {
                return Err(invalid("qualified_reuse_observation_required"));
            }
            return Ok(StepSpec {
                key: format!("issue:{}", task.task_id),
                kind: Kind::Reuse,
                target: task.target.clone(),
                logical_digest: digest(
                    &json!({"task_id":task.task_id,"profile":task.profile,"target":task.target,"input":task.input,"payload":task.payload,"metadata_digest":task.metadata_digest,"existing_issue":existing,"observation":task.existing_issue_observation}),
                )?,
                route: "reviewed-existing-issue".into(),
                expected_resources: vec![ResourceRef {
                    resource_type: "issue".into(),
                    id: existing.issue_id.clone(),
                    ..task.target.clone()
                }],
                origin_operation_id: Some(existing.origin_operation_id.clone()),
            });
        }
        Ok(StepSpec {
            key: format!("issue:{}", task.task_id),
            kind: Kind::Issue,
            target: task.target.clone(),
            logical_digest: digest(
                &json!({"task_id":task.task_id,"profile":task.profile,"target":task.target,"input":task.input,"payload":task.payload,"metadata_digest":task.metadata_digest}),
            )?,
            route: craft_provider_jira::created_issue::ROUTE.into(),
            expected_resources: vec![],
            origin_operation_id: None,
        })
    }
    fn remember(&mut self, spec: &StepSpec, observed: &Observed) {
        if matches!(spec.kind, Kind::Issue | Kind::Reuse) {
            let mut known = observed.report.resources.clone();
            if let Some(resources) = observed
                .envelope
                .as_ref()
                .and_then(|e| e.data.as_ref())
                .and_then(|d| d["receipt"]["confirmed_resources"].as_array())
            {
                for resource in resources {
                    if let Ok(resource) = serde_json::from_value::<ResourceRef>(resource.clone())
                        && !known.contains(&resource)
                    {
                        known.push(resource);
                    }
                }
            }
            if known.len() == 1
                && known[0].tenant_ref() == spec.target.tenant_ref()
                && known[0].resource_type == "issue"
                && numeric(&known[0].id)
                && (observed.report.state == RequestState::Confirmed || observed.envelope.is_some())
            {
                let task_id = spec.key.trim_start_matches("issue:").to_owned();
                if let Some(mapping) = self
                    .data
                    .known_issues
                    .iter_mut()
                    .find(|m| m.task_id == task_id)
                {
                    mapping.issue = known[0].clone();
                } else {
                    self.data.known_issues.push(IssueMapping {
                        task_id,
                        issue: known[0].clone(),
                    });
                    self.data
                        .known_issues
                        .sort_by(|a, b| a.task_id.cmp(&b.task_id));
                }
            }
        }
        if spec.kind == Kind::ResultsPage
            && observed.report.state == RequestState::Confirmed
            && observed.report.resources.len() == 1
            && observed.report.resources[0].resource_type == "page"
            && observed.report.resources[0].tenant_ref() == spec.target.tenant_ref()
            && numeric(&observed.report.resources[0].id)
        {
            self.data.result_page = Some(observed.report.resources[0].clone());
        }
        self.reports
            .insert(spec.key.clone(), observed.report.clone());
        self.data.links_confirmed = self
            .reports
            .values()
            .filter(|r| r.kind == Kind::Link && r.complete)
            .count() as u32;
    }
    fn remember_native_confirmation(&mut self, spec: &StepSpec, envelope: &Envelope) {
        let Some(data) = &envelope.data else { return };
        if data["receipt"]["operation_id"].as_str() != self.data.current_operation_id.as_deref() {
            return;
        }
        let Ok(resources) = serde_json::from_value::<Vec<ResourceRef>>(
            data["receipt"]["confirmed_resources"].clone(),
        ) else {
            return;
        };
        if resources.len() != 1
            || resources[0].tenant_ref() != spec.target.tenant_ref()
            || !numeric(&resources[0].id)
        {
            return;
        }
        if spec.kind == Kind::Issue && resources[0].resource_type == "issue" {
            let task_id = spec.key.trim_start_matches("issue:");
            if let Some(mapping) = self
                .data
                .known_issues
                .iter_mut()
                .find(|m| m.task_id == task_id)
            {
                mapping.issue = resources[0].clone();
            } else {
                self.data.known_issues.push(IssueMapping {
                    task_id: task_id.into(),
                    issue: resources[0].clone(),
                });
                self.data
                    .known_issues
                    .sort_by(|a, b| a.task_id.cmp(&b.task_id));
            }
        }
        if spec.kind == Kind::ResultsPage && resources[0].resource_type == "page" {
            self.data.result_page = Some(resources[0].clone());
        }
    }
    fn save_intent(
        &mut self,
        spec: &StepSpec,
        binding: &craft_core::storage::OperationBinding,
    ) -> Result<(), CraftError> {
        let result = ledger::prepare_intent(
            &self.run_digest,
            spec,
            binding,
            self.artifacts,
            &self.preview.source,
        );
        if let Err(error) = &result {
            self.data.artifact_write_state = error.details.write_state;
            self.data.artifact_error = Some(error.clone());
        }
        result
    }
    fn observe(&mut self, spec: &StepSpec) -> Result<Observed, CraftError> {
        self.data.current_operation_id = Some(ledger::operation_id(&self.run_digest, &spec.key)?);
        let observed = if spec.kind == Kind::Reuse {
            let descriptor = self.reuse_descriptor()?;
            reuse::inspect(
                &self.run_digest,
                spec,
                self.artifacts,
                &self.preview.source,
                &descriptor,
            )?
        } else {
            ledger::inspect(
                &self.run_digest,
                spec,
                self.artifacts,
                self.journal,
                &self.preview.source,
            )?
        };
        self.remember(spec, &observed);
        if let Some(evidence) = &observed.reuse {
            self.data
                .reused_issues
                .retain(|entry| entry.task_id != evidence.task_id);
            self.data.reused_issues.push(evidence.clone());
            self.data
                .reused_issues
                .sort_by(|a, b| a.task_id.cmp(&b.task_id));
            let current = self
                .journal
                .get(&evidence.origin.binding.operation_id)
                .map_err(local_error)?;
            if current.as_ref() != Some(&evidence.origin) {
                self.data.unresolved_record = current;
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "Original create evidence changed after reviewed reuse; retain the mapping and obtain explicit reconciliation.",
                ));
            }
        }
        Ok(observed)
    }
    fn refuse_pending(&mut self, observed: Observed) -> CraftError {
        self.data.current_operation_id = Some(observed.report.operation_id);
        self.data.unresolved_record = observed.record;
        self.data.current_attempt = observed.envelope;
        CraftError::new(
            if observed.report.state == RequestState::SentUnknown {
                ErrorCode::WriteOutcomeUnknown
            } else {
                ErrorCode::PartialApply
            },
            "A prior ticket step requires reconciliation; it was not replayed. Confirmed native identities remain in the mapping.",
        )
    }
    fn capture(&mut self, spec: &StepSpec, envelope: Envelope) -> Result<(), CraftError> {
        if let Some(cache) = envelope
            .data
            .as_ref()
            .and_then(|d| d.get("credential_cache_update"))
            .filter(|v| !v.is_null())
        {
            self.data.credential_cache_updates.push(
                serde_json::from_value(cache.clone())
                    .map_err(|_| invalid("invalid_native_cache_receipt"))?,
            );
        }
        self.data.current_attempt = Some(envelope.clone());
        self.remember_native_confirmation(spec, &envelope);
        let saved = ledger::save_outcome(
            &self.run_digest,
            spec,
            &envelope,
            self.artifacts,
            &self.preview.source,
        );
        if let Err(error) = &saved {
            self.data.artifact_write_state = error.details.write_state;
            self.data.artifact_error = Some(error.clone());
        }
        let observed = self.observe(spec)?;
        if !envelope.ok {
            self.data.unresolved_record = observed.record;
            return Err(envelope
                .error
                .unwrap_or_else(|| invalid("missing_native_failure")));
        }
        saved?;
        if !observed.report.complete {
            return Err(self.refuse_pending(observed));
        }
        self.data.current_attempt = None;
        Ok(())
    }
    fn mapping(&self, id: &str) -> Result<ResourceRef, CraftError> {
        self.data
            .known_issues
            .iter()
            .find(|m| m.task_id == id)
            .map(|m| m.issue.clone())
            .ok_or_else(|| invalid("confirmed_dependency_mapping_missing"))
    }
    fn link_spec(
        &self,
        link: &tickets::DependencyLink,
    ) -> Result<
        (
            StepSpec,
            craft_provider_jira::issue_links::write::LinkInput,
            String,
        ),
        CraftError,
    > {
        let outward = self.mapping(&link.outward_task_id)?;
        let inward = self.mapping(&link.inward_task_id)?;
        let task = self.preview.task(&link.outward_task_id)?;
        let inward_task = self.preview.task(&link.inward_task_id)?;
        let input = craft_provider_jira::issue_links::write::LinkInput {
            outward_issue_id: outward.id.clone(),
            inward_issue_id: inward.id.clone(),
            inward_project_id: inward_task.target.id.clone(),
            link_type: link.link_type.clone(),
        };
        Ok((
            StepSpec {
                key: format!("link:{}:{}", link.outward_task_id, link.inward_task_id),
                kind: Kind::Link,
                target: task.target.clone(),
                logical_digest: digest(
                    &json!({"link":link,"profile":task.profile,"input":input,"target":task.target}),
                )?,
                route: craft_provider_jira::issue_links::write::CREATE_ROUTE.into(),
                expected_resources: vec![outward, inward],
                origin_operation_id: None,
            },
            input,
            task.profile.clone(),
        ))
    }
    fn execute(&mut self) -> Result<(), CraftError> {
        let mut preflight_done = false;
        for batch_ids in self.preview.batches.clone() {
            self.environment.control.check()?;
            let mut pending = false;
            for id in &batch_ids {
                let spec = Self::issue_spec(self.preview.task(id)?)?;
                let observed = self.observe(&spec)?;
                if !observed.report.complete {
                    if !observed.ready() {
                        return Err(self.refuse_pending(observed));
                    }
                    pending = true;
                }
            }
            let links: Vec<_> = self
                .preview
                .links
                .iter()
                .filter(|l| batch_ids.contains(&l.dependent_task_id))
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()
                .map_err(|_| invalid("invalid_manifest_links"))?;
            for link in &links {
                let link: tickets::DependencyLink = serde_json::from_value(link.clone())
                    .map_err(|_| invalid("invalid_manifest_link"))?;
                if self
                    .data
                    .known_issues
                    .iter()
                    .any(|m| m.task_id == link.outward_task_id)
                    && self
                        .data
                        .known_issues
                        .iter()
                        .any(|m| m.task_id == link.inward_task_id)
                {
                    let (spec, _, _) = self.link_spec(&link)?;
                    let observed = self.observe(&spec)?;
                    if !observed.report.complete {
                        if !observed.ready() {
                            return Err(self.refuse_pending(observed));
                        }
                        pending = true;
                    }
                } else {
                    pending = true;
                }
            }
            if !pending {
                continue;
            }
            if !preflight_done {
                self.preflight_policies()?;
                preflight_done = true;
            }
            let verification = verification::revalidate_ticket_manifest(
                &self.preview.review_id,
                &self.args.preview_artifact,
                self.profile,
                self.request_id,
                self.environment(),
                self.artifacts,
            );
            self.data
                .credential_cache_updates
                .extend(verification.credential_cache_updates);
            if let Some(error) = verification.error {
                return Err(error);
            }
            let validated = verification
                .validated
                .ok_or_else(|| invalid("missing_live_ticket_validation"))?;
            if validated.manifest().summary()?.manifest_digest != self.preview.manifest_digest {
                return Err(invalid("revalidated_manifest_mismatch"));
            }
            for id in batch_ids {
                let task = validated
                    .manifest()
                    .context
                    .tasks
                    .iter()
                    .find(|t| t.task_id == id)
                    .ok_or_else(|| invalid("revalidated_task_missing"))?;
                let spec = Self::issue_spec(task)?;
                let observed = self.observe(&spec)?;
                if observed.report.complete {
                    continue;
                }
                if !observed.ready() {
                    return Err(self.refuse_pending(observed));
                }
                if task.existing_issue.is_some() {
                    self.reuse_issue(task, &spec, &validated)?;
                } else {
                    self.create_issue(task, &spec, &validated)?;
                }
            }
            for link in links {
                let link: tickets::DependencyLink =
                    serde_json::from_value(link).map_err(|_| invalid("invalid_manifest_link"))?;
                let (spec, input, profile) = self.link_spec(&link)?;
                let observed = self.observe(&spec)?;
                if observed.report.complete {
                    continue;
                }
                if !observed.ready() {
                    return Err(self.refuse_pending(observed));
                }
                self.create_link(&spec, input, &profile, &validated)?;
            }
        }
        self.data.resources_complete = true;
        self.publish_results()?;
        self.data.complete = true;
        Ok(())
    }
    fn progress(&mut self) -> Result<(), CraftError> {
        let name = format!(
            "ticket-progress-{}.json",
            &digest(&json!({"run":self.run_digest,"request":self.request_id}))?[7..]
        );
        self.data.progress_artifact_name = Some(name.clone());
        let mut progress = json!({"format":"craft.ticket-progress/v1","run_digest":self.run_digest,"run_intent":self.data.run_intent,"run_intent_name":self.data.run_intent_name,"steps":self.reports,"known_issues":self.data.known_issues,"result_page":self.data.result_page,"result_artifact":self.data.result_artifact,"result_artifact_name":self.data.result_artifact_name,"complete":self.data.complete,"stop_error":self.data.stop_error,"artifact_error":self.data.artifact_error,"artifact_write_state":self.data.artifact_write_state,"current_operation_id":self.data.current_operation_id,"current_attempt":self.data.current_attempt,"unresolved_record":self.data.unresolved_record,"credential_cache_updates":self.data.credential_cache_updates});
        if !self.data.reused_issues.is_empty() {
            progress["reused_issues"] = json!(self.data.reused_issues);
        }
        let receipt = ledger::ensure(
            self.artifacts,
            &name,
            &progress,
            "application/vnd.craft.ticket-progress+json",
            &self.preview.source,
        )?;
        self.data.progress_artifact = Some(receipt);
        Ok(())
    }
}
pub fn apply_native<'a>(
    args: ApplyArgs,
    resume: bool,
    profile: &'a str,
    request_id: &'a str,
    environment: BindingEnvironment<'a>,
    artifacts: &'a dyn ArtifactStore,
    journal: &'a NativeJournal,
) -> (ExecutionData, Option<CraftError>) {
    let mut data = empty(&args);
    let prepare = (|| {
        args.validate(
            &[],
            &InvocationOptions {
                profile: Some(profile),
                request_id,
                state_root: Some(environment.store.path()),
                max_bytes: OUTPUT,
            },
        )?;
        let preview = Preview::load(&args.preview_artifact, artifacts)?;
        let value = json!({"format":"craft.ticket-run/v1","profile":profile,"run_id":args.run_id,"review_id":preview.review_id,"preview":preview.receipt,"manifest_digest":preview.manifest_digest,"result_title":args.result_title,"consistency":args.consistency});
        let run_digest = digest(&value)?;
        data.run_digest = Some(run_digest.clone());
        let name = format!(
            "ticket-run-{}.json",
            &digest(&json!({"profile":profile,"run_id":args.run_id}))?[7..]
        );
        data.run_intent_name = Some(name.clone());
        let exists = artifacts.receipt(&name).map_err(local_error)?.is_some();
        if exists != resume {
            return Err(CraftError::new(
                ErrorCode::OperationIdReused,
                if resume {
                    "Ticket run intent is missing; resume creates no new run."
                } else {
                    "Ticket run already exists; inspect it and use tickets-resume with the same preview and result title."
                },
            ));
        }
        data.run_intent = Some(ledger::ensure(
            artifacts,
            &name,
            &value,
            "application/vnd.craft.ticket-run+json",
            &preview.source,
        )?);
        data.artifact_write_state = RequestState::Confirmed;
        Ok((preview, run_digest))
    })();
    let (preview, run_digest) = match prepare {
        Ok(v) => v,
        Err(error) => {
            if error.details.write_state == RequestState::SentUnknown {
                data.artifact_write_state = RequestState::SentUnknown;
                data.artifact_error = Some(error.clone());
            }
            return (data, Some(error));
        }
    };
    let mut run = Run {
        args,
        profile,
        request_id,
        environment,
        artifacts,
        journal,
        preview,
        run_digest,
        data,
        reports: BTreeMap::new(),
        reuse_descriptor_digest: None,
    };
    let mut error = run.execute().err();
    if let Some(failure) = &error {
        run.data.stop_error = Some(failure.clone());
        if let Some(id) = &run.data.current_operation_id
            && !run
                .reports
                .values()
                .any(|report| report.operation_id == *id && report.kind == Kind::Reuse)
            && let Ok(record) = journal.get(id)
        {
            run.data.unresolved_record = record;
        }
    }
    if let Err(failure) = run.progress() {
        run.data.complete = false;
        run.data.artifact_write_state = failure.details.write_state;
        run.data.artifact_error = Some(failure.clone());
        error.get_or_insert(failure);
    }
    if let Some(failure) = &mut error {
        if has_uncertainty(&run.data, &run.reports, failure) {
            failure.details.write_state = RequestState::SentUnknown;
        } else if !run.data.known_issues.is_empty()
            || run.data.result_page.is_some()
            || run.data.links_confirmed > 0
            || current_receipt_state(&run.data) == Some(RequestState::Confirmed)
            || run
                .reports
                .values()
                .any(|r| r.state == RequestState::Confirmed && !r.resources.is_empty())
        {
            failure.details.write_state = RequestState::Confirmed;
            failure.code = ErrorCode::PartialApply;
        } else if failure.details.write_state == RequestState::Failed
            || current_receipt_state(&run.data) == Some(RequestState::Failed)
            || run
                .reports
                .values()
                .any(|r| r.state == RequestState::Failed)
            || run.data.unresolved_record.as_ref().is_some_and(|r| {
                Some(&r.binding.operation_id) == run.data.current_operation_id.as_ref()
                    && r.steps
                        .values()
                        .any(|s| s.state == craft_core::storage::SubmissionState::Failed)
            })
        {
            failure.details.write_state = RequestState::Failed;
        } else {
            failure.details.write_state = RequestState::NotSent;
        }
        failure.next_action=match failure.details.write_state {
            RequestState::SentUnknown=>"Preserve this run, current_operation_id and progress artifact. Inspect operation get before human reconciliation; unknown native attempts are never replayed by tickets-resume.",
            RequestState::Failed=>"Inspect the refused step and progress artifact. The same native claim cannot replay; resolve it explicitly rather than generating a replacement run as a retry.",
            _=>"Preserve known_issues and the progress artifact. Resume with the same run inputs only after resolving the reported cause; never-claimed work and result publication may continue, but changed source/approvals/material need human reconciliation and a new review.",
        }.into();
    }
    (run.data, error)
}
