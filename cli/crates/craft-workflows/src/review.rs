//! Review authority comes from host policy and current observations, never approval-looking labels.
use crate::documents::Document;
use crate::plans::{DesignReference, Plan};
use craft_core::contracts::{ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, parse_object};
use craft_core::pagination::valid_digest;
use craft_core::storage::Clock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MAX_COMMENTS: usize = 1024;
const MAX_BODY_BYTES: usize = 65_536;
const MAX_INVENTORY_BYTES: usize = 8_388_608;
const MAX_AGE_MS: u64 = 30_000;
const FORMAT: &str = "craft.review/v1";
fn invalid(path: &str, reason: &str) -> CraftError {
    CraftError::field(path, reason)
}
fn refused(reason: &str) -> CraftError {
    CraftError::new(ErrorCode::PolicyDenied, reason)
}
fn changed() -> CraftError {
    CraftError::new(
        ErrorCode::VersionConflict,
        "Review source, policy or comment evidence changed; read and review the current revision.",
    )
}
fn digest(v: &impl Serialize) -> Result<String, CraftError> {
    canonical_digest(
        &serde_json::to_value(v)
            .map_err(|_| invalid("/review", "cannot_encode_review_evidence"))?,
    )
}
fn numeric(id: &str) -> bool {
    id.parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == id)
}
fn resource(r: &ResourceRef, provider: &str, kind: &str) -> Result<(), CraftError> {
    r.validate()?;
    let cloud = &r.tenant;
    let uuid = cloud.len() == 36
        && cloud.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        });
    if r.provider != provider
        || r.deployment != "cloud"
        || r.resource_type != kind
        || !numeric(&r.id)
        || !uuid
    {
        return Err(invalid("/review/target", "expected_exact_cloud_resource"));
    }
    Ok(())
}
fn identity(id: &str) -> Result<(), CraftError> {
    validate_identity("/review/actor", id)?;
    if matches!(id, "unknown" | "anonymous") {
        return Err(invalid("/review/actor", "unestablished_actor_identity"));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    Storage,
    Adf,
}
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewBinding {
    pub plan_id: String,
    pub page: ResourceRef,
    pub space: ResourceRef,
    pub version: u32,
    pub representation: Representation,
    pub page_digest: String,
    pub document_digest: String,
    pub mapping_digest: String,
    pub jira_targets: Vec<ResourceRef>,
    pub designs: Vec<DesignReference>,
}
impl ReviewBinding {
    /// The host must derive page/design observations and field mappings from verified live reads.
    pub fn from_plan(
        plan: &Plan,
        page: ResourceRef,
        version: u32,
        representation: Representation,
        page_digest: String,
        projects: BTreeMap<String, ResourceRef>,
        fields_digest: String,
    ) -> Result<Self, CraftError> {
        plan.validate()?;
        let required: BTreeSet<_> = plan
            .tasks
            .iter()
            .filter(|t| t.include_in_jira)
            .map(|t| t.jira.project_key.clone())
            .collect();
        if required.is_empty()
            || required != projects.keys().cloned().collect()
            || !valid_digest(&fields_digest)
        {
            return Err(invalid(
                "/review/mapping",
                "complete_included_project_and_field_mapping_required",
            ));
        }
        let mut targets = BTreeMap::new();
        for target in projects.values() {
            resource(target, "jira", "project")?;
            if target.tenant != plan.target.jira_cloud_id {
                return Err(invalid("/review/mapping", "jira_cloud_mismatch"));
            }
            targets.insert(digest(target)?, target.clone());
        }
        let mut designs = plan.designs.clone();
        designs.sort_by(|a, b| a.design_id.cmp(&b.design_id));
        let binding = Self {
            plan_id: plan.plan_id.clone(),
            space: ResourceRef {
                provider: "confluence".into(),
                deployment: "cloud".into(),
                tenant: plan.target.confluence.confluence_cloud_id.clone(),
                resource_type: "space".into(),
                id: plan.target.confluence.space_id.clone(),
            },
            page,
            version,
            representation,
            page_digest,
            document_digest: plan.digest()?,
            mapping_digest: digest(&json!({"projects":projects,"fields_digest":fields_digest}))?,
            jira_targets: targets.into_values().collect(),
            designs,
        };
        binding.validate()?;
        Ok(binding)
    }
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_identity("/plan_id", &self.plan_id)?;
        if self.plan_id.len() > 64
            || !self
                .plan_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(invalid("/plan_id", "invalid_plan_identity"));
        }
        resource(&self.page, "confluence", "page")?;
        resource(&self.space, "confluence", "space")?;
        if self.page.tenant != self.space.tenant
            || self.version == 0
            || self.version > i32::MAX as u32
            || [
                &self.page_digest,
                &self.document_digest,
                &self.mapping_digest,
            ]
            .iter()
            .any(|d| !valid_digest(d))
            || self.jira_targets.is_empty()
            || self.jira_targets.len() > 256
            || self.designs.len() > 64
        {
            return Err(invalid("/review/binding", "invalid_exact_review_binding"));
        }
        let mut seen = BTreeSet::new();
        for target in &self.jira_targets {
            resource(target, "jira", "project")?;
            if !seen.insert(digest(target)?) {
                return Err(invalid("/review/mapping", "duplicate_jira_target"));
            }
        }
        let mut previous = None;
        for d in &self.designs {
            validate_identity("/design_id", &d.design_id)?;
            resource(
                &ResourceRef {
                    provider: "confluence".into(),
                    deployment: "cloud".into(),
                    tenant: d.cloud_id.clone(),
                    resource_type: "page".into(),
                    id: d.page_id.clone(),
                },
                "confluence",
                "page",
            )?;
            if d.version == 0
                || d.version > i32::MAX as u64
                || !valid_digest(&d.digest)
                || previous.is_some_and(|p: &str| p >= d.design_id.as_str())
            {
                return Err(invalid(
                    "/review/designs",
                    "invalid_or_noncanonical_design_revisions",
                ));
            }
            previous = Some(d.design_id.as_str());
        }
        Ok(())
    }
}

/// Construct only from operator-managed configuration. Native account types do not prove humanity.
#[derive(Clone, Serialize)]
pub struct TrustedReviewerPolicy {
    revision: u64,
    space: ResourceRef,
    service_actor: String,
    reviewers: BTreeSet<String>,
    excluded: BTreeSet<String>,
    quorum: usize,
    enabled: bool,
}
impl TrustedReviewerPolicy {
    pub fn from_host_configuration(
        revision: u64,
        space: ResourceRef,
        service_actor: String,
        reviewers: Vec<String>,
        excluded: Vec<String>,
        quorum: usize,
        enabled: bool,
    ) -> Result<Self, CraftError> {
        resource(&space, "confluence", "space")?;
        identity(&service_actor)?;
        if revision == 0
            || revision > i64::MAX as u64
            || reviewers.is_empty()
            || reviewers.len() > 64
            || excluded.len() > 64
            || quorum == 0
            || quorum > reviewers.len()
        {
            return Err(invalid("/review/policy", "invalid_reviewer_policy_bounds"));
        }
        for actor in reviewers.iter().chain(&excluded) {
            identity(actor)?;
        }
        let eligible: BTreeSet<_> = reviewers.iter().cloned().collect();
        let denied: BTreeSet<_> = excluded.iter().cloned().collect();
        if eligible.len() != reviewers.len()
            || denied.len() != excluded.len()
            || eligible.contains(&service_actor)
            || !eligible.is_disjoint(&denied)
        {
            return Err(invalid(
                "/review/policy",
                "reviewers_must_be_unique_and_exclude_worker_identities",
            ));
        }
        Ok(Self {
            revision,
            space,
            service_actor,
            reviewers: eligible,
            excluded: denied,
            quorum,
            enabled,
        })
    }
    fn eligible(&self, id: &str) -> bool {
        id != self.service_actor && self.reviewers.contains(id) && !self.excluded.contains(id)
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    ChangesRequested,
    Withdraw,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewStatement {
    pub format: String,
    pub decision: Decision,
    pub plan_id: String,
    pub page: ResourceRef,
    pub binding_digest: String,
}
impl ReviewStatement {
    pub fn new(decision: Decision, binding: ReviewBinding) -> Result<Self, CraftError> {
        binding.validate()?;
        Ok(Self {
            format: FORMAT.into(),
            decision,
            plan_id: binding.plan_id.clone(),
            page: binding.page.clone(),
            binding_digest: digest(&binding)?,
        })
    }
}
#[derive(Clone, Serialize)]
pub enum CommentBody {
    Storage(String),
    Adf(Value),
}
#[derive(Clone, Serialize)]
pub struct CommentObservation {
    pub id: String,
    pub page: ResourceRef,
    pub status: String,
    pub version: u32,
    pub version_author_id: String,
    pub version_created_at: String,
    pub body: CommentBody,
}
#[derive(Clone, Copy)]
pub enum Coverage {
    Complete,
    Partial,
    Unknown,
}
pub struct CommentInventory {
    pub coverage: Coverage,
    pub comments: Vec<CommentObservation>,
}
/// Implementations own authenticated reads, complete root/reply traversal and retained cache receipts.
/// Caller-provided evidence JSON is not an implementation of this port.
pub trait ReviewEvidencePort {
    /// Return the authenticated Confluence execution identity, never a statement's claimed actor.
    fn execution_actor(&mut self) -> Result<String, CraftError>;
    /// Load operator-managed configuration bound to the authenticated execution identity.
    fn policy(&mut self) -> Result<TrustedReviewerPolicy, CraftError>;
    /// Re-read the visible plan, all design revisions and complete resolved Jira field/target mapping.
    fn binding(&mut self) -> Result<ReviewBinding, CraftError>;
    /// Complete means every current root and reply is observed without unreadable gaps or cursors.
    /// Native adapters must enforce invocation budgets and retain every credential-cache receipt.
    fn comments(&mut self) -> Result<CommentInventory, CraftError>;
}
#[derive(Clone, Serialize)]
pub struct ApprovedComment {
    pub id: String,
    pub version: u32,
    pub actor: String,
    pub digest: String,
}
/// This result cannot be deserialized from a stored receipt or caller-supplied JSON.
///
/// ```compile_fail
/// let _: craft_workflows::review::VerifiedReview = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Serialize)]
pub struct VerifiedReview {
    binding: ReviewBinding,
    policy_digest: String,
    approvals: Vec<ApprovedComment>,
}
impl VerifiedReview {
    pub fn binding(&self) -> &ReviewBinding {
        &self.binding
    }
    pub fn approvals(&self) -> &[ApprovedComment] {
        &self.approvals
    }
}

fn body_value(body: &CommentBody) -> Result<Value, CraftError> {
    let encoded =
        serde_json::to_vec(body).map_err(|_| invalid("/review/comment", "invalid_comment_body"))?;
    if encoded.len() > MAX_BODY_BYTES {
        return Err(invalid(
            "/review/comment",
            "review_comment_body_limit_exceeded",
        ));
    }
    let document = match body {
        CommentBody::Storage(s) => Document::from_storage(s),
        CommentBody::Adf(v) => Document::from_adf(
            &serde_json::to_vec(v).map_err(|_| invalid("/review/comment", "invalid_adf"))?,
        ),
    }
    .map_err(|e| invalid("/review/comment", e.reason))?;
    document
        .to_adf()
        .map_err(|e| invalid("/review/comment", e.reason))
}
fn statement(body: &CommentBody) -> Result<ReviewStatement, CraftError> {
    let adf = body_value(body)?;
    let blocks = adf["content"]
        .as_array()
        .ok_or_else(|| invalid("/review/comment", "visible_review_statement_required"))?;
    if blocks.len() != 1 || !matches!(blocks[0]["type"].as_str(), Some("paragraph" | "codeBlock")) {
        return Err(refused(
            "An eligible reviewer's comment has unresolved or ambiguous visible content.",
        ));
    }
    let nodes = blocks[0]["content"]
        .as_array()
        .ok_or_else(|| invalid("/review/comment", "visible_review_statement_required"))?;
    let mut text = String::new();
    for node in nodes {
        if node["type"] != "text" || node.get("marks").is_some() {
            return Err(refused(
                "An eligible reviewer's statement must be unambiguous visible JSON.",
            ));
        }
        text.push_str(
            node["text"]
                .as_str()
                .ok_or_else(|| invalid("/review/comment", "visible_review_statement_required"))?,
        );
    }
    let parsed = parse_object(text.trim().as_bytes())
        .map_err(|_| refused("An eligible reviewer's unstructured comment remains unresolved."))?;
    let statement: ReviewStatement = serde_json::from_value(parsed)
        .map_err(|_| refused("Review statement has unknown, missing or ambiguous fields."))?;
    if statement.format != FORMAT {
        return Err(refused("Review statement format is unsupported."));
    }
    resource(&statement.page, "confluence", "page")?;
    validate_identity("/review/plan_id", &statement.plan_id)?;
    if statement.plan_id.len() > 64 || !valid_digest(&statement.binding_digest) {
        return Err(invalid(
            "/review/statement",
            "invalid_review_binding_identity",
        ));
    }
    Ok(statement)
}
fn inventory_digest(
    inventory: &CommentInventory,
    binding: &ReviewBinding,
) -> Result<String, CraftError> {
    if !matches!(inventory.coverage, Coverage::Complete) || inventory.comments.len() > MAX_COMMENTS
    {
        return Err(refused(
            "Complete bounded root and reply coverage is required for review.",
        ));
    }
    let mut rows = BTreeMap::new();
    let mut bytes = 0usize;
    for comment in &inventory.comments {
        if !numeric(&comment.id)
            || comment.status != "current"
            || comment.page != binding.page
            || comment.version == 0
            || comment.version > i32::MAX as u32
            || comment.version_created_at.is_empty()
            || comment.version_created_at.len() > 128
            || comment.version_created_at.chars().any(char::is_control)
        {
            return Err(refused(
                "Comment identity, association or current-version metadata is unestablished.",
            ));
        }
        identity(&comment.version_author_id)?;
        let encoded = serde_json::to_vec(&comment.body)
            .map_err(|_| invalid("/review/comment", "invalid_body"))?;
        bytes = bytes.saturating_add(encoded.len());
        if encoded.len() > MAX_BODY_BYTES
            || bytes > MAX_INVENTORY_BYTES
            || rows.insert(comment.id.clone(), digest(comment)?).is_some()
        {
            return Err(refused(
                "Comment inventory is duplicated or exceeds its bound.",
            ));
        }
    }
    digest(&rows)
}
fn evaluate(
    policy: &TrustedReviewerPolicy,
    binding: &ReviewBinding,
    inventory: &CommentInventory,
) -> Result<Vec<ApprovedComment>, CraftError> {
    if !policy.enabled || policy.space != binding.space {
        return Err(refused(
            "Trusted reviewer policy does not enable this exact plan space.",
        ));
    }
    let expected = digest(binding)?;
    let mut approvals = BTreeMap::new();
    let mut withdrawn = BTreeSet::new();
    for comment in &inventory.comments {
        if !policy.eligible(&comment.version_author_id) {
            continue;
        }
        let statement = statement(&comment.body)?;
        if statement.page != binding.page || statement.plan_id != binding.plan_id {
            return Err(refused("Review statement names a different plan or page."));
        }
        match statement.decision {
            Decision::ChangesRequested => {
                return Err(refused(
                    "An eligible reviewer has an unresolved changes request.",
                ));
            }
            Decision::Withdraw => {
                withdrawn.insert(comment.version_author_id.clone());
            }
            Decision::Approve if statement.binding_digest == expected => {
                approvals.insert(
                    (comment.version_author_id.clone(), comment.id.clone()),
                    ApprovedComment {
                        id: comment.id.clone(),
                        version: comment.version,
                        actor: comment.version_author_id.clone(),
                        digest: digest(comment)?,
                    },
                );
            }
            Decision::Approve => {}
        }
    }
    approvals.retain(|(actor, _), _| !withdrawn.contains(actor));
    let distinct: BTreeSet<_> = approvals.keys().map(|(actor, _)| actor).collect();
    if distinct.len() < policy.quorum {
        return Err(refused(
            "Distinct eligible current approvals do not meet the configured quorum.",
        ));
    }
    Ok(approvals.into_values().collect())
}
/// Repeated observations bound races; they do not create an atomic lock across cloud systems.
pub fn verify(
    port: &mut dyn ReviewEvidencePort,
    clock: &dyn Clock,
    control: &InvocationControl,
) -> Result<VerifiedReview, CraftError> {
    control.check()?;
    let wall = clock.unix_millis();
    let monotonic = clock.monotonic_millis();
    let policy = port.policy()?;
    if !policy.enabled {
        return Err(refused("Trusted reviewer policy is disabled."));
    }
    let actor = port.execution_actor()?;
    identity(&actor)?;
    if actor != policy.service_actor {
        return Err(refused(
            "Authenticated execution actor differs from the trusted reviewer policy.",
        ));
    }
    control.check()?;
    let binding = port.binding()?;
    binding.validate()?;
    control.check()?;
    let first = port.comments()?;
    let comments_digest = inventory_digest(&first, &binding)?;
    let approvals = evaluate(&policy, &binding, &first)?;
    control.check()?;
    if digest(&port.binding()?)? != digest(&binding)? {
        return Err(changed());
    }
    control.check()?;
    let second = port.comments()?;
    if inventory_digest(&second, &binding)? != comments_digest {
        return Err(changed());
    }
    control.check()?;
    if digest(&port.policy()?)? != digest(&policy)?
        || digest(&port.binding()?)? != digest(&binding)?
        || port.execution_actor()? != actor
    {
        return Err(changed());
    }
    control.check()?;
    if clock
        .unix_millis()
        .checked_sub(wall)
        .is_none_or(|age| age >= MAX_AGE_MS)
        || clock
            .monotonic_millis()
            .checked_sub(monotonic)
            .is_none_or(|age| age >= MAX_AGE_MS)
    {
        return Err(changed());
    }
    Ok(VerifiedReview {
        binding,
        policy_digest: digest(&policy)?,
        approvals,
    })
}
pub fn revalidate(
    expected: &VerifiedReview,
    port: &mut dyn ReviewEvidencePort,
    clock: &dyn Clock,
    control: &InvocationControl,
) -> Result<(), CraftError> {
    let current = verify(port, clock, control)?;
    if current.policy_digest != expected.policy_digest
        || digest(&current.binding)? != digest(&expected.binding)?
        || expected.approvals.iter().any(|prior| {
            !current.approvals.iter().any(|now| {
                now.id == prior.id
                    && now.version == prior.version
                    && now.actor == prior.actor
                    && now.digest == prior.digest
            })
        })
    {
        return Err(changed());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
