use std::path::Path;

use craft_core::contracts::{Envelope, GrantRef, Page};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::canonical_digest;
use craft_core::profiles::{
    ProfileInput, ProfileSource, RepositoryHints, select_profile, validate_profile_name,
};
use craft_core::projection::CompiledProjection;
use craft_core::registry::Registry;
use craft_runtime::control::SystemClock;
use craft_runtime::profile_pages::{ProfileListRequest, ProfilePage};
use craft_runtime::profiles::{
    ProfileStore, commit_outcome_unknown, load_repository_hints, resolve_state_root,
};
use serde_json::{Value, json};

use crate::args::{ProfileAction, ProfileCreateArgs};
use crate::catalog::{ProfileData, ProfilesData};
use crate::dispatch::DispatchOutput;

pub struct InvocationOptions<'a> {
    pub state_root: Option<&'a Path>,
    pub profile: Option<&'a str>,
    pub request_id: &'a str,
    pub max_bytes: usize,
}

fn encode(value: impl serde::Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode profile response."))
}

fn project(data: Value, projection: &Option<CompiledProjection>) -> Result<Value, CraftError> {
    match projection {
        Some(projection) => projection.data(&data),
        None => Ok(data),
    }
}

fn fits(
    command: &str,
    data: &Value,
    page: Option<&Page>,
    options: &InvocationOptions<'_>,
) -> Result<bool, CraftError> {
    let mut envelope = Envelope::success(command, options.request_id.into(), data.clone());
    envelope.page = page.cloned();
    Ok(serde_json::to_vec(&envelope)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode profile envelope."))?
        .len()
        < options.max_bytes)
}

fn preflight_write(
    command: &str,
    data: &Value,
    options: &InvocationOptions<'_>,
) -> Result<(), CraftError> {
    let failure = Envelope::failure(command, options.request_id.into(), commit_outcome_unknown());
    let failure_size = serde_json::to_vec(&failure)
        .map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot encode profile recovery envelope.",
            )
        })?
        .len()
        + 1;
    if failure_size > options.max_bytes || !fits(command, data, None, options)? {
        let mut error = CraftError::new(
            ErrorCode::OutputBudgetTooSmall,
            "Output budget cannot carry the profile write result and recovery contract.",
        );
        error.next_action =
            "Increase --max-bytes; no profile or active selection was changed.".into();
        return Err(error);
    }
    Ok(())
}

fn create_input(arguments: &ProfileCreateArgs) -> Result<ProfileInput, CraftError> {
    let input = ProfileInput {
        name: arguments.name.clone(),
        provider: arguments.provider.clone(),
        deployment: arguments.deployment.clone(),
        auth_mode: arguments.auth_mode.parse()?,
        grant: arguments.grant.clone().map(|id| GrantRef { id }),
        tenant: arguments.tenant.clone(),
        defaults: arguments.defaults.clone(),
    };
    input.authority()?;
    input.with_grant(input.grant.clone().unwrap_or(GrantRef {
        id: "pending".into(),
    }))?;
    Ok(input)
}

pub fn validate_input(command: &str, input: &Value) -> Result<(), CraftError> {
    if command == "profile.create" {
        let arguments: ProfileCreateArgs = serde_json::from_value(input.clone())
            .map_err(|_| CraftError::field("/profile", "invalid_profile_input"))?;
        create_input(&arguments)?;
    }
    Ok(())
}

pub(crate) fn selected_profile(
    store: &ProfileStore,
    name: Option<&str>,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<(String, ProfileSource), CraftError> {
    let (name, source, _) = selected_profile_context(store, name, options, control)?;
    Ok((name, source))
}

pub(crate) fn selected_profile_context(
    store: &ProfileStore,
    name: Option<&str>,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<(String, ProfileSource, RepositoryHints), CraftError> {
    if name
        .zip(options.profile)
        .is_some_and(|(name, profile)| name != profile)
    {
        return Err(CraftError::field("/name", "name_and_profile_conflict"));
    }
    let explicit = name.or(options.profile);
    let hints = if explicit.is_none() {
        let directory = std::env::current_dir().map_err(|_| {
            CraftError::new(
                ErrorCode::InputInvalid,
                "Cannot resolve repository hint directory.",
            )
        })?;
        load_repository_hints(&directory, control)?
    } else {
        RepositoryHints::default()
    };
    let active = if explicit.is_none() && hints.profile.is_none() {
        store.active(control)?
    } else {
        None
    };
    let (name, source) = select_profile(explicit, &hints, active.as_deref())?;
    Ok((name.into(), source, hints))
}

fn list_data(
    page: &ProfilePage,
    projection: &Option<CompiledProjection>,
) -> Result<Value, CraftError> {
    project(
        encode(ProfilesData {
            items: page.items.clone(),
            observed_at: page.observed_at,
            cached: page.cached,
        })?,
        projection,
    )
}

pub fn execute(
    registry: &Registry,
    action: ProfileAction,
    projection: Option<CompiledProjection>,
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<DispatchOutput, CraftError> {
    if options.profile.is_some()
        && matches!(&action, ProfileAction::Create(_) | ProfileAction::List(_))
    {
        return Err(CraftError::field(
            "/profile",
            "profile_selection_not_applicable",
        ));
    }
    // Reject local semantic errors before even creating the private metadata database.
    let input = match &action {
        ProfileAction::Create(arguments) => Some(create_input(arguments)?),
        ProfileAction::Show(arguments) | ProfileAction::Use(arguments) => {
            if let Some(name) = arguments.name.as_deref().or(options.profile) {
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
            None
        }
        _ => None,
    };
    let path = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
    let store = ProfileStore::open(&path, control)?;
    match action {
        ProfileAction::Create(_) => {
            let input = input.ok_or_else(|| {
                CraftError::new(ErrorCode::Internal, "Missing prepared profile input.")
            })?;
            let mut output = None;
            store.create(&input, control, |profile| {
                let data = project(
                    encode(ProfileData {
                        profile: profile.clone(),
                        selection_source: ProfileSource::Explicit,
                        active: false,
                    })?,
                    &projection,
                )?;
                preflight_write("profile.create", &data, options)?;
                output = Some(data);
                Ok(())
            })?;
            Ok(DispatchOutput {
                data: output.ok_or_else(|| {
                    CraftError::new(ErrorCode::Internal, "Missing committed profile response.")
                })?,
                page: None,
            })
        }
        ProfileAction::Show(arguments) => {
            let (name, source) =
                selected_profile(&store, arguments.name.as_deref(), options, control)?;
            let profile = store.get(&name, control)?;
            let active = store.active(control)?.as_deref() == Some(name.as_str());
            Ok(DispatchOutput {
                data: project(
                    encode(ProfileData {
                        profile,
                        selection_source: source,
                        active,
                    })?,
                    &projection,
                )?,
                page: None,
            })
        }
        ProfileAction::Use(arguments) => {
            let (name, source) =
                selected_profile(&store, arguments.name.as_deref(), options, control)?;
            let mut output = None;
            store.use_profile(&name, control, |profile| {
                let data = project(
                    encode(ProfileData {
                        profile: profile.clone(),
                        selection_source: source,
                        active: true,
                    })?,
                    &projection,
                )?;
                preflight_write("profile.use", &data, options)?;
                output = Some(data);
                Ok(())
            })?;
            Ok(DispatchOutput {
                data: output.ok_or_else(|| {
                    CraftError::new(ErrorCode::Internal, "Missing committed selection response.")
                })?,
                page: None,
            })
        }
        ProfileAction::List(arguments) => {
            let descriptor = registry.descriptor("profile.list")?;
            let binding = canonical_digest(
                &json!({"command":"profile.list","descriptor":canonical_digest(&encode(descriptor)?)?,"response":projection.as_ref().map(|projection| projection.response_schema(&descriptor.response_schema)).transpose()?}),
            )?;
            let page = store.list_page(
                ProfileListRequest {
                    cursor: arguments.cursor.as_deref(),
                    binding: &binding,
                    limit: arguments.limit,
                },
                &SystemClock::default(),
                control,
                |page| {
                    fits(
                        "profile.list",
                        &list_data(page, &projection)?,
                        Some(&page.page),
                        options,
                    )
                },
            )?;
            Ok(DispatchOutput {
                data: list_data(&page, &projection)?,
                page: Some(page.page),
            })
        }
    }
}
