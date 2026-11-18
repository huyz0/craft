//! Static-credential provisioning for providers that issue a long-lived key or token.
//!
//! The secret enters only through the secure pipe, is proven against the provider's fixed
//! identity route and the profile's configured tenant, and is stored in the profile's grant.

use clap::Args;
use craft_core::contracts::{AuthMode, EffectClass, Envelope, GrantRef, RequestState, TenantRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::validate_profile_name;
use craft_core::projection::{CompiledProjection, ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema, schema_for,
};
use craft_runtime::auth::{ServiceOAuthClient, read_static_credentials_from_stdin};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::NativeCredentials;
use craft_runtime::transport::TlsPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::profiles::InvocationOptions;

pub const COMMAND: &str = "auth.provision-static";

#[derive(Args, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvisionStaticArgs {
    /// Profile whose provider is `trello` or `bitbucket`. Its grant receives the secret.
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 64))]
    pub name: String,
    /// Required: the secret is read from a native pipe, never from an argument or file.
    #[arg(long)]
    #[serde(default)]
    pub secret_stdin: bool,
    #[arg(long)]
    #[serde(default)]
    pub replace: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvisionStaticData {
    pub grant: GrantRef,
    pub profile: String,
    pub tenant: TenantRef,
    pub actor: Option<String>,
    pub credential_write_state: RequestState,
    pub cleanup_pending: Option<bool>,
}

fn projection_contract() -> ProjectionContract {
    let fields = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    ProjectionContract {
        item_pointer: "".into(),
        always_returned: fields(&["grant", "profile", "tenant", "credential_write_state"]),
        default_selected: fields(&[
            "grant",
            "profile",
            "tenant",
            "actor",
            "credential_write_state",
            "cleanup_pending",
        ]),
        mode: ProjectionMode::Local,
    }
}

fn projection(fields: &[String]) -> Result<CompiledProjection, CraftError> {
    projection_contract().compile(&response_schema::<ProvisionStaticData>()?, fields)
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let data = json!({
        "grant":{"id":"gr_00000000000000000000000000000000"},
        "profile":"cards",
        "tenant":{"provider":"trello","deployment":"cloud","tenant":"000000000000000000000000"},
        "actor":"000000000000000000000000",
        "credential_write_state":"confirmed",
        "cleanup_pending":false
    });
    registry.register_typed::<ProvisionStaticArgs>(OperationDescriptor {
        id: COMMAND.into(),
        provider: "craft".into(),
        contract_version: craft_core::contracts::SCHEMA_VERSION.into(),
        api_revision: "static-credential-identity-proof".into(),
        description: "Prove and store one static key or token for a Trello or Bitbucket profile."
            .into(),
        effect: EffectClass::Write,
        input_schema: schema_for::<ProvisionStaticArgs>()?,
        response_schema: response_schema::<ProvisionStaticData>()?,
        example: json!({"name":"cards","secret_stdin":true}),
        response_example: serde_json::to_value(Envelope::success(
            COMMAND,
            "example-request".into(),
            data,
        ))
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode provisioning example."))?,
        projection: Some(projection_contract()),
        scope_alternatives: Vec::new(),
        permissions: Vec::new(),
        tenant_conditions: vec![
            "The secret must resolve to the profile's configured Trello member or Bitbucket workspace; resource permissions are not established.".into(),
        ],
        routes: Vec::new(),
        retry_class: RetryClass::ReconcileBeforeReplay,
        reconciliation: ReconciliationClass::KnownResource,
    })
}

fn validate_arguments(
    args: &ProvisionStaticArgs,
    options: &InvocationOptions<'_>,
    stdin_consumed: bool,
) -> Result<(), CraftError> {
    if options.profile.is_some() {
        return Err(CraftError::field("/profile", "use_name_for_provisioning"));
    }
    validate_profile_name(&args.name)?;
    if !args.secret_stdin {
        return Err(CraftError::field("/secret_stdin", "secret_stdin_required"));
    }
    if stdin_consumed {
        return Err(CraftError::field(
            "/secret_stdin",
            "stdin_can_only_be_consumed_once",
        ));
    }
    Ok(())
}

fn recovery_error(error: CraftError) -> CraftError {
    let mut safe = CraftError::new(
        error.code,
        "Static credential provisioning did not complete.",
    );
    safe.details.write_state = error.details.write_state;
    safe.next_action =
        "Inspect the profile, provider access and local auth status before repeating.".into();
    safe
}

fn data_value(
    data: &ProvisionStaticData,
    projection: &CompiledProjection,
) -> Result<Value, CraftError> {
    let value = serde_json::to_value(data)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode provisioning result."))?;
    projection.data(&value)
}

/// Provisions one profile's grant. Credentials enter only through the secure pipe.
pub fn execute(
    args: ProvisionStaticArgs,
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
        let profile = store.get(&args.name, control)?;
        if profile.auth_mode != AuthMode::Service
            || !matches!(profile.provider.as_str(), "trello" | "bitbucket")
        {
            return Err(CraftError::field(
                "/name",
                "trello_or_bitbucket_service_profile_required",
            ));
        }
        let tenant = profile.tenant_ref()?;
        let mut data = ProvisionStaticData {
            grant: profile.grant.clone(),
            profile: profile.name.clone(),
            tenant: tenant.clone(),
            actor: None,
            credential_write_state: RequestState::NotSent,
            cleanup_pending: None,
        };
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(&profile.grant, control)?;
        let state = locked.inspect()?;
        if state.recovery_required {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Resolve pending credential state before provisioning.",
            ));
        }
        if state.active && !args.replace {
            return Err(CraftError::new(
                ErrorCode::VersionConflict,
                "Existing credentials require explicit replacement.",
            ));
        }
        let secret = read_static_credentials_from_stdin(&profile.provider, control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        match client.provision_static_grant(&locked, &tenant, secret) {
            Ok(receipt) => {
                data.actor = Some(receipt.actor);
                data.credential_write_state = RequestState::Confirmed;
                data.cleanup_pending = Some(receipt.generation.cleanup_pending);
                Ok(Envelope::success(
                    COMMAND,
                    options.request_id.into(),
                    data_value(&data, &projection)?,
                ))
            }
            Err(failure) => {
                data.credential_write_state = failure
                    .credential_write_state
                    .unwrap_or(RequestState::NotSent);
                Ok(Envelope::failure_with_data(
                    COMMAND,
                    options.request_id.into(),
                    recovery_error(failure.error),
                    data_value(&data, &projection)?,
                ))
            }
        }
    })();
    result.unwrap_or_else(|error| Envelope::failure(COMMAND, options.request_id.into(), error))
}
