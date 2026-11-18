use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contracts::{
    AuthMode, GrantRef, ProviderRef, ResourceRef, TenantRef, validate_identity,
};
use crate::error::{CraftError, ErrorCode};

pub fn validate_profile_name(name: &str) -> Result<(), CraftError> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(CraftError::field("/name", "expected_profile_alias"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub provider: String,
    pub deployment: String,
    pub auth_mode: AuthMode,
    pub grant: GrantRef,
    pub tenant: Option<String>,
    #[serde(default)]
    pub defaults: BTreeMap<String, ResourceRef>,
}

impl Profile {
    pub fn provider_ref(&self) -> ProviderRef {
        ProviderRef {
            provider: self.provider.clone(),
            deployment: self.deployment.clone(),
        }
    }
    pub fn tenant_ref(&self) -> Result<TenantRef, CraftError> {
        let tenant = self.tenant.as_ref().ok_or_else(|| {
            CraftError::new(
                ErrorCode::TargetAmbiguous,
                "Profile has no selected tenant.",
            )
        })?;
        Ok(TenantRef {
            provider: self.provider.clone(),
            deployment: self.deployment.clone(),
            tenant: tenant.clone(),
        })
    }
    pub fn validate(&self) -> Result<(), CraftError> {
        validate_profile_name(&self.name)?;
        self.provider_ref().validate()?;
        self.grant.validate()?;
        if let Some(tenant) = &self.tenant {
            validate_identity("/tenant", tenant)?;
        }
        if self.defaults.len() > 32 {
            return Err(CraftError::field("/defaults", "too_many_default_targets"));
        }
        for (kind, target) in &self.defaults {
            self.validate_target(kind, target)?;
        }
        Ok(())
    }
    pub fn validate_target(&self, kind: &str, target: &ResourceRef) -> Result<(), CraftError> {
        validate_identity("/resource_type", kind)?;
        target.validate()?;
        if target.resource_type != kind || target.tenant_ref() != self.tenant_ref()? {
            return Err(CraftError::field(
                "/target",
                "profile_target_identity_mismatch",
            ));
        }
        Ok(())
    }

    /// Hints select context only; permission and policy checks still apply before a remote call.
    pub fn resolve_target(
        &self,
        kind: &str,
        explicit: Option<&ResourceRef>,
        hints: &RepositoryHints,
    ) -> Result<ResourceRef, CraftError> {
        let target = explicit
            .or_else(|| hints.targets.get(kind))
            .or_else(|| self.defaults.get(kind))
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::TargetAmbiguous,
                    "No explicit or configured target exists for this resource kind.",
                )
            })?;
        self.validate_target(kind, target)?;
        Ok(target.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantMetadata {
    pub grant: GrantRef,
    pub authority: String,
    pub auth_mode: AuthMode,
}

impl GrantMetadata {
    pub fn validate(&self) -> Result<(), CraftError> {
        self.grant.validate()?;
        validate_identity("/authority", &self.authority)
    }
}

/// Local configuration only; a tenant or grant here does not establish remote access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub name: String,
    pub provider: String,
    pub deployment: String,
    pub auth_mode: AuthMode,
    pub grant: Option<GrantRef>,
    pub tenant: Option<String>,
    #[serde(default)]
    pub defaults: BTreeMap<String, ResourceRef>,
}

impl ProfileInput {
    pub fn with_grant(&self, grant: GrantRef) -> Result<Profile, CraftError> {
        let profile = Profile {
            name: self.name.clone(),
            provider: self.provider.clone(),
            deployment: self.deployment.clone(),
            auth_mode: self.auth_mode,
            grant,
            tenant: self.tenant.clone(),
            defaults: self.defaults.clone(),
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Configuration templates are independent of compiled provider capabilities.
    pub fn authority(&self) -> Result<&'static str, CraftError> {
        if self.deployment != "cloud"
            || !matches!(
                self.provider.as_str(),
                "jira" | "confluence" | "trello" | "bitbucket"
            )
        {
            return Err(CraftError::new(
                ErrorCode::UnsupportedCapability,
                "No profile configuration template exists for this provider/deployment.",
            ));
        }
        let kind = match self.provider.as_str() {
            "jira" => "project",
            "trello" => "board",
            "bitbucket" => "repository",
            _ => "space",
        };
        if self.defaults.keys().any(|key| key != kind) {
            return Err(CraftError::field(
                "/defaults",
                "unsupported_default_resource_kind",
            ));
        }
        // Trello and Bitbucket grants hold a static key/token pair, never an Atlassian OAuth grant.
        Ok(match self.provider.as_str() {
            "trello" => "trello-key-token",
            "bitbucket" => "bitbucket-api-token",
            _ => "atlassian-cloud",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileSummary {
    pub name: String,
    pub provider: String,
    pub deployment: String,
    pub auth_mode: AuthMode,
    pub tenant: Option<String>,
}

impl From<&Profile> for ProfileSummary {
    fn from(profile: &Profile) -> Self {
        Self {
            name: profile.name.clone(),
            provider: profile.provider.clone(),
            deployment: profile.deployment.clone(),
            auth_mode: profile.auth_mode,
            tenant: profile.tenant.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct RepositoryHints {
    pub profile: Option<String>,
    #[serde(default)]
    pub targets: BTreeMap<String, ResourceRef>,
}

impl RepositoryHints {
    pub fn validate(&self) -> Result<(), CraftError> {
        if let Some(profile) = &self.profile {
            validate_profile_name(profile)?;
        }
        if self.targets.len() > 32 {
            return Err(CraftError::field("/targets", "too_many_target_hints"));
        }
        for (kind, target) in &self.targets {
            validate_identity("/resource_type", kind)?;
            target.validate()?;
            if kind != &target.resource_type {
                return Err(CraftError::field("/targets", "resource_kind_mismatch"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    Explicit,
    Repository,
    Active,
}

pub fn select_profile<'a>(
    explicit: Option<&'a str>,
    hints: &'a RepositoryHints,
    active: Option<&'a str>,
) -> Result<(&'a str, ProfileSource), CraftError> {
    let (name, source) = if let Some(name) = explicit {
        (name, ProfileSource::Explicit)
    } else if let Some(name) = hints.profile.as_deref() {
        (name, ProfileSource::Repository)
    } else if let Some(name) = active {
        (name, ProfileSource::Active)
    } else {
        return Err(CraftError::new(
            ErrorCode::TargetAmbiguous,
            "Select a profile explicitly or configure an active profile.",
        ));
    };
    validate_profile_name(name)?;
    Ok((name, source))
}
