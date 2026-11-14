use std::collections::BTreeMap;

use craft_core::contracts::{AuthMode, GrantRef, ResourceRef};
use craft_core::error::ErrorCode;
use craft_core::profiles::{
    ProfileInput, ProfileSource, RepositoryHints, select_profile, validate_profile_name,
};
use serde_json::json;

fn target(tenant: &str) -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: tenant.into(),
        resource_type: "project".into(),
        id: "PAY".into(),
    }
}

fn input() -> ProfileInput {
    ProfileInput {
        name: "finance".into(),
        provider: "jira".into(),
        deployment: "cloud".into(),
        auth_mode: AuthMode::Delegated,
        grant: None,
        tenant: Some("primary".into()),
        defaults: BTreeMap::from([("project".into(), target("primary"))]),
    }
}

#[test]
fn selects_explicit_then_repository_then_active_without_first_profile_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let hints = RepositoryHints {
        profile: Some("repo".into()),
        ..RepositoryHints::default()
    };
    assert_eq!(
        select_profile(Some("explicit"), &hints, Some("active"))?,
        ("explicit", ProfileSource::Explicit)
    );
    assert_eq!(
        select_profile(None, &hints, Some("active"))?,
        ("repo", ProfileSource::Repository)
    );
    assert_eq!(
        select_profile(None, &RepositoryHints::default(), Some("active"))?,
        ("active", ProfileSource::Active)
    );
    assert_eq!(
        select_profile(None, &RepositoryHints::default(), None)
            .err()
            .ok_or("Expected ambiguity")?
            .code,
        ErrorCode::TargetAmbiguous
    );
    assert!(select_profile(Some("../other"), &hints, Some("active")).is_err());
    Ok(())
}

#[test]
fn rejects_repository_security_overrides_and_mismatched_resource_kinds() {
    for name in [
        "state_root",
        "credential_backend",
        "auth_host",
        "broker",
        "reviewers",
    ] {
        assert!(serde_json::from_value::<RepositoryHints>(json!({name: "untrusted"})).is_err());
    }
    let hints = RepositoryHints {
        targets: BTreeMap::from([("space".into(), target("primary"))]),
        ..RepositoryHints::default()
    };
    assert!(hints.validate().is_err());
}

#[test]
fn resolves_targets_without_cross_tenant_or_cross_kind_fallback()
-> Result<(), Box<dyn std::error::Error>> {
    let profile = input().with_grant(GrantRef {
        id: "shared".into(),
    })?;
    let mut from_repository = target("primary");
    from_repository.id = "REPO".into();
    let hints = RepositoryHints {
        targets: BTreeMap::from([("project".into(), from_repository.clone())]),
        ..RepositoryHints::default()
    };
    let mut explicit = target("primary");
    explicit.id = "EXPLICIT".into();
    assert_eq!(
        profile.resolve_target("project", Some(&explicit), &hints)?,
        explicit
    );
    assert_eq!(
        profile.resolve_target("project", None, &hints)?,
        from_repository
    );
    assert_eq!(
        profile.resolve_target("project", None, &RepositoryHints::default())?,
        target("primary")
    );
    assert!(
        profile
            .resolve_target("project", Some(&target("foreign")), &hints)
            .is_err()
    );
    assert!(
        profile
            .resolve_target("space", Some(&explicit), &hints)
            .is_err()
    );
    let mut missing_tenant = input();
    missing_tenant.tenant = None;
    assert!(missing_tenant.with_grant(profile.grant.clone()).is_err());
    missing_tenant.defaults.clear();
    let unselected = missing_tenant.with_grant(profile.grant)?;
    assert_eq!(
        unselected
            .tenant_ref()
            .err()
            .ok_or("Expected ambiguity")?
            .code,
        ErrorCode::TargetAmbiguous
    );
    Ok(())
}

#[test]
fn profile_templates_share_authority_without_claiming_capabilities()
-> Result<(), Box<dyn std::error::Error>> {
    let jira = input();
    let mut confluence = input();
    confluence.provider = "confluence".into();
    confluence.defaults.clear();
    assert_eq!(jira.authority()?, confluence.authority()?);
    confluence.defaults = jira.defaults.clone();
    assert!(confluence.authority().is_err());
    confluence.defaults.clear();
    confluence.deployment = "server".into();
    assert!(confluence.authority().is_err());
    Ok(())
}

#[test]
fn aliases_and_auth_modes_are_strict_and_do_not_normalize() {
    for invalid in [
        "",
        ".",
        "..",
        "-hidden",
        "_hidden",
        "with space",
        "finance/other",
        "équipe",
        "UPPER ",
    ] {
        assert!(validate_profile_name(invalid).is_err(), "{invalid}");
    }
    assert!(validate_profile_name(&"a".repeat(65)).is_err());
    assert!(validate_profile_name("Finance.eu-2_main").is_ok());
    assert_eq!(
        "delegated".parse::<AuthMode>().ok(),
        Some(AuthMode::Delegated)
    );
    assert_eq!("service".parse::<AuthMode>().ok(), Some(AuthMode::Service));
    assert!("Delegated".parse::<AuthMode>().is_err());
}
