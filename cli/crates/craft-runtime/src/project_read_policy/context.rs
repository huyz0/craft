//! Explicit per-project permissions share identity binding, not the legacy listing enablement.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProjectContextOperation {
    #[serde(rename = "jira.project.get")]
    Get,
    #[serde(rename = "jira.project.statuses")]
    Statuses,
}
impl ProjectContextOperation {
    pub fn command(self) -> &'static str {
        match self {
            Self::Get => "jira.project.get",
            Self::Statuses => "jira.project.statuses",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextPermission {
    pub target: ResourceRef,
    pub operations: Vec<ProjectContextOperation>,
    pub enabled: bool,
}
pub struct ProjectContextPermissionUpdate {
    pub target: ResourceRef,
    pub operations: Vec<ProjectContextOperation>,
    pub enabled: bool,
    pub expected_revision: u64,
}
fn validate_target(profile: &Profile, target: &ResourceRef) -> Result<(), CraftError> {
    profile.validate_target("project", target)?;
    crate::transport::product_identity_url(&target.tenant_ref())?;
    let id = &target.id;
    let numeric = !id.is_empty() && id.len() <= 512 && id.bytes().all(|byte| byte.is_ascii_digit());
    let key = !id.is_empty()
        && id.len() <= 512
        && id.as_bytes()[0].is_ascii_alphabetic()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !(numeric || key) {
        return Err(CraftError::field(
            "/target",
            "expected_numeric_project_id_or_case_sensitive_key",
        ));
    }
    Ok(())
}
pub(super) fn validate_permissions(
    profile: &Profile,
    permissions: &[ProjectContextPermission],
) -> Result<(), CraftError> {
    if permissions.len() > 32
        || permissions
            .windows(2)
            .any(|pair| pair[0].target.id >= pair[1].target.id)
    {
        return Err(CraftError::field(
            "/context_reads",
            "expected_at_most_thirty_two_unique_canonical_project_targets",
        ));
    }
    for permission in permissions {
        validate_target(profile, &permission.target)?;
        if permission.operations.is_empty()
            || permission.operations.len() > 2
            || permission
                .operations
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(CraftError::field(
                "/operations",
                "expected_nonempty_unique_canonical_project_context_operations",
            ));
        }
    }
    Ok(())
}
impl ProfileStore {
    /// Edits one exact project's permissions under whole-record CAS, preserving listing/other intent.
    /// Existing format-one records gain no permissions until this explicit applied update succeeds.
    pub fn set_project_context_permission(
        &self,
        profile: &str,
        mut update: ProjectContextPermissionUpdate,
        apply: bool,
        control: &InvocationControl,
        preflight: impl FnOnce(&ProjectReadPolicyRecord) -> Result<(), CraftError>,
    ) -> Result<ProjectReadPolicyRecord, CraftError> {
        control.check()?;
        if !apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Local project-context permission changes require explicit apply.",
            ));
        }
        if update.operations.len() > 2 {
            return Err(CraftError::field(
                "/operations",
                "expected_nonempty_unique_canonical_project_context_operations",
            ));
        }
        update.operations.sort();
        let transaction = begin(&self.connection, control)?;
        let profile = self.get(profile, control)?;
        let mut record=read(self,&profile)?.ok_or_else(||CraftError::new(ErrorCode::PolicyDenied,"Configure the exact project-read site and actor before adding project-context permissions."))?;
        if record.revision != update.expected_revision {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Project-read configuration revision changed; inspect it before updating.",
            ));
        }
        let permission = ProjectContextPermission {
            target: update.target,
            operations: update.operations,
            enabled: update.enabled,
        };
        validate_permissions(&profile, std::slice::from_ref(&permission))?;
        let mut permissions = record.context_reads.take().unwrap_or_default();
        match permissions.binary_search_by(|existing| existing.target.id.cmp(&permission.target.id))
        {
            Ok(index) => permissions[index] = permission,
            Err(index) => permissions.insert(index, permission),
        }
        validate_permissions(&profile, &permissions)?;
        record.format = 2;
        record.context_reads = Some(permissions);
        record.revision = record
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
            .ok_or_else(invalid_state)?;
        record.validate(self, &profile)?;
        preflight(&record)?;
        persist(transaction, &record, control)?;
        Ok(record)
    }
    /// Checks the exact target/operation permission and full captured record before result release.
    pub fn verify_project_context_policy(
        &self,
        expected: &ProjectReadPolicyRecord,
        target: &ResourceRef,
        operation: ProjectContextOperation,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        let current = self
            .project_read_policy(&expected.profile, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted project-read configuration is present.",
                )
            })?;
        if !current
            .context_reads
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|permission| {
                permission.target == *target
                    && permission.enabled
                    && permission.operations.contains(&operation)
            })
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "No enabled permission authorizes this exact project and context operation.",
            ));
        }
        if current != *expected {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Trusted project-read configuration changed during execution.",
            ));
        }
        Ok(())
    }
}
