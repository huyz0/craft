use super::*;
use craft_core::control::CancellationToken;
use craft_core::storage::SecretBytes;
use serde_json::json;
use std::cell::Cell;

#[derive(clap::Parser)]
struct CandidateArguments {
    #[command(flatten)]
    arguments: ProvisionArgs,
}

#[test]
fn candidate_flags_and_json_agree_for_multiple_profiles_and_strict_source_selection()
-> Result<(), Box<dyn std::error::Error>> {
    use clap::{CommandFactory, Parser};
    let mut registry = Registry::default();
    register_candidate(&mut registry)?;
    let descriptor = registry.descriptor(COMMAND)?;
    let mut example = descriptor.example.clone();
    example["profiles"] = json!(["factory", "wiki"]);
    example["replace"] = json!(true);
    let mut parser = CandidateArguments::command();
    parser.build();
    let flags = crate::contracts::example_arguments(&[], &parser, &example)?;
    let parsed = CandidateArguments::try_parse_from(&flags)?.arguments;
    let actual = serde_json::to_value(parsed)?;
    registry.validate(COMMAND, &actual)?;
    let expected: ProvisionArgs = serde_json::from_value(example.clone())?;
    assert_eq!(actual, serde_json::to_value(expected)?);
    let mut conflict = flags.clone();
    conflict.push("--interactive".into());
    assert!(CandidateArguments::try_parse_from(conflict).is_err());
    for input in [
        json!({"profiles":["factory"],"expected_actor":"actor"}),
        json!({"profiles":["factory"],"expected_actor":"actor","interactive":true,"secret_stdin":true}),
        json!({"profiles":[],"expected_actor":"actor","secret_stdin":true}),
        json!({"profiles":["factory"],"expected_actor":"actor","secret_stdin":true,"client_secret":"synthetic-secret"}),
    ] {
        assert!(registry.validate(COMMAND, &input).is_err());
    }
    example["interactive"] = json!(true);
    example
        .as_object_mut()
        .ok_or("Missing argument object")?
        .remove("secret_stdin");
    registry.validate(COMMAND, &example)?;
    assert!(
        crate::catalog::local_registry()?
            .descriptor(COMMAND)
            .is_err()
    );
    assert!(crate::args::Cli::try_parse_from(["craft", "auth", "provision"]).is_ok());
    Ok(())
}

#[test]
fn dispatch_preserves_request_and_stdin_refusal_before_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = crate::catalog::local_registry()?;
    register_candidate(&mut registry)?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!(
            "provision-refused-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
    assert!(!root.exists());
    let invocation = InvocationOptions {
        state_root: Some(&root),
        ..options(16_384)
    };
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let result = crate::dispatch::execute(
        &registry,
        crate::args::Action::Auth {
            action: crate::args::AuthAction::Provision(args()),
        },
        &[],
        &control,
        &invocation,
        true,
    )?;
    assert!(!root.exists());
    assert!(!result.envelope.ok);
    assert_eq!(result.envelope.command, COMMAND);
    assert_eq!(result.envelope.request_id, invocation.request_id);
    assert!(
        result
            .envelope
            .error
            .as_ref()
            .ok_or("Missing refusal")?
            .details
            .issues
            .iter()
            .any(|issue| issue.path == "/secret_stdin")
    );
    let encoded =
        crate::output::encode(result.envelope, invocation.max_bytes, result.output_policy)?;
    assert_eq!(encoded.exit, ErrorCode::InputInvalid.exit_code());
    let body: Value = serde_json::from_slice(&encoded.bytes)?;
    assert_eq!(body["ok"], false);
    assert_eq!(body["request_id"], invocation.request_id);
    Ok(())
}

fn args() -> ProvisionArgs {
    ProvisionArgs {
        profiles: vec!["factory".into()],
        expected_actor: "service-actor".into(),
        interactive: false,
        secret_stdin: true,
        replace: false,
    }
}
fn context() -> Context {
    Context {
        data: ProvisionData {
            grant: GrantRef {
                id: "gr_00000000000000000000000000000000".into(),
            },
            profiles: vec!["factory".into()],
            shared_profiles_observed: vec!["factory".into(), "wiki".into()],
            credential_write_state: RequestState::NotSent,
            cleanup_pending: None,
            retry_after: None,
            token: None,
            verified_products: None,
        },
        targets: vec![TenantRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
        }],
        actor: "service-actor".into(),
    }
}
fn options(maximum: usize) -> InvocationOptions<'static> {
    InvocationOptions {
        state_root: None,
        profile: None,
        request_id: "request-provision",
        max_bytes: maximum,
    }
}
fn credentials() -> Result<ServiceCredentials, CraftError> {
    ServiceCredentials::from_provisioning(
        SecretBytes::new(
            br#"{"client_id":"synthetic-id","client_secret":"synthetic-secret"}"#.to_vec(),
        )
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Fixture failed"))?,
    )
}
struct FakeBackend {
    mode: &'static str,
    writes: usize,
    active: bool,
    pending: bool,
}
impl FakeBackend {
    fn new(mode: &'static str) -> Self {
        Self {
            mode,
            writes: 0,
            active: false,
            pending: false,
        }
    }
}
impl Backend for FakeBackend {
    fn state(&self) -> Result<(bool, bool), CraftError> {
        Ok((self.active, self.pending))
    }
    fn activate(
        &mut self,
        _: ServiceCredentials,
        targets: &[TenantRef],
        actor: &str,
        preflight: &mut ProvisionPreflight<'_>,
    ) -> Result<bool, AuthFailure> {
        if self.mode == "token-unknown" {
            let mut failure = AuthFailure::from(CraftError::new(
                ErrorCode::RemoteUnavailable,
                "synthetic-secret",
            ));
            failure.error.details.write_state = RequestState::SentUnknown;
            failure.retry_after = Some(RetryAfter::DelaySeconds(7));
            return Err(failure);
        }
        if self.mode == "missing-preflight" {
            self.writes += 1;
            return Ok(false);
        }
        let metadata = TokenMetadata {
            issued_at_unix_ms: 1_000,
            expires_at_unix_ms: 3_601_000,
            reacquire_at_unix_ms: 3_541_000,
            scopes: Some(if self.mode == "large" {
                (0..100)
                    .map(|index| format!("read:scope:{index}:{}", "x".repeat(64)))
                    .collect()
            } else {
                vec!["read:jira-user".into()]
            }),
        };
        let proofs: Vec<_> = targets
            .iter()
            .map(|target| ProductProof {
                tenant: target.clone(),
                actor: if self.mode == "wrong-proof" {
                    "other-actor"
                } else {
                    actor
                }
                .into(),
                verified_at_unix_ms: 1_000,
            })
            .collect();
        preflight(&metadata, &proofs)?;
        self.writes += 1;
        if self.mode == "storage-unknown" {
            let mut failure = AuthFailure::from(CraftError::new(
                ErrorCode::WriteOutcomeUnknown,
                "synthetic-secret",
            ));
            failure.error.details.write_state = RequestState::SentUnknown;
            failure.credential_write_state = Some(RequestState::SentUnknown);
            return Err(failure);
        }
        Ok(self.mode == "cleanup-pending")
    }
}

#[test]
fn provisioning_freezes_matching_selected_success_and_preserves_cleanup_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["success", "cleanup-pending"] {
        let mut backend = FakeBackend::new(mode);
        let projection = projection(&["token.expires_at_unix_ms".into()])?;
        let response = run(
            &args(),
            context(),
            &mut backend,
            credentials,
            &projection,
            &options(16_384),
        )?;
        assert!(response.ok);
        assert_eq!(backend.writes, 1);
        let encoded = serde_json::to_value(&response)?;
        assert_eq!(encoded["data"]["credential_write_state"], "confirmed");
        assert_eq!(
            encoded["data"]["cleanup_pending"],
            mode == "cleanup-pending"
        );
        assert_eq!(encoded["data"]["token"]["expires_at_unix_ms"], 3_601_000);
        assert!(encoded["data"]["token"].get("scopes").is_none());
        let schema = projection.response_schema(&response_schema::<ProvisionData>()?)?;
        assert!(jsonschema::validator_for(&schema)?.is_valid(&encoded));
        assert!(!serde_json::to_string(&response)?.contains("synthetic-secret"));
    }
    Ok(())
}

#[test]
fn output_and_replacement_preconditions_fail_before_secret_input_or_activation()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["budget", "active", "pending"] {
        let mut backend = FakeBackend::new("success");
        backend.active = mode == "active";
        backend.pending = mode == "pending";
        let mut context = context();
        if mode == "budget" {
            context.data.profiles = (0..32)
                .map(|index| format!("{index}{}", "x".repeat(62)))
                .collect();
        }
        let reads = Cell::new(0);
        let error = run(
            &args(),
            context,
            &mut backend,
            || {
                reads.set(reads.get() + 1);
                credentials()
            },
            &projection(&[])?,
            &options(512),
        )
        .err()
        .ok_or("Expected local refusal")?;
        assert!(matches!(
            error.code,
            ErrorCode::VersionConflict | ErrorCode::OutputBudgetTooSmall
        ));
        assert_eq!(reads.get(), 0);
        assert_eq!(backend.writes, 0);
    }
    Ok(())
}

#[test]
fn oversized_output_and_wrong_proofs_refuse_activation_after_qualification()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["large", "wrong-proof"] {
        let mut backend = FakeBackend::new(mode);
        let response = run(
            &args(),
            context(),
            &mut backend,
            credentials,
            &projection(&["token.scopes".into()])?,
            &options(1_024),
        )?;
        assert!(!response.ok);
        assert_eq!(backend.writes, 0);
        assert_eq!(
            response.data.as_ref().ok_or("Expected recovery data")?["credential_write_state"],
            "not_sent"
        );
        assert!(fits(&response, 1_024));
    }
    Ok(())
}

#[test]
fn token_and_activation_uncertainty_stay_distinct_with_rate_advice_and_no_secret_echo()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["token-unknown", "storage-unknown", "missing-preflight"] {
        let mut backend = FakeBackend::new(mode);
        let selected = projection(&["token.expires_at_unix_ms".into()])?;
        let response = run(
            &args(),
            context(),
            &mut backend,
            credentials,
            &selected,
            &options(1_024),
        )?;
        assert!(!response.ok);
        assert!(fits(&response, 1_024));
        let encoded = serde_json::to_value(response)?;
        assert!(!encoded.to_string().contains("synthetic-secret"));
        assert_eq!(
            encoded["data"]["credential_write_state"],
            match mode {
                "token-unknown" => "not_sent",
                "storage-unknown" => "sent_unknown",
                _ => "confirmed",
            }
        );
        if mode == "token-unknown" {
            assert_eq!(encoded["data"]["retry_after"]["delay_seconds"], 7);
            assert_eq!(backend.writes, 0);
        }
        assert!(
            jsonschema::validator_for(
                &selected.response_schema(&response_schema::<ProvisionData>()?)?
            )?
            .is_valid(&encoded)
        );
    }
    Ok(())
}

#[test]
fn provisioning_argument_contract_refuses_secret_properties_ambiguous_sources_and_implicit_context()
-> Result<(), Box<dyn std::error::Error>> {
    assert!(serde_json::from_value::<ProvisionArgs>(json!({"profiles":["factory"],"expected_actor":"service-actor","secret_stdin":true,"client_secret":"synthetic-secret"})).is_err());
    let mut bad = args();
    bad.interactive = true;
    assert!(validate_arguments(&bad, &options(16_384), false).is_err());
    let mut bad = args();
    bad.profiles.push("factory".into());
    assert!(validate_arguments(&bad, &options(16_384), false).is_err());
    assert!(validate_arguments(&args(), &options(16_384), true).is_err());
    let mut selected = options(16_384);
    selected.profile = Some("factory");
    assert!(validate_arguments(&args(), &selected, false).is_err());
    let public = crate::catalog::local_registry()?;
    assert!(public.descriptor(COMMAND).is_err());
    Ok(())
}

#[test]
fn trusted_profile_selection_requires_service_mode_shared_grant_and_real_cloud_identifiers()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_core::profiles::ProfileInput;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Cleanup {
        parent: PathBuf,
        path: PathBuf,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(path) = self.path.canonicalize()
                && path.starts_with(&self.parent)
                && path != self.parent
            {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
    struct Fixture {
        store: ProfileStore,
        _cleanup: Cleanup,
    }
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = parent.join(format!(
        "provision-selection-{}-{stamp}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let fixture = Fixture {
        store: ProfileStore::open(&root, &control)?,
        _cleanup: Cleanup { parent, path: root },
    };
    let create = |name: &str,
                  provider: &str,
                  grant: Option<GrantRef>,
                  mode: AuthMode,
                  tenant: Option<&str>| {
        fixture.store.create(
            &ProfileInput {
                name: name.into(),
                provider: provider.into(),
                deployment: "cloud".into(),
                auth_mode: mode,
                grant,
                tenant: tenant.map(str::to_owned),
                defaults: Default::default(),
            },
            &control,
            |_| Ok(()),
        )
    };
    let cloud = "00000000-1111-2222-3333-444444444444";
    let factory = create("factory", "jira", None, AuthMode::Service, Some(cloud))?;
    create(
        "narrow",
        "jira",
        Some(factory.grant.clone()),
        AuthMode::Service,
        Some(cloud),
    )?;
    create(
        "wiki",
        "confluence",
        Some(factory.grant.clone()),
        AuthMode::Service,
        Some(cloud),
    )?;
    create("other", "jira", None, AuthMode::Service, Some(cloud))?;
    create("personal", "jira", None, AuthMode::Delegated, Some(cloud))?;
    create(
        "unselected",
        "jira",
        Some(factory.grant.clone()),
        AuthMode::Service,
        None,
    )?;
    create(
        "guessed",
        "jira",
        Some(factory.grant.clone()),
        AuthMode::Service,
        Some("guessed-site"),
    )?;
    let mut request = args();
    request.profiles = vec!["factory".into(), "narrow".into(), "wiki".into()];
    let selected = super::context(&fixture.store, &request, &control)?;
    assert_eq!(selected.targets.len(), 2);
    assert_eq!(selected.data.grant, factory.grant);
    assert!(
        selected
            .data
            .shared_profiles_observed
            .contains(&"unselected".into())
    );
    assert!(
        !selected
            .data
            .shared_profiles_observed
            .contains(&"other".into())
    );
    for profiles in [
        vec!["factory", "other"],
        vec!["personal"],
        vec!["unselected"],
        vec!["guessed"],
    ] {
        request.profiles = profiles.into_iter().map(str::to_owned).collect();
        assert!(super::context(&fixture.store, &request, &control).is_err());
    }
    Ok(())
}
