use super::*;
use clap::{CommandFactory, Parser};
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::project_read_policy::ProjectReadPolicyUpdate;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct Candidate {
    #[command(flatten)]
    args: ProjectListArgs,
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn args() -> Result<ProjectListArgs, serde_json::Error> {
    serde_json::from_value(json!({}))
}
fn target() -> ResourceRef {
    site("00000000-1111-2222-3333-444444444444")
}
fn update(revision: Option<u64>, enabled: bool) -> ProjectReadPolicyUpdate {
    ProjectReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        continuation_site: "https://craft-test.atlassian.net".into(),
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
            root: parent.join(format!("project-command-{}-{stamp}", std::process::id())),
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
fn flags_and_json_share_defaults_and_scalar_semantics_without_authority_fields() -> TestResult {
    let mut registry = Registry::default();
    register_candidate(&mut registry)?;
    let mut parser = Candidate::command();
    parser.build();
    let example = json!({"cloud_id":target().tenant,"query":"null & Unicode 世界","page_size":50,"limit":7,"cursor":"cur_00000000000000000000000000000000"});
    let flags = crate::contracts::example_arguments(&[], &parser, &example)?;
    assert_eq!(
        serde_json::to_value(Candidate::try_parse_from(flags)?.args)?,
        serde_json::to_value(serde_json::from_value::<ProjectListArgs>(example)?)?
    );
    assert_eq!(args()?.page_size, 50);
    assert_eq!(args()?.limit, 20);
    for bad in [
        json!({"expected_actor":"claimed"}),
        json!({"continuation_site":"https://evil.invalid"}),
        json!({"limit":"20"}),
        json!({"page_size":0}),
    ] {
        assert!(registry.validate(COMMAND, &bad).is_err());
    }
    assert!(crate::args::Cli::try_parse_from(["craft", "jira", "project", "list"]).is_ok());
    assert!(
        crate::catalog::local_registry()?
            .descriptor(COMMAND)
            .is_err()
    );
    Ok(())
}

#[test]
fn malformed_arguments_projection_and_small_budget_create_no_state() -> TestResult {
    let files = Files::new()?;
    for mode in ["cloud", "query", "cursor", "fields", "budget"] {
        let mut args = args()?;
        let mut fields = Vec::new();
        let bytes = if mode == "budget" { 512 } else { 16_384 };
        match mode {
            "cloud" => args.cloud_id = Some("not-a-uuid".into()),
            "query" => args.query = Some("bad\n".into()),
            "cursor" => args.cursor = Some("wrong".into()),
            "fields" => fields = vec!["description".into()],
            _ => {}
        }
        let options = InvocationOptions {
            state_root: Some(&files.root),
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: bytes,
        };
        let result = execute(args, &fields, &options, &control()?);
        assert!(!result.envelope.ok);
        assert_eq!(result.envelope.request_id, "original-request");
        assert!(!files.root.try_exists()?);
    }
    Ok(())
}

#[test]
fn exact_enabled_profile_configuration_controls_site_selection() -> TestResult {
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
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)
            .err()
            .ok_or("Expected missing configuration")?
            .code,
        ErrorCode::PolicyDenied
    );
    store.set_project_read_policy("factory", update(None, false), true, &control, |_| Ok(()))?;
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)
            .err()
            .ok_or("Expected disabled configuration")?
            .code,
        ErrorCode::PolicyDenied
    );
    store.set_project_read_policy("factory", update(Some(1), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?.grant(),
        &profile.grant
    );
    let mut other = args()?;
    other.cloud_id = Some("11111111-1111-2222-3333-444444444444".into());
    assert!(PreparedInvocation::prepare(other, &[], &options, &store, &control).is_err());
    Ok(())
}

#[test]
fn repository_profile_and_site_precedence_are_isolated_from_the_callers_working_directory()
-> TestResult {
    use craft_test_support::process::{RunOptions, run};
    if let Some(root) = std::env::var_os("CRAFT_PROJECT_CONTEXT_ROOT") {
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
        let mut chosen = args()?;
        chosen.cloud_id = Some(target().tenant);
        assert!(PreparedInvocation::prepare(chosen, &[], &inherited, &store, &control).is_ok());
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
    store.set_project_read_policy("factory", update(None, true), true, &control, |_| Ok(()))?;
    let repository = files.root.join("repository");
    fs::create_dir(&repository)?;
    fs::write(repository.join(".craft.json"), b"invalid repository JSON")?;
    let output = run(
        &std::env::current_exe()?,
        &[
            "--exact",
            "project_list::command::tests::repository_profile_and_site_precedence_are_isolated_from_the_callers_working_directory",
            "--nocapture",
        ],
        None,
        RunOptions {
            current_dir: Some(repository),
            environment: vec![(
                "CRAFT_PROJECT_CONTEXT_ROOT".into(),
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
