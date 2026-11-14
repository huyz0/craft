use super::*;
use clap::Parser;
use craft_core::control::CancellationToken;
use craft_core::profiles::ProfileInput;
use craft_runtime::confluence_write_policy::ConfluenceWritePolicyUpdate;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct CreateCli {
    #[command(flatten)]
    args: PageCreateArgs,
}
fn input() -> Value {
    json!({"operation_id":"publish-plan","apply":true,"title":"Plan draft","placement":{"kind":"root"},"body":{"representation":"storage","value":"<p>Review</p>"}})
}
fn options<'a>(root: Option<&'a std::path::Path>) -> InvocationOptions<'a> {
    InvocationOptions {
        state_root: root,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16384,
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
fn page_write_candidates_match_flags_schemas_and_strict_complex_inputs() -> TestResult {
    let cli = CreateCli::try_parse_from([
        "create",
        "--operation-id",
        "publish-plan",
        "--apply",
        "--title",
        "Plan draft",
        "--body",
        "{\"representation\":\"storage\",\"value\":\"<p>Review</p>\"}",
    ])?;
    let args = PageWriteArgs::Create(cli.args);
    args.validate(&[], &options(None))?;
    assert_eq!(
        args.arguments()?,
        PageWriteArgs::decode(PAGE_CREATE, input())?.arguments()?
    );
    assert!(
        CreateCli::try_parse_from([
            "create",
            "--body",
            "{\"representation\":\"storage\",\"value\":\"a\",\"value\":\"b\"}"
        ])
        .is_err()
    );
    let mut r = crate::catalog::local_registry()?;
    register_candidates(&mut r)?;
    for command in [PAGE_CREATE, PAGE_UPDATE] {
        let d = r.descriptor(command)?;
        r.validate(command, &d.example)?;
        assert!(jsonschema::validator_for(&d.response_schema)?.is_valid(&d.response_example));
        PageWriteArgs::decode(command, d.example.clone())?.validate(&[], &options(None))?;
        assert!(
            d.response_schema["properties"]["data"]["properties"]
                .get("receipt")
                .is_some()
        );
    }
    assert!(
        crate::catalog::local_registry()?
            .descriptor(PAGE_CREATE)
            .is_err()
    );
    Ok(())
}
#[test]
fn malformed_page_writes_fail_before_state_and_cannot_choose_identity_or_bypass_revision()
-> TestResult {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    let root = craft_test_support::state::fixture_root(
        &parent,
        "invalid-page-command",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let c = control()?;
    for (key, value) in [
        ("apply", json!(false)),
        ("operation_id", json!("")),
        ("space_id", json!("0100")),
        ("cloud_id", json!("wrong-site")),
        ("title", json!("")),
    ] {
        let mut v = input();
        v[key] = value;
        let result = execute(
            PageWriteArgs::decode(PAGE_CREATE, v)?,
            &[],
            &options(Some(&root)),
            &c,
        );
        assert!(!result.envelope.ok, "{key}");
        assert!(!root.exists());
    }
    for key in ["grant", "expected_actor", "status", "subtype", "url"] {
        let mut v = input();
        v[key] = json!("untrusted");
        assert!(PageWriteArgs::decode(PAGE_CREATE, v).is_err(), "{key}");
    }
    let mut tiny = options(Some(&root));
    tiny.max_bytes = 512;
    assert!(
        PageWriteArgs::decode(PAGE_CREATE, input())?
            .validate(&[], &tiny)
            .is_err()
    );
    assert!(
        PageWriteArgs::decode(PAGE_CREATE, input())?
            .validate(&["title".into()], &options(Some(&root)))
            .is_err()
    );
    assert!(!root.exists());
    Ok(())
}
struct Files {
    root: std::path::PathBuf,
    parent: std::path::PathBuf,
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
#[test]
fn normal_profile_and_space_selection_require_separate_captured_write_authority() -> TestResult {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let files = Files {
        root: craft_test_support::state::fixture_root(
            &parent,
            "page-command-policy",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        ),
        parent,
    };
    let c = control()?;
    let store = ProfileStore::open(&files.root, &c)?;
    let target = site("00000000-1111-2222-3333-444444444444");
    let space = ResourceRef {
        resource_type: "space".into(),
        id: "100".into(),
        ..target.clone()
    };
    let profile = store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "confluence".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(target.tenant.clone()),
            defaults: BTreeMap::from([("space".into(), space.clone())]),
        },
        &c,
        |_| Ok(()),
    )?;
    let o = options(Some(&files.root));
    let args = || PageWriteArgs::decode(PAGE_CREATE, input());
    store.set_confluence_read_policy(
        "factory",
        craft_runtime::confluence_read_policy::ConfluenceReadPolicyUpdate {
            target: target.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
            operations: vec![
                craft_runtime::confluence_read_policy::ConfluenceReadOperation::PageGet,
            ],
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    assert!(PreparedInvocation::prepare(args()?, &[], &o, &store, &c).is_err());
    store.set_confluence_write_policy(
        "factory",
        ConfluenceWritePolicyUpdate {
            target: space.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![Operation::Create],
            expected_revision: None,
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    let prepared = PreparedInvocation::prepare(args()?, &[], &o, &store, &c)?;
    assert_eq!(prepared.grant(), &profile.grant);
    assert_eq!(prepared.policy.target, space);
    for (key, value) in [
        ("space_id", "200"),
        ("cloud_id", "11111111-1111-2222-3333-444444444444"),
    ] {
        let mut v = input();
        v[key] = json!(value);
        assert!(
            PreparedInvocation::prepare(
                PageWriteArgs::decode(PAGE_CREATE, v)?,
                &[],
                &o,
                &store,
                &c
            )
            .is_err()
        );
    }
    store.set_confluence_write_policy(
        "factory",
        ConfluenceWritePolicyUpdate {
            target: space,
            expected_actor: "actor-one".into(),
            enabled: false,
            operations: vec![Operation::Create],
            expected_revision: Some(1),
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    assert_eq!(
        store
            .verify_confluence_write_policy(&prepared.policy, Operation::Create, &c)
            .err()
            .ok_or("revoked")?
            .code,
        ErrorCode::PolicyDenied
    );
    Ok(())
}
