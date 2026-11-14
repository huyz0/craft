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
struct Candidate {
    #[command(flatten)]
    args: PermissionCheckArgs,
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn args() -> Result<PermissionCheckArgs, serde_json::Error> {
    serde_json::from_value(json!({"permissions":["EDIT_ISSUES"]}))
}
fn target() -> ResourceRef {
    site("00000000-1111-2222-3333-444444444444")
}
fn update(revision: Option<u64>, enabled: bool) -> IssueReadPolicyUpdate {
    IssueReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![IssueReadOperation::Permissions],
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
                "permission-command-{}-{stamp}-{sequence}",
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
fn permission_flags_json_defaults_and_native_contexts_match_without_csv_or_authority_input()
-> TestResult {
    let mut registry = Registry::default();
    register_candidate(&mut registry)?;
    for input in [
        json!({"permissions":["EDIT_ISSUES","app.plugin:permission"],"issue":"OLD-1","limit":3}),
        json!({"permissions":["BROWSE_PROJECTS"],"comment_id":"12","cursor":"cur_00000000000000000000000000000000"}),
        json!({"permissions":["EDIT_ISSUES"],"project":"ED"}),
    ] {
        let mut parser = Candidate::command();
        parser.build();
        let flags = crate::contracts::example_arguments(&[], &parser, &input)?;
        assert_eq!(
            encoded(&Candidate::try_parse_from(flags)?.args)?,
            encoded(&serde_json::from_value::<PermissionCheckArgs>(input)?)?
        );
    }
    for input in [
        json!({}),
        json!({"permissions":["EDIT_ISSUES"],"grant":"claimed"}),
        json!({"permissions":["EDIT_ISSUES"],"expected_actor":"claimed"}),
        json!({"permissions":"EDIT_ISSUES,BROWSE_PROJECTS"}),
        json!({"permissions":["EDIT_ISSUES"],"account_id":"other"}),
        json!({"permissions":["EDIT_ISSUES"],"projectUuid":"claimed"}),
    ] {
        assert!(registry.validate(COMMAND, &input).is_err());
    }
    let parsed =
        Candidate::try_parse_from(["candidate", "--permission", "EDIT_ISSUES,BROWSE_PROJECTS"])?
            .args;
    assert!(
        parsed
            .validate(
                &[],
                &InvocationOptions {
                    state_root: None,
                    profile: None,
                    request_id: "request",
                    max_bytes: 16_384
                }
            )
            .is_err()
    );
    assert_eq!(
        encoded(&Candidate::try_parse_from(["candidate", "--permission", "EDIT_ISSUES"])?.args)?,
        encoded(&args()?)?
    );
    assert!(
        crate::catalog::local_registry()?
            .descriptor(COMMAND)
            .is_err()
    );
    assert!(
        crate::args::Cli::try_parse_from([
            "craft",
            "jira",
            "permission",
            "check",
            "--permission",
            "EDIT_ISSUES"
        ])
        .is_ok()
    );
    Ok(())
}
#[test]
fn invalid_permission_contexts_fields_cursors_and_budgets_create_no_state() -> TestResult {
    let files = Files::new()?;
    for mode in [
        "keys", "csv", "cloud", "cursor", "conflict", "comment", "fields", "budget",
    ] {
        let mut input = args()?;
        let mut fields = vec![];
        match mode {
            "keys" => input.permissions.clear(),
            "csv" => input.permissions = vec!["EDIT_ISSUES,BROWSE_PROJECTS".into()],
            "cloud" => input.cloud_id = Some("bad".into()),
            "cursor" => input.cursor = Some("bad".into()),
            "conflict" => {
                input.project = Some("ED".into());
                input.issue = Some("ED-1".into());
            }
            "comment" => input.comment_id = Some("12".into()),
            "fields" => fields = vec!["unsupported".into()],
            _ => {}
        }
        let options = InvocationOptions {
            state_root: Some(&files.root),
            profile: Some("factory"),
            request_id: "request",
            max_bytes: if mode == "budget" { 512 } else { 16_384 },
        };
        let result = execute(input, &fields, &options, &control()?);
        assert!(!result.envelope.ok, "{mode}");
        assert!(!files.root.try_exists()?, "{mode}");
    }
    Ok(())
}
#[test]
fn permission_candidates_require_a_new_allowance_and_capture_site_actor_grant_and_revision()
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
    old.operations = vec![
        IssueReadOperation::Get,
        IssueReadOperation::Search,
        IssueReadOperation::EditMetadata,
        IssueReadOperation::Transitions,
    ];
    store.set_issue_read_policy("factory", old, true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    store.set_issue_read_policy("factory", update(Some(1), true), true, &control, |_| Ok(()))?;
    let prepared = PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?;
    assert_eq!(prepared.grant(), &store.get("factory", &control)?.grant);
    let mut other = args()?;
    other.cloud_id = Some("11111111-1111-2222-3333-444444444444".into());
    assert!(PreparedInvocation::prepare(other, &[], &options, &store, &control).is_err());
    store.set_issue_read_policy("factory", update(Some(2), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_issue_read_policy(&prepared.policy, IssueReadOperation::Permissions, &control)
            .err()
            .ok_or("Expected revision refusal")?
            .code,
        ErrorCode::VersionConflict
    );
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
fn permission_profile_selection_uses_explicit_repository_then_active_context() -> TestResult {
    use craft_test_support::process::{RunOptions, run};
    const TEST: &str = "permission_reads::command::tests::permission_profile_selection_uses_explicit_repository_then_active_context";
    if let Some(root) = std::env::var_os("CRAFT_PERMISSION_CONTEXT_ROOT") {
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
        let explicit_site = serde_json::from_value(
            json!({"permissions":["EDIT_ISSUES"],"cloud_id":target().tenant}),
        )?;
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
                "CRAFT_PERMISSION_CONTEXT_ROOT".into(),
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
