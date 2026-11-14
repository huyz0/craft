use super::*;
use clap::{CommandFactory, Parser};
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::issue_read_policy::IssueReadPolicyUpdate;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct GetCandidate {
    #[command(flatten)]
    args: IssueGetArgs,
}
#[derive(Parser)]
struct SearchCandidate {
    #[command(flatten)]
    args: IssueSearchArgs,
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn args() -> Result<IssueReadArgs, serde_json::Error> {
    serde_json::from_value(json!({"jql":"project = ED"})).map(IssueReadArgs::Search)
}
fn target() -> ResourceRef {
    site("00000000-1111-2222-3333-444444444444")
}
fn update(revision: Option<u64>, enabled: bool) -> IssueReadPolicyUpdate {
    IssueReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        operations: vec![IssueReadOperation::Get, IssueReadOperation::Search],
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
        Ok(Self {
            root: parent.join(format!("issue-command-{}-{stamp}", std::process::id())),
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
#[test]
fn issue_flags_and_json_share_types_defaults_arrays_without_authority_fields() -> TestResult {
    let mut registry = Registry::default();
    register_candidates(&mut registry)?;
    let get = json!({"issue":"ED-7","cloud_id":target().tenant});
    let mut parser = GetCandidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &get)?;
    assert_eq!(
        encoded(&GetCandidate::try_parse_from(flags)?.args)?,
        encoded(&serde_json::from_value::<IssueGetArgs>(get)?)?
    );
    let search = json!({"jql":"project = ED\nORDER BY key","cloud_id":target().tenant,"reconcile_issue_ids":["1","9223372036854775807"],"page_size":7,"limit":3,"cursor":"cur_00000000000000000000000000000000"});
    let mut parser = SearchCandidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &search)?;
    assert_eq!(
        encoded(&SearchCandidate::try_parse_from(flags)?.args)?,
        encoded(&serde_json::from_value::<IssueSearchArgs>(search)?)?
    );
    let json_default: IssueSearchArgs = serde_json::from_value(json!({"jql":"project = ED"}))?;
    let flag_default =
        SearchCandidate::try_parse_from(["candidate", "--jql", "project = ED"])?.args;
    assert_eq!(encoded(&json_default)?, encoded(&flag_default)?);
    for bad in [
        json!({"jql":"project = ED","expected_actor":"claimed"}),
        json!({"jql":"project = ED","grant":"claimed"}),
        json!({"jql":"project = ED","reconcile_issue_ids":"1,2"}),
        json!({"jql":"project = ED","limit":"20"}),
        json!({"jql":"project = ED","page_size":101}),
    ] {
        assert!(registry.validate(SEARCH, &bad).is_err());
    }
    assert!(
        registry
            .validate(
                GET,
                &json!({"issue":"ED-1","cursor":"cur_00000000000000000000000000000000"})
            )
            .is_err()
    );
    for command in [GET, SEARCH] {
        assert!(
            crate::catalog::local_registry()?
                .descriptor(command)
                .is_err()
        );
    }
    assert!(
        crate::args::Cli::try_parse_from(["craft", "jira", "issue", "get", "--issue", "ED-1"])
            .is_ok()
    );
    assert!(
        crate::args::Cli::try_parse_from([
            "craft",
            "jira",
            "issue",
            "search",
            "--jql",
            "project=ED"
        ])
        .is_ok()
    );
    Ok(())
}
#[test]
fn invalid_issue_arguments_fields_and_recovery_budgets_create_no_state() -> TestResult {
    let files = Files::new()?;
    for mode in ["get", "jql", "cloud", "cursor", "ids", "fields", "budget"] {
        let mut args: IssueSearchArgs = serde_json::from_value(json!({"jql":"project = ED"}))?;
        let mut fields = Vec::new();
        match mode {
            "jql" => args.jql = " ".into(),
            "cloud" => args.cloud_id = Some("not-a-uuid".into()),
            "cursor" => args.cursor = Some("bad".into()),
            "ids" => args.reconcile_issue_ids = vec!["01".into()],
            "fields" => fields = vec!["description.content".into()],
            _ => {}
        }
        let args = if mode == "get" {
            IssueReadArgs::Get(IssueGetArgs {
                cloud_id: None,
                issue: "ED-1/extra".into(),
            })
        } else {
            IssueReadArgs::Search(args)
        };
        let options = InvocationOptions {
            state_root: Some(&files.root),
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: if mode == "budget" { 512 } else { 16_384 },
        };
        let result = execute(args, &fields, &options, &control()?);
        assert!(!result.envelope.ok, "{mode}");
        assert_eq!(result.envelope.request_id, "original-request");
        assert!(!files.root.try_exists()?);
    }
    Ok(())
}
#[test]
fn separate_enabled_issue_configuration_and_operation_allowance_control_selection() -> TestResult {
    let files = Files::new()?;
    let control = control()?;
    let store = ProfileStore::open(&files.root, &control)?;
    let profile = store.create(
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
    let options = InvocationOptions {
        state_root: Some(&files.root),
        profile: Some("factory"),
        request_id: "request",
        max_bytes: 16_384,
    };
    store.set_project_read_policy(
        "factory",
        craft_runtime::project_read_policy::ProjectReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            continuation_site: "https://craft-test.atlassian.net".into(),
            enabled: true,
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    store.set_issue_read_policy("factory", update(None, false), true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let mut only_get = update(Some(1), true);
    only_get.operations = vec![IssueReadOperation::Get];
    store.set_issue_read_policy("factory", only_get, true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let get = IssueReadArgs::Get(IssueGetArgs {
        cloud_id: None,
        issue: "ED-1".into(),
    });
    assert_eq!(
        PreparedInvocation::prepare(get, &[], &options, &store, &control)?.grant(),
        &profile.grant
    );
    store.set_issue_read_policy("factory", update(Some(2), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?.grant(),
        &profile.grant
    );
    let other = IssueReadArgs::Search(IssueSearchArgs {
        cloud_id: Some("11111111-1111-2222-3333-444444444444".into()),
        ..serde_json::from_value(json!({"jql":"project = ED"}))?
    });
    assert!(PreparedInvocation::prepare(other, &[], &options, &store, &control).is_err());
    Ok(())
}

#[test]
fn issue_repository_profile_and_site_precedence_are_isolated_from_the_callers_working_directory()
-> TestResult {
    use craft_test_support::process::{RunOptions, run};
    if let Some(root) = std::env::var_os("CRAFT_ISSUE_CONTEXT_ROOT") {
        let control = control()?;
        let store = ProfileStore::open(Path::new(&root), &control)?;
        let explicit = InvocationOptions {
            state_root: Some(Path::new(&root)),
            profile: Some("factory"),
            request_id: "request",
            max_bytes: 16_384,
        };
        let inherited = InvocationOptions {
            profile: None,
            ..explicit
        };
        assert!(PreparedInvocation::prepare(args()?, &[], &explicit, &store, &control).is_ok());
        assert!(PreparedInvocation::prepare(args()?, &[], &inherited, &store, &control).is_err());
        let other = site("11111111-1111-2222-3333-444444444444");
        fs::write(
            ".craft.json",
            serde_json::to_vec(&json!({"profile":"factory","targets":{"site":other}}))?,
        )?;
        assert!(PreparedInvocation::prepare(args()?, &[], &inherited, &store, &control).is_err());
        let mut chosen: IssueSearchArgs = serde_json::from_value(json!({"jql":"project = ED"}))?;
        chosen.cloud_id = Some(target().tenant);
        assert!(
            PreparedInvocation::prepare(
                IssueReadArgs::Search(chosen),
                &[],
                &inherited,
                &store,
                &control
            )
            .is_ok()
        );
        fs::write(".craft.json", br#"{"profile":"missing"}"#)?;
        assert!(PreparedInvocation::prepare(args()?, &[], &inherited, &store, &control).is_err());
        assert!(PreparedInvocation::prepare(args()?, &[], &explicit, &store, &control).is_ok());
        return Ok(());
    }
    let files = Files::new()?;
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
    store.use_profile("factory", &control, |_| Ok(()))?;
    store.set_issue_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let repository = files.root.join("repository");
    fs::create_dir(&repository)?;
    fs::write(repository.join(".craft.json"), b"invalid repository JSON")?;
    let output = run(
        &std::env::current_exe()?,
        &[
            "--exact",
            "issue_reads::command::tests::issue_repository_profile_and_site_precedence_are_isolated_from_the_callers_working_directory",
            "--nocapture",
        ],
        None,
        RunOptions {
            current_dir: Some(repository),
            environment: vec![(
                "CRAFT_ISSUE_CONTEXT_ROOT".into(),
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
