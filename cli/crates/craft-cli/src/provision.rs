//! Service provisioning with explicit targets and frozen activation/recovery output.

use clap::Args;
use craft_core::contracts::{AuthMode, EffectClass, Envelope, GrantRef, RequestState, TenantRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::RetryAfter;
use craft_core::profiles::validate_profile_name;
use craft_core::projection::{CompiledProjection, ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_runtime::auth::{
    AuthFailure, ProductIdentity, ServiceCredentials, ServiceOAuthClient, TokenMetadata,
    validate_service_target,
};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;
use std::sync::Arc;

use crate::profiles::InvocationOptions;

const COMMAND: &str = "auth.provision";

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("oneOf" = [
    {"properties":{"interactive":{"const":true},"secret_stdin":{"const":false}},"required":["interactive"]},
    {"properties":{"interactive":{"const":false},"secret_stdin":{"const":true}},"required":["secret_stdin"]}
]))]
pub struct ProvisionArgs {
    #[arg(long, value_delimiter = ',', num_args = 1)]
    #[schemars(length(min = 1, max = 32))]
    pub profiles: Vec<String>,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 512))]
    pub expected_actor: String,
    #[arg(long, conflicts_with = "secret_stdin")]
    #[serde(default)]
    pub interactive: bool,
    #[arg(long, conflicts_with = "interactive")]
    #[serde(default)]
    pub secret_stdin: bool,
    #[arg(long)]
    #[serde(default)]
    pub replace: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductProof {
    pub tenant: TenantRef,
    pub actor: String,
    pub verified_at_unix_ms: u64,
}
impl From<&ProductIdentity> for ProductProof {
    fn from(value: &ProductIdentity) -> Self {
        Self {
            tenant: value.tenant().clone(),
            actor: value.actor().into(),
            verified_at_unix_ms: value.verified_at_unix_ms(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvisionTokenInfo {
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub reacquire_at_unix_ms: u64,
    pub scopes: Option<Vec<String>>,
}
impl From<&TokenMetadata> for ProvisionTokenInfo {
    fn from(value: &TokenMetadata) -> Self {
        Self {
            issued_at_unix_ms: value.issued_at_unix_ms,
            expires_at_unix_ms: value.expires_at_unix_ms,
            reacquire_at_unix_ms: value.reacquire_at_unix_ms,
            scopes: value.scopes.clone(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryAdvice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_unix_ms: Option<u64>,
}
impl From<RetryAfter> for RetryAdvice {
    fn from(value: RetryAfter) -> Self {
        match value {
            RetryAfter::DelaySeconds(seconds) => Self {
                delay_seconds: Some(seconds),
                at_unix_ms: None,
            },
            RetryAfter::AtUnixMillis(time) => Self {
                delay_seconds: None,
                at_unix_ms: Some(time),
            },
        }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvisionData {
    pub grant: GrantRef,
    pub profiles: Vec<String>,
    pub shared_profiles_observed: Vec<String>,
    pub credential_write_state: RequestState,
    pub cleanup_pending: Option<bool>,
    pub retry_after: Option<RetryAdvice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<ProvisionTokenInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_products: Option<Vec<ProductProof>>,
}

struct Context {
    data: ProvisionData,
    targets: Vec<TenantRef>,
    actor: String,
}

fn validate_arguments(
    args: &ProvisionArgs,
    options: &InvocationOptions<'_>,
    stdin_consumed: bool,
) -> Result<(), CraftError> {
    if options.profile.is_some() {
        return Err(CraftError::field(
            "/profile",
            "use_explicit_profiles_for_provisioning",
        ));
    }
    if args.profiles.is_empty() || args.profiles.len() > 32 {
        return Err(CraftError::field(
            "/profiles",
            "expected_bounded_explicit_profiles",
        ));
    }
    for (index, name) in args.profiles.iter().enumerate() {
        validate_profile_name(name)?;
        if args.profiles[..index].contains(name) {
            return Err(CraftError::field("/profiles", "duplicate_profile"));
        }
    }
    craft_core::contracts::validate_identity("/expected_actor", &args.expected_actor)?;
    if matches!(args.expected_actor.as_str(), "unknown" | "anonymous") {
        return Err(CraftError::field(
            "/expected_actor",
            "expected_service_actor",
        ));
    }
    if args.interactive == args.secret_stdin {
        return Err(CraftError::field(
            "/interactive",
            "select_one_secret_source",
        ));
    }
    if stdin_consumed && args.secret_stdin {
        return Err(CraftError::field(
            "/secret_stdin",
            "stdin_can_only_be_consumed_once",
        ));
    }
    if !(512..=16_777_216).contains(&options.max_bytes) || options.request_id.len() > 80 {
        return Err(CraftError::field(
            "/output",
            "invalid_provisioning_output_controls",
        ));
    }
    Ok(())
}

fn context(
    store: &ProfileStore,
    args: &ProvisionArgs,
    control: &InvocationControl,
) -> Result<Context, CraftError> {
    let mut grant = None;
    let mut targets = Vec::new();
    for name in &args.profiles {
        let profile = store.get(name, control)?;
        if profile.auth_mode != AuthMode::Service {
            return Err(CraftError::field("/profiles", "service_profiles_required"));
        }
        if grant.as_ref().is_some_and(|grant| grant != &profile.grant) {
            return Err(CraftError::field("/profiles", "profiles_must_share_grant"));
        }
        grant = Some(profile.grant.clone());
        let tenant = profile.tenant_ref()?;
        validate_service_target(&tenant)?;
        if !targets.contains(&tenant) {
            targets.push(tenant);
        }
    }
    let grant = grant.ok_or_else(|| CraftError::field("/profiles", "profile_required"))?;
    let shared = store.shared_profiles(&grant, control)?;
    Ok(Context {
        data: ProvisionData {
            grant,
            profiles: args.profiles.clone(),
            shared_profiles_observed: shared,
            credential_write_state: RequestState::NotSent,
            cleanup_pending: None,
            retry_after: None,
            token: None,
            verified_products: None,
        },
        targets,
        actor: args.expected_actor.clone(),
    })
}

fn projection_contract() -> ProjectionContract {
    ProjectionContract {
        item_pointer: "".into(),
        always_returned: [
            "grant",
            "profiles",
            "credential_write_state",
            "cleanup_pending",
            "retry_after",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        default_selected: [
            "grant",
            "profiles",
            "credential_write_state",
            "cleanup_pending",
            "retry_after",
            "verified_products",
            "token.expires_at_unix_ms",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        mode: ProjectionMode::Local,
    }
}

fn projection(fields: &[String]) -> Result<CompiledProjection, CraftError> {
    projection_contract().compile(&response_schema::<ProvisionData>()?, fields)
}

/// Builds the review candidate's typed contract without enabling a public command.
/// Startup registration deliberately remains owned by the public catalog.
pub fn register_candidate(registry: &mut Registry) -> Result<(), CraftError> {
    let data = json!({
        "grant":{"id":"gr_00000000000000000000000000000000"},
        "profiles":["factory"],"shared_profiles_observed":["factory"],
        "credential_write_state":"confirmed","cleanup_pending":false,"retry_after":null,
        "token":{"issued_at_unix_ms":1000,"expires_at_unix_ms":3601000,
            "reacquire_at_unix_ms":3541000,"scopes":["read:jira-user"]},
        "verified_products":[{"tenant":{"provider":"jira","deployment":"cloud",
            "tenant":"00000000-1111-2222-3333-444444444444"},
            "actor":"example-service-actor","verified_at_unix_ms":1000}]
    });
    registry.register_typed::<ProvisionArgs>(OperationDescriptor {
        id: COMMAND.into(), provider: "craft".into(),
        contract_version: craft_core::contracts::SCHEMA_VERSION.into(),
        api_revision: "atlassian-cloud-service-oauth".into(),
        description: "Provision one native service grant for explicit profiles and actor.".into(),
        effect: EffectClass::Write,
        input_schema: schema_for::<ProvisionArgs>()?,
        response_schema: response_schema::<ProvisionData>()?,
        example: json!({"profiles":["factory"],"expected_actor":"example-service-actor","secret_stdin":true}),
        response_example: serde_json::to_value(Envelope::success(COMMAND,"example-request".into(),data))
            .map_err(|_| CraftError::new(ErrorCode::Internal,"Cannot encode provisioning example."))?,
        projection: Some(projection_contract()),
        scope_alternatives: Vec::new(),
        permissions: Vec::new(),
        tenant_conditions: vec!["Selected products require reported probe scopes and matching current-user proof; resource mutation permission is not established.".into()],
        routes: Vec::new(), retry_class: RetryClass::ReconcileBeforeReplay,
        reconciliation: ReconciliationClass::KnownResource,
    })
}

type ProvisionPreflight<'a> =
    dyn FnMut(&TokenMetadata, &[ProductProof]) -> Result<(), CraftError> + 'a;

trait Backend {
    fn state(&self) -> Result<(bool, bool), CraftError>;
    fn activate(
        &mut self,
        credentials: ServiceCredentials,
        targets: &[TenantRef],
        actor: &str,
        preflight: &mut ProvisionPreflight<'_>,
    ) -> Result<bool, AuthFailure>;
}

struct NativeBackend<'a, 'b> {
    client: &'a ServiceOAuthClient,
    locked: &'a LockedCredentials<'b>,
}
impl Backend for NativeBackend<'_, '_> {
    fn state(&self) -> Result<(bool, bool), CraftError> {
        let state = self.locked.inspect()?;
        Ok((state.active, state.recovery_required))
    }
    fn activate(
        &mut self,
        credentials: ServiceCredentials,
        targets: &[TenantRef],
        actor: &str,
        preflight: &mut ProvisionPreflight<'_>,
    ) -> Result<bool, AuthFailure> {
        let receipt = self.client.provision_grant_with_preflight(
            self.locked,
            credentials,
            targets,
            Some(actor),
            |metadata, proofs| {
                preflight(
                    metadata,
                    &proofs.iter().map(ProductProof::from).collect::<Vec<_>>(),
                )
            },
        )?;
        Ok(receipt.generation.cleanup_pending)
    }
}

fn data_value(data: &ProvisionData, projection: &CompiledProjection) -> Result<Value, CraftError> {
    let value = serde_json::to_value(data)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode provisioning result."))?;
    projection.data(&value)
}
fn fits(envelope: &Envelope, maximum: usize) -> bool {
    serde_json::to_vec(envelope).is_ok_and(|bytes| bytes.len() < maximum)
}
fn budget_error() -> CraftError {
    let mut error = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Output budget cannot carry provisioning and recovery evidence.",
    );
    error.next_action = "Increase --max-bytes; no credential activation was attempted.".into();
    error
}
fn recovery_error(error: CraftError) -> CraftError {
    let mut safe = CraftError::new(error.code, "Service provisioning did not complete.");
    safe.details.write_state = error.details.write_state;
    safe.next_action =
        "Inspect local auth status/doctor and reconcile credential state before repeating.".into();
    safe
}

fn run(
    args: &ProvisionArgs,
    context: Context,
    backend: &mut impl Backend,
    read: impl FnOnce() -> Result<ServiceCredentials, CraftError>,
    projection: &CompiledProjection,
    options: &InvocationOptions<'_>,
) -> Result<Envelope, CraftError> {
    let (active, pending) = backend.state()?;
    if pending {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Resolve pending credential state before provisioning.",
        ));
    }
    if active && !args.replace {
        return Err(CraftError::new(
            ErrorCode::VersionConflict,
            "Existing credentials require explicit replacement.",
        ));
    }
    let mut worst = context.data.clone();
    worst.credential_write_state = RequestState::SentUnknown;
    worst.cleanup_pending = Some(false);
    worst.retry_after = Some(RetryAdvice {
        delay_seconds: Some(u64::MAX),
        at_unix_ms: Some(u64::MAX),
    });
    let mut error = recovery_error(CraftError::new(ErrorCode::OutputBudgetTooSmall, ""));
    error.details.write_state = RequestState::SentUnknown;
    let recovery = Envelope::failure_with_data(
        COMMAND,
        options.request_id.into(),
        error,
        data_value(&worst, projection)?,
    );
    if !fits(&recovery, options.max_bytes) {
        return Err(budget_error());
    }
    let credentials = read()?;
    let mut frozen = None;
    let result = backend.activate(
        credentials,
        &context.targets,
        &context.actor,
        &mut |metadata, proofs| {
            if proofs.len() != context.targets.len()
                || context.targets.iter().any(|target| {
                    proofs
                        .iter()
                        .filter(|proof| &proof.tenant == target && proof.actor == context.actor)
                        .count()
                        != 1
                })
            {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Provisioning proof does not match the selected actor and targets.",
                ));
            }
            let mut data = context.data.clone();
            data.credential_write_state = RequestState::Confirmed;
            data.token = Some(metadata.into());
            data.verified_products = Some(proofs.to_vec());
            let mut views = Vec::new();
            for cleanup in [false, true] {
                data.cleanup_pending = Some(cleanup);
                let envelope = Envelope::success(
                    COMMAND,
                    options.request_id.into(),
                    data_value(&data, projection)?,
                );
                if !fits(&envelope, options.max_bytes) {
                    return Err(budget_error());
                }
                views.push(envelope);
            }
            frozen = Some(views);
            Ok(())
        },
    );
    match result {
        Ok(cleanup) => match frozen {
            Some(mut views) => Ok(views.swap_remove(usize::from(cleanup))),
            None => {
                let mut data = context.data;
                data.credential_write_state = RequestState::Confirmed;
                data.cleanup_pending = Some(cleanup);
                Ok(Envelope::failure_with_data(
                    COMMAND,
                    options.request_id.into(),
                    recovery_error(CraftError::new(ErrorCode::Internal, "")),
                    data_value(&data, projection)?,
                ))
            }
        },
        Err(failure) => {
            let mut data = context.data;
            data.credential_write_state = failure
                .credential_write_state
                .unwrap_or(RequestState::NotSent);
            if let Some(receipt) = failure.credential_update {
                data.credential_write_state = RequestState::Confirmed;
                data.cleanup_pending = Some(receipt.cleanup_pending);
            }
            data.retry_after = failure.retry_after.map(RetryAdvice::from);
            Ok(Envelope::failure_with_data(
                COMMAND,
                options.request_id.into(),
                recovery_error(failure.error),
                data_value(&data, projection)?,
            ))
        }
    }
}

/// Resolves explicit service profiles, locks their grant and preflights activation/recovery output.
/// Credentials enter only through the secure pipe or explicit interactive reader, never arguments.
pub fn execute(
    args: ProvisionArgs,
    fields: &[String],
    stdin_consumed: bool,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Envelope {
    let result = (|| {
        validate_arguments(&args, options, stdin_consumed)?;
        let projection = projection(fields)?;
        let path = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&path, control)?;
        let context = context(&store, &args, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(&context.data.grant, control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        run(
            &args,
            context,
            &mut NativeBackend {
                client: &client,
                locked: &locked,
            },
            || {
                if args.interactive {
                    craft_runtime::auth::read_service_credentials_interactively(control)
                } else {
                    craft_runtime::auth::read_service_credentials_from_stdin(control)
                }
            },
            &projection,
            options,
        )
    })();
    result.unwrap_or_else(|error| Envelope::failure(COMMAND, options.request_id.into(), error))
}
