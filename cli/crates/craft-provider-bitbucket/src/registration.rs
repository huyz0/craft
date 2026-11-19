use crate::models::*;
use crate::table::{ENTRIES, Entry, Kind, Param, is_get, route};
use craft_core::contracts::{CanonicalRequest, EffectClass, Envelope, ResourceRef, SCHEMA_VERSION};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{OperationContext, OperationHandler, PreparedOperation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::{
    OperationDescriptor, ProviderManifest, ProviderModule, ReconciliationClass, Registry,
    RetryClass, response_schema, schema_for,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct BitbucketContext;

const EXAMPLE_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The exact Bitbucket workspace a profile is bound to. Repository access is whatever the
/// credential can see; a policy narrows operations, not repositories.
pub fn validate_target(target: &ResourceRef) -> Result<(), CraftError> {
    target.validate()?;
    if target.provider != "bitbucket"
        || target.deployment != "cloud"
        || target.resource_type != "workspace"
        || target.id != target.tenant
        || !slug(&target.tenant)
    {
        return Err(CraftError::field(
            "/target",
            "expected_exact_bitbucket_cloud_workspace",
        ));
    }
    Ok(())
}

fn defaults(command: &str) -> &'static [&'static str] {
    match command {
        "bitbucket.repository.list" | "bitbucket.repository.get" => {
            &["id", "full_name", "is_private", "main_branch", "updated_on"]
        }
        "bitbucket.branch.list" => &["id", "commit", "date", "author"],
        "bitbucket.commit.list" => &["id", "message", "date", "author"],
        "bitbucket.pullrequest.list" | "bitbucket.pullrequest.get" => &[
            "id",
            "title",
            "state",
            "author",
            "source_branch",
            "destination_branch",
            "updated_on",
        ],
        "bitbucket.pullrequest.diffstat" => &["id", "status", "lines_added", "lines_removed"],
        "bitbucket.comment.list" => &["id", "text", "author", "created_on", "inline"],
        "bitbucket.pipeline.list" | "bitbucket.pipeline.get" => &[
            "id",
            "build_number",
            "state",
            "result",
            "ref_name",
            "created_on",
        ],
        _ => &["id", "name", "state", "result", "duration_seconds"],
    }
}

pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: if is_get(command) { "/item" } else { "/items" }.into(),
        always_returned: vec!["id".into()],
        default_selected: defaults(command).iter().map(|f| (*f).into()).collect(),
        mode: ProjectionMode::Mixed,
    }
}

pub fn response_schema_for(command: &str) -> Result<Value, CraftError> {
    macro_rules! shape {
        ($item:ty) => {
            if is_get(command) {
                response_schema::<GetData<$item>>()
            } else {
                response_schema::<ListData<$item>>()
            }
        };
    }
    match command {
        "bitbucket.repository.list" | "bitbucket.repository.get" => shape!(RepositorySummary),
        "bitbucket.branch.list" => shape!(BranchSummary),
        "bitbucket.commit.list" => shape!(CommitSummary),
        "bitbucket.pullrequest.list" | "bitbucket.pullrequest.get" => shape!(PullRequestSummary),
        "bitbucket.pullrequest.diffstat" => shape!(DiffstatSummary),
        "bitbucket.comment.list" => shape!(CommentSummary),
        "bitbucket.pipeline.list" | "bitbucket.pipeline.get" => shape!(PipelineSummary),
        "bitbucket.step.list" => shape!(StepSummary),
        _ => Err(CraftError::field("/command", "unknown_bitbucket_read")),
    }
}

fn item_example(command: &str) -> Value {
    match command {
        "bitbucket.repository.list" | "bitbucket.repository.get" => json!({
            "id":"service","uuid":EXAMPLE_UUID,"name":"Service","full_name":"team/service",
            "description":null,"is_private":true,"language":"rust","main_branch":"main",
            "project_key":null,"created_on":null,"updated_on":null,"url":null
        }),
        "bitbucket.branch.list" => json!({
            "id":"feature/login","commit":"0123456789abcdef","date":null,"author":"Agent","message":null
        }),
        "bitbucket.commit.list" => json!({
            "id":"0123456789abcdef","message":"Fix login","date":null,"author":"Agent",
            "parents":[],"url":null
        }),
        "bitbucket.pullrequest.list" | "bitbucket.pullrequest.get" => json!({
            "id":1,"title":"Fix login","description":null,"state":"OPEN","author":"Agent",
            "source_branch":"feature/login","source_repository":"team/service",
            "destination_branch":"main","created_on":null,"updated_on":null,
            "comment_count":0,"task_count":0,"close_source_branch":false,"merge_commit":null,
            "reviewers":[],"participants":[],"url":null
        }),
        "bitbucket.pullrequest.diffstat" => json!({
            "id":"src/lib.rs","status":"modified","lines_added":3,"lines_removed":1,
            "old_path":"src/lib.rs","new_path":"src/lib.rs"
        }),
        "bitbucket.comment.list" => json!({
            "id":1,"text":"Looks good","author":"Agent","created_on":null,"updated_on":null,
            "deleted":false,"parent_id":null,
            "inline":{"path":"src/lib.rs","from_line":null,"to_line":10}
        }),
        "bitbucket.pipeline.list" | "bitbucket.pipeline.get" => json!({
            "id":EXAMPLE_UUID,"build_number":1,"state":"COMPLETED","result":"SUCCESSFUL",
            "ref_name":"main","ref_type":"branch","commit":"0123456789abcdef","creator":"Agent",
            "trigger":"PUSH","created_on":null,"completed_on":null,"duration_seconds":60
        }),
        _ => json!({
            "id":EXAMPLE_UUID,"name":"Build","state":"COMPLETED","result":"SUCCESSFUL",
            "started_on":null,"completed_on":null,"duration_seconds":30
        }),
    }
}

fn input_example(entry: &Entry) -> Value {
    let mut example = serde_json::Map::new();
    for param in entry.params {
        example.insert(
            param.name().into(),
            match param {
                Param::Repo => json!("service"),
                Param::PullRequest => json!(1),
                Param::Pipeline => json!(EXAMPLE_UUID),
            },
        );
    }
    if entry.kind == Kind::Paged {
        example.insert("page_size".into(), json!(25));
    }
    Value::Object(example)
}

fn register_one<I: DeserializeOwned + schemars::JsonSchema>(
    registry: &mut Registry,
    entry: &Entry,
) -> Result<(), CraftError> {
    let command = entry.id;
    let target = json!({"provider":"bitbucket","deployment":"cloud","tenant":"team","resource_type":"workspace","id":"team"});
    let data = if is_get(command) {
        json!({"target":target,"item":item_example(command),"observed_at":0})
    } else {
        json!({"target":target,"items":[item_example(command)],"observation":null,"cached":false})
    };
    let descriptor = OperationDescriptor {
        id: command.into(),
        provider: "bitbucket".into(),
        contract_version: SCHEMA_VERSION.into(),
        api_revision: "bitbucket-rest-2.0".into(),
        description: entry.description.into(),
        effect: EffectClass::Read,
        input_schema: schema_for::<I>()?,
        response_schema: response_schema_for(command)?,
        example: input_example(entry),
        response_example: json!(Envelope::success(command, "example".into(), data)),
        projection: Some(projection(command)),
        scope_alternatives: vec![vec!["read".into()]],
        permissions: vec![
            "Bitbucket repository, pull request and pipeline visibility evaluated by Bitbucket"
                .into(),
        ],
        tenant_conditions: vec![
            "Exact workspace bound to the profile; visibility is not absence or write authority."
                .into(),
        ],
        routes: vec![route(entry)],
        retry_class: RetryClass::SafeRead,
        reconciliation: ReconciliationClass::NotApplicable,
    };
    registry.register_typed_handler::<I>(descriptor, Arc::new(Handler))
}

impl ProviderModule for BitbucketContext {
    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            id: "bitbucket".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            deployments: vec!["cloud".into()],
            contract_version: SCHEMA_VERSION.into(),
        }
    }

    fn register(&self, registry: &mut Registry) -> Result<(), CraftError> {
        for entry in ENTRIES {
            match entry.id {
                "bitbucket.repository.list" => register_one::<PageInput>(registry, entry)?,
                "bitbucket.repository.get" => register_one::<RepoInput>(registry, entry)?,
                "bitbucket.branch.list" => register_one::<BranchListInput>(registry, entry)?,
                "bitbucket.commit.list" => register_one::<CommitListInput>(registry, entry)?,
                "bitbucket.pullrequest.list" => {
                    register_one::<PullRequestListInput>(registry, entry)?
                }
                "bitbucket.pullrequest.get" => register_one::<PullRequestInput>(registry, entry)?,
                "bitbucket.pullrequest.diffstat" | "bitbucket.comment.list" => {
                    register_one::<PullRequestPageInput>(registry, entry)?
                }
                "bitbucket.pipeline.list" => register_one::<RepoPageInput>(registry, entry)?,
                "bitbucket.pipeline.get" => register_one::<PipelineInput>(registry, entry)?,
                _ => register_one::<PipelinePageInput>(registry, entry)?,
            }
        }
        crate::writes::register(registry)?;
        Ok(())
    }
}

struct Handler;
impl OperationHandler for Handler {
    fn validate(&self, request: &CanonicalRequest) -> Result<(), CraftError> {
        validate_target(&request.target)?;
        let get = is_get(&request.command);
        if request.preconditions.is_some()
            || (get
                && (request.selection.cursor.is_some()
                    || request.selection.limit
                        != craft_core::contracts::Selection::default().limit))
        {
            return Err(CraftError::field(
                "/selection",
                "unsupported_get_paging_or_read_preconditions",
            ));
        }
        let parse = || CraftError::field("/input", "invalid_bitbucket_read_input");
        macro_rules! input {
            ($ty:ty) => {
                serde_json::from_value::<$ty>(request.input.clone()).map_err(|_| parse())?
            };
        }
        match request.command.as_str() {
            "bitbucket.repository.list" => page_size_field(input!(PageInput).page_size)?,
            "bitbucket.repository.get" => repo_field(&input!(RepoInput).repo)?,
            "bitbucket.branch.list" => {
                let input = input!(BranchListInput);
                repo_field(&input.repo)?;
                if let Some(fragment) = &input.name_contains {
                    name_filter_field(fragment)?;
                }
                page_size_field(input.page_size)?;
            }
            "bitbucket.commit.list" => {
                let input = input!(CommitListInput);
                repo_field(&input.repo)?;
                if let Some(branch) = &input.branch {
                    branch_field("/branch", branch)?;
                }
                page_size_field(input.page_size)?;
            }
            "bitbucket.pullrequest.list" => {
                let input = input!(PullRequestListInput);
                repo_field(&input.repo)?;
                if let Some(state) = &input.state {
                    state_field(state)?;
                }
                page_size_field(input.page_size)?;
            }
            "bitbucket.pullrequest.get" => {
                let input = input!(PullRequestInput);
                repo_field(&input.repo)?;
                pull_request_field(input.pull_request_id)?;
            }
            "bitbucket.pullrequest.diffstat" | "bitbucket.comment.list" => {
                let input = input!(PullRequestPageInput);
                repo_field(&input.repo)?;
                pull_request_field(input.pull_request_id)?;
                page_size_field(input.page_size)?;
            }
            "bitbucket.pipeline.list" => {
                let input = input!(RepoPageInput);
                repo_field(&input.repo)?;
                page_size_field(input.page_size)?;
            }
            "bitbucket.pipeline.get" => {
                let input = input!(PipelineInput);
                repo_field(&input.repo)?;
                pipeline_field(&input.pipeline_uuid)?;
            }
            "bitbucket.step.list" => {
                let input = input!(PipelinePageInput);
                repo_field(&input.repo)?;
                pipeline_field(&input.pipeline_uuid)?;
                page_size_field(input.page_size)?;
            }
            _ => return Err(parse()),
        }
        Ok(())
    }

    fn execute(
        &self,
        _: &PreparedOperation,
        _: &mut dyn OperationContext,
    ) -> Result<Value, CraftError> {
        Err(CraftError::new(
            ErrorCode::PolicyDenied,
            "Bitbucket reads require the verified bounded host.",
        ))
    }
}
