//! Live source and native create preparation for human review. No remote resource write occurs here.
use craft_core::contracts::{CredentialCacheUpdate, GrantRef, RequestState, ResourceRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::canonical_digest;
use craft_core::profiles::validate_profile_name;
use craft_provider_jira::create_metadata::CreateInput;
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::confluence_read_policy::{
    ConfluenceReadOperation as CfOperation, ConfluenceReadPolicyRecord,
};
use craft_runtime::create_policy::CreatePolicyRecord;
use craft_runtime::profiles::ProfileStore;
use craft_runtime::review_policy::ReviewPolicyRecord;
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use craft_workflows::plans::{Design, Plan};
use craft_workflows::review::{Decision, Representation, ReviewBinding, ReviewStatement};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub mod application;
pub mod command;
mod documents;
mod metadata;
#[cfg(test)]
mod tests;
pub mod tickets;
pub mod verification;

pub(crate) fn invalid(reason: &str) -> CraftError {
    CraftError::field("/review", reason)
}
pub(crate) fn denied(message: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, message)
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String, CraftError> {
    canonical_digest(
        &serde_json::to_value(value).map_err(|_| invalid("cannot_encode_review_binding"))?,
    )
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn numeric(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == value)
}
fn cloud_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
const SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
fn parse_task(value: &str) -> Result<TaskProposal, String> {
    craft_core::input::parse_object(value.as_bytes())
        .and_then(|v| serde_json::from_value(v).map_err(|_| invalid("invalid_task_proposal")))
        .map_err(|_| "expected a strict JSON task proposal".into())
}
fn parse_design_profiles(value: &str) -> Result<BTreeMap<String, String>, String> {
    craft_core::input::parse_object(value.as_bytes())
        .and_then(|v| serde_json::from_value(v).map_err(|_| invalid("invalid_design_profiles")))
        .map_err(|_| "expected a JSON object of cloud IDs to profile names".into())
}
fn encoded_size(value: &impl Serialize) -> Result<usize, CraftError> {
    Ok(serde_json::to_vec(value)
        .map_err(|_| invalid("cannot_encode_review_snapshot"))?
        .len())
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskProposal {
    pub task_id: String,
    pub profile: String,
    pub project_id: String,
    pub input: CreateInput,
}
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum DependencyDirection {
    Inward,
    Outward,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct DependencyType {
    pub link_type: craft_provider_jira::issue_links::LinkType,
    pub dependent_direction: DependencyDirection,
}
#[derive(clap::Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingRequest {
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub page_id: String,
    #[arg(long, default_value = "")]
    #[schemars(regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub space_id: String,
    #[arg(long,default_value="{}",value_parser=parse_design_profiles)]
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub design_profiles: BTreeMap<String, String>,
    #[arg(long="task",value_parser=parse_task)]
    #[schemars(length(min = 1, max = 256))]
    pub tasks: Vec<TaskProposal>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_link_type_id: Option<String>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_link_direction: Option<DependencyDirection>,
}
impl BindingRequest {
    pub fn validate(&self) -> Result<(), CraftError> {
        craft_core::input::parse_object(
            &serde_json::to_vec(self).map_err(|_| invalid("cannot_encode_review_request"))?,
        )?;
        if !numeric(&self.page_id)
            || !numeric(&self.space_id)
            || self.tasks.is_empty()
            || self.tasks.len() > 256
            || self.design_profiles.len() > 64
        {
            return Err(invalid("invalid_source_or_proposal_bounds"));
        }
        if self
            .dependency_link_type_id
            .as_ref()
            .is_some_and(|id| !numeric(id))
        {
            return Err(invalid("expected_numeric_dependency_link_type"));
        }
        if self.dependency_link_type_id.is_some() != self.dependency_link_direction.is_some() {
            return Err(invalid(
                "dependency_type_and_dependent_direction_required_together",
            ));
        }
        let mut ids = BTreeSet::new();
        for task in &self.tasks {
            if !identifier(&task.task_id)
                || !ids.insert(&task.task_id)
                || !numeric(&task.project_id)
                || !numeric(&task.input.issue_type_id)
            {
                return Err(invalid("invalid_or_duplicate_task_proposal"));
            }
            validate_profile_name(&task.profile)?;
            task.input.validate_basic()?;
        }
        for (cloud, profile) in &self.design_profiles {
            validate_profile_name(profile)?;
            if !cloud_id(cloud) {
                return Err(invalid("invalid_design_site_identity"));
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSnapshot<T> {
    pub source: crate::planning_read::DocumentSource,
    pub body_storage: String,
    pub document: T,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedTask {
    pub task_id: String,
    pub profile: String,
    pub target: ResourceRef,
    pub input: Value,
    pub payload: Value,
    pub metadata: Value,
    pub metadata_digest: String,
    pub dependency_link_type: Option<DependencyType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_issue: Option<craft_workflows::plans::ExistingIssue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_issue_observation: Option<Value>,
}
#[derive(Serialize)]
pub struct BindingSnapshot {
    pub binding: ReviewBinding,
    pub statement: ReviewStatement,
    pub plan: DocumentSnapshot<Plan>,
    pub designs: Vec<DocumentSnapshot<Design>>,
    pub tasks: Vec<ResolvedTask>,
    pub reviewer_policy_digest: String,
}

/// Implementations can return only native, grant-locked credential sessions; raw tokens are not accepted.
pub trait CredentialLocker {
    fn lock<'a>(
        &'a self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<LockedCredentials<'a>, CraftError>;
}
pub struct BindingEnvironment<'a> {
    pub store: &'a ProfileStore,
    pub credentials: &'a dyn CredentialLocker,
    pub client: &'a ServiceOAuthClient,
    pub tls: TlsPolicy,
    pub control: InvocationControl,
}
impl CredentialLocker for NativeCredentials {
    fn lock<'a>(
        &'a self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<LockedCredentials<'a>, CraftError> {
        self.lock(grant, control)
    }
}
#[cfg(feature = "synthetic-acceptance")]
impl CredentialLocker for craft_runtime::synthetic_acceptance::ServiceFixture {
    fn lock<'a>(
        &'a self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<LockedCredentials<'a>, CraftError> {
        self.lock(grant, control)
    }
}

pub struct LiveBindingHost<'a> {
    store: &'a ProfileStore,
    credentials: &'a dyn CredentialLocker,
    client: &'a ServiceOAuthClient,
    tls: TlsPolicy,
    control: InvocationControl,
    request_id: String,
    profile: String,
    request: BindingRequest,
    review_policy: ReviewPolicyRecord,
    cf_policies: BTreeMap<String, ConfluenceReadPolicyRecord>,
    jira_policies: BTreeMap<String, CreatePolicyRecord>,
    receipts: Vec<CredentialCacheUpdate>,
    opens: usize,
}
impl<'a> LiveBindingHost<'a> {
    pub fn new(
        environment: BindingEnvironment<'a>,
        profile: &str,
        request_id: &str,
        request: BindingRequest,
    ) -> Result<Self, CraftError> {
        let BindingEnvironment {
            store,
            credentials,
            client,
            tls,
            control,
        } = environment;
        request.validate()?;
        validate_profile_name(profile)?;
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        let review_policy = store
            .review_policy(profile, &request.space_id, &control)?
            .ok_or_else(|| {
                denied("No trusted reviewer policy is configured for this profile and space.")
            })?;
        store.verify_review_policy(&review_policy, &control)?;
        crate::review_policy::trusted_policy(store, &review_policy, &control)?;
        let mut host = Self {
            store,
            credentials,
            client,
            tls,
            control,
            request_id: request_id.into(),
            profile: profile.into(),
            request,
            review_policy,
            cf_policies: BTreeMap::new(),
            jira_policies: BTreeMap::new(),
            receipts: vec![],
            opens: 0,
        };
        let primary = host.capture_cf(profile)?;
        if primary.target.tenant != host.review_policy.target.tenant
            || primary.grant != host.review_policy.grant
            || primary.expected_actor != host.review_policy.expected_actor
        {
            return Err(denied(
                "Plan read authority differs from the trusted review actor, grant or site.",
            ));
        }
        let design_profiles: Vec<_> = host
            .request
            .design_profiles
            .iter()
            .map(|(c, p)| (c.clone(), p.clone()))
            .collect();
        for (cloud, profile) in design_profiles {
            if cloud == primary.target.tenant || host.capture_cf(&profile)?.target.tenant != cloud {
                return Err(invalid(
                    "design_profile_cloud_mismatch_or_redundant_override",
                ));
            }
        }
        let proposals: Vec<_> = host
            .request
            .tasks
            .iter()
            .map(|t| (t.profile.clone(), t.project_id.clone()))
            .collect();
        for (name, id) in proposals {
            let p = host.capture_jira(&name)?;
            if p.target.id != id {
                return Err(denied(
                    "Task project differs from its configured create-preparation policy.",
                ));
            }
        }
        host.check_all()?;
        Ok(host)
    }
    pub fn receipts(&self) -> &[CredentialCacheUpdate] {
        &self.receipts
    }
    pub fn request(&self) -> &BindingRequest {
        &self.request
    }
    fn reserve_open(&mut self) -> Result<(), CraftError> {
        self.control.check()?;
        if self.opens >= 1024 {
            return Err(invalid("native_binding_acquisition_limit"));
        }
        self.opens += 1;
        Ok(())
    }
    fn check_review(&self) -> Result<(), CraftError> {
        self.store
            .verify_review_policy(&self.review_policy, &self.control)
    }
    fn check_all(&self) -> Result<(), CraftError> {
        self.control.check()?;
        self.check_review()?;
        for p in self.cf_policies.values() {
            self.store
                .verify_confluence_read_policy(p, CfOperation::PageGet, &self.control)?;
        }
        for p in self.jira_policies.values() {
            self.store.verify_create_policy(p, &self.control)?;
        }
        self.control.check()
    }
    fn capture_cf(&mut self, profile: &str) -> Result<ConfluenceReadPolicyRecord, CraftError> {
        if let Some(p) = self.cf_policies.get(profile) {
            self.store
                .verify_confluence_read_policy(p, CfOperation::PageGet, &self.control)?;
            return Ok(p.clone());
        }
        let p = self
            .store
            .confluence_read_policy(profile, &self.control)?
            .ok_or_else(|| denied("A source profile has no trusted page-read policy."))?;
        self.store
            .verify_confluence_read_policy(&p, CfOperation::PageGet, &self.control)?;
        self.cf_policies.insert(profile.into(), p.clone());
        Ok(p)
    }
    fn capture_jira(&mut self, profile: &str) -> Result<CreatePolicyRecord, CraftError> {
        if let Some(p) = self.jira_policies.get(profile) {
            self.store.verify_create_policy(p, &self.control)?;
            return Ok(p.clone());
        }
        let p = self
            .store
            .create_policy(profile, &self.control)?
            .ok_or_else(|| denied("A task profile has no configured create-preparation policy."))?;
        self.store.verify_create_policy(&p, &self.control)?;
        self.jira_policies.insert(profile.into(), p.clone());
        Ok(p)
    }
    pub fn compile(&mut self) -> Result<BindingSnapshot, CraftError> {
        self.check_all()?;
        let profile = self.profile.clone();
        let page_id = self.request.page_id.clone();
        let plan = self.read_plan(&profile, &page_id)?;
        let mut remaining = SNAPSHOT_BYTES
            .checked_sub(encoded_size(&plan)?)
            .ok_or_else(|| invalid("review_snapshot_byte_limit"))?;
        if plan.source.space_id != self.request.space_id
            || plan.source.page.tenant != self.review_policy.target.tenant
        {
            return Err(invalid("plan_page_space_or_site_mismatch"));
        }
        let included: BTreeMap<_, _> = plan
            .document
            .tasks
            .iter()
            .filter(|t| t.include_in_jira)
            .map(|t| (t.task_id.as_str(), t))
            .collect();
        let has_dependencies = included.values().any(|task| !task.depends_on.is_empty());
        if has_dependencies != self.request.dependency_link_type_id.is_some() {
            return Err(invalid(
                "explicit_link_type_required_exactly_when_plan_has_dependencies",
            ));
        }
        if included.keys().copied().collect::<BTreeSet<_>>()
            != self
                .request
                .tasks
                .iter()
                .map(|t| t.task_id.as_str())
                .collect()
        {
            return Err(invalid("proposals_must_match_every_included_task_exactly"));
        }
        for t in &self.request.tasks {
            if included[&t.task_id.as_str()].title != t.input.summary
                || self.jira_policies[&t.profile].target.tenant
                    != plan.document.target.jira_cloud_id
            {
                return Err(invalid("task_summary_or_jira_site_differs_from_live_plan"));
            }
        }
        let mut designs = vec![];
        let mut used = BTreeSet::new();
        for reference in &plan.document.designs {
            let name = if reference.cloud_id == plan.source.page.tenant {
                profile.clone()
            } else {
                used.insert(reference.cloud_id.clone());
                self.request
                    .design_profiles
                    .get(&reference.cloud_id)
                    .cloned()
                    .ok_or_else(|| invalid("missing_design_site_profile"))?
            };
            let design = self.read_design(&name, &reference.page_id)?;
            if design.source.page.tenant != reference.cloud_id
                || u64::from(design.source.revision.version) != reference.version
                || design.document.design_id != reference.design_id
                || design.source.document_digest.as_ref() != Some(&reference.digest)
            {
                return Err(CraftError::new(
                    ErrorCode::VersionConflict,
                    "An included design no longer matches its named page, version or semantic digest.",
                ));
            }
            remaining = remaining
                .checked_sub(encoded_size(&design)?)
                .ok_or_else(|| invalid("review_snapshot_byte_limit"))?;
            designs.push(design);
        }
        if used != self.request.design_profiles.keys().cloned().collect() {
            return Err(invalid("unused_design_profile_override"));
        }
        let tasks = self.qualify_tasks(&plan.document, remaining)?;
        let mut projects = BTreeMap::new();
        let mut material = BTreeMap::new();
        for task in &tasks {
            let key = &included[&task.task_id.as_str()].jira.project_key;
            if projects
                .insert(key.clone(), task.target.clone())
                .is_some_and(|p| p != task.target)
            {
                return Err(invalid(
                    "one_project_key_resolved_to_multiple_native_projects",
                ));
            }
            let mut value = json!({"target":task.target,"payload":task.payload,"metadata_digest":task.metadata_digest,"dependency_link_type":task.dependency_link_type});
            if task.existing_issue.is_some() {
                value["existing_issue"] = json!(task.existing_issue);
                value["existing_issue_observation"] = json!(task.existing_issue_observation);
            }
            material.insert(task.task_id.clone(), value);
        }
        let binding = ReviewBinding::from_plan(
            &plan.document,
            plan.source.page.clone(),
            plan.source.revision.version,
            Representation::Storage,
            plan.source.revision.digest.clone(),
            projects,
            digest(&material)?,
        )?;
        let statement = ReviewStatement::new(Decision::Approve, binding.clone())?;
        let policy =
            crate::review_policy::trusted_policy(self.store, &self.review_policy, &self.control)?;
        self.check_all()?;
        let snapshot = BindingSnapshot {
            binding,
            statement,
            plan,
            designs,
            tasks,
            reviewer_policy_digest: digest(&policy)?,
        };
        if encoded_size(&snapshot)? > SNAPSHOT_BYTES {
            return Err(invalid("review_snapshot_byte_limit"));
        }
        Ok(snapshot)
    }
    fn record_auth_failure(
        &mut self,
        grant: GrantRef,
        failure: craft_runtime::auth::AuthFailure,
    ) -> CraftError {
        if let Some(r) = failure.credential_update {
            self.receipts.push(CredentialCacheUpdate {
                grant,
                write_state: RequestState::Confirmed,
                cleanup_pending: Some(r.cleanup_pending),
            });
        } else if let Some(state) = failure.credential_write_state {
            self.receipts.push(CredentialCacheUpdate {
                grant,
                write_state: state,
                cleanup_pending: None,
            });
        }
        let mut error = failure.error;
        error.details.write_state = RequestState::NotSent;
        error
    }
}
