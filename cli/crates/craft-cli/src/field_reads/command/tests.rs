use super::*;
use clap::{CommandFactory, Parser};
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::field_read_policy::FieldReadPolicyUpdate;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct ListCandidate {
    #[command(flatten)]
    args: FieldListArgs,
}
#[derive(Parser)]
struct SearchCandidate {
    #[command(flatten)]
    args: FieldSearchArgs,
}
fn control() -> Result<InvocationControl, CraftError> {
    InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )
}
fn args() -> Result<FieldReadArgs, serde_json::Error> {
    serde_json::from_value(json!({})).map(FieldReadArgs::Search)
}
fn target() -> ResourceRef {
    site("00000000-1111-2222-3333-444444444444")
}
fn update(revision: Option<u64>, enabled: bool) -> FieldReadPolicyUpdate {
    FieldReadPolicyUpdate {
        target: target(),
        expected_actor: "actor-one".into(),
        continuation_site: "https://craft-test.atlassian.net".into(),
        operations: vec![FieldReadOperation::List, FieldReadOperation::Search],
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
            root: parent.join(format!("field-command-{}-{stamp}", std::process::id())),
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
fn field_flags_and_json_preserve_typed_defaults_repeated_filters_and_literal_commas() -> TestResult
{
    let mut registry = Registry::default();
    register_candidates(&mut registry)?;
    let list = json!({"cloud_id":target().tenant,"limit":7,"cursor":"cur_00000000000000000000000000000000"});
    let mut parser = ListCandidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &list)?;
    assert_eq!(
        encoded(&ListCandidate::try_parse_from(flags)?.args)?,
        encoded(&serde_json::from_value::<FieldListArgs>(list)?)?
    );
    let search = json!({"ids":["customfield_1","customfield_2"],"types":["custom","system"],"project_ids":[1,9223372036854775807_u64],"query":"literal, + & 文字","order_by":"-name","page_size":7,"limit":3});
    let mut parser = SearchCandidate::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &search)?;
    let parsed = SearchCandidate::try_parse_from(flags)?.args;
    assert_eq!(
        encoded(&parsed)?,
        encoded(&serde_json::from_value::<FieldSearchArgs>(search)?)?
    );
    assert_eq!(parsed.query.as_deref(), Some("literal, + & 文字"));
    assert_eq!(
        encoded(&SearchCandidate::try_parse_from(["candidate"])?.args)?,
        encoded(&serde_json::from_value::<FieldSearchArgs>(json!({}))?)?
    );
    assert_eq!(
        encoded(&ListCandidate::try_parse_from(["candidate"])?.args)?,
        encoded(&serde_json::from_value::<FieldListArgs>(json!({}))?)?
    );
    for input in [
        json!({"grant":"claimed"}),
        json!({"expected_actor":"claimed"}),
        json!({"continuation_site":"https://other.atlassian.net"}),
        json!({"project_ids":[1.0]}),
        json!({"ids":"customfield_1,customfield_2"}),
        json!({"expand":"all"}),
        json!({"startAt":1}),
        json!({"order_by":"key"}),
    ] {
        assert!(registry.validate(SEARCH, &input).is_err());
    }
    assert!(SearchCandidate::try_parse_from(["candidate", "--type", "custom,system"]).is_err());
    assert!(SearchCandidate::try_parse_from(["candidate", "--project-id", "1,2"]).is_err());
    for command in [LIST, SEARCH] {
        assert!(
            crate::catalog::local_registry()?
                .descriptor(command)
                .is_err()
        );
    }
    assert!(crate::args::Cli::try_parse_from(["craft", "jira", "field", "list"]).is_ok());
    Ok(())
}

#[test]
fn malformed_field_inputs_projection_and_recovery_budgets_create_no_state() -> TestResult {
    let files = Files::new()?;
    for mode in [
        "cloud",
        "cursor",
        "ids",
        "types",
        "project_ids",
        "query",
        "fields",
        "budget",
        "list_budget",
    ] {
        let mut search: FieldSearchArgs = serde_json::from_value(json!({}))?;
        let mut fields = vec![];
        match mode {
            "cloud" => search.cloud_id = Some("bad".into()),
            "cursor" => search.cursor = Some("bad".into()),
            "ids" => search.ids = vec!["customfield_01".into()],
            "types" => search.types = vec![FieldType::Custom, FieldType::Custom],
            "project_ids" => search.project_ids = vec![0],
            "query" => search.query = Some("x\n".into()),
            "fields" => fields = vec!["unsupported".into()],
            _ => {}
        }
        let args = if mode == "list_budget" {
            FieldReadArgs::List(serde_json::from_value(json!({}))?)
        } else {
            FieldReadArgs::Search(search)
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
        assert!(!files.root.try_exists()?);
    }
    Ok(())
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
fn field_candidates_require_separate_enabled_operation_configuration_and_capture_revisions()
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
    store.set_issue_read_policy(
        "factory",
        craft_runtime::issue_read_policy::IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![
                craft_runtime::issue_read_policy::IssueReadOperation::Get,
                craft_runtime::issue_read_policy::IssueReadOperation::Search,
            ],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
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
    store.set_field_read_policy("factory", update(None, false), true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let mut list_only = update(Some(1), true);
    list_only.operations = vec![FieldReadOperation::List];
    store.set_field_read_policy("factory", list_only, true, &control, |_| Ok(()))?;
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_err());
    let list = FieldReadArgs::List(serde_json::from_value(json!({}))?);
    let prepared = PreparedInvocation::prepare(list, &[], &options, &store, &control)?;
    assert_eq!(prepared.grant(), &store.get("factory", &control)?.grant);
    store.set_field_read_policy("factory", update(Some(2), true), true, &control, |_| Ok(()))?;
    assert_eq!(
        store
            .verify_field_read_policy(&prepared.policy, FieldReadOperation::List, &control)
            .err()
            .ok_or("Expected revision refusal")?
            .code,
        ErrorCode::VersionConflict
    );
    assert!(PreparedInvocation::prepare(args()?, &[], &options, &store, &control).is_ok());
    let other = FieldReadArgs::Search(serde_json::from_value(
        json!({"cloud_id":"11111111-1111-2222-3333-444444444444"}),
    )?);
    assert!(PreparedInvocation::prepare(other, &[], &options, &store, &control).is_err());
    Ok(())
}

#[test]
fn field_profile_selection_uses_explicit_repository_then_active_context() -> TestResult {
    use craft_test_support::process::{RunOptions, run};
    const TEST: &str = "field_reads::command::tests::field_profile_selection_uses_explicit_repository_then_active_context";
    if let Some(root) = std::env::var_os("CRAFT_FIELD_CONTEXT_ROOT") {
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
        let explicit_site =
            FieldReadArgs::Search(serde_json::from_value(json!({"cloud_id":target().tenant}))?);
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
        store.set_field_read_policy(name, update(None, true), true, &control, |_| Ok(()))?;
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
                "CRAFT_FIELD_CONTEXT_ROOT".into(),
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
