use std::path::PathBuf;

use crate::{
    Result, reject,
    release_trace::{
        Bundle, Deployment, Inventory, KubernetesContainer, Observation, Target, Version,
        kubernetes_identifier as valid_identifier, read_evidence, valid_digest,
    },
    sha256,
};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const MAX_SNAPSHOT_BYTES: u64 = 1024 * 1024;
const DEPLOYMENT_ANNOTATION: &str = "craft.factory/deployment-id";
const CONFIG_ANNOTATION: &str = "craft.factory/config-revision";
const REPOSITORY_ANNOTATION: &str = "craft.factory/source-repository";

#[derive(Args)]
pub struct Options {
    /// Existing release evidence bundle.
    bundle: PathBuf,
    #[arg(long)]
    pod_snapshot: PathBuf,
    #[command(flatten)]
    scope: PodScope,
}

#[derive(Args, Deserialize, serde::Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PodScope {
    /// Cluster identifier from the trusted collector.
    #[arg(long)]
    pub cluster: String,
    #[arg(long)]
    pub namespace: String,
    #[arg(long)]
    pub pod: String,
    /// Expected Pod UID from the incident or inventory record.
    #[arg(long)]
    pub pod_uid: String,
    #[arg(long)]
    pub container: String,
    #[arg(long)]
    pub service: String,
    #[arg(long)]
    pub environment: String,
    #[arg(long)]
    pub region: String,
    #[arg(long)]
    pub resource: String,
    #[arg(long)]
    pub owner: String,
    /// UTC Unix seconds recorded by the collector when reading the Pod.
    #[arg(long)]
    pub observed_at: u64,
}

#[derive(Args)]
pub struct DeploymentOptions {
    #[command(flatten)]
    inventory: Options,
    #[arg(long)]
    replica_set_snapshot: PathBuf,
    #[command(flatten)]
    scope: ReplicaSetScope,
}

#[derive(Args, Deserialize, serde::Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplicaSetScope {
    /// UTC Unix seconds recorded by the collector when reading the ReplicaSet.
    #[arg(long)]
    pub replica_set_observed_at: u64,
    #[arg(long)]
    pub deployment_name: String,
    #[arg(long)]
    pub deployment_uid: String,
    /// Exact repository identity from the approved service-to-repository mapping.
    #[arg(long)]
    pub repository: String,
}

pub fn run(options: Options) -> Result<Bundle> {
    let pod = read_evidence(&options.pod_snapshot, MAX_SNAPSHOT_BYTES)
        .map_err(|error| error.to_string())?;
    let (_, inventory) = read_pod(&options.scope, &pod)?;
    Bundle::with_inventory(&options.bundle, inventory).map_err(|error| error.to_string().into())
}

pub fn import_deployment(options: DeploymentOptions) -> Result<Bundle> {
    let pod = read_evidence(&options.inventory.pod_snapshot, MAX_SNAPSHOT_BYTES)
        .map_err(|error| error.to_string())?;
    let replica_set = read_evidence(&options.replica_set_snapshot, MAX_SNAPSHOT_BYTES)
        .map_err(|error| error.to_string())?;
    let (inventory, deployment) =
        collect_deployment(&options.inventory.scope, &options.scope, &pod, &replica_set)?;
    Bundle::with_inventory_and_deployment(&options.inventory.bundle, inventory, deployment)
        .map_err(|error| error.to_string().into())
}

pub fn collect_deployment(
    scope: &PodScope,
    options: &ReplicaSetScope,
    pod_bytes: &[u8],
    bytes: &[u8],
) -> Result<(Inventory, Deployment)> {
    if !valid_identifier(&options.deployment_name) || !valid_identifier(&options.deployment_uid) {
        return reject("provide the exact Deployment name and UID");
    }
    let (pod, inventory) = read_pod(scope, pod_bytes)?;
    let pod_meta = &pod["metadata"];
    let rs_owner = controller(pod_meta, "ReplicaSet")?;
    let rs_name = rs_owner["name"]
        .as_str()
        .filter(|name| valid_identifier(name))
        .ok_or("the Pod has no valid controlling ReplicaSet name")?;
    let rs_uid = rs_owner["uid"]
        .as_str()
        .filter(|uid| valid_identifier(uid))
        .ok_or("the Pod has no valid controlling ReplicaSet UID")?;
    let replica_set: Value = serde_json::from_slice(bytes)?;
    let rs_meta = &replica_set["metadata"];
    if replica_set["apiVersion"] != "apps/v1"
        || replica_set["kind"] != "ReplicaSet"
        || rs_meta["namespace"] != scope.namespace
        || rs_meta["name"] != rs_name
        || rs_meta["uid"] != rs_uid
        || !rs_meta["deletionTimestamp"].is_null()
    {
        return reject("ReplicaSet snapshot does not match the Pod's controlling ReplicaSet");
    }
    let deployment = controller(rs_meta, "Deployment")?;
    if deployment["name"] != options.deployment_name || deployment["uid"] != options.deployment_uid
    {
        return reject("ReplicaSet is not controlled by the expected Deployment");
    }
    let template = &replica_set["spec"]["template"];
    let labels = &template["metadata"]["labels"];
    let pod_hash = pod_meta["labels"]["pod-template-hash"]
        .as_str()
        .filter(|hash| valid_identifier(hash))
        .ok_or("the Pod is missing its pod-template-hash label")?;
    if labels["pod-template-hash"] != pod_hash {
        return reject("Pod template hash does not match its ReplicaSet");
    }
    let annotations = &template["metadata"]["annotations"];
    let running = &inventory.running;
    let image = named_container(&template["spec"]["containers"], &scope.container)?;
    if annotations[DEPLOYMENT_ANNOTATION] != running.deployment_id
        || annotations[CONFIG_ANNOTATION] != running.config_revision
        || annotations[REPOSITORY_ANNOTATION] != options.repository
        || pod_meta["annotations"][REPOSITORY_ANNOTATION] != options.repository
        || pinned_digest(&image["image"]) != Some(running.artifact_digest.as_str())
    {
        return reject(
            "ReplicaSet template does not match the Pod's version and approved repository",
        );
    }
    let applied_at = timestamp(&pod_meta["creationTimestamp"])
        .ok_or("the Pod has no valid creation timestamp")?;
    let replica_set_created = timestamp(&rs_meta["creationTimestamp"])
        .ok_or("the ReplicaSet has no valid creation timestamp")?;
    if replica_set_created > applied_at
        || applied_at > options.replica_set_observed_at
        || inventory
            .running_since
            .is_none_or(|started| applied_at > started)
    {
        return reject(
            "Pod and ReplicaSet evidence have inconsistent creation or observation times",
        );
    }
    let deployment = Deployment {
        target: inventory.target.clone(),
        version: inventory.running.clone(),
        repository: options.repository.clone(),
        deployed_at: applied_at,
        observation: Observation {
            source: "kubernetes-replicaset".to_owned(),
            reference: format!(
                "kubernetes:{}/{}/{}/{}#{}",
                scope.cluster,
                scope.namespace,
                rs_name,
                rs_uid,
                sha256(bytes)
            ),
            observed_at: options.replica_set_observed_at,
        },
    };
    Ok((inventory, deployment))
}

fn read_pod(options: &PodScope, bytes: &[u8]) -> Result<(Value, Inventory)> {
    if [
        &options.cluster,
        &options.namespace,
        &options.pod,
        &options.pod_uid,
        &options.container,
    ]
    .iter()
    .any(|value| !valid_identifier(value))
    {
        return reject("provide exact cluster, namespace, Pod, UID, and container identifiers");
    }
    let pod: Value = serde_json::from_slice(bytes)?;
    let metadata = &pod["metadata"];
    if pod["apiVersion"] != "v1"
        || pod["kind"] != "Pod"
        || metadata["namespace"] != options.namespace
        || metadata["name"] != options.pod
        || metadata["uid"] != options.pod_uid
    {
        return reject("Kubernetes snapshot does not identify the expected Pod");
    }
    if !metadata["deletionTimestamp"].is_null() || pod["status"]["phase"] != "Running" {
        return reject("Kubernetes snapshot does not show a non-terminating running Pod");
    }

    let spec = named_container(&pod["spec"]["containers"], &options.container)?;
    let status = named_container(&pod["status"]["containerStatuses"], &options.container)?;
    let pin = pinned_digest(&spec["image"])
        .ok_or("the selected container image is not pinned to a SHA-256 digest")?;
    let image_id = status["imageID"].as_str().unwrap_or_default();
    let resolved = image_id
        .split_once("://")
        .map_or(image_id, |(_, value)| value);
    let running_digest = resolved
        .rsplit_once('@')
        .map_or(resolved, |(_, digest)| digest);
    if running_digest != pin {
        return reject("runtime image digest does not match the selected container's pin");
    }
    let state = &status["state"];
    if !state["waiting"].is_null() || !state["terminated"].is_null() {
        return reject("the selected container is not running");
    }
    let started_at = timestamp(&state["running"]["startedAt"])
        .ok_or("the selected container is missing a valid running start time")?;
    if started_at > options.observed_at {
        return reject("the selected container started after the snapshot observation time");
    }
    let kubernetes_container = status["containerID"]
        .as_str()
        .and_then(|id| id.split_once("://"))
        .filter(|(runtime, id)| valid_identifier(runtime) && valid_identifier(id))
        .map(|(_, id)| KubernetesContainer {
            pod_uid: options.pod_uid.clone(),
            container_name: options.container.clone(),
            container_id: id.to_owned(),
        });

    let annotations = &metadata["annotations"];
    let inventory = Inventory {
        target: Target {
            service: options.service.clone(),
            environment: options.environment.clone(),
            region: options.region.clone(),
            resource: options.resource.clone(),
        },
        owner: options.owner.clone(),
        running: Version {
            deployment_id: annotations[DEPLOYMENT_ANNOTATION]
                .as_str()
                .ok_or("the Pod has no Craft deployment ID annotation")?
                .to_owned(),
            config_revision: annotations[CONFIG_ANNOTATION]
                .as_str()
                .ok_or("the Pod has no Craft configuration revision annotation")?
                .to_owned(),
            artifact_digest: pin.to_owned(),
        },
        running_since: Some(started_at),
        kubernetes_container,
        observation: Observation {
            source: "kubernetes-pod".to_owned(),
            reference: format!(
                "kubernetes:{}/{}/{}/{}/{}#{}",
                options.cluster,
                options.namespace,
                options.pod,
                options.pod_uid,
                options.container,
                sha256(bytes),
            ),
            observed_at: options.observed_at,
        },
    };
    Ok((pod, inventory))
}

fn controller<'a>(metadata: &'a Value, kind: &str) -> Result<&'a Value> {
    let mut owners = metadata["ownerReferences"]
        .as_array()
        .ok_or("Kubernetes object is missing owner references")?
        .iter()
        .filter(|owner| owner["controller"] == true);
    let owner = owners
        .next()
        .ok_or("Kubernetes object is missing a controller")?;
    if owners.next().is_some() || owner["kind"] != kind || owner["apiVersion"] != "apps/v1" {
        return reject("Kubernetes object does not have the expected controller");
    }
    Ok(owner)
}

fn pinned_digest(image: &Value) -> Option<&str> {
    let (name, digest) = image.as_str()?.split_once('@')?;
    (!name.is_empty() && valid_digest(digest)).then_some(digest)
}

fn timestamp(value: &Value) -> Option<u64> {
    let time = OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok()?;
    u64::try_from(time.unix_timestamp())
        .ok()
        .filter(|time| *time > 0)
}

fn named_container<'a>(items: &'a Value, name: &str) -> Result<&'a Value> {
    let mut named = items
        .as_array()
        .ok_or("Kubernetes snapshot is missing the container list")?
        .iter()
        .filter(|item| item["name"] == name);
    let selected = named
        .next()
        .ok_or("Kubernetes snapshot does not contain the selected container")?;
    if named.next().is_some() {
        return reject("Kubernetes snapshot repeats the selected container");
    }
    Ok(selected)
}
