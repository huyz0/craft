use super::*;
use clap::{CommandFactory, Parser};
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::project_read_policy::{ProjectContextPermissionUpdate, ProjectReadPolicyUpdate};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct Get {
    #[command(flatten)]
    args: ProjectGetArgs,
}
#[derive(Parser)]
struct Statuses {
    #[command(flatten)]
    args: ProjectStatusesArgs,
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn project(id: &str) -> ResourceRef {
    target("00000000-1111-2222-3333-444444444444", id)
}
fn args() -> Result<ProjectContextArgs, serde_json::Error> {
    serde_json::from_value(json!({})).map(ProjectContextArgs::Statuses)
}
fn site_update() -> ProjectReadPolicyUpdate {
    ProjectReadPolicyUpdate {
        target: ResourceRef {
            resource_type: "site".into(),
            id: project("10000").tenant.clone(),
            ..project("10000")
        },
        expected_actor: "actor-one".into(),
        continuation_site: "https://craft-test.atlassian.net".into(),
        enabled: false,
        expected_revision: None,
    }
}
fn permission(
    id: &str,
    revision: u64,
    operations: Vec<ProjectContextOperation>,
) -> ProjectContextPermissionUpdate {
    ProjectContextPermissionUpdate {
        target: project(id),
        operations,
        enabled: true,
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
            root: parent.join(format!(
                "project-context-command-{}-{stamp}",
                std::process::id()
            )),
            parent,
        })
    }
    fn setup(&self) -> Result<ProfileStore, Box<dyn std::error::Error>> {
        let control = control()?;
        let store = ProfileStore::open(&self.root, &control)?;
        store.create(
            &ProfileInput {
                name: "factory".into(),
                provider: "jira".into(),
                deployment: "cloud".into(),
                auth_mode: AuthMode::Service,
                grant: None,
                tenant: Some(project("10000").tenant),
                defaults: BTreeMap::from([("project".into(), project("10000"))]),
            },
            &control,
            |_| Ok(()),
        )?;
        Ok(store)
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
fn project_context_flags_json_and_defaults_are_typed_without_authority_fields() -> TestResult {
    let mut registry = Registry::default();
    register_candidates(&mut registry)?;
    let example = json!({"project":"ED","cloud_id":project("ED").tenant});
    let mut parser = Get::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &example)?;
    assert_eq!(
        value(&Get::try_parse_from(flags)?.args)?,
        value(&serde_json::from_value::<ProjectGetArgs>(example)?)?
    );
    let example = json!({"project":"10000","cloud_id":project("10000").tenant,"limit":7,"cursor":"cur_00000000000000000000000000000000"});
    let mut parser = Statuses::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &example)?;
    assert_eq!(
        value(&Statuses::try_parse_from(flags)?.args)?,
        value(&serde_json::from_value::<ProjectStatusesArgs>(example)?)?
    );
    assert_eq!(
        value(&Statuses::try_parse_from(["candidate"])?.args)?,
        value(&serde_json::from_value::<ProjectStatusesArgs>(json!({}))?)?
    );
    for bad in [
        json!({"expected_actor":"claimed"}),
        json!({"grant":"claimed"}),
        json!({"limit":"20"}),
        json!({"limit":0}),
        json!({"expand":"lead"}),
    ] {
        assert!(registry.validate(STATUSES, &bad).is_err());
    }
    assert!(
        registry
            .validate(GET, &json!({"project":"ED","cursor":"opaque"}))
            .is_err()
    );
    for command in [GET, STATUSES] {
        assert!(
            crate::catalog::local_registry()?
                .descriptor(command)
                .is_err()
        );
    }
    for command in ["get", "statuses"] {
        assert!(
            crate::args::Cli::try_parse_from([
                "craft",
                "jira",
                "project",
                command,
                "--project",
                "ED"
            ])
            .is_ok()
        );
    }
    Ok(())
}

#[test]
fn invalid_project_context_input_projection_cursor_and_budget_create_no_state() -> TestResult {
    let files = Files::new()?;
    for mode in ["project", "cloud", "cursor", "fields", "budget"] {
        let mut args: ProjectStatusesArgs = serde_json::from_value(json!({}))?;
        let mut fields = Vec::new();
        match mode {
            "project" => args.project = Some("ED/other".into()),
            "cloud" => args.cloud_id = Some("not-a-uuid".into()),
            "cursor" => args.cursor = Some("bad".into()),
            "fields" => fields = vec!["lead.email".into()],
            _ => {}
        }
        let options = InvocationOptions {
            state_root: Some(&files.root),
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: if mode == "budget" { 512 } else { 16_384 },
        };
        let result = execute(
            ProjectContextArgs::Statuses(args),
            &fields,
            &options,
            &control()?,
        );
        assert!(!result.envelope.ok);
        assert_eq!(result.envelope.request_id, "original-request");
        assert!(!files.root.try_exists()?);
    }
    Ok(())
}

#[test]
fn exact_operation_permission_controls_default_and_explicit_project_selection() -> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let options = InvocationOptions {
        state_root: Some(&files.root),
        profile: Some("factory"),
        request_id: "request",
        max_bytes: 16_384,
    };
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let old =
        store.set_project_read_policy("factory", site_update(), true, &control, |_| Ok(()))?;
    assert_eq!(old.format, 1);
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    store.set_project_context_permission(
        "factory",
        permission("10000", 1, vec![ProjectContextOperation::Statuses]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)?.grant(),
        &store.get("factory", &control)?.grant
    );
    assert!(
        PreparedInvocation::prepare(
            ProjectContextArgs::Get(serde_json::from_value(json!({}))?),
            &[],
            &options,
            &store,
            &control
        )
        .is_err()
    );
    store.set_project_context_permission(
        "factory",
        permission("ED", 2, vec![ProjectContextOperation::Get]),
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(
        PreparedInvocation::prepare(
            ProjectContextArgs::Get(serde_json::from_value(json!({"project":"ED"}))?),
            &[],
            &options,
            &store,
            &control
        )
        .is_ok()
    );
    for input in [
        json!({"project":"ed"}),
        json!({"project":"OTHER"}),
        json!({"cloud_id":"11111111-1111-2222-3333-444444444444"}),
    ] {
        assert!(
            PreparedInvocation::prepare(
                ProjectContextArgs::Get(serde_json::from_value(input)?),
                &[],
                &options,
                &store,
                &control
            )
            .is_err()
        );
    }
    store.create(
        &ProfileInput {
            name: "without-default".into(),
            provider: "jira".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(project("10000").tenant),
            defaults: BTreeMap::new(),
        },
        &control,
        |_| Ok(()),
    )?;
    let options = InvocationOptions {
        profile: Some("without-default"),
        ..options
    };
    assert_eq!(
        PreparedInvocation::prepare(args()?, &[], &options, &store, &control)
            .err()
            .ok_or("Expected missing project context")?
            .code,
        ErrorCode::TargetAmbiguous
    );
    Ok(())
}

#[test]
fn project_context_precedence_is_verified_in_an_isolated_repository_process() -> TestResult {
    use craft_test_support::process::{RunOptions, run};
    if let Some(root) = std::env::var_os("CRAFT_PROJECT_READ_CONTEXT_ROOT") {
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
        fs::write(
            ".craft.json",
            serde_json::to_vec(
                &json!({"profile":"factory","targets":{"project":project("OTHER")}}),
            )?,
        )?;
        assert!(PreparedInvocation::prepare(args()?, &[], &inherited, &store, &control).is_err());
        let chosen =
            ProjectContextArgs::Statuses(serde_json::from_value(json!({"project":"10000"}))?);
        assert!(PreparedInvocation::prepare(chosen, &[], &inherited, &store, &control).is_ok());
        let mut foreign = project("10000");
        foreign.tenant = "11111111-1111-2222-3333-444444444444".into();
        fs::write(
            ".craft.json",
            serde_json::to_vec(&json!({"profile":"factory","targets":{"project":foreign}}))?,
        )?;
        assert!(PreparedInvocation::prepare(args()?, &[], &inherited, &store, &control).is_err());
        fs::write(".craft.json", br#"{"profile":"missing"}"#)?;
        assert!(PreparedInvocation::prepare(args()?, &[], &inherited, &store, &control).is_err());
        assert!(PreparedInvocation::prepare(args()?, &[], &explicit, &store, &control).is_ok());
        return Ok(());
    }
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    store.use_profile("factory", &control, |_| Ok(()))?;
    store.set_project_read_policy("factory", site_update(), true, &control, |_| Ok(()))?;
    store.set_project_context_permission(
        "factory",
        permission(
            "10000",
            1,
            vec![
                ProjectContextOperation::Get,
                ProjectContextOperation::Statuses,
            ],
        ),
        true,
        &control,
        |_| Ok(()),
    )?;
    let repository = files.root.join("repository");
    fs::create_dir(&repository)?;
    fs::write(repository.join(".craft.json"), b"invalid JSON")?;
    let output = run(
        &std::env::current_exe()?,
        &[
            "--exact",
            "project_context::command::tests::project_context_precedence_is_verified_in_an_isolated_repository_process",
            "--nocapture",
        ],
        None,
        RunOptions {
            current_dir: Some(repository),
            environment: vec![(
                "CRAFT_PROJECT_READ_CONTEXT_ROOT".into(),
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
