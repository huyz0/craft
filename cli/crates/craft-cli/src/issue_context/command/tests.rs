use super::*;
use clap::{CommandFactory, Parser};
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::issue_read_policy::IssueReadPolicyUpdate;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct EditCandidate {
    #[command(flatten)]
    args: EditMetadataArgs,
}
#[derive(Parser)]
struct TransitionCandidate {
    #[command(flatten)]
    args: TransitionListArgs,
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn args() -> Result<IssueContextArgs, serde_json::Error> {
    serde_json::from_value(json!({"issue":"ED-1"})).map(IssueContextArgs::Transitions)
}
fn target() -> ResourceRef {
    site("00000000-1111-2222-3333-444444444444")
}

#[test]
fn metadata_preflight_reserves_long_resolution_and_escaped_update_evidence() -> TestResult {
    let target = issue_target(&target(), "ED-1");
    let request = |max_bytes| CanonicalRequest {
        command: EDIT.into(),
        target: target.clone(),
        input: json!({"issue":"ED-1"}),
        selection: Selection {
            max_bytes,
            ..Selection::default()
        },
        preconditions: None,
    };
    let policy = || IssueContextPolicy {
        target: target.clone(),
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        expected_actor: "actor-one".into(),
    };
    let host = PreparedIssueContext::prepare(request(16_384), "request", policy())?;
    let mut probe = craft_runtime::pagination::recovery_probe();
    probe.context = Some(
        json!({"requested_issue":"ED-1","resolved_issue":{"provider":"jira","deployment":"cloud","tenant":target.tenant,"resource_type":"issue","id":"9".repeat(512)},"resolution":"resolved_key","state":{"id":"9".repeat(512),"key":format!("{}-1","A".repeat(510)),"project_id":"9".repeat(512),"issue_type_id":"9".repeat(512),"status_id":"9".repeat(512),"updated":"\\".repeat(128)}}),
    );
    let cache = Some(craft_core::contracts::CredentialCacheUpdate {
        grant: policy().grant,
        write_state: craft_core::contracts::RequestState::SentUnknown,
        cleanup_pending: Some(false),
    });
    let required = serde_json::to_vec(&host.envelope(Some(&probe), cache, None)?)?.len() + 1;
    let bytes = u32::try_from(required - 1)?;
    assert_eq!(
        PreparedIssueContext::prepare(request(bytes), "request", policy())
            .err()
            .ok_or("Expected complete evidence reservation")?
            .code,
        ErrorCode::OutputBudgetTooSmall
    );
    assert!(
        PreparedIssueContext::prepare(request(u32::try_from(required)?), "request", policy())
            .is_ok()
    );
    Ok(())
}
fn update(revision: Option<u64>, enabled: bool) -> IssueReadPolicyUpdate {
    IssueReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![
            IssueReadOperation::EditMetadata,
            IssueReadOperation::Transitions,
        ],
        enabled,
        expected_revision: revision,
    }
}
struct Files {
    parent: PathBuf,
    root: PathBuf,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let sequence = ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            root: parent.join(format!(
                "issue-metadata-command-{}-{stamp}-{sequence}",
                std::process::id()
            )),
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
            let _ = fs::remove_dir_all(root);
        }
    }
}
fn setup(files: &Files) -> Result<ProfileStore, Box<dyn std::error::Error>> {
    let control = control()?;
    let store = ProfileStore::open(&files.root, &control)?;
    store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(target().tenant),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    Ok(store)
}

#[test]
fn metadata_flags_and_json_preserve_issue_filters_defaults_and_strict_contracts() -> TestResult {
    let mut registry = Registry::default();
    register_candidates(&mut registry)?;
    let edit = json!({"issue":"OLD-1","limit":3,"cursor":"cur_00000000000000000000000000000000","cloud_id":target().tenant});
    let mut parser = EditCandidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &edit)?;
    assert_eq!(
        encoded(&EditCandidate::try_parse_from(flags)?.args)?,
        encoded(&serde_json::from_value::<EditMetadataArgs>(edit)?)?
    );
    let input = json!({"issue":"OLD-1","transition_id":"31","include_unavailable":true,"sort_by_status":true,"limit":7});
    let mut parser = TransitionCandidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &input)?;
    assert_eq!(
        encoded(&TransitionCandidate::try_parse_from(flags)?.args)?,
        encoded(&serde_json::from_value::<TransitionListArgs>(input)?)?
    );
    for command in [EDIT, TRANSITIONS] {
        assert!(
            crate::catalog::local_registry()?
                .descriptor(command)
                .is_err()
        );
        for name in [
            "grant",
            "expected_actor",
            "overrideScreenSecurity",
            "overrideEditableFlag",
            "skipRemoteOnlyCondition",
            "status",
            "expand",
        ] {
            let mut input = json!({"issue":"ED-1"});
            input[name] = json!(true);
            assert!(
                registry.validate(command, &input).is_err(),
                "{command}/{name}"
            );
        }
    }
    assert!(
        crate::args::Cli::try_parse_from(["craft", "jira", "metadata", "edit", "--issue", "ED-1"])
            .is_ok()
    );
    assert!(
        crate::args::Cli::try_parse_from([
            "craft",
            "jira",
            "transition",
            "list",
            "--issue",
            "ED-1"
        ])
        .is_ok()
    );
    assert_eq!(
        encoded(&TransitionCandidate::try_parse_from(["candidate", "--issue", "ED-1"])?.args)?,
        encoded(&serde_json::from_value::<TransitionListArgs>(
            json!({"issue":"ED-1"})
        )?)?
    );
    Ok(())
}
#[test]
fn malformed_metadata_inputs_projection_and_recovery_budgets_create_no_state() -> TestResult {
    let files = Files::new()?;
    for mode in [
        "cloud",
        "cursor",
        "issue",
        "transition",
        "fields",
        "budget",
        "edit_budget",
    ] {
        let mut input: TransitionListArgs = serde_json::from_value(json!({"issue":"ED-1"}))?;
        let mut fields = vec![];
        match mode {
            "cloud" => input.cloud_id = Some("bad".into()),
            "cursor" => input.cursor = Some("bad".into()),
            "issue" => input.issue = "../issue".into(),
            "transition" => input.transition_id = Some("bad\n".into()),
            "fields" => fields = vec!["unsupported".into()],
            _ => {}
        }
        let args = if mode == "edit_budget" {
            IssueContextArgs::Edit(serde_json::from_value(json!({"issue":"ED-1"}))?)
        } else {
            IssueContextArgs::Transitions(input)
        };
        let options = InvocationOptions {
            state_root: Some(&files.root),
            profile: Some("factory"),
            request_id: "request",
            max_bytes: if mode.ends_with("budget") {
                512
            } else {
                16_384
            },
        };
        let result = execute(args, &fields, &options, &control()?);
        assert!(!result.envelope.ok, "{mode}");
        assert!(
            !files.root.try_exists()?,
            "{mode}: {:?}",
            result.envelope.error
        );
    }
    Ok(())
}
#[test]
fn metadata_candidates_require_individual_allowances_and_capture_exact_record_revisions()
-> TestResult {
    let files = Files::new()?;
    let store = setup(&files)?;
    let control = control()?;
    let options = InvocationOptions {
        state_root: Some(&files.root),
        profile: Some("factory"),
        request_id: "request",
        max_bytes: 16_384,
    };
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let mut old = update(None, true);
    old.operations = vec![IssueReadOperation::Get, IssueReadOperation::Search];
    store.set_issue_read_policy("factory", old, true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let mut edit_only = update(Some(1), true);
    edit_only.operations = vec![IssueReadOperation::EditMetadata];
    store.set_issue_read_policy("factory", edit_only, true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let prepared = PreparedInvocation::prepare(
        IssueContextArgs::Edit(serde_json::from_value(json!({"issue":"ED-1"}))?),
        &[],
        &options,
        &store,
        &control,
    )?;
    assert_eq!(prepared.grant(), &store.get("factory", &control)?.grant);
    assert_eq!(
        prepared.host.binding.target,
        issue_target(&target(), "ED-1")
    );
    store.set_issue_read_policy("factory", update(Some(2), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_issue_read_policy(&prepared.policy, IssueReadOperation::EditMetadata, &control)
            .err()
            .ok_or("Expected captured record refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_ok());
    let other = IssueContextArgs::Transitions(serde_json::from_value(
        json!({"issue":"ED-1","cloud_id":"11111111-1111-2222-3333-444444444444"}),
    )?);
    assert!(PreparedInvocation::prepare(other, &[], &options, &store, &control).is_err());
    store.set_issue_read_policy(
        "factory",
        update(Some(3), false),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    Ok(())
}
#[test]
fn issue_metadata_profile_selection_uses_explicit_repository_then_active_context() -> TestResult {
    use craft_test_support::process::{RunOptions, run};
    const TEST: &str = "issue_context::command::tests::issue_metadata_profile_selection_uses_explicit_repository_then_active_context";
    if let Some(root) = std::env::var_os("CRAFT_ISSUE_METADATA_CONTEXT_ROOT") {
        let root = PathBuf::from(root);
        let control = control()?;
        let store = ProfileStore::open(&root, &control)?;
        let options = InvocationOptions {
            state_root: Some(&root),
            profile: None,
            request_id: "request",
            max_bytes: 16_384,
        };
        assert_eq!(
            PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?
                .policy
                .profile,
            "second"
        );
        fs::write(
            ".craft.json",
            serde_json::to_vec(
                &json!({"profile":"second","targets":{"site":site("11111111-1111-2222-3333-444444444444")}}),
            )?,
        )?;
        assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
        let explicit_site = IssueContextArgs::Transitions(serde_json::from_value(
            json!({"issue":"ED-1","cloud_id":target().tenant}),
        )?);
        assert_eq!(
            PreparedInvocation::prepare(explicit_site, &[], &options, &store, &control)?
                .policy
                .profile,
            "second"
        );
        fs::write(".craft.json", b"invalid repository JSON")?;
        assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
        let explicit = InvocationOptions {
            profile: Some("factory"),
            ..options
        };
        assert_eq!(
            PreparedInvocation::prepare(args()?, &[], &explicit, &store, &control)?
                .policy
                .profile,
            "factory"
        );
        fs::remove_file(".craft.json")?;
        assert_eq!(
            PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?
                .policy
                .profile,
            "factory"
        );
        return Ok(());
    }
    let files = Files::new()?;
    let store = setup(&files)?;
    let control = control()?;
    store.create(
        &ProfileInput {
            name: "second".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(target().tenant),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    for name in ["factory", "second"] {
        store.set_issue_read_policy(name, update(None, true), true, &control, |_| Ok(()))?;
    }
    store.use_profile("factory", &control, |_| Ok(()))?;
    let local = files.root.join("checkout");
    fs::create_dir(&local)?;
    fs::write(
        local.join(".craft.json"),
        br#"{"profile":"second","targets":{}}"#,
    )?;
    let output = run(
        &std::env::current_exe()?,
        &["--exact", TEST, "--nocapture"],
        None,
        RunOptions {
            current_dir: Some(local),
            environment: vec![(
                "CRAFT_ISSUE_METADATA_CONTEXT_ROOT".into(),
                Some(files.root.clone().into_os_string()),
            )],
            ..RunOptions::default()
        },
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
