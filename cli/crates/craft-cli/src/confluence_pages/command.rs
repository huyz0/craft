//! Typed page-write candidates. Arguments select targets; saved private records supply authority.
#[cfg(test)]
mod tests;
use super::*;
use crate::profiles::{InvocationOptions, selected_profile_context};
use clap::Args;
use craft_core::contracts::{AuthMode, Envelope};
use craft_core::presence::InputField;
use craft_core::profiles::validate_profile_name;
use craft_core::registry::schema_for;
use craft_runtime::confluence_write_policy::{
    ConfluenceWriteOperation as Operation, ConfluenceWritePolicyRecord,
};
use craft_runtime::control::SystemClock;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::secrets::NativeCredentials;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    let value = craft_core::input::parse_object(text.as_bytes())
        .map_err(|_| "expected_strict_json_object")?;
    serde_json::from_value(value).map_err(|_| "unexpected_or_missing_object_fields".into())
}
fn body(text: &str) -> Result<pages::PageBody, String> {
    parse(text)
}
fn placement(text: &str) -> Result<PagePlacement, String> {
    parse(text)
}
fn revision(text: &str) -> Result<pages::PageRevision, String> {
    parse(text)
}
fn draft_policy(text: &str) -> Result<pages::EditorDraftPolicy, String> {
    serde_json::from_value(Value::String(text.into()))
        .map_err(|_| "expected_native_reconciliation_may_overwrite".into())
}
fn changes(text: &str) -> Result<PageChanges, String> {
    parse(text)
}

#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageCreateArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    pub cloud_id: Option<String>,
    #[arg(long)]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    pub title: String,
    #[arg(long, value_parser = placement, default_value = "{\"kind\":\"root\"}")]
    pub placement: PagePlacement,
    #[arg(long, value_parser = body, default_value = "{\"representation\":\"storage\",\"value\":\"\"}")]
    pub body: pages::PageBody,
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChanges {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub title: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub body: InputField<pages::PageBody>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageUpdateArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    pub cloud_id: Option<String>,
    #[arg(long)]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    pub id: String,
    #[arg(long, value_parser = revision, default_value = "{\"version\":0,\"digest\":\"\",\"representation\":\"storage\"}")]
    pub expected: pages::PageRevision,
    #[arg(long, value_parser = changes, default_value = "{}")]
    #[serde(default)]
    pub changes: PageChanges,
    /// Required acknowledgment that publication may overwrite divergent unpublished editor drafts.
    #[arg(long, value_parser = draft_policy)]
    #[schemars(required)]
    pub editor_draft_policy: Option<pages::EditorDraftPolicy>,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageTrashArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    pub cloud_id: Option<String>,
    #[arg(long)]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    pub id: String,
    #[arg(long, value_parser = revision, default_value = "{\"version\":0,\"digest\":\"\",\"representation\":\"storage\"}")]
    pub expected: pages::PageRevision,
}
fn comment_revision(
    text: &str,
) -> Result<craft_provider_confluence::comments::CommentRevision, String> {
    parse(text)
}
const NO_COMMENT_REVISION: &str = "{\"version\":0,\"digest\":\"\",\"representation\":\"storage\"}";
const EMPTY_BODY: &str = "{\"representation\":\"storage\",\"value\":\"\"}";
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentCreateArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    pub cloud_id: Option<String>,
    #[arg(long)]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    pub page_id: String,
    /// Reply to this current footer comment on the same page.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<String>,
    #[arg(long, value_parser = body, default_value = EMPTY_BODY)]
    pub body: pages::PageBody,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentUpdateArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    pub cloud_id: Option<String>,
    #[arg(long)]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    pub page_id: String,
    #[arg(long, default_value = "")]
    pub id: String,
    #[arg(long, value_parser = comment_revision, default_value = NO_COMMENT_REVISION)]
    pub expected: craft_provider_confluence::comments::CommentRevision,
    #[arg(long, value_parser = body, default_value = EMPTY_BODY)]
    pub body: pages::PageBody,
}
#[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentDeleteArgs {
    #[arg(long, default_value = "")]
    pub operation_id: String,
    #[arg(long)]
    #[serde(default)]
    pub apply: bool,
    #[arg(long)]
    pub cloud_id: Option<String>,
    #[arg(long)]
    pub space_id: Option<String>,
    #[arg(long, default_value = "")]
    pub page_id: String,
    #[arg(long, default_value = "")]
    pub id: String,
    #[arg(long, value_parser = comment_revision, default_value = NO_COMMENT_REVISION)]
    pub expected: craft_provider_confluence::comments::CommentRevision,
}
pub enum PageWriteArgs {
    Create(PageCreateArgs),
    Update(PageUpdateArgs),
    Trash(PageTrashArgs),
    CommentCreate(CommentCreateArgs),
    CommentUpdate(CommentUpdateArgs),
    CommentDelete(CommentDeleteArgs),
}
/// Maps a saved space allowance to the host write it permits.
fn kind_of(operation: Operation) -> PageWriteKind {
    match operation {
        Operation::Create => PageWriteKind::Create,
        Operation::Update => PageWriteKind::Update,
        Operation::Trash => PageWriteKind::Trash,
        Operation::CommentCreate => PageWriteKind::CommentCreate,
        Operation::CommentUpdate => PageWriteKind::CommentUpdate,
        Operation::CommentDelete => PageWriteKind::CommentDelete,
    }
}
fn site(cloud: &str) -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: cloud.into(),
        resource_type: "site".into(),
        id: cloud.into(),
    }
}
fn encoded(value: &impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_page_write_arguments"))
}
impl PageWriteArgs {
    pub fn operation(&self) -> Operation {
        match self {
            Self::Create(_) => Operation::Create,
            Self::Update(_) => Operation::Update,
            Self::Trash(_) => Operation::Trash,
            Self::CommentCreate(_) => Operation::CommentCreate,
            Self::CommentUpdate(_) => Operation::CommentUpdate,
            Self::CommentDelete(_) => Operation::CommentDelete,
        }
    }
    pub fn decode(command: &str, value: Value) -> Result<Self, CraftError> {
        match command {
            PAGE_CREATE => Ok(Self::Create(decode(&value)?)),
            PAGE_UPDATE => Ok(Self::Update(decode(&value)?)),
            PAGE_TRASH => Ok(Self::Trash(decode(&value)?)),
            COMMENT_CREATE => Ok(Self::CommentCreate(decode(&value)?)),
            COMMENT_UPDATE => Ok(Self::CommentUpdate(decode(&value)?)),
            COMMENT_DELETE => Ok(Self::CommentDelete(decode(&value)?)),
            _ => Err(CraftError::field("/command", "expected_page_write")),
        }
    }
    pub fn arguments(&self) -> Result<Value, CraftError> {
        match self {
            Self::Create(v) => encoded(v),
            Self::Update(v) => encoded(v),
            Self::Trash(v) => encoded(v),
            Self::CommentCreate(v) => encoded(v),
            Self::CommentUpdate(v) => encoded(v),
            Self::CommentDelete(v) => encoded(v),
        }
    }
    fn cloud(&self) -> Option<&str> {
        match self {
            Self::Create(v) => v.cloud_id.as_deref(),
            Self::Update(v) => v.cloud_id.as_deref(),
            Self::Trash(v) => v.cloud_id.as_deref(),
            Self::CommentCreate(v) => v.cloud_id.as_deref(),
            Self::CommentUpdate(v) => v.cloud_id.as_deref(),
            Self::CommentDelete(v) => v.cloud_id.as_deref(),
        }
    }
    fn space(&self) -> Option<&str> {
        match self {
            Self::Create(v) => v.space_id.as_deref(),
            Self::Update(v) => v.space_id.as_deref(),
            Self::Trash(v) => v.space_id.as_deref(),
            Self::CommentCreate(v) => v.space_id.as_deref(),
            Self::CommentUpdate(v) => v.space_id.as_deref(),
            Self::CommentDelete(v) => v.space_id.as_deref(),
        }
    }
    fn operation_id(&self) -> &str {
        match self {
            Self::Create(v) => &v.operation_id,
            Self::Update(v) => &v.operation_id,
            Self::Trash(v) => &v.operation_id,
            Self::CommentCreate(v) => &v.operation_id,
            Self::CommentUpdate(v) => &v.operation_id,
            Self::CommentDelete(v) => &v.operation_id,
        }
    }
    fn apply(&self) -> bool {
        match self {
            Self::Create(v) => v.apply,
            Self::Update(v) => v.apply,
            Self::Trash(v) => v.apply,
            Self::CommentCreate(v) => v.apply,
            Self::CommentUpdate(v) => v.apply,
            Self::CommentDelete(v) => v.apply,
        }
    }
    fn input(&self, space: &str) -> Result<Value, CraftError> {
        match self {
            Self::Create(v) => encoded(&PageCreateInput {
                space_id: space.into(),
                title: v.title.clone(),
                placement: v.placement.clone(),
                body: v.body.clone(),
            }),
            Self::Update(v) => encoded(&PageUpdateInput {
                id: v.id.clone(),
                space_id: space.into(),
                expected: v.expected.clone(),
                editor_draft_policy: v.editor_draft_policy.ok_or_else(|| {
                    CraftError::field(
                        "/editor_draft_policy",
                        "expected_native_reconciliation_may_overwrite",
                    )
                })?,
                title: v.changes.title.clone(),
                body: v.changes.body.clone(),
            }),
            Self::Trash(v) => encoded(&PageTrashInput {
                id: v.id.clone(),
                space_id: space.into(),
                expected: v.expected.clone(),
            }),
            Self::CommentCreate(v) => encoded(&pages::CommentCreateInput {
                space_id: space.into(),
                page_id: v.page_id.clone(),
                parent_comment_id: v.parent_comment_id.clone(),
                body: v.body.clone(),
            }),
            Self::CommentUpdate(v) => encoded(&pages::CommentUpdateInput {
                space_id: space.into(),
                page_id: v.page_id.clone(),
                id: v.id.clone(),
                expected: v.expected.clone(),
                body: v.body.clone(),
            }),
            Self::CommentDelete(v) => encoded(&pages::CommentDeleteInput {
                space_id: space.into(),
                page_id: v.page_id.clone(),
                id: v.id.clone(),
                expected: v.expected.clone(),
            }),
        }
    }
    fn request(
        &self,
        target: ResourceRef,
        space: &str,
        fields: &[String],
        bytes: usize,
    ) -> Result<CanonicalRequest, CraftError> {
        Ok(CanonicalRequest {
            command: self.operation().command().into(),
            target,
            input: self.input(space)?,
            selection: Selection {
                fields: fields.to_vec(),
                max_bytes: bytes
                    .try_into()
                    .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
                ..Selection::default()
            },
            preconditions: None,
        })
    }
    pub fn validate(
        &self,
        fields: &[String],
        options: &InvocationOptions<'_>,
    ) -> Result<(), CraftError> {
        if let Some(profile) = options.profile {
            validate_profile_name(profile)?;
        }
        craft_core::input::parse_object(
            &serde_json::to_vec(&self.arguments()?).map_err(|_| denied())?,
        )?;
        let target = site(
            self.cloud()
                .unwrap_or("00000000-1111-2222-3333-444444444444"),
        );
        let space = self.space().unwrap_or("1");
        PreparedPagePublication::prepare(
            self.request(target.clone(), space, fields, options.max_bytes)?,
            self.operation_id(),
            self.apply(),
            options.request_id,
            PageWritePolicy {
                target,
                space_id: space.into(),
                grant: GrantRef {
                    id: "g".repeat(512),
                },
                expected_actor: "a".repeat(512),
                enabled: true,
                allowed: PageWriteKind::ALL.to_vec(),
            },
        )?;
        Ok(())
    }
}
impl From<PageWriteArgs> for crate::args::ConfluenceAction {
    fn from(value: PageWriteArgs) -> Self {
        use crate::args::{ConfluenceCommentAction as C, ConfluencePageAction as P};
        let page = |action| Self::Page { action };
        let comment = |action| Self::Comment { action };
        match value {
            PageWriteArgs::Create(v) => page(P::Create(Box::new(v))),
            PageWriteArgs::Update(v) => page(P::Update(Box::new(v))),
            PageWriteArgs::Trash(v) => page(P::Trash(Box::new(v))),
            PageWriteArgs::CommentCreate(v) => comment(C::Create(Box::new(v))),
            PageWriteArgs::CommentUpdate(v) => comment(C::Update(Box::new(v))),
            PageWriteArgs::CommentDelete(v) => comment(C::Delete(Box::new(v))),
        }
    }
}
impl TryFrom<crate::args::ConfluenceAction> for PageWriteArgs {
    type Error = CraftError;
    fn try_from(value: crate::args::ConfluenceAction) -> Result<Self, Self::Error> {
        use crate::args::{
            ConfluenceAction as A, ConfluenceCommentAction as C, ConfluencePageAction as P,
        };
        match value {
            A::Page {
                action: P::Create(v),
            } => Ok(Self::Create(*v)),
            A::Page {
                action: P::Update(v),
            } => Ok(Self::Update(*v)),
            A::Page {
                action: P::Trash(v),
            } => Ok(Self::Trash(*v)),
            A::Comment {
                action: C::Create(v),
            } => Ok(Self::CommentCreate(*v)),
            A::Comment {
                action: C::Update(v),
            } => Ok(Self::CommentUpdate(*v)),
            A::Comment {
                action: C::Delete(v),
            } => Ok(Self::CommentDelete(*v)),
            _ => Err(CraftError::field("/command", "expected_page_write")),
        }
    }
}
/// Candidate discovery must expose the complete host receipt rather than provider metadata alone.
pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
    register_commands(
        registry,
        &[
            PAGE_CREATE,
            PAGE_UPDATE,
            PAGE_TRASH,
            COMMENT_CREATE,
            COMMENT_UPDATE,
            COMMENT_DELETE,
        ],
    )
}
/// Updates are public only with the explicit native editor-draft reconciliation policy.
pub fn register_public(registry: &mut Registry) -> Result<(), CraftError> {
    register_commands(
        registry,
        &[
            PAGE_CREATE,
            PAGE_UPDATE,
            PAGE_TRASH,
            COMMENT_CREATE,
            COMMENT_UPDATE,
            COMMENT_DELETE,
        ],
    )
}
fn register_commands(registry: &mut Registry, commands: &[&str]) -> Result<(), CraftError> {
    let mut native = Registry::default();
    native.register_provider(&ConfluencePages)?;
    for &command in commands {
        let mut d = native.descriptor(command)?.clone();
        let mut example = d.example.clone();
        if command == PAGE_UPDATE {
            let title = example
                .as_object_mut()
                .ok_or_else(denied)?
                .remove("title")
                .ok_or_else(denied)?;
            example["changes"] = json!({"title":title});
            example["editor_draft_policy"] = example
                .as_object_mut()
                .ok_or_else(denied)?
                .remove("editor_draft_policy")
                .ok_or_else(denied)?;
        }
        example["operation_id"] = json!("publish-plan");
        example["apply"] = json!(true);
        d.example = example;
        d.response_schema = super::schema()?;
        let kind = PageWriteKind::from_command(command).ok_or_else(denied)?;
        let resource = kind.resource_type();
        let version = match kind {
            PageWriteKind::Update | PageWriteKind::CommentUpdate => Value::from(2),
            PageWriteKind::Trash | PageWriteKind::CommentDelete => Value::Null,
            _ => Value::from(1),
        };
        let status = match kind {
            PageWriteKind::Trash | PageWriteKind::CommentDelete => 204,
            PageWriteKind::CommentCreate => 201,
            _ => 200,
        };
        d.response_example = json!(Envelope::success(
            command,
            "example".into(),
            json!({
                "receipt":{"operation_id":"publish-plan","request_state":"confirmed","confirmed_resources":[{"provider":"confluence","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":resource,"id":if resource == "page" {"101"} else {"201"}}],"job":null},
                "target":site("00000000-1111-2222-3333-444444444444"),"space_id":"100","version":version,"persistence":"recorded","journal":null,"http_status":status,"provider_request_id":null,"retry_after":null,"reconciliation_required":false,"replay_authorized":false,"editor_draft_policy":if command==PAGE_UPDATE {json!("native_reconciliation_may_overwrite")} else {Value::Null},"editor_draft_preservation_verified":false,"credential_cache_update":null
            })
        ));
        match command {
            PAGE_CREATE => {
                d.input_schema = schema_for::<PageCreateArgs>()?;
                registry.register_typed::<PageCreateArgs>(d)?;
            }
            PAGE_UPDATE => {
                d.input_schema = schema_for::<PageUpdateArgs>()?;
                registry.register_typed::<PageUpdateArgs>(d)?;
            }
            PAGE_TRASH => {
                d.input_schema = schema_for::<PageTrashArgs>()?;
                registry.register_typed::<PageTrashArgs>(d)?;
            }
            COMMENT_CREATE => {
                d.input_schema = schema_for::<CommentCreateArgs>()?;
                registry.register_typed::<CommentCreateArgs>(d)?;
            }
            COMMENT_UPDATE => {
                d.input_schema = schema_for::<CommentUpdateArgs>()?;
                registry.register_typed::<CommentUpdateArgs>(d)?;
            }
            _ => {
                d.input_schema = schema_for::<CommentDeleteArgs>()?;
                registry.register_typed::<CommentDeleteArgs>(d)?;
            }
        }
    }
    Ok(())
}
pub struct PreparedInvocation {
    host: PreparedPagePublication,
    policy: ConfluenceWritePolicyRecord,
    operation: Operation,
}
impl PreparedInvocation {
    pub fn prepare(
        args: PageWriteArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        args.validate(fields, options)?;
        control.check()?;
        let (name, _, hints) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "confluence"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(denied());
        }
        let explicit = args.cloud().map(site);
        let site = if explicit.is_some()
            || hints.targets.contains_key("site")
            || profile.defaults.contains_key("site")
        {
            profile.resolve_target("site", explicit.as_ref(), &hints)?
        } else {
            site(&profile.tenant_ref()?.tenant)
        };
        profile.validate_target("site", &site)?;
        let explicit_space = args.space().map(|id| ResourceRef {
            resource_type: "space".into(),
            id: id.into(),
            ..site.clone()
        });
        let space = profile.resolve_target("space", explicit_space.as_ref(), &hints)?;
        profile.validate_target("space", &space)?;
        if space.tenant_ref() != site.tenant_ref() {
            return Err(denied());
        }
        let operation = args.operation();
        let policy = store
            .confluence_write_policy(&name, &space.id, control)?
            .ok_or_else(denied)?;
        if policy.target != space {
            return Err(denied());
        }
        store.verify_confluence_write_policy(&policy, operation, control)?;
        let host = PreparedPagePublication::prepare(
            args.request(site.clone(), &space.id, fields, options.max_bytes)?,
            args.operation_id(),
            args.apply(),
            options.request_id,
            PageWritePolicy {
                target: site,
                space_id: space.id,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                enabled: policy.enabled,
                allowed: policy.operations.iter().copied().map(kind_of).collect(),
            },
        )?;
        Ok(Self {
            host,
            policy,
            operation,
        })
    }
    pub fn grant(&self) -> &GrantRef {
        &self.policy.grant
    }
    pub fn binding(&self) -> &craft_core::storage::OperationBinding {
        self.host.binding()
    }
    pub fn execute_cached(
        self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        journal: &NativeJournal,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        let check = control.clone();
        self.host
            .execute_cached_with_policy_check(client, locked, journal, tls, control, || {
                store.verify_confluence_write_policy(&self.policy, self.operation, &check)
            })
    }
}
pub fn execute(
    args: PageWriteArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.operation().command();
    let result = (|| {
        args.validate(fields, options)?;
        let root = resolve_state_root(options.state_root, |k| std::env::var_os(k))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let journal = NativeJournal::open(&store, control).map_err(|_| {
            CraftError::new(
                ErrorCode::Internal,
                "Cannot open the selected page publication journal.",
            )
        })?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute_cached(
            &store,
            &client,
            &locked,
            &journal,
            TlsPolicy::Platform,
            control.clone(),
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: crate::output::OutputPolicy::PreserveEvidence,
    })
}
