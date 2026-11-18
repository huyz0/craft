use super::*;
use clap::Parser;
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::control::SystemClock;
use std::sync::Arc;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct SetCli {
    #[command(flatten)]
    args: PolicySetArgs,
}
fn target(kind: PolicyKind) -> ResourceRef {
    let confluence = matches!(
        kind,
        PolicyKind::ConfluenceRead | PolicyKind::ConfluenceWrite | PolicyKind::ConfluenceAdmin
    );
    let resource = match kind {
        PolicyKind::ConfluenceRead
        | PolicyKind::ConfluenceAdmin
        | PolicyKind::JiraRead
        | PolicyKind::JiraProjectRead
        | PolicyKind::JiraFieldRead => "site",
        PolicyKind::ConfluenceWrite => "space",
        _ => "project",
    };
    let cloud = "00000000-1111-2222-3333-444444444444";
    ResourceRef {
        provider: if confluence { "confluence" } else { "jira" }.into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: resource.into(),
        id: if resource == "site" { cloud } else { "100" }.into(),
    }
}
fn args(kind: PolicyKind) -> PolicySetArgs {
    PolicySetArgs {
        kind,
        target: target(kind),
        expected_actor: "service-actor".into(),
        enabled: Some(true),
        operations: vec![
            match kind {
                PolicyKind::ConfluenceRead => "confluence.page.get",
                PolicyKind::ConfluenceAdmin => "confluence.space.create",
                PolicyKind::ConfluenceWrite => "confluence.page.create",
                PolicyKind::JiraCreate | PolicyKind::JiraRecovery => "jira.issue.create",
                PolicyKind::JiraRead => "jira.issue.get",
                PolicyKind::JiraProjectRead => "jira.project.list",
                PolicyKind::JiraProjectContext => "jira.project.get",
                PolicyKind::JiraFieldRead => "jira.field.list",
                PolicyKind::JiraMutation => "jira.issue.update",
                PolicyKind::TrelloRead => "trello.board.list",
            }
            .into(),
        ],
        expected_revision: None,
        continuation_site: if matches!(
            kind,
            PolicyKind::JiraProjectRead | PolicyKind::JiraFieldRead
        ) {
            Some("https://craft-test.atlassian.net".into())
        } else {
            None
        },
        apply: true,
    }
}

#[test]
fn public_policy_schema_and_validation_accept_ten_explicit_jira_reads_only() -> TestResult {
    let mut a = args(PolicyKind::JiraRead);
    a.operations = [
        "jira.issue.get",
        "jira.issue.search",
        "jira.metadata.edit",
        "jira.transition.list",
        "jira.permission.check",
        "jira.link.get",
        "jira.link.list",
        "jira.link.types",
        "jira.comment.get",
        "jira.comment.list",
    ]
    .map(String::from)
    .to_vec();
    let registry = crate::catalog::registry()?;
    registry.validate(SET, &json!(a))?;
    a.validate(&[], &options(None))?;
    let files = Files::new()?;
    let c = control()?;
    let store = ProfileStore::open(&files.root, &c)?;
    store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: craft_core::contracts::AuthMode::Service,
            grant: None,
            tenant: Some(a.target.tenant.clone()),
            defaults: Default::default(),
        },
        &c,
        |_| Ok(()),
    )?;
    let result = execute(
        PolicyArgs::Set(a.clone()),
        &[],
        &options(Some(&files.root)),
        &c,
    );
    assert!(result.envelope.ok, "{:?}", result.envelope.error);
    assert_eq!(
        result.envelope.data.as_ref().ok_or("data")?["policy"]["operations"]
            .as_array()
            .ok_or("operations")?
            .len(),
        10
    );
    assert_eq!(
        store
            .issue_read_policy("factory", &c)?
            .ok_or("record")?
            .format,
        3
    );
    a.kind = PolicyKind::JiraMutation;
    assert!(a.validate(&[], &options(None)).is_err());
    Ok(())
}
fn options<'a>(root: Option<&'a std::path::Path>) -> InvocationOptions<'a> {
    InvocationOptions {
        state_root: root,
        profile: Some("factory"),
        request_id: "setup-request",
        max_bytes: 16384,
    }
}
#[test]
fn policy_flags_schemas_and_operation_kinds_are_strict() -> TestResult {
    let a = args(PolicyKind::ConfluenceRead);
    let target = serde_json::to_string(&a.target)?;
    let parsed = SetCli::try_parse_from([
        "set",
        "--target",
        &target,
        "--expected-actor",
        "service-actor",
        "--enabled",
        "true",
        "--operations",
        "confluence.page.get",
        "--apply",
    ])?;
    assert_eq!(serde_json::to_value(parsed.args)?, serde_json::to_value(a)?);
    let mut r = crate::catalog::local_registry()?;
    register_candidates(&mut r)?;
    for command in [GET, SET] {
        let d = r.descriptor(command)?;
        r.validate(command, &d.example)?;
        assert!(jsonschema::validator_for(&d.response_schema)?.is_valid(&d.response_example));
    }
    assert!(crate::catalog::local_registry()?.descriptor(SET).is_err());
    let mut a = args(PolicyKind::ConfluenceRead);
    a.operations = vec!["confluence.page.create".into()];
    assert!(a.validate(&[], &options(None)).is_err());
    a.operations = vec!["confluence.page.get".into(); 2];
    assert!(a.validate(&[], &options(None)).is_err());
    Ok(())
}

struct Files {
    root: std::path::PathBuf,
    parent: std::path::PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        Ok(Self {
            root: craft_test_support::state::fixture_root(
                &parent,
                "policy-setup",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos(),
            ),
            parent,
        })
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root.starts_with(&self.parent)
            && root != self.parent
        {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
#[test]
fn site_and_resource_policies_round_trip_actual_grants_and_preserve_revision_cas() -> TestResult {
    for kind in [
        PolicyKind::ConfluenceRead,
        PolicyKind::ConfluenceWrite,
        PolicyKind::JiraCreate,
        PolicyKind::JiraRead,
        PolicyKind::JiraProjectRead,
        PolicyKind::JiraFieldRead,
        PolicyKind::JiraMutation,
        PolicyKind::JiraRecovery,
    ] {
        let files = Files::new()?;
        let c = control()?;
        let store = ProfileStore::open(&files.root, &c)?;
        let o = options(Some(&files.root));
        let mut a = args(kind);
        if kind == PolicyKind::JiraMutation {
            a.operations = vec![
                "jira.issue.delete".into(),
                "jira.issue.delete-all-subtasks".into(),
            ];
        }
        let profile = store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: a.target.provider.clone(),
                deployment: "cloud".into(),
                auth_mode: craft_core::contracts::AuthMode::Service,
                grant: None,
                tenant: Some(a.target.tenant.clone()),
                defaults: std::collections::BTreeMap::new(),
            },
            &c,
            |_| Ok(()),
        )?;
        let get = PolicyGetArgs {
            kind,
            target: a.target.clone(),
        };
        assert!(
            get_with_store(&get, &[], &o, &store, &c)?
                .envelope
                .data
                .ok_or("data")?["policy"]
                .is_null()
        );
        let first = set_with_store(&a, &[], &o, &store, &c)?;
        assert!(first.envelope.ok, "{:?}", first.envelope.error);
        let data = first.envelope.data.ok_or("data")?;
        assert!(jsonschema::validator_for(&schema_for::<PolicyData>()?)?.is_valid(&data));
        assert_eq!(
            data["policy"]["grant"],
            serde_json::to_value(&profile.grant)?
        );
        assert_eq!(data["configuration_write_state"], "confirmed");
        assert_eq!(data["policy"]["operations"], json!(a.operations));
        assert_eq!(data["live_verified"], false);
        assert!(data["policy"].get("installation").is_none());
        assert!(data["policy"].get("format").is_none());
        let loaded = get_with_store(&get, &[], &o, &store, &c)?
            .envelope
            .data
            .ok_or("data")?;
        assert_eq!(loaded["policy"], data["policy"]);
        assert!(!set_with_store(&a, &[], &o, &store, &c)?.envelope.ok);
        a.expected_revision = Some(1);
        a.enabled = Some(false);
        let second = set_with_store(&a, &[], &o, &store, &c)?;
        assert!(second.envelope.ok);
        assert_eq!(second.envelope.data.ok_or("data")?["policy"]["revision"], 2);
        assert_eq!(
            set_with_store(&a, &[], &o, &store, &c)?
                .envelope
                .error
                .ok_or("cas")?
                .code,
            ErrorCode::VersionConflict
        );
        let reopened = ProfileStore::open(&files.root, &c)?;
        assert_eq!(
            get_with_store(&get, &[], &o, &reopened, &c)?
                .envelope
                .data
                .ok_or("data")?["policy"]["enabled"],
            false
        );
    }
    Ok(())
}
#[test]
fn project_context_updates_preserve_siblings_listing_and_shared_actor_revision() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let store = ProfileStore::open(&files.root, &c)?;
    let o = options(Some(&files.root));
    let mut site = args(PolicyKind::JiraProjectRead);
    store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: craft_core::contracts::AuthMode::Service,
            grant: None,
            tenant: Some(site.target.tenant.clone()),
            defaults: Default::default(),
        },
        &c,
        |_| Ok(()),
    )?;
    let mut context = args(PolicyKind::JiraProjectContext);
    context.expected_revision = Some(1);
    assert!(!set_with_store(&context, &[], &o, &store, &c)?.envelope.ok);
    assert!(set_with_store(&site, &[], &o, &store, &c)?.envelope.ok);
    let first = set_with_store(&context, &[], &o, &store, &c)?;
    assert!(first.envelope.ok, "{:?}", first.envelope.error);
    let full_data = first.envelope.data.ok_or("data")?;
    assert!(jsonschema::validator_for(&schema_for::<PolicyData>()?)?.is_valid(&full_data));
    let snapshot = full_data["policy"].clone();
    assert_eq!(snapshot["revision"], 2);
    assert_eq!(snapshot["target"], json!(site.target));
    assert_eq!(
        snapshot["continuation_site"],
        json!(
            craft_runtime::transport::JiraProjectSite::parse(
                site.continuation_site.as_deref().ok_or("origin")?
            )?
            .as_str()
        )
    );
    assert_eq!(
        snapshot["context_reads"][0]["target"],
        json!(context.target)
    );
    let observed = get_with_store(
        &PolicyGetArgs {
            kind: context.kind,
            target: context.target.clone(),
        },
        &[],
        &o,
        &store,
        &c,
    )?
    .envelope
    .data
    .ok_or("data")?;
    assert_eq!(observed["policy"], snapshot);
    context.target.id = "OTHER".into();
    context.expected_revision = Some(2);
    assert!(set_with_store(&context, &[], &o, &store, &c)?.envelope.ok);
    site.expected_revision = Some(3);
    site.enabled = Some(false);
    assert!(set_with_store(&site, &[], &o, &store, &c)?.envelope.ok);
    let record = store.project_read_policy("factory", &c)?.ok_or("record")?;
    assert_eq!(record.revision, 4);
    assert!(!record.enabled);
    assert_eq!(record.context_reads.as_ref().ok_or("context")?.len(), 2);
    context.expected_revision = Some(4);
    context.expected_actor = "other-actor".into();
    assert_eq!(
        set_with_store(&context, &[], &o, &store, &c)?
            .envelope
            .error
            .ok_or("error")?
            .code,
        ErrorCode::PolicyDenied
    );
    context.expected_actor = "service-actor".into();
    context.expected_revision = Some(3);
    assert_eq!(
        set_with_store(&context, &[], &o, &store, &c)?
            .envelope
            .error
            .ok_or("error")?
            .code,
        ErrorCode::VersionConflict
    );
    assert_eq!(
        store.project_read_policy("factory", &c)?.ok_or("record")?,
        record
    );
    Ok(())
}
#[test]
fn read_policy_origin_context_revision_and_operation_mix_refuse_before_state() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    let o = options(Some(&files.root));
    for kind in [PolicyKind::JiraProjectRead, PolicyKind::JiraFieldRead] {
        for origin in [
            None,
            Some("https://evil.invalid"),
            Some("https://craft-test.atlassian.net/path"),
        ] {
            let mut a = args(kind);
            a.continuation_site = origin.map(String::from);
            assert!(!execute(PolicyArgs::Set(a), &[], &o, &c).envelope.ok);
            assert!(!files.root.exists());
        }
        let mut a = args(kind);
        a.operations = vec!["jira.issue.update".into()];
        assert!(!execute(PolicyArgs::Set(a), &[], &o, &c).envelope.ok);
        assert!(!files.root.exists());
    }
    let a = args(PolicyKind::JiraProjectContext);
    assert!(!execute(PolicyArgs::Set(a), &[], &o, &c).envelope.ok);
    let mut a = args(PolicyKind::JiraRead);
    a.continuation_site = Some("https://craft-test.atlassian.net".into());
    assert!(!execute(PolicyArgs::Set(a), &[], &o, &c).envelope.ok);
    assert!(!files.root.exists());
    Ok(())
}
#[test]
fn malformed_or_implicit_policy_administration_never_creates_state() -> TestResult {
    let files = Files::new()?;
    let c = control()?;
    for mode in [
        "apply",
        "enabled",
        "actor",
        "revision",
        "target",
        "operations",
        "profile",
        "budget",
    ] {
        let mut a = args(PolicyKind::ConfluenceRead);
        let mut o = options(Some(&files.root));
        match mode {
            "apply" => a.apply = false,
            "enabled" => a.enabled = None,
            "actor" => a.expected_actor = "anonymous".into(),
            "revision" => a.expected_revision = Some(0),
            "target" => a.target.resource_type = "space".into(),
            "operations" => a.operations = vec!["confluence.page.create".into()],
            "profile" => o.profile = None,
            "budget" => o.max_bytes = 512,
            _ => {}
        }
        let result = execute(PolicyArgs::Set(a), &[], &o, &c);
        assert!(!result.envelope.ok, "{mode}");
        assert!(!files.root.exists(), "{mode}");
    }
    let mut v = serde_json::to_value(args(PolicyKind::ConfluenceRead))?;
    v["grant"] = json!({"id":"supplied"});
    assert!(serde_json::from_value::<PolicySetArgs>(v).is_err());
    Ok(())
}
#[test]
fn uncertain_configuration_keeps_proposed_revision_separate_from_observed_policy() -> TestResult {
    let a = args(PolicyKind::ConfluenceRead);
    let o = options(None);
    let proposed = PolicySnapshot {
        revision: 2,
        grant: GrantRef {
            id: "actual-grant".into(),
        },
        target: a.target.clone(),
        expected_actor: a.expected_actor.clone(),
        enabled: true,
        operations: a.operations.clone(),
        continuation_site: None,
        context_reads: None,
    };
    let result = finish_set(
        &a,
        &o,
        "factory",
        Some(proposed),
        Err(CraftError::new(
            ErrorCode::WriteOutcomeUnknown,
            "Commit acknowledgment was lost.",
        )),
    );
    assert_eq!(
        result.envelope.error.ok_or("error")?.details.write_state,
        RequestState::SentUnknown
    );
    let data = result.envelope.data.ok_or("data")?;
    assert!(data["policy"].is_null());
    assert_eq!(data["proposed_policy"]["revision"], 2);
    assert_eq!(data["proposed_policy"]["grant"]["id"], "actual-grant");
    assert_eq!(data["configuration_write_state"], "sent_unknown");
    assert!(matches!(
        result.output_policy,
        OutputPolicy::PreserveEvidence
    ));
    Ok(())
}
