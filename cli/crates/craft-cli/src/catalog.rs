use crate::args::{
    CapabilitiesArgs, EmptyArgs, FindArgs, HelpArgs, ProfileCreateArgs, ProfileListArgs,
    ProfileNameArgs, SchemaArgs, ValidateArgs,
};
use crate::auth::{AuthDoctorData, AuthStatusData};
use craft_core::contracts::{Capability, EffectClass, Envelope};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ReconciliationClass, Registry, RetryClass,
    response_schema, schema_for,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionData {
    pub product: String,
    pub version: String,
    pub contract: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandSummary {
    pub id: String,
    pub description: String,
    pub provider: String,
    pub effect: EffectClass,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchData {
    pub matches: Vec<CommandSummary>,
    pub more: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaData {
    pub command: String,
    pub contract_version: String,
    pub provider: String,
    pub provider_version: Option<String>,
    pub api_revision: String,
    pub direction: String,
    pub view: String,
    pub complete: bool,
    pub digest: String,
    pub schema: Value,
    pub projection: Option<ProjectionContract>,
    pub selectable_fields: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationData {
    pub command: String,
    pub valid: bool,
    pub digest: String,
    pub live_conditions_checked: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvidersData {
    pub items: Vec<ProviderManifest>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySummary {
    pub command: String,
    pub authentication: String,
    pub availability: Capability,
    pub effect: EffectClass,
    pub scope_alternatives: craft_core::registry::ScopeAlternatives,
    pub permissions: Vec<String>,
    pub tenant_conditions: Vec<String>,
    pub retry_class: RetryClass,
    pub reconciliation: ReconciliationClass,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesData {
    pub items: Vec<CapabilitySummary>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HelpData {
    pub commands: Vec<CommandSummary>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileData {
    pub profile: craft_core::profiles::Profile,
    pub selection_source: craft_core::profiles::ProfileSource,
    pub active: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfilesData {
    pub items: Vec<craft_core::profiles::ProfileSummary>,
    pub observed_at: u64,
    pub cached: bool,
}

pub(crate) fn register<I: JsonSchema + serde::de::DeserializeOwned, O: JsonSchema>(
    registry: &mut Registry,
    id: &str,
    description: &str,
    example: Value,
    output: Value,
) -> Result<(), CraftError> {
    let mut response_example = Envelope::success(id, "example-request".into(), output);
    if id == "profile.list" {
        response_example.page = Some(craft_core::contracts::Page {
            returned: 0,
            has_more: false,
            next_cursor: None,
            truncated: false,
            stop_reason: "exhausted".into(),
            consistency: "live".into(),
        });
    }
    registry.register_typed::<I>(OperationDescriptor {
        id: id.into(),
        provider: "craft".into(),
        contract_version: craft_core::contracts::SCHEMA_VERSION.into(),
        api_revision: "local".into(),
        description: description.into(),
        effect: if matches!(id, "profile.create" | "profile.use") {
            EffectClass::Write
        } else {
            EffectClass::Read
        },
        input_schema: schema_for::<I>()?,
        response_schema: response_schema::<O>()?,
        example,
        response_example: serde_json::to_value(response_example)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode command example."))?,
        projection: local_projection(id),
        scope_alternatives: Vec::new(),
        permissions: Vec::new(),
        tenant_conditions: Vec::new(),
        routes: Vec::new(),
        retry_class: if matches!(id, "profile.create" | "profile.use") {
            RetryClass::ReconcileBeforeReplay
        } else {
            RetryClass::LocalRead
        },
        reconciliation: if matches!(id, "profile.create" | "profile.use") {
            ReconciliationClass::KnownResource
        } else {
            ReconciliationClass::NotApplicable
        },
    })
}

fn local_projection(command: &str) -> Option<ProjectionContract> {
    let (pointer, always, defaults): (&str, &[&str], &[&str]) = match command {
        "operation.get" => (
            "",
            &[
                "operation_id",
                "found",
                "reconciliation_required",
                "evidence_scope",
                "live_verified",
                "replay_authorized",
            ],
            &[
                "operation_id",
                "found",
                "binding",
                "revision",
                "steps",
                "reconciliation_required",
                "evidence_scope",
                "live_verified",
                "replay_authorized",
            ],
        ),
        "plan.scaffold" | "design.scaffold" => (
            "",
            &[
                "ready",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
            &[
                "document",
                "ready",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
        ),
        "plan.validate" => (
            "",
            &[
                "valid",
                "digest",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
            &[
                "valid",
                "digest",
                "task_count",
                "included_tasks",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
        ),
        "design.validate" => (
            "",
            &[
                "valid",
                "digest",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
            &[
                "valid",
                "digest",
                "section_count",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
        ),
        "whiteboard.encode" => (
            "",
            &["element_count", "scene_digest", "board_written"],
            &[
                "element_count",
                "connector_count",
                "scene_digest",
                "clipboard_html",
                "board_written",
            ],
        ),
        "whiteboard.layout" => ("", &["moved_count"], &["moved_count", "report", "scene"]),
        "whiteboard.check" => (
            "",
            &["warning_count", "info_count", "score"],
            &["warning_count", "info_count", "score", "findings"],
        ),
        "whiteboard.decode" => (
            "",
            &["element_count", "raw_count", "private_fields_removed"],
            &[
                "element_count",
                "raw_count",
                "private_fields_removed",
                "warnings",
                "scene",
            ],
        ),
        "plan.render" | "design.render" => (
            "",
            &[
                "format",
                "document_digest",
                "body_digest",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
            &[
                "format",
                "document_digest",
                "body_digest",
                "storage",
                "adf",
                "review_verified",
                "live_conditions_checked",
                "tickets_created",
            ],
        ),
        "auth.status" => (
            "",
            &["profile", "grant", "auth_mode", "tenant", "live_verified"],
            &[
                "profile",
                "grant",
                "auth_mode",
                "tenant",
                "credential_state",
                "cleanup_pending",
                "token_state",
                "live_verified",
            ],
        ),
        "auth.doctor" => (
            "",
            &["profile", "grant", "auth_mode", "live_verified"],
            &[
                "profile",
                "grant",
                "auth_mode",
                "health.backend",
                "health.read_access",
                "health.write_access",
                "live_verified",
            ],
        ),
        "version" => (
            "",
            &["product", "contract"],
            &["product", "version", "contract"],
        ),
        "command.find" => (
            "/matches",
            &["id"],
            // Provider is derivable from the id prefix and stays selectable with --fields.
            &["id", "description", "effect"],
        ),
        "provider.list" => ("/items", &["id"], &["id", "version", "deployments"]),
        "profile.list" => (
            "/items",
            &["name", "provider", "deployment", "tenant"],
            &["name", "provider", "deployment", "tenant", "auth_mode"],
        ),
        "profile.show" | "profile.create" | "profile.use" => (
            "/profile",
            &[
                "name",
                "provider",
                "deployment",
                "tenant",
                "auth_mode",
                "grant",
            ],
            &[
                "name",
                "provider",
                "deployment",
                "tenant",
                "auth_mode",
                "grant",
                "defaults",
            ],
        ),
        "capabilities" => (
            "/items",
            &["command"],
            &["command", "authentication", "availability"],
        ),
        "help" => (
            "/commands",
            &["id"],
            // Provider is derivable from the id prefix and stays selectable with --fields.
            &["id", "description", "effect"],
        ),
        _ => return None,
    };
    Some(ProjectionContract {
        item_pointer: pointer.into(),
        always_returned: always.iter().map(|field| (*field).into()).collect(),
        default_selected: defaults.iter().map(|field| (*field).into()).collect(),
        mode: ProjectionMode::Local,
    })
}

pub fn registry() -> Result<Registry, CraftError> {
    let mut registry = local_registry()?;
    crate::provision::register_candidate(&mut registry)?;
    crate::policies::register_candidates(&mut registry)?;
    crate::review_policy::register(&mut registry)?;
    crate::planning_review::command::register(&mut registry)?;
    crate::planning_review::verification::register(&mut registry)?;
    crate::planning_review::tickets::register(&mut registry)?;
    crate::planning_review::application::command::register(&mut registry)?;
    crate::public_commands::register(&mut registry)?;
    Ok(registry)
}

/// Local-only registry for isolated adapter tests and tooling, not executable startup.
pub fn local_registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    crate::local_artifacts::register(&mut registry)?;
    crate::planning::register(&mut registry)?;
    crate::whiteboard_scene::register(&mut registry)?;
    register::<crate::args::OperationGetArgs, crate::operations::OperationData>(
        &mut registry,
        "operation.get",
        "Read durable local operation evidence without live verification or replay authority.",
        json!({"operation_id":"publish-summary"}),
        json!({"operation_id":"publish-summary","found":false,"binding":null,"revision":null,"steps":[],"reconciliation_required":null,"evidence_scope":"local_installation","live_verified":false,"replay_authorized":false}),
    )?;
    register::<crate::args::OperationPreviewArgs, crate::operation_preview::PreviewData>(
        &mut registry,
        "operation.preview",
        "Validate a mutating command's input and describe its effect, digests and stated preconditions; --check-remote also runs the read that observes its existing target. Sends no write, holds no lock and grants no approval.",
        json!({"command":"jira.issue.delete","file":"delete.json"}),
        json!({"command":"jira.issue.delete","valid":true,"input_digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","descriptor_digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","effect":"permanent","destructive":true,"retry_class":"reconcile_before_replay","reconciliation":"known_resource","operation_id":"delete-ed-1","preconditions":{},"apply_required":true,"remote":null,"write_sent":false,"lock_held":false,"approval_granted":false}),
    )?;
    register::<crate::args::OperationListArgs, crate::operations::OperationListData>(
        &mut registry,
        "operation.list",
        "List durable local operation records in identity order, optionally only those needing reconciliation, without live verification or replay authority.",
        json!({"limit":20,"unresolved_only":true}),
        json!({"items":[],"has_more":false,"next_after_operation_id":null,"scanned":0,"evidence_scope":"local_installation","live_verified":false,"replay_authorized":false}),
    )?;
    register::<ProfileNameArgs, AuthStatusData>(
        &mut registry,
        "auth.status",
        "Inspect local credential index metadata without token reads or login.",
        json!({"name":"team"}),
        json!({"profile":"team","grant":{"id":"gr_00000000000000000000000000000000"},"auth_mode":"service","tenant":null,"selection_source":"explicit","credential_state":"absent","cleanup_pending":false,"shared_profiles":["team"],"token_state":"unknown","live_verified":false}),
    )?;
    register::<ProfileNameArgs, AuthDoctorData>(
        &mut registry,
        "auth.doctor",
        "Probe configured native vault read health without unlock, credential writes or login.",
        json!({"name":"team"}),
        json!({"profile":"team","grant":{"id":"gr_00000000000000000000000000000000"},"auth_mode":"service","selection_source":"explicit","health":{"backend":"windows_credential_manager","principal":null,"persistence":"local_machine","read_access":"available","write_access":"unknown","item_limit_bytes":2560},"live_verified":false}),
    )?;
    let profile = json!({"profile":{"name":"team","provider":"jira","deployment":"cloud","auth_mode":"delegated","grant":{"id":"gr_00000000000000000000000000000000"},"tenant":null,"defaults":{}},"selection_source":"explicit","active":false});
    register::<ProfileCreateArgs, ProfileData>(
        &mut registry,
        "profile.create",
        "Create trusted local profile metadata; no credentials or cloud access.",
        json!({"name":"team","provider":"jira","deployment":"cloud","auth_mode":"delegated"}),
        profile.clone(),
    )?;
    register::<ProfileNameArgs, ProfileData>(
        &mut registry,
        "profile.show",
        "Read an explicit, repository-selected or active local profile.",
        json!({"name":"team"}),
        profile.clone(),
    )?;
    let mut active_profile = profile;
    active_profile["active"] = json!(true);
    register::<ProfileNameArgs, ProfileData>(
        &mut registry,
        "profile.use",
        "Select an existing profile as the trusted active default.",
        json!({"name":"team"}),
        active_profile,
    )?;
    register::<ProfileListArgs, ProfilesData>(
        &mut registry,
        "profile.list",
        "List local profiles with opaque, lossless continuation.",
        json!({"limit":20}),
        json!({"items":[],"observed_at":0,"cached":false}),
    )?;
    register::<EmptyArgs, VersionData>(
        &mut registry,
        "version",
        "Identify the Craft distribution and contract.",
        json!({}),
        json!({"product":craft_core::PRODUCT,"version":craft_core::VERSION,"contract":craft_core::contracts::SCHEMA_VERSION}),
    )?;
    register::<FindArgs, SearchData>(
        &mut registry,
        "command.find",
        "Search implemented commands without loading their schemas.",
        json!({"query":"schema","limit":5}),
        json!({"matches":[],"more":false}),
    )?;
    register::<SchemaArgs, SchemaData>(
        &mut registry,
        "schema",
        "Inspect selected request or response contracts.",
        json!({"command":"version"}),
        json!({"command":"version","contract_version":craft_core::contracts::SCHEMA_VERSION,"provider":"craft","provider_version":craft_core::VERSION,"api_revision":"local","direction":"input","view":"full","complete":true,"digest":"sha256:example","schema":{},"selectable_fields":[]}),
    )?;
    register::<ValidateArgs, ValidationData>(
        &mut registry,
        "input.validate",
        "Validate a bounded JSON object locally; no auth or network.",
        json!({"command":"version","file":"-"}),
        json!({"command":"version","valid":true,"digest":"sha256:example","live_conditions_checked":false}),
    )?;
    register::<EmptyArgs, ProvidersData>(
        &mut registry,
        "provider.list",
        "List implemented cloud provider modules.",
        json!({}),
        json!({"items":[]}),
    )?;
    register::<CapabilitiesArgs, CapabilitiesData>(
        &mut registry,
        "capabilities",
        "Describe implemented operation availability.",
        json!({}),
        json!({"items":[]}),
    )?;
    register::<HelpArgs, HelpData>(
        &mut registry,
        "help",
        "Return concise machine-readable command purposes.",
        json!({"json":true}),
        json!({"commands":[]}),
    )?;
    Ok(registry)
}

pub fn summary(operation: &OperationDescriptor) -> CommandSummary {
    CommandSummary {
        id: operation.id.clone(),
        description: operation.description.clone(),
        provider: operation.provider.clone(),
        effect: operation.effect,
    }
}
