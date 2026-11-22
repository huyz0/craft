use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkOrder {
    pub schema_version: u32,
    pub id: String,
    pub issue: String,
    pub owner: String,
    pub repository: PathBuf,
    pub base: String,
    pub intent: String,
    pub acceptance: Vec<String>,
    pub scope: Vec<String>,
    pub context: Vec<PinnedReference>,
    pub skill: PinnedReference,
    #[serde(default)]
    pub require_native_checks: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_intake_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_failure: Option<CiFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_policy_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_anchor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_profile: Option<ExecutionProfile>,
    #[serde(default, skip_serializing_if = "AgentAdapter::is_codex")]
    pub agent_adapter: AgentAdapter,
}

pub use craft_factory_evidence::ci::CiFailure;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentAdapter {
    #[default]
    Codex,
    Claude,
}

impl AgentAdapter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub fn is_codex(&self) -> bool {
        *self == Self::Codex
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProfile {
    pub id: String,
    pub model: String,
    pub reasoning_effort: ReasoningEffort,
    pub timeout_seconds: u64,
    pub max_attempts: u64,
    pub approval_reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

impl ExecutionProfile {
    pub(crate) fn supports_adapter(&self, adapter: AgentAdapter) -> bool {
        adapter.is_codex() || self.reasoning_effort != ReasoningEffort::Ultra
    }

    pub fn valid(&self) -> bool {
        [&self.id, &self.model].into_iter().all(|name| {
            name.len() <= 100
                && name
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }) && (1..=3600).contains(&self.timeout_seconds)
            && (1..=12).contains(&self.max_attempts)
            && !self.approval_reference.trim().is_empty()
            && self.approval_reference.len() <= 500
            && !self.approval_reference.chars().any(char::is_control)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedReference {
    pub location: String,
    pub revision: String,
}

pub use craft_factory_evidence::valid_oid;

pub(crate) fn valid_scope(path: &str) -> bool {
    !path.contains(['\\', ':', '\0', '\n', '\r'])
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | "..") && !part.eq_ignore_ascii_case(".git"))
}

impl WorkOrder {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_fields(true)
    }

    pub(crate) fn validate_shared(&self) -> Result<(), &'static str> {
        self.validate_fields(false)
    }

    fn validate_fields(&self, require_local_checkout: bool) -> Result<(), &'static str> {
        if self.ci_anchor.as_deref().is_some_and(|anchor| {
            self.ci_policy_digest.is_none() || anchor.len() != self.base.len() || !valid_oid(anchor)
        }) {
            return Err("a CI branch anchor needs a pinned CI policy and a full commit");
        }
        if self.ci_policy_digest.as_deref().is_some_and(|pin| {
            self.ci_failure.is_none()
                || !pin
                    .strip_prefix("sha256:")
                    .is_some_and(|hex| hex.len() == 64 && valid_oid(hex))
        }) {
            return Err(
                "a CI policy pin requires a recorded failure and an approved SHA-256 digest",
            );
        }
        if self
            .execution_profile
            .as_ref()
            .is_some_and(|profile| !profile.supports_adapter(self.agent_adapter))
        {
            return Err("Claude execution profiles do not support the ultra effort");
        }
        if self.dependency_intake_digest.as_deref().is_some_and(|pin| {
            self.ci_failure.is_some()
                || !self.require_native_checks
                || !pin
                    .strip_prefix("sha256:")
                    .is_some_and(|hex| hex.len() == 64 && valid_oid(hex))
        }) {
            return Err(
                "a dependency intake pin requires native checks and an approved SHA-256 digest",
            );
        }
        if self.schema_version != 1 {
            return Err("unsupported work order version");
        }
        if self.id.is_empty()
            || self.id.len() > 100
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(
                "work order ID must contain only ASCII letters, digits, hyphens or underscores",
            );
        }
        if self.issue.trim().is_empty()
            || self.owner.trim().is_empty()
            || self.intent.trim().is_empty()
        {
            return Err("issue, owner and intent are required");
        }
        if (require_local_checkout && !self.repository.is_absolute()) || !valid_oid(&self.base) {
            if !require_local_checkout {
                return Err("a full lowercase base commit ID is required");
            }
            return Err(
                "an absolute repository path and a full lowercase base commit ID are required",
            );
        }
        if self.acceptance.is_empty() || self.acceptance.iter().any(|item| item.trim().is_empty()) {
            return Err("nonempty acceptance criteria are required");
        }
        if self.scope.is_empty() || self.scope.iter().any(|path| !valid_scope(path)) {
            return Err("scope must list repository-relative files or directory prefixes");
        }
        if self.context.is_empty()
            || self.context.iter().chain([&self.skill]).any(|reference| {
                reference.location.trim().is_empty() || reference.revision.trim().is_empty()
            })
        {
            return Err("context and skill must have explicit locations and pinned revisions");
        }
        if self
            .ci_failure
            .as_ref()
            .is_some_and(|failure| failure.commit() != self.base || !self.require_native_checks)
        {
            return Err("CI repair must use the recorded failure commit and require native checks");
        }
        if self
            .execution_profile
            .as_ref()
            .is_some_and(|profile| !profile.valid())
        {
            return Err(
                "execution profile requires valid identifiers, budgets and an approval reference",
            );
        }
        Ok(())
    }

    pub fn allows(&self, path: &str) -> bool {
        valid_scope(path)
            && self.scope.iter().any(|allowed| {
                path == allowed
                    || path
                        .strip_prefix(allowed)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
    }
}
