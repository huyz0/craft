use super::*;
use clap::Parser;
use craft_core::control::CancellationToken;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[derive(Parser)]
struct GetCli {
    #[command(flatten)]
    args: ConfluenceGetArgs,
}
#[derive(Parser)]
struct PagesCli {
    #[command(flatten)]
    args: ConfluencePageListArgs,
}
#[derive(Parser)]
struct SearchCli {
    #[command(flatten)]
    args: ConfluenceSearchArgs,
}

#[test]
fn confluence_profile_resolution_requires_exact_separate_operation_authority() -> TestResult {
    use craft_core::profiles::ProfileInput;
    use craft_runtime::confluence_read_policy::ConfluenceReadPolicyUpdate;
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let root = craft_test_support::state::fixture_root(
        &parent,
        "confluence-command-policy",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )?;
    let store = ProfileStore::open(&root, &control)?;
    let target = site("00000000-1111-2222-3333-444444444444");
    let profile = store.create(
        &ProfileInput {
            name: "factory".into(),
            provider: "confluence".into(),
            deployment: "cloud".into(),
            auth_mode: AuthMode::Service,
            grant: None,
            tenant: Some(target.tenant.clone()),
            defaults: std::collections::BTreeMap::from([(
                "space".into(),
                ResourceRef {
                    resource_type: "space".into(),
                    id: "100".into(),
                    ..target.clone()
                },
            )]),
        },
        &control,
        |_| Ok(()),
    )?;
    let options = InvocationOptions {
        state_root: Some(&root),
        profile: Some("factory"),
        request_id: "request",
        max_bytes: 16384,
    };
    let get = || ConfluenceReadArgs::decode(PAGE_GET, json!({"id":"100"}));
    assert!(PreparedInvocation::prepare(get()?, &[], &options, &store, &control).is_err());
    let record = store.set_confluence_read_policy(
        "factory",
        ConfluenceReadPolicyUpdate {
            target: target.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![Operation::PageGet],
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let prepared = PreparedInvocation::prepare(get()?, &[], &options, &store, &control)?;
    assert_eq!(prepared.grant(), &profile.grant);
    assert!(
        PreparedInvocation::prepare(
            ConfluenceReadArgs::decode(SPACE_GET, json!({"id":"100"}))?,
            &[],
            &options,
            &store,
            &control
        )
        .is_err()
    );
    assert!(
        PreparedInvocation::prepare(
            ConfluenceReadArgs::decode(
                PAGE_GET,
                json!({"id":"100","cloud_id":"11111111-1111-2222-3333-444444444444"})
            )?,
            &[],
            &options,
            &store,
            &control
        )
        .is_err()
    );
    store.set_confluence_read_policy(
        "factory",
        ConfluenceReadPolicyUpdate {
            target,
            expected_actor: "actor-one".into(),
            enabled: false,
            operations: vec![Operation::PageGet],
            expected_revision: Some(record.revision),
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    assert!(
        store
            .verify_confluence_read_policy(&prepared.policy, Operation::PageGet, &control)
            .is_err()
    );
    drop(prepared);
    drop(store);
    let resolved = root.canonicalize()?;
    assert!(resolved.starts_with(&parent) && resolved != parent);
    std::fs::remove_dir_all(resolved)?;
    Ok(())
}
#[test]
fn confluence_flags_json_and_candidate_schemas_agree_on_filters_and_defaults() -> TestResult {
    let get = GetCli::try_parse_from(["candidate", "--id", "100"])?;
    assert_eq!(encoded(&get.args)?, json!({"cloud_id":null,"id":"100"}));
    let pages = PagesCli::try_parse_from([
        "candidate",
        "--space-id",
        "100",
        "--status",
        "archived",
        "--page-size",
        "50",
        "--limit",
        "10",
    ])?;
    let decoded: ConfluencePageListArgs = serde_json::from_value(
        json!({"space_id":"100","status":"archived","page_size":50,"limit":10}),
    )?;
    assert_eq!(encoded(&pages.args)?, encoded(&decoded)?);
    let search = SearchCli::try_parse_from(["candidate", "--cql", "type = page\nAND space = ENG"])?;
    assert_eq!(search.args.cql, "type = page\nAND space = ENG");
    let mut registry = crate::catalog::local_registry()?;
    register_candidates(&mut registry)?;
    for command in [SPACE_GET, SPACE_LIST, PAGE_GET, PAGE_LIST, PAGE_SEARCH] {
        let descriptor = registry.descriptor(command)?;
        registry.validate(command, &descriptor.example)?;
        assert!(
            jsonschema::validator_for(&descriptor.response_schema)?
                .is_valid(&descriptor.response_example)
        );
    }
    assert!(
        serde_json::from_value::<ConfluenceGetArgs>(json!({"id":"100","actor":"injected"}))
            .is_err()
    );
    assert!(serde_json::from_value::<ConfluenceSearchArgs>(json!({"cql":null})).is_err());
    assert!(PagesCli::try_parse_from(["candidate", "--status", "draft"]).is_err());
    Ok(())
}
#[test]
fn invalid_confluence_input_fields_cursor_and_output_budget_open_no_private_state() -> TestResult {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let root = craft_test_support::state::fixture_root(
        &parent,
        "invalid-confluence-command",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        15_000,
    )?;
    for (command, input, fields, maximum) in [
        (PAGE_GET, json!({"id":"ENG"}), vec![], 16384),
        (PAGE_SEARCH, json!({"cql":" "}), vec![], 16384),
        (PAGE_LIST, json!({"cursor":"bad"}), vec![], 16384),
        (SPACE_LIST, json!({}), vec!["ownerId".into()], 16384),
        (PAGE_GET, json!({"id":"100"}), vec![], 512),
        (
            PAGE_GET,
            json!({"id":"100"}),
            vec!["body_storage".into(), "body_atlas_doc_format".into()],
            16384,
        ),
    ] {
        let args = ConfluenceReadArgs::decode(command, input)?;
        let options = InvocationOptions {
            state_root: Some(&root),
            profile: Some("factory"),
            request_id: "request",
            max_bytes: maximum,
        };
        let result = execute(args, &fields, &options, &control);
        assert!(!result.envelope.ok);
        assert!(!root.exists());
    }
    Ok(())
}
