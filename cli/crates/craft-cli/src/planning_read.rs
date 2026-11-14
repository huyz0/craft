//! Import the current visible page; local authoring files cannot override remote human edits.
use crate::confluence_reads::command::{ConfluenceGetArgs, ConfluenceReadArgs};
use crate::dispatch::CommandResult;
use crate::profiles::InvocationOptions;
use clap::{Args, ValueEnum};
use craft_core::contracts::{CredentialCacheUpdate, Envelope, GrantRef, RequestState, ResourceRef};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::projection::{CompiledProjection, ProjectionContract, ProjectionMode};
use craft_core::registry::{Registry, response_schema, schema_for};
use craft_provider_confluence::PAGE_GET;
use craft_provider_confluence::pages::{PageRepresentation, PageRevision};
use craft_workflows::documents::Document;
use craft_workflows::plans::{Design, DocumentTarget, Plan};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub const PLAN_GET: &str = "plan.page-get";
pub const DESIGN_GET: &str = "design.page-get";
const NATIVE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Default, ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageFormat {
    #[default]
    Storage,
    Adf,
}
impl PageFormat {
    fn field(self) -> &'static str {
        match self {
            Self::Storage => "body_storage",
            Self::Adf => "body_atlas_doc_format",
        }
    }
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentGetArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub page: ConfluenceGetArgs,
    #[arg(long, value_enum, default_value = "storage")]
    #[serde(default)]
    pub format: PageFormat,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentSource {
    pub page: ResourceRef,
    pub space_id: String,
    pub title: String,
    pub revision: PageRevision,
    pub observed_at: u64,
    pub document_digest: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteDocumentData<T> {
    pub valid: bool,
    pub source: Option<DocumentSource>,
    pub document: Option<T>,
    pub task_order: Vec<String>,
    pub task_count: Option<usize>,
    pub section_count: Option<usize>,
    #[schemars(extend("const" = false))]
    pub review_verified: bool,
    #[schemars(extend("const" = 0))]
    pub tickets_created: u32,
    pub credential_cache_update: Option<CredentialCacheUpdate>,
}
fn empty(cache: Value) -> Value {
    json!({"valid":false,"source":null,"document":null,"task_order":[],"task_count":null,"section_count":null,"review_verified":false,"tickets_created":0,"credential_cache_update":cache})
}
fn internal() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Cannot import the qualified remote document.",
    )
}
fn budget() -> CraftError {
    let mut e = CraftError::new(
        ErrorCode::OutputBudgetTooSmall,
        "Output cannot retain the selected remote document and credential-cache evidence.",
    );
    e.next_action = "Increase --max-bytes or select fewer document fields; inspect the retained credential receipt before provisioning again.".into();
    e
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    for command in [PLAN_GET, DESIGN_GET] {
        let mut descriptor = registry.descriptor(PAGE_GET)?.clone();
        descriptor.id = command.into();
        descriptor.description = "Import the current visible shared document with its revision; no approval, tickets or local fallback.".into();
        descriptor.input_schema = schema_for::<DocumentGetArgs>()?;
        descriptor.response_schema = if command == PLAN_GET {
            response_schema::<RemoteDocumentData<Plan>>()?
        } else {
            response_schema::<RemoteDocumentData<Design>>()?
        };
        descriptor.projection = Some(ProjectionContract {
            item_pointer: "".into(),
            always_returned: [
                "valid",
                "source",
                "review_verified",
                "tickets_created",
                "credential_cache_update",
            ]
            .map(String::from)
            .into(),
            default_selected: [
                "valid",
                "source",
                "task_count",
                "section_count",
                "review_verified",
                "tickets_created",
                "credential_cache_update",
            ]
            .map(String::from)
            .into(),
            mode: ProjectionMode::Local,
        });
        descriptor.example = json!({"id":"101","format":"storage"});
        let mut example = empty(Value::Null);
        example["valid"] = json!(true);
        example["source"] = json!(DocumentSource {
            page: ResourceRef {
                provider: "confluence".into(),
                deployment: "cloud".into(),
                tenant: "00000000-1111-2222-3333-444444444444".into(),
                resource_type: "page".into(),
                id: "101".into()
            },
            space_id: "100".into(),
            title: "Shared proposal".into(),
            revision: PageRevision {
                version: 1,
                digest: format!("sha256:{}", "a".repeat(64)),
                representation: PageRepresentation::Storage
            },
            observed_at: 1000,
            document_digest: Some(format!("sha256:{}", "b".repeat(64))),
        });
        descriptor.response_example = json!(Envelope::success(command, "example".into(), example));
        registry.register_typed_handler::<DocumentGetArgs>(
            descriptor,
            Arc::new(crate::public_commands::VerifiedHostRequired),
        )?;
    }
    Ok(())
}

pub struct PreparedDocumentRead {
    command: &'static str,
    args: DocumentGetArgs,
    projection: CompiledProjection,
}
pub fn native_options<'a>(options: &InvocationOptions<'a>) -> InvocationOptions<'a> {
    InvocationOptions {
        max_bytes: NATIVE_BYTES,
        state_root: options.state_root,
        profile: options.profile,
        request_id: options.request_id,
    }
}
impl PreparedDocumentRead {
    pub fn prepare(
        registry: &Registry,
        command: &str,
        args: DocumentGetArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<Self, CraftError> {
        let command = match command {
            PLAN_GET => PLAN_GET,
            DESIGN_GET => DESIGN_GET,
            _ => {
                return Err(CraftError::field(
                    "/command",
                    "expected_remote_document_read",
                ));
            }
        };
        registry.validate(
            command,
            &serde_json::to_value(&args).map_err(|_| internal())?,
        )?;
        craft_core::contracts::validate_identity("/request_id", options.request_id)?;
        if options.request_id.len() > 80 {
            return Err(CraftError::field(
                "/request_id",
                "expected_at_most_80_bytes",
            ));
        }
        let projection = registry.projection(command, fields)?.ok_or_else(internal)?;
        let prepared = Self {
            command,
            args,
            projection,
        };
        prepared
            .native_args()
            .validate(&prepared.native_fields(), &native_options(options))?;
        let mut error = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        error.next_action = "x".repeat(1000);
        let cache = json!(CredentialCacheUpdate {
            grant: GrantRef {
                id: "g".repeat(512)
            },
            write_state: RequestState::SentUnknown,
            cleanup_pending: Some(false)
        });
        let mut envelope =
            Envelope::failure_with_data(command, options.request_id.into(), error, empty(cache));
        envelope.warnings = vec!["x".repeat(256), "x".repeat(256)];
        if !fits(&envelope, options.max_bytes) {
            return Err(budget());
        }
        Ok(prepared)
    }
    pub fn native_args(&self) -> ConfluenceReadArgs {
        ConfluenceReadArgs::PageGet(self.args.page.clone())
    }
    pub fn native_fields(&self) -> Vec<String> {
        [
            "id",
            "title",
            "status",
            "space_id",
            "revision",
            self.args.format.field(),
        ]
        .map(String::from)
        .into()
    }

    fn import(
        &self,
        native: &Value,
        data: &mut Value,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        let site: ResourceRef =
            serde_json::from_value(native["target"].clone()).map_err(|_| internal())?;
        site.validate()?;
        let page = &native["item"];
        if site.provider != "confluence"
            || site.deployment != "cloud"
            || site.resource_type != "site"
            || site.id != site.tenant
            || self
                .args
                .page
                .cloud_id
                .as_ref()
                .is_some_and(|cloud| cloud != &site.tenant)
            || page["id"] != self.args.page.id
            || page["status"] != "current"
        {
            return Err(CraftError::field(
                "/source",
                "remote_page_identity_or_current_state_mismatch",
            ));
        }
        let revision: PageRevision =
            serde_json::from_value(page["revision"].clone()).map_err(|_| internal())?;
        if revision.version == 0
            || !craft_core::pagination::valid_digest(&revision.digest)
            || !matches!(
                (self.args.format, revision.representation),
                (PageFormat::Storage, PageRepresentation::Storage)
                    | (PageFormat::Adf, PageRepresentation::AtlasDocFormat)
            )
        {
            return Err(CraftError::field(
                "/source/revision",
                "invalid_remote_page_revision",
            ));
        }
        let source = DocumentSource {
            page: ResourceRef {
                resource_type: "page".into(),
                id: self.args.page.id.clone(),
                ..site
            },
            space_id: text(page, "space_id")?.into(),
            title: text(page, "title")?.into(),
            revision,
            observed_at: native["observed_at"].as_u64().ok_or_else(internal)?,
            document_digest: None,
        };
        data["source"] = json!(source);
        let body = text(page, self.args.format.field())?;
        let document = match self.args.format {
            PageFormat::Storage => Document::from_storage(body),
            PageFormat::Adf => Document::from_adf(body.as_bytes()),
        }
        .map_err(|e| {
            let mut error = CraftError::field("/source/body", e.reason);
            error.message = e.to_string();
            error
        })?;
        let (target, document, digest) = if self.command == PLAN_GET {
            let mut plan = Plan::from_document(&document)?;
            // Confluence's page title is authoritative over the standalone document heading.
            plan.title = source.title.clone();
            data["task_order"] = json!(plan.validate()?);
            data["task_count"] = json!(plan.tasks.len());
            (plan.target.confluence.clone(), json!(plan), plan.digest()?)
        } else {
            let mut design = Design::from_document(&document)?;
            design.title = source.title.clone();
            design.validate()?;
            data["section_count"] = json!(design.sections.len());
            (design.target.clone(), json!(design), design.digest()?)
        };
        check_target(&target, &source)?;
        control.check()?;
        data["source"]["document_digest"] = json!(digest);
        data["document"] = document;
        data["valid"] = json!(true);
        Ok(())
    }
    pub fn finish(
        self,
        mut native: CommandResult,
        options: &InvocationOptions<'_>,
        control: &InvocationControl,
    ) -> CommandResult {
        let native_data = native.envelope.data.take().unwrap_or(Value::Null);
        let cache = native_data
            .get("credential_cache_update")
            .cloned()
            .unwrap_or(Value::Null);
        let recovery = self
            .projection
            .data(&empty(cache.clone()))
            .unwrap_or_else(|_| empty(cache.clone()));
        let mut data = empty(cache.clone());
        let error = if native.envelope.ok {
            self.import(&native_data, &mut data, control).err()
        } else {
            Some(native.envelope.error.take().unwrap_or_else(internal))
        };
        if error.is_some() {
            data["valid"] = json!(false);
            data["document"] = Value::Null;
            data["task_order"] = json!([]);
            data["task_count"] = Value::Null;
            data["section_count"] = Value::Null;
        }
        let (data, error) = match self.projection.data(&data) {
            Ok(v) => (v, error),
            Err(e) => (recovery.clone(), Some(e)),
        };
        let mut envelope = match error {
            Some(e) => {
                Envelope::failure_with_data(self.command, options.request_id.into(), e, data)
            }
            None => Envelope::success(self.command, options.request_id.into(), data),
        };
        let warnings = native.envelope.warnings;
        envelope.warnings = warnings.clone();
        if !fits(&envelope, options.max_bytes) {
            envelope = Envelope::failure_with_data(
                self.command,
                options.request_id.into(),
                budget(),
                recovery,
            );
            envelope.warnings = warnings;
        }
        CommandResult {
            envelope,
            output_policy: if cache.is_null() {
                crate::output::OutputPolicy::Bounded
            } else {
                crate::output::OutputPolicy::PreserveEvidence
            },
        }
    }
}
fn fits(envelope: &Envelope, bytes: usize) -> bool {
    serde_json::to_vec(envelope).is_ok_and(|v| v.len().saturating_add(1) <= bytes)
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, CraftError> {
    value[key]
        .as_str()
        .ok_or_else(|| CraftError::field("/source", "required_remote_page_field_not_returned"))
}
fn check_target(target: &DocumentTarget, source: &DocumentSource) -> Result<(), CraftError> {
    if target.confluence_cloud_id != source.page.tenant || target.space_id != source.space_id {
        return Err(CraftError::field(
            "/document/target",
            "document_destination_differs_from_observed_page",
        ));
    }
    Ok(())
}
pub fn execute(
    registry: &Registry,
    command: &str,
    args: DocumentGetArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> Result<CommandResult, CraftError> {
    let prepared = PreparedDocumentRead::prepare(registry, command, args, fields, options)?;
    let native = crate::confluence_reads::command::execute(
        prepared.native_args(),
        &prepared.native_fields(),
        &native_options(options),
        control,
    );
    Ok(prepared.finish(native, options, control))
}

#[cfg(test)]
mod tests;
