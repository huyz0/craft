use craft_core::contracts::{AuthMode, GrantRef, TenantRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::profiles::{ProfileSource, validate_profile_name};
use craft_core::projection::CompiledProjection;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::{CredentialState, NativeCredentials};
use craft_runtime::vault::NativeVault;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args::AuthAction;
use crate::dispatch::DispatchOutput;
use crate::profiles::InvocationOptions;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialPresence {
    Absent,
    StoredUnverified,
    RecoveryRequired,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthStatusData {
    pub profile: String,
    pub grant: GrantRef,
    pub auth_mode: AuthMode,
    pub tenant: Option<TenantRef>,
    pub selection_source: ProfileSource,
    pub credential_state: CredentialPresence,
    pub cleanup_pending: bool,
    pub shared_profiles: Vec<String>,
    #[schemars(regex(pattern = "^unknown$"))]
    pub token_state: String,
    #[schemars(extend("const" = false))]
    pub live_verified: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackend {
    WindowsCredentialManager,
    MacosKeychain,
    LinuxSecretService,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessState {
    Available,
    Unavailable,
    Locked,
    Unknown,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeHealthData {
    pub backend: CredentialBackend,
    pub principal: Option<String>,
    pub persistence: String,
    pub read_access: AccessState,
    #[schemars(extend("const" = "unknown"))]
    pub write_access: String,
    pub item_limit_bytes: usize,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthDoctorData {
    pub profile: String,
    pub grant: GrantRef,
    pub auth_mode: AuthMode,
    pub selection_source: ProfileSource,
    pub health: NativeHealthData,
    #[schemars(extend("const" = false))]
    pub live_verified: bool,
}

fn presence(state: &CredentialState) -> CredentialPresence {
    if state.recovery_required {
        CredentialPresence::RecoveryRequired
    } else if state.active {
        CredentialPresence::StoredUnverified
    } else {
        CredentialPresence::Absent
    }
}

/// Local diagnostics never load token chunks, acquire/refresh credentials or call a product API.
pub fn execute(
    action: AuthAction,
    projection: Option<CompiledProjection>,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<DispatchOutput, CraftError> {
    let arguments = match &action {
        AuthAction::Status(arguments) | AuthAction::Doctor(arguments) => arguments,
        AuthAction::Provision(_) => return Err(invalid_output()),
    };
    for name in [arguments.name.as_deref(), options.profile]
        .into_iter()
        .flatten()
    {
        validate_profile_name(name)?;
    }
    if arguments
        .name
        .as_deref()
        .zip(options.profile)
        .is_some_and(|(name, profile)| name != profile)
    {
        return Err(CraftError::field("/name", "name_and_profile_conflict"));
    }
    let path = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
    let store = ProfileStore::open(&path, control)?;
    let (name, source) =
        crate::profiles::selected_profile(&store, arguments.name.as_deref(), options, control)?;
    let profile = store.get(&name, control)?;
    let data = match action {
        AuthAction::Status(_) => {
            let credentials = NativeCredentials::open(&store)?;
            let locked = credentials.lock(&profile.grant, control)?;
            let state = locked.inspect()?;
            serde_json::to_value(AuthStatusData {
                profile: name,
                grant: profile.grant.clone(),
                auth_mode: profile.auth_mode,
                tenant: profile.tenant.as_ref().map(|tenant| TenantRef {
                    provider: profile.provider.clone(),
                    deployment: profile.deployment.clone(),
                    tenant: tenant.clone(),
                }),
                selection_source: source,
                credential_state: presence(&state),
                cleanup_pending: state.cleanup_pending,
                shared_profiles: store.shared_profiles(&profile.grant, control)?,
                token_state: "unknown".into(),
                live_verified: false,
            })
        }
        AuthAction::Doctor(_) => {
            let settings = store.settings();
            let vault = NativeVault::open(settings.credential_backend, &settings.installation_id)
                .map_err(|_| {
                CraftError::new(
                    ErrorCode::KeyringUnavailable,
                    "Configured native vault is unavailable.",
                )
            })?;
            control.check()?;
            let health: NativeHealthData = serde_json::from_value(
                serde_json::to_value(vault.health()).map_err(|_| invalid_output())?,
            )
            .map_err(|_| invalid_output())?;
            control.check()?;
            serde_json::to_value(AuthDoctorData {
                profile: name,
                grant: profile.grant,
                auth_mode: profile.auth_mode,
                selection_source: source,
                health,
                live_verified: false,
            })
        }
        AuthAction::Provision(_) => return Err(invalid_output()),
    }
    .map_err(|_| invalid_output())?;
    let data = match projection {
        Some(projection) => projection.data(&data)?,
        None => data,
    };
    Ok(DispatchOutput { data, page: None })
}

fn invalid_output() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot encode local authentication diagnostics.",
    )
}
