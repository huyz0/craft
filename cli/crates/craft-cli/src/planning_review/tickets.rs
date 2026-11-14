//! Complete ticket manifests retain exact reviewed inputs and symbolic dependency endpoints.
use super::*;
use crate::dispatch::CommandResult;
use crate::profiles::InvocationOptions;
use craft_core::registry::Registry;
use craft_core::storage::ArtifactStore;
use craft_provider_jira::issue_links::LinkType;

#[cfg(test)]
mod tests;

pub const PREVIEW: &str = "plan.tickets-preview";
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewSummary {
    pub manifest_digest: String,
    pub task_count: u16,
    pub batch_count: u16,
    pub dependency_count: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyLink {
    pub prerequisite_task_id: String,
    pub dependent_task_id: String,
    pub dependent_label: String,
    pub outward_task_id: String,
    pub inward_task_id: String,
    pub link_type: LinkType,
}
#[derive(Serialize)]
pub struct TicketManifest {
    pub format: &'static str,
    pub binding_digest: String,
    pub batches: Vec<Vec<String>>,
    pub links: Vec<DependencyLink>,
    pub context: BindingSnapshot,
}
impl TicketManifest {
    pub fn summary(&self) -> Result<PreviewSummary, CraftError> {
        let tasks = self
            .context
            .tasks
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid("cannot_encode_manifest_tasks"))?;
        material_summary(
            self.format,
            &self.binding_digest,
            &self.batches,
            &self.links,
            &tasks,
        )
    }
}
pub(super) fn material_summary(
    format: &str,
    binding_digest: &str,
    batches: &[Vec<String>],
    links: &[DependencyLink],
    tasks: &[Value],
) -> Result<PreviewSummary, CraftError> {
    let material:Vec<_>=tasks.iter().map(|t|json!({"task_id":t["task_id"],"profile":t["profile"],"target":t["target"],"input":t["input"],"payload":t["payload"],"metadata_digest":t["metadata_digest"],"dependency_link_type":t["dependency_link_type"]})).collect();
    Ok(PreviewSummary {
        manifest_digest: digest(
            &json!({"format":format,"binding_digest":binding_digest,"tasks":material,"batches":batches,"links":links}),
        )?,
        task_count: tasks
            .len()
            .try_into()
            .map_err(|_| invalid("manifest_task_bound"))?,
        batch_count: batches
            .len()
            .try_into()
            .map_err(|_| invalid("manifest_batch_bound"))?,
        dependency_count: links
            .len()
            .try_into()
            .map_err(|_| invalid("manifest_dependency_bound"))?,
    })
}
pub(super) fn compile(mut context: BindingSnapshot) -> Result<TicketManifest, CraftError> {
    let order = context.plan.document.validate()?;
    let plan = &context.plan.document;
    let tasks: BTreeMap<_, _> = plan
        .tasks
        .iter()
        .filter(|t| t.include_in_jira)
        .map(|t| (t.task_id.clone(), t))
        .collect();
    context.tasks.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    let resolved: BTreeMap<_, _> = context
        .tasks
        .iter()
        .map(|t| (t.task_id.clone(), t))
        .collect();
    if resolved.len() != context.tasks.len() || tasks.keys().ne(resolved.keys()) {
        return Err(invalid("complete_exact_manifest_task_mapping_required"));
    }
    let mut levels = BTreeMap::<String, usize>::new();
    let mut batches = Vec::<Vec<String>>::new();
    let mut links = Vec::new();
    for id in order {
        let Some(task) = tasks.get(&id) else { continue };
        let mut level = 0;
        let native = resolved[&id];
        if task.depends_on.is_empty() != native.dependency_link_type.is_none() {
            return Err(invalid("manifest_dependency_link_mapping_mismatch"));
        }
        for dependency in &task.depends_on {
            level = level.max(
                levels
                    .get(dependency)
                    .ok_or_else(|| invalid("manifest_dependency_not_ordered"))?
                    + 1,
            );
            let prerequisite = resolved
                .get(dependency)
                .ok_or_else(|| invalid("manifest_dependency_not_included"))?;
            if prerequisite.target.tenant != native.target.tenant {
                return Err(invalid("cross_site_jira_dependencies_unsupported"));
            }
            let relation = native
                .dependency_link_type
                .as_ref()
                .ok_or_else(|| invalid("missing_native_dependency_link_type"))?;
            // Jira endpoint names identify the neighbor's label from each issue's view.
            let (inward, outward, label) = match relation.dependent_direction {
                DependencyDirection::Inward => (dependency, &id, &relation.link_type.inward),
                DependencyDirection::Outward => (&id, dependency, &relation.link_type.outward),
            };
            links.push(DependencyLink {
                prerequisite_task_id: dependency.clone(),
                dependent_task_id: id.clone(),
                outward_task_id: outward.clone(),
                inward_task_id: inward.clone(),
                dependent_label: label.clone(),
                link_type: relation.link_type.clone(),
            });
        }
        while batches.len() <= level {
            batches.push(vec![]);
        }
        batches[level].push(id.clone());
        levels.insert(id, level);
    }
    for batch in &mut batches {
        batch.sort();
    }
    links.sort_by(|a, b| {
        (&a.outward_task_id, &a.inward_task_id).cmp(&(&b.outward_task_id, &b.inward_task_id))
    });
    Ok(TicketManifest {
        format: "craft.ticket-manifest/v1",
        binding_digest: digest(&context.binding)?,
        batches,
        links,
        context,
    })
}
pub fn preview_native(
    args: verification::VerifyArgs,
    profile: &str,
    request_id: &str,
    environment: BindingEnvironment<'_>,
    artifacts: &dyn ArtifactStore,
) -> (verification::VerificationData, Option<CraftError>) {
    verification::run_native(args, profile, request_id, environment, artifacts, true)
}
pub fn execute(
    args: verification::VerifyArgs,
    fields: &[String],
    options: &InvocationOptions<'_>,
    control: &InvocationControl,
) -> CommandResult {
    verification::execute_command(PREVIEW, args, fields, options, control)
}
pub fn register(registry: &mut Registry) -> Result<(), CraftError> {
    verification::register_command(registry, PREVIEW)
}
