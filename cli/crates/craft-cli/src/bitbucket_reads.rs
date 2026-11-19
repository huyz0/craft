//! Bitbucket reads through a provisioned static grant, a separately enabled read policy and the
//! shared buffered paging engine. Preparation touches no credential or network.

use crate::dispatch::CommandResult;
use crate::output::OutputPolicy;
use crate::profiles::{InvocationOptions, selected_profile_context};
use clap::{Args, Subcommand};
use craft_core::contracts::{
    AuthMode, CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection,
};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::input::canonical_digest;
use craft_core::pagination::{
    ListBinding, ListPage, ListRequest, NativePage, NativePosition, PageSource,
};
use craft_core::projection::CompiledProjection;
use craft_core::registry::{Registry, schema_for};
use craft_provider_bitbucket::{self as provider, BitbucketContext, commands, is_get};
use craft_runtime::auth::ServiceOAuthClient;
use craft_runtime::control::SystemClock;
use craft_runtime::pagination::recovery_probe;
use craft_runtime::profiles::{ProfileStore, resolve_state_root};
use craft_runtime::provider_policy::ProviderPolicyRecord;
use craft_runtime::secrets::{LockedCredentials, NativeCredentials};
use craft_runtime::transport::{HttpContext, TlsPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const READ_CLASS: &str = "bitbucket.read";

fn page_size() -> u16 {
    25
}
fn limit() -> u16 {
    20
}

/// The output controls every list shares. A get has none.
trait Controls {
    fn controls(&self) -> (u16, Option<String>) {
        (20, None)
    }
}

/// Declares one argument struct. `get` structs carry only the native selectors; `paged` structs add
/// the page size and the output controls (`--limit`, `--cursor`). The provider input is the
/// serialized struct without the output controls, so flags, JSON input and the provider agree.
macro_rules! args {
    (get $name:ident { $( $(#[$m:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        #[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $( $(#[$m])* pub $field: $ty, )*
        }
        impl Controls for $name {}
    };
    (paged $name:ident { $( $(#[$m:meta])* $field:ident : $ty:ty ),* $(,)? }) => {
        #[derive(Args, Clone, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $( $(#[$m])* pub $field: $ty, )*
            #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u16).range(1..=50))]
            #[serde(default = "page_size")]
            #[schemars(range(min = 1, max = 50))]
            pub page_size: u16,
            #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
            #[serde(default = "limit")]
            #[schemars(range(min = 1, max = 100))]
            pub limit: u16,
            #[arg(long)]
            #[schemars(length(min = 36, max = 36))]
            pub cursor: Option<String>,
        }
        impl Controls for $name {
            fn controls(&self) -> (u16, Option<String>) {
                (self.limit, self.cursor.clone())
            }
        }
    };
}

args!(paged PageArgs {});
args!(get RepoArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
});
args!(paged RepoPageArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
});
args!(paged BranchListArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 100))]
    name_contains: Option<String>,
});
args!(paged CommitListArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long)]
    #[schemars(length(min = 1, max = 250))]
    branch: Option<String>,
});
args!(paged PullRequestListArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, value_parser = ["OPEN", "MERGED", "DECLINED", "SUPERSEDED"])]
    #[schemars(regex(pattern = "^(OPEN|MERGED|DECLINED|SUPERSEDED)$"))]
    state: Option<String>,
});
args!(get PullRequestArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
});
args!(paged PullRequestPageArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value_t = 0)]
    #[schemars(range(min = 1, max = 2147483647))]
    pull_request_id: u64,
});
args!(get PipelineArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 36, max = 38))]
    pipeline_uuid: String,
});
args!(paged PipelinePageArgs {
    #[arg(long, default_value = "")]
    #[schemars(length(min = 1, max = 100), regex(pattern = "^[a-z0-9][a-z0-9._-]*$"))]
    repo: String,
    #[arg(long, default_value = "")]
    #[schemars(length(min = 36, max = 38))]
    pipeline_uuid: String,
});

#[derive(Subcommand)]
pub enum BitbucketAction {
    Repository {
        #[command(subcommand)]
        action: RepositoryAction,
    },
    Branch {
        #[command(subcommand)]
        action: BranchAction,
    },
    Commit {
        #[command(subcommand)]
        action: CommitAction,
    },
    Pullrequest {
        #[command(subcommand)]
        action: PullrequestAction,
    },
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },
    Pipeline {
        #[command(subcommand)]
        action: PipelineAction,
    },
    Step {
        #[command(subcommand)]
        action: StepAction,
    },
}
#[derive(Subcommand)]
pub enum RepositoryAction {
    List(PageArgs),
    Get(RepoArgs),
}
#[derive(Subcommand)]
pub enum BranchAction {
    List(BranchListArgs),
}
#[derive(Subcommand)]
pub enum CommitAction {
    List(CommitListArgs),
}
#[derive(Subcommand)]
pub enum PullrequestAction {
    List(PullRequestListArgs),
    Get(PullRequestArgs),
    Diffstat(PullRequestPageArgs),
}
#[derive(Subcommand)]
pub enum CommentAction {
    /// List a pull request's comments.
    List(PullRequestPageArgs),
}
#[derive(Subcommand)]
pub enum PipelineAction {
    List(RepoPageArgs),
    Get(PipelineArgs),
}
#[derive(Subcommand)]
pub enum StepAction {
    /// List a pipeline run's steps.
    List(PipelinePageArgs),
}

/// One row per read: the flat variant, its argument type and its command id. Everything else
/// (command id, provider input, output controls, schema registration) derives from the rows.
macro_rules! reads {
    ($( $variant:ident($ty:ty) => $command:literal ),* $(,)?) => {
        /// Flat view of every Bitbucket read, shared by parsing, JSON input and the host.
        #[derive(Clone)]
        pub enum BitbucketReadArgs {
            $( $variant($ty), )*
        }

        impl BitbucketReadArgs {
            pub fn command(&self) -> &'static str {
                match self { $( Self::$variant(_) => $command, )* }
            }

            pub fn arguments(&self) -> Result<Value, CraftError> {
                match self { $( Self::$variant(v) => encoded(v), )* }
            }

            fn controls(&self) -> (u16, Option<String>) {
                match self { $( Self::$variant(v) => v.controls(), )* }
            }

            /// Rebuilds the arguments from a JSON object for the command the parser selected.
            fn from_input(command: &str, input: Value) -> Result<Self, CraftError> {
                match command {
                    $( $command => Ok(Self::$variant(decode(input)?)), )*
                    _ => Err(CraftError::field("/command", "unknown_bitbucket_read")),
                }
            }
        }

        /// Registers the CLI-level schemas (with output controls) over the provider's descriptors.
        pub fn register_candidates(registry: &mut Registry) -> Result<(), CraftError> {
            let mut native = Registry::default();
            native.register_provider(&BitbucketContext)?;
            for command in commands() {
                let mut descriptor = native.descriptor(command)?.clone();
                match command {
                    $( $command => {
                        descriptor.input_schema = schema_for::<$ty>()?;
                        registry.register_typed::<$ty>(descriptor)?
                    } )*
                    _ => return Err(CraftError::field("/command", "unknown_bitbucket_read")),
                }
            }
            Ok(())
        }
    };
}

reads! {
    RepositoryList(PageArgs) => "bitbucket.repository.list",
    RepositoryGet(RepoArgs) => "bitbucket.repository.get",
    BranchList(BranchListArgs) => "bitbucket.branch.list",
    CommitList(CommitListArgs) => "bitbucket.commit.list",
    PullRequestList(PullRequestListArgs) => "bitbucket.pullrequest.list",
    PullRequestGet(PullRequestArgs) => "bitbucket.pullrequest.get",
    PullRequestDiffstat(PullRequestPageArgs) => "bitbucket.pullrequest.diffstat",
    CommentList(PullRequestPageArgs) => "bitbucket.comment.list",
    PipelineList(RepoPageArgs) => "bitbucket.pipeline.list",
    PipelineGet(PipelineArgs) => "bitbucket.pipeline.get",
    StepList(PipelinePageArgs) => "bitbucket.step.list",
}

fn encoded(value: &impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_bitbucket_arguments"))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CraftError> {
    serde_json::from_value(value)
        .map_err(|_| CraftError::field("/input", "invalid_typed_bitbucket_arguments"))
}

impl BitbucketAction {
    pub fn read(&self) -> BitbucketReadArgs {
        use BitbucketReadArgs as R;
        match self {
            Self::Repository { action } => match action {
                RepositoryAction::List(v) => R::RepositoryList(v.clone()),
                RepositoryAction::Get(v) => R::RepositoryGet(v.clone()),
            },
            Self::Branch {
                action: BranchAction::List(v),
            } => R::BranchList(v.clone()),
            Self::Commit {
                action: CommitAction::List(v),
            } => R::CommitList(v.clone()),
            Self::Pullrequest { action } => match action {
                PullrequestAction::List(v) => R::PullRequestList(v.clone()),
                PullrequestAction::Get(v) => R::PullRequestGet(v.clone()),
                PullrequestAction::Diffstat(v) => R::PullRequestDiffstat(v.clone()),
            },
            Self::Comment {
                action: CommentAction::List(v),
            } => R::CommentList(v.clone()),
            Self::Pipeline { action } => match action {
                PipelineAction::List(v) => R::PipelineList(v.clone()),
                PipelineAction::Get(v) => R::PipelineGet(v.clone()),
            },
            Self::Step {
                action: StepAction::List(v),
            } => R::StepList(v.clone()),
        }
    }

    /// Rebuilds the action from a JSON object for the command the parser selected.
    pub fn with_input(&self, input: Value) -> Result<Self, CraftError> {
        let read = BitbucketReadArgs::from_input(self.read().command(), input)?;
        use BitbucketReadArgs as R;
        Ok(match read {
            R::RepositoryList(v) => Self::Repository {
                action: RepositoryAction::List(v),
            },
            R::RepositoryGet(v) => Self::Repository {
                action: RepositoryAction::Get(v),
            },
            R::BranchList(v) => Self::Branch {
                action: BranchAction::List(v),
            },
            R::CommitList(v) => Self::Commit {
                action: CommitAction::List(v),
            },
            R::PullRequestList(v) => Self::Pullrequest {
                action: PullrequestAction::List(v),
            },
            R::PullRequestGet(v) => Self::Pullrequest {
                action: PullrequestAction::Get(v),
            },
            R::PullRequestDiffstat(v) => Self::Pullrequest {
                action: PullrequestAction::Diffstat(v),
            },
            R::CommentList(v) => Self::Comment {
                action: CommentAction::List(v),
            },
            R::PipelineList(v) => Self::Pipeline {
                action: PipelineAction::List(v),
            },
            R::PipelineGet(v) => Self::Pipeline {
                action: PipelineAction::Get(v),
            },
            R::StepList(v) => Self::Step {
                action: StepAction::List(v),
            },
        })
    }
}

impl BitbucketReadArgs {
    /// The provider's input: the native selectors and page size, without output controls.
    fn input(&self) -> Result<Value, CraftError> {
        let mut input = self.arguments()?;
        if let Some(object) = input.as_object_mut() {
            object.remove("limit");
            object.remove("cursor");
        }
        Ok(input)
    }

    fn selection(&self, fields: &[String], bytes: usize) -> Result<Selection, CraftError> {
        let (limit, cursor) = self.controls();
        if cursor.as_ref().is_some_and(|c| {
            !c.strip_prefix("cur_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) {
            return Err(CraftError::field(
                "/cursor",
                "expected_opaque_cursor_handle",
            ));
        }
        Ok(Selection {
            fields: fields.to_vec(),
            limit,
            cursor,
            max_bytes: bytes
                .try_into()
                .map_err(|_| CraftError::field("/max_bytes", "invalid_output_budget"))?,
        })
    }
}

fn workspace(tenant: &str) -> ResourceRef {
    ResourceRef {
        provider: "bitbucket".into(),
        deployment: "cloud".into(),
        tenant: tenant.into(),
        resource_type: "workspace".into(),
        id: tenant.into(),
    }
}

/// Trusted host authority for one read, independent of any provider response.
pub struct BitbucketReadPolicy {
    pub target: ResourceRef,
    pub grant: GrantRef,
    pub expected_actor: String,
    pub enabled: bool,
    pub allowed_commands: Vec<String>,
}

/// A fully prepared read. It performs no state, credential or network access.
pub struct PreparedBitbucketRead {
    registry: Registry,
    operation: PreparedOperation,
    projection: CompiledProjection,
    binding: ListBinding,
    request_id: String,
}

fn internal() -> CraftError {
    CraftError::new(ErrorCode::Internal, "Cannot encode Bitbucket read output.")
}

impl PreparedBitbucketRead {
    pub fn prepare(
        request: CanonicalRequest,
        request_id: &str,
        policy: BitbucketReadPolicy,
    ) -> Result<Self, CraftError> {
        craft_core::contracts::validate_identity("/request_id", request_id)?;
        policy.grant.validate()?;
        if !policy.grant.id.strip_prefix("gr_").is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) || !policy.enabled
            || !policy
                .allowed_commands
                .iter()
                .any(|command| command == &request.command)
            || policy.target != request.target
            || !commands().any(|command| command == request.command)
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Bitbucket read differs from the exact configured workspace, grant or enabled operations.",
            ));
        }
        let mut registry = Registry::default();
        registry.register_provider(&BitbucketContext)?;
        let operation = registry.prepare(request)?;
        let command = operation.request().command.clone();
        let projection = registry
            .projection(&command, &operation.request().selection.fields)?
            .ok_or_else(internal)?;
        let descriptor = registry.prepared_descriptor(&operation)?;
        let binding = ListBinding {
            operation: command.clone(),
            descriptor_digest: canonical_digest(
                &serde_json::to_value(descriptor).map_err(|_| internal())?,
            )?,
            schema_digest: registry.schema_digest(&command, true)?,
            projection_digest: canonical_digest(
                &projection.response_schema(&descriptor.response_schema)?,
            )?,
            api_revision: descriptor.api_revision.clone(),
            target: operation.request().target.clone(),
            grant: policy.grant,
            actor: policy.expected_actor,
            query: BTreeMap::from([("input".into(), operation.request().input.clone())]),
        };
        binding.digest()?;
        let prepared = Self {
            registry,
            operation,
            projection,
            binding,
            request_id: request_id.into(),
        };
        // Reserve room for the largest error envelope before any credential or network use.
        let mut worst = CraftError::new(ErrorCode::Internal, "x".repeat(500));
        worst.next_action = "x".repeat(1000);
        if !is_get(&command)
            && !prepared.fits(&prepared.envelope(Some(&recovery_probe()), None)?)?
            || !prepared.fits(&prepared.envelope(None, Some(worst))?)?
        {
            let mut error = CraftError::new(
                ErrorCode::OutputBudgetTooSmall,
                "Bitbucket reads require room for complete error and paging evidence.",
            );
            error.next_action = "Increase --max-bytes before authentication.".into();
            return Err(error);
        }
        Ok(prepared)
    }

    pub fn grant(&self) -> &GrantRef {
        &self.binding.grant
    }

    fn fits(&self, envelope: &Envelope) -> Result<bool, CraftError> {
        Ok(serde_json::to_vec(envelope)
            .map_err(|_| internal())?
            .len()
            .saturating_add(1)
            <= self.operation.request().selection.max_bytes as usize)
    }

    fn envelope(
        &self,
        page: Option<&ListPage>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let command = &self.binding.operation;
        let mut out_page = None;
        let mut error = error;
        let data = if is_get(command) {
            json!({"target":self.binding.target,"item":null,"observed_at":null})
        } else {
            out_page = page.map(|page| page.page.clone());
            error = error.or_else(|| page.and_then(|page| page.error.clone()));
            json!({
                "target":self.binding.target,
                "items":page.map(|page| page.items.as_slice()).unwrap_or(&[]),
                "observation":page.map(|page| &page.observation),
                "cached":page.is_some_and(|page| page.cached),
            })
        };
        self.finish_envelope(data, out_page, error)
    }

    fn finish_envelope(
        &self,
        data: Value,
        page: Option<craft_core::contracts::Page>,
        error: Option<CraftError>,
    ) -> Result<Envelope, CraftError> {
        let data = self.projection.data(&data)?;
        let command = &self.binding.operation;
        let mut envelope = match error {
            Some(error) => {
                Envelope::failure_with_data(command, self.request_id.clone(), error, data)
            }
            None => Envelope::success(command, self.request_id.clone(), data),
        };
        envelope.page = page;
        Ok(envelope)
    }

    fn result(&self, envelope: Result<Envelope, CraftError>) -> CommandResult {
        CommandResult {
            envelope: envelope.unwrap_or_else(|error| {
                Envelope::failure(&self.binding.operation, self.request_id.clone(), error)
            }),
            output_policy: OutputPolicy::Bounded,
        }
    }

    /// Opens the stored static grant, rechecks policy at every native boundary and reads.
    pub fn execute(
        &self,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        store: &ProfileStore,
        tls: TlsPolicy,
        control: InvocationControl,
        check_policy: &dyn Fn() -> Result<(), CraftError>,
    ) -> CommandResult {
        let before = (|| {
            control.check()?;
            check_policy()?;
            if locked.grant() != self.binding.grant {
                return Err(CraftError::new(
                    ErrorCode::PolicyDenied,
                    "Locked grant differs from the prepared Bitbucket-read binding.",
                ));
            }
            if !is_get(&self.binding.operation) {
                store.check_provider_cursor(&self.request(), &SystemClock::default(), &control)?;
            }
            Ok(())
        })();
        if let Err(error) = before {
            return self.result(self.envelope(None, Some(error)));
        }
        let client = match client.for_invocation(control.clone()) {
            Ok(client) => client,
            Err(error) => return self.result(self.envelope(None, Some(error))),
        };
        let mut source = BitbucketSource {
            prepared: self,
            client: &client,
            locked,
            tls: tls.clone(),
            check_policy,
        };
        if is_get(&self.binding.operation) {
            let result = (|| {
                source.check_access(&self.binding, &control)?;
                let mut context = source.context(&control)?;
                let item = provider::read_get(
                    &self.operation,
                    &mut Guarded {
                        native: &mut context,
                        check: check_policy,
                    },
                )?;
                source.check_access(&self.binding, &control)?;
                let envelope = self.finish_envelope(
                    json!({
                        "target":self.binding.target,
                        "item":item,
                        "observed_at":craft_core::storage::Clock::unix_millis(&SystemClock::default()),
                    }),
                    None,
                    None,
                )?;
                if !self.fits(&envelope)? {
                    let mut error = CraftError::new(
                        ErrorCode::ItemTooLarge,
                        "The selected Bitbucket item cannot fit in the complete output envelope.",
                    );
                    error.next_action =
                        "Select fewer fields or increase --max-bytes and perform a new live read."
                            .into();
                    return Err(error);
                }
                check_policy()?;
                control.check()?;
                Ok(envelope)
            })();
            return self.result(result.or_else(|error| self.envelope(None, Some(error))));
        }
        let result = store.provider_page(
            self.request(),
            &SystemClock::default(),
            &control,
            &mut source,
            |page| self.fits(&self.envelope(Some(page), None)?),
        );
        match result {
            Ok(page) => match check_policy().and_then(|()| control.check()) {
                Ok(()) => self.result(self.envelope(Some(&page), None)),
                Err(error) => self.result(self.envelope(None, Some(error))),
            },
            Err(error) => self.result(self.envelope(None, Some(error))),
        }
    }

    fn request(&self) -> ListRequest<'_> {
        ListRequest {
            binding: &self.binding,
            cursor: self.operation.request().selection.cursor.as_deref(),
            limit: self.operation.request().selection.limit,
        }
    }
}

struct BitbucketSource<'a> {
    prepared: &'a PreparedBitbucketRead,
    client: &'a ServiceOAuthClient,
    locked: &'a LockedCredentials<'a>,
    tls: TlsPolicy,
    check_policy: &'a dyn Fn() -> Result<(), CraftError>,
}

impl BitbucketSource<'_> {
    fn context(&self, control: &InvocationControl) -> Result<HttpContext, CraftError> {
        let session = self
            .client
            .open_static_session(
                self.locked,
                &self.prepared.binding.target.tenant_ref(),
                &self.prepared.binding.actor,
            )
            .map_err(|failure| failure.error)?;
        HttpContext::for_operation(
            &self.prepared.registry,
            &self.prepared.operation,
            &self.prepared.binding.grant,
            session,
            self.tls.clone(),
            control.clone(),
        )
    }
}

impl PageSource for BitbucketSource<'_> {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        (self.check_policy)()?;
        if binding != &self.prepared.binding || self.locked.grant() != binding.grant {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Bitbucket traversal binding changed.",
            ));
        }
        Ok(())
    }

    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.check_access(binding, control)?;
        let mut context = self.context(control)?;
        provider::read_page(
            &self.prepared.operation,
            &mut Guarded {
                native: &mut context,
                check: self.check_policy,
            },
            position,
        )
    }
}

/// Rechecks trusted policy before every native call the provider makes.
struct Guarded<'a> {
    native: &'a mut dyn craft_core::execution::OperationContext,
    check: &'a dyn Fn() -> Result<(), CraftError>,
}
impl craft_core::execution::OperationContext for Guarded<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(op)
    }
    fn invoke(
        &mut self,
        call: &craft_core::execution::NativeCall,
    ) -> Result<craft_core::execution::NativeResponse, CraftError> {
        self.checkpoint()?;
        self.native.invoke(call)
    }
    fn normalize_next(
        &self,
        call: &craft_core::execution::NativeCall,
        link: &str,
    ) -> Result<craft_core::execution::NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
}

/// Everything needed to run one read after resolving profile, policy and request.
pub struct PreparedInvocation {
    read: PreparedBitbucketRead,
    policy: ProviderPolicyRecord,
}

impl PreparedInvocation {
    pub fn prepare(
        args: &BitbucketReadArgs,
        fields: &[String],
        options: &InvocationOptions<'_>,
        store: &ProfileStore,
        control: &InvocationControl,
    ) -> Result<Self, CraftError> {
        control.check()?;
        let (name, _, _) = selected_profile_context(store, None, options, control)?;
        let profile = store.get(&name, control)?;
        if profile.provider != "bitbucket"
            || profile.deployment != "cloud"
            || profile.auth_mode != AuthMode::Service
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Bitbucket reads require a configured Bitbucket Cloud service profile.",
            ));
        }
        let target = workspace(&profile.tenant_ref()?.tenant);
        let policy = store
            .provider_policy(&name, READ_CLASS, control)?
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::PolicyDenied,
                    "No trusted Bitbucket read policy is present.",
                )
            })?;
        store.verify_provider_policy(&policy, args.command(), control)?;
        let read = PreparedBitbucketRead::prepare(
            CanonicalRequest {
                command: args.command().into(),
                target: target.clone(),
                input: args.input()?,
                selection: args.selection(fields, options.max_bytes)?,
                preconditions: None,
            },
            options.request_id,
            BitbucketReadPolicy {
                target,
                grant: policy.grant.clone(),
                expected_actor: policy.expected_actor.clone(),
                enabled: policy.enabled,
                allowed_commands: policy.operations.clone(),
            },
        )?;
        Ok(Self { read, policy })
    }

    pub fn grant(&self) -> &GrantRef {
        self.read.grant()
    }

    pub fn execute(
        &self,
        store: &ProfileStore,
        client: &ServiceOAuthClient,
        locked: &LockedCredentials<'_>,
        tls: TlsPolicy,
        control: InvocationControl,
    ) -> CommandResult {
        let check = control.clone();
        let command = self.read.binding.operation.clone();
        self.read.execute(client, locked, store, tls, control, &|| {
            store.verify_provider_policy(&self.policy, &command, &check)
        })
    }
}

/// Validates before private state and uses only the selected profile's native credential grant.
pub fn execute(
    args: BitbucketReadArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    let command = args.command();
    let result = (|| {
        let root = resolve_state_root(options.state_root, |key| std::env::var_os(key))?;
        let store = ProfileStore::open(&root, control)?;
        let prepared = PreparedInvocation::prepare(&args, fields, options, &store, control)?;
        let credentials = NativeCredentials::open(&store)?;
        let locked = credentials.lock(prepared.grant(), control)?;
        let client = ServiceOAuthClient::new(
            TlsPolicy::Platform,
            control.clone(),
            Arc::new(SystemClock::default()),
        )?;
        Ok(prepared.execute(
            &store,
            &client,
            &locked,
            TlsPolicy::Platform,
            control.clone(),
        ))
    })();
    result.unwrap_or_else(|error| CommandResult {
        envelope: Envelope::failure(command, options.request_id.into(), error),
        output_policy: OutputPolicy::Bounded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "team";

    fn policy(command: &str) -> BitbucketReadPolicy {
        BitbucketReadPolicy {
            target: workspace(WORKSPACE),
            grant: GrantRef {
                id: format!("gr_{}", "0".repeat(32)),
            },
            expected_actor: "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".into(),
            enabled: true,
            allowed_commands: vec![command.into()],
        }
    }

    fn request(command: &str, input: Value) -> CanonicalRequest {
        CanonicalRequest {
            command: command.into(),
            target: workspace(WORKSPACE),
            input,
            selection: Selection::default(),
            preconditions: None,
        }
    }

    #[test]
    fn preparation_refuses_untrusted_authority_input_fields_and_recovery_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = json!({"repo": "service"});
        for mode in [
            "disabled", "excluded", "grant", "target", "fields", "input", "budget",
        ] {
            let mut policy = policy("bitbucket.repository.get");
            let mut request = request("bitbucket.repository.get", input.clone());
            match mode {
                "disabled" => policy.enabled = false,
                "excluded" => policy.allowed_commands.clear(),
                "grant" => policy.grant.id = "unissued".into(),
                "target" => request.target.provider = "jira".into(),
                "fields" => request.selection.fields = vec!["secret_token".into()],
                "input" => request.input["workspace"] = json!("other"),
                _ => request.selection.max_bytes = 512,
            }
            let error = PreparedBitbucketRead::prepare(request, "request", policy)
                .err()
                .ok_or("expected refusal")?;
            if mode == "budget" {
                assert_eq!(error.code, ErrorCode::OutputBudgetTooSmall);
            }
        }
        Ok(())
    }

    #[test]
    fn a_policy_for_one_command_does_not_allow_another() {
        let result = PreparedBitbucketRead::prepare(
            request("bitbucket.pipeline.list", json!({"repo": "service"})),
            "request",
            policy("bitbucket.repository.list"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_policy_for_another_workspace_does_not_apply() {
        let mut other = policy("bitbucket.repository.get");
        other.target = workspace("other");
        let result = PreparedBitbucketRead::prepare(
            request("bitbucket.repository.get", json!({"repo": "service"})),
            "request",
            other,
        );
        assert!(result.is_err());
    }

    #[test]
    fn cursor_binding_ignores_field_order_and_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let mut first = request(
            "bitbucket.pipeline.list",
            json!({"repo": "service", "page_size": 25}),
        );
        first.selection.fields = vec!["id".into(), "state".into()];
        let mut second = first.clone();
        second.selection.fields.reverse();
        let a = PreparedBitbucketRead::prepare(first, "a", policy("bitbucket.pipeline.list"))?;
        let b = PreparedBitbucketRead::prepare(second, "b", policy("bitbucket.pipeline.list"))?;
        assert_eq!(a.binding, b.binding);
        Ok(())
    }

    #[test]
    fn flags_and_json_decode_to_the_same_provider_input() -> Result<(), Box<dyn std::error::Error>>
    {
        let args = BitbucketReadArgs::from_input(
            "bitbucket.pullrequest.list",
            json!({"repo": "service", "state": "MERGED"}),
        )?;
        assert_eq!(
            args.input()?,
            json!({"repo": "service", "state": "MERGED", "page_size": 25})
        );
        assert_eq!(args.command(), "bitbucket.pullrequest.list");
        assert!(
            BitbucketReadArgs::from_input(
                "bitbucket.pullrequest.list",
                json!({"repo": "service", "unknown": 1})
            )
            .is_err()
        );
        assert!(BitbucketReadArgs::from_input("bitbucket.pullrequest.merge", json!({})).is_err());
        Ok(())
    }

    #[test]
    fn output_controls_never_reach_the_provider_input() -> Result<(), Box<dyn std::error::Error>> {
        let args = BitbucketReadArgs::from_input(
            "bitbucket.commit.list",
            json!({"repo": "service", "limit": 5, "page_size": 10}),
        )?;
        assert_eq!(args.controls(), (5, None));
        let input = args.input()?;
        assert!(input.get("limit").is_none() && input.get("cursor").is_none());
        assert_eq!(input["page_size"], 10);
        Ok(())
    }

    #[test]
    fn a_get_has_no_paging_controls_and_every_read_has_a_row() -> Result<(), CraftError> {
        const UUID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
        let samples = [
            ("bitbucket.repository.list", json!({})),
            ("bitbucket.repository.get", json!({"repo": "r"})),
            ("bitbucket.branch.list", json!({"repo": "r"})),
            ("bitbucket.commit.list", json!({"repo": "r"})),
            ("bitbucket.pullrequest.list", json!({"repo": "r"})),
            (
                "bitbucket.pullrequest.get",
                json!({"repo": "r", "pull_request_id": 1}),
            ),
            (
                "bitbucket.pullrequest.diffstat",
                json!({"repo": "r", "pull_request_id": 1}),
            ),
            (
                "bitbucket.comment.list",
                json!({"repo": "r", "pull_request_id": 1}),
            ),
            ("bitbucket.pipeline.list", json!({"repo": "r"})),
            (
                "bitbucket.pipeline.get",
                json!({"repo": "r", "pipeline_uuid": UUID}),
            ),
            (
                "bitbucket.step.list",
                json!({"repo": "r", "pipeline_uuid": UUID}),
            ),
        ];
        assert_eq!(samples.len(), commands().count());
        for (command, input) in samples {
            assert!(commands().any(|known| known == command), "{command}");
            let args = BitbucketReadArgs::from_input(command, input)?;
            assert_eq!(args.command(), command);
            if is_get(command) {
                assert_eq!(args.controls(), (20, None), "{command}");
            }
        }
        Ok(())
    }
}
