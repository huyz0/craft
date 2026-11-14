//! Local authoring and validation. No profile, credential or cloud service is consulted.

use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, content_digest};
use craft_core::projection::CompiledProjection;
use craft_core::registry::Registry;
use craft_workflows::documents::Document;
use craft_workflows::plans::{Design, DocumentTarget, Plan, PlanTarget};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::args::*;
use crate::dispatch::DispatchOutput;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanScaffoldData {
    pub document: Plan,
    pub ready: bool,
    pub review_verified: bool,
    pub live_conditions_checked: bool,
    pub tickets_created: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignScaffoldData {
    pub document: Design,
    pub ready: bool,
    pub review_verified: bool,
    pub live_conditions_checked: bool,
    pub tickets_created: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanValidationData {
    pub valid: bool,
    pub digest: String,
    pub task_count: usize,
    pub included_tasks: usize,
    pub task_order: Vec<String>,
    pub document: Plan,
    pub review_verified: bool,
    pub live_conditions_checked: bool,
    pub tickets_created: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignValidationData {
    pub valid: bool,
    pub digest: String,
    pub section_count: usize,
    pub document: Design,
    pub review_verified: bool,
    pub live_conditions_checked: bool,
    pub tickets_created: u32,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderData {
    pub format: DocumentOutputFormat,
    pub document_digest: String,
    pub body_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adf: Option<Value>,
    pub review_verified: bool,
    pub live_conditions_checked: bool,
    pub tickets_created: u32,
}

fn encode(value: impl Serialize) -> Result<Value, CraftError> {
    serde_json::to_value(value)
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode planning response."))
}
fn scaffold_error(mut error: CraftError) -> CraftError {
    for issue in &mut error.details.issues {
        if let Some(path) = issue
            .path
            .strip_prefix("/target/confluence/")
            .or_else(|| issue.path.strip_prefix("/target/"))
        {
            issue.path = format!("/{path}");
        }
    }
    error
}
fn plan_scaffold(args: PlanScaffoldArgs) -> Result<Plan, CraftError> {
    Plan::scaffold(
        args.plan_id,
        args.title,
        PlanTarget {
            confluence: args.confluence.into(),
            jira_cloud_id: args.jira_cloud_id,
            project_key: args.project_key,
        },
    )
    .map_err(scaffold_error)
}
fn design_scaffold(args: DesignScaffoldArgs) -> Result<Design, CraftError> {
    Design::scaffold(
        args.design_id,
        args.title,
        args.kind,
        args.confluence.into(),
        args.compact,
    )
    .map_err(scaffold_error)
}

pub fn validate_input(command: &str, input: &Value) -> Result<(), CraftError> {
    match command {
        "plan.scaffold" => {
            let args = serde_json::from_value(input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_plan_seed"))?;
            plan_scaffold(args)?;
        }
        "design.scaffold" => {
            let args = serde_json::from_value(input.clone())
                .map_err(|_| CraftError::field("/input", "invalid_design_seed"))?;
            design_scaffold(args)?;
        }
        _ => {}
    }
    Ok(())
}

fn source_document(source: &[u8], format: DocumentSourceFormat) -> Result<Document, CraftError> {
    match format {
        DocumentSourceFormat::Storage => Document::from_storage(
            std::str::from_utf8(source)
                .map_err(|_| CraftError::field("/file", "expected_utf8_document"))?,
        ),
        DocumentSourceFormat::Adf => Document::from_adf(source),
        DocumentSourceFormat::Json => {
            return Err(CraftError::field(
                "/source_format",
                "expected_structured_page_format",
            ));
        }
    }
    .map_err(|error| {
        let mut result = CraftError::field("/file", error.reason);
        result.message = error.to_string();
        result
    })
}

pub(crate) fn load_plan(
    args: &DocumentFileArgs,
    control: &InvocationControl,
) -> Result<Plan, CraftError> {
    let source = crate::dispatch::read_source(&args.file, control)?;
    control.check()?;
    match args.source_format {
        DocumentSourceFormat::Json => Plan::from_json(&source),
        format => Plan::from_document(&source_document(&source, format)?),
    }
}
pub(crate) fn load_design(
    args: &DocumentFileArgs,
    control: &InvocationControl,
) -> Result<Design, CraftError> {
    let source = crate::dispatch::read_source(&args.file, control)?;
    control.check()?;
    match args.source_format {
        DocumentSourceFormat::Json => Design::from_json(&source),
        format => Design::from_document(&source_document(&source, format)?),
    }
}

fn render(
    document: Document,
    digest: String,
    format: DocumentOutputFormat,
) -> Result<RenderData, CraftError> {
    let (storage, adf, body_digest) = match format {
        DocumentOutputFormat::Storage => {
            let body = document
                .to_storage()
                .map_err(|error| CraftError::field("/document", error.reason))?;
            let digest = content_digest(body.as_bytes());
            (Some(body), None, digest)
        }
        DocumentOutputFormat::Adf => {
            let body = document
                .to_adf()
                .map_err(|error| CraftError::field("/document", error.reason))?;
            let digest = canonical_digest(&body)?;
            (None, Some(body), digest)
        }
    };
    Ok(RenderData {
        format,
        document_digest: digest,
        body_digest,
        storage,
        adf,
        review_verified: false,
        live_conditions_checked: false,
        tickets_created: 0,
    })
}

pub fn execute(
    action: Action,
    projection: Option<CompiledProjection>,
    control: &InvocationControl,
) -> Result<DispatchOutput, CraftError> {
    control.check()?;
    let data = match action {
        Action::Plan {
            action: PlanAction::Scaffold(args),
        } => encode(PlanScaffoldData {
            document: plan_scaffold(*args)?,
            ready: false,
            review_verified: false,
            live_conditions_checked: false,
            tickets_created: 0,
        })?,
        Action::Design {
            action: DesignAction::Scaffold(args),
        } => encode(DesignScaffoldData {
            document: design_scaffold(*args)?,
            ready: false,
            review_verified: false,
            live_conditions_checked: false,
            tickets_created: 0,
        })?,
        Action::Plan {
            action: PlanAction::Validate(args),
        } => {
            let plan = load_plan(&args, control)?;
            encode(PlanValidationData {
                valid: true,
                digest: plan.digest()?,
                task_count: plan.tasks.len(),
                included_tasks: plan
                    .tasks
                    .iter()
                    .filter(|task| task.include_in_jira)
                    .count(),
                task_order: plan.validate()?,
                document: plan,
                review_verified: false,
                live_conditions_checked: false,
                tickets_created: 0,
            })?
        }
        Action::Design {
            action: DesignAction::Validate(args),
        } => {
            let design = load_design(&args, control)?;
            encode(DesignValidationData {
                valid: true,
                digest: design.digest()?,
                section_count: design.sections.len(),
                document: design,
                review_verified: false,
                live_conditions_checked: false,
                tickets_created: 0,
            })?
        }
        Action::Plan {
            action: PlanAction::Render(args),
        } => {
            let plan = load_plan(&args.source, control)?;
            encode(render(plan.render()?, plan.digest()?, args.format)?)?
        }
        Action::Design {
            action: DesignAction::Render(args),
        } => {
            let design = load_design(&args.source, control)?;
            encode(render(design.render()?, design.digest()?, args.format)?)?
        }
        _ => {
            return Err(CraftError::new(
                ErrorCode::Internal,
                "Invalid planning dispatch.",
            ));
        }
    };
    control.check()?;
    let data = match projection {
        Some(projection) => projection.data(&data)?,
        None => data,
    };
    Ok(DispatchOutput { data, page: None })
}

pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    let target = PlanTarget {
        confluence: DocumentTarget {
            confluence_cloud_id: "00000000-1111-2222-3333-444444444444".into(),
            space_id: "1".into(),
            parent_page_id: None,
        },
        jira_cloud_id: "00000000-1111-2222-3333-444444444444".into(),
        project_key: "PAY".into(),
    };
    let plan = Plan::scaffold(
        "invoice-replay".into(),
        "Prevent invoice replay".into(),
        target.clone(),
    )?;
    let design = Design::scaffold(
        "invoice-design".into(),
        "Invoice design".into(),
        craft_workflows::plans::DesignKind::Hld,
        target.confluence.clone(),
        false,
    )?;
    let mut authored_plan = plan.clone();
    authored_plan.context = craft_workflows::plans::RichText::Markdown(
        "Repeated delivery must preserve one ledger entry.".into(),
    );
    authored_plan.tasks.push(craft_workflows::plans::PlanTask {
        task_id: "event-identity".into(),
        title: "Define event identity".into(),
        jira: craft_workflows::plans::JiraMapping {
            project_key: "PAY".into(),
            issue_type: "Task".into(),
            existing_issue: None,
        },
        description: craft_workflows::plans::RichText::Markdown(
            "Document the event identity used by producers and consumers.".into(),
        ),
        acceptance: vec![craft_workflows::plans::RichText::Markdown(
            "Producer and consumer agree on event identity.".into(),
        )],
        repositories: vec!["billing-api".into()],
        depends_on: Vec::new(),
        designs: Vec::new(),
        include_in_jira: true,
        estimate: None,
    });
    let mut authored_design = design.clone();
    for section in &mut authored_design.sections {
        section.body = craft_workflows::plans::RichText::Markdown(format!(
            "Explain {} for the invoice service.",
            section.title
        ));
    }
    crate::catalog::register::<PlanScaffoldArgs, PlanScaffoldData>(
        registry,
        "plan.scaffold",
        "Create an incomplete local plan template with explicit targets; no cloud writes.",
        json!({"plan_id":"invoice-replay","title":"Prevent invoice replay",
            "confluence_cloud_id":target.confluence.confluence_cloud_id,
            "space_id":target.confluence.space_id,
            "jira_cloud_id":target.jira_cloud_id,"project_key":target.project_key}),
        encode(PlanScaffoldData {
            document: plan.clone(),
            ready: false,
            review_verified: false,
            live_conditions_checked: false,
            tickets_created: 0,
        })?,
    )?;
    crate::catalog::register::<DesignScaffoldArgs, DesignScaffoldData>(
        registry,
        "design.scaffold",
        "Create an incomplete local design template; no cloud writes.",
        json!({"design_id":"invoice-design","title":"Invoice design",
            "confluence_cloud_id":design.target.confluence_cloud_id,
            "space_id":design.target.space_id}),
        encode(DesignScaffoldData {
            document: design.clone(),
            ready: false,
            review_verified: false,
            live_conditions_checked: false,
            tickets_created: 0,
        })?,
    )?;
    crate::catalog::register::<DocumentFileArgs, PlanValidationData>(
        registry,
        "plan.validate",
        "Validate a local plan or visible page and graph; no approval or live checks.",
        json!({"file":"plan.json"}),
        encode(PlanValidationData {
            valid: true,
            digest: authored_plan.digest()?,
            task_count: authored_plan.tasks.len(),
            included_tasks: authored_plan.tasks.len(),
            task_order: authored_plan.validate()?,
            document: authored_plan.clone(),
            review_verified: false,
            live_conditions_checked: false,
            tickets_created: 0,
        })?,
    )?;
    crate::catalog::register::<DocumentFileArgs, DesignValidationData>(
        registry,
        "design.validate",
        "Validate a local design or visible page; no approval or live checks.",
        json!({"file":"design.json"}),
        encode(DesignValidationData {
            valid: true,
            digest: authored_design.digest()?,
            section_count: authored_design.sections.len(),
            document: authored_design.clone(),
            review_verified: false,
            live_conditions_checked: false,
            tickets_created: 0,
        })?,
    )?;
    for command in ["plan.render", "design.render"] {
        let example = if command == "plan.render" {
            render(
                authored_plan.render()?,
                authored_plan.digest()?,
                DocumentOutputFormat::Storage,
            )?
        } else {
            render(
                authored_design.render()?,
                authored_design.digest()?,
                DocumentOutputFormat::Storage,
            )?
        };
        crate::catalog::register::<DocumentRenderArgs, RenderData>(
            registry,
            command,
            "Render a validated local document as storage or ADF; no publication.",
            json!({"file":if command=="plan.render"{"plan.json"}else{"design.json"}}),
            encode(example)?,
        )?;
    }
    Ok(())
}
