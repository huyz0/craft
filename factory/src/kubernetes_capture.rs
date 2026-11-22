use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use clap::Args;
use craft_factory_evidence::{
    kubernetes_inventory::{MAX_SNAPSHOT_BYTES, PodScope, ReplicaSetScope, collect_deployment},
    release_trace::{Target, read_evidence, trace_observations, valid_digest},
    sha256,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    Result,
    evidence_capture::{create_directory, now, require_new_directory, write_new},
    process_tree::read_bounded,
    reject,
};

#[derive(Args)]
pub struct Options {
    policy: PathBuf,
    #[arg(long)]
    policy_digest: String,
    /// Absolute path to the trusted kubectl executable.
    #[arg(long)]
    kubectl: PathBuf,
    /// Absolute path to the collector's approved kubeconfig.
    #[arg(long)]
    kubeconfig: PathBuf,
    /// New directory for the original evidence and source manifest.
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    context: String,
    kubeconfig_digest: String,
    cluster: String,
    namespace: String,
    pod: String,
    pod_uid: String,
    replica_set: String,
    container: String,
    deployment_name: String,
    deployment_uid: String,
    repository: String,
    target: Target,
    owner: String,
    max_age_seconds: u64,
}

pub fn run(options: Options) -> Result<Value> {
    let raw = read_evidence(&options.policy, 32 * 1024).map_err(|error| error.to_string())?;
    let policy_digest = sha256(&raw);
    if policy_digest != options.policy_digest {
        return reject("Kubernetes capture policy does not match the approved digest");
    }
    let policy: Policy = serde_json::from_slice(&raw)?;
    if policy.schema_version != 1
        || !(1..=3600).contains(&policy.max_age_seconds)
        || policy.context.is_empty()
        || policy.context.len() > 253
        || policy.context.chars().any(char::is_control)
        || !valid_digest(&policy.kubeconfig_digest)
        || !resource_name(&policy.namespace, 63)
        || policy.namespace.contains('.')
        || !resource_name(&policy.pod, 253)
        || !resource_name(&policy.replica_set, 253)
    {
        return reject("invalid Kubernetes capture scope");
    }
    if !options.kubectl.is_absolute()
        || !options.kubectl.is_file()
        || (cfg!(windows)
            && !options
                .kubectl
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")))
        || !options.kubeconfig.is_absolute()
        || !options.kubeconfig.is_file()
    {
        return reject("provide absolute paths to kubectl and a regular kubeconfig file");
    }
    require_new_directory(&options.destination)?;

    let pod_route = format!(
        "/api/v1/namespaces/{}/pods/{}",
        policy.namespace, policy.pod
    );
    let rs_route = format!(
        "/apis/apps/v1/namespaces/{}/replicasets/{}",
        policy.namespace, policy.replica_set
    );
    let first = get(&options, &policy, &pod_route)?;
    let original: Value =
        serde_json::from_slice(&first).map_err(|_| "invalid Kubernetes Pod response")?;
    let revision = pod_revision(&original, &policy)?;
    let replica_set = get(&options, &policy, &rs_route)?;
    let replica_set_observed_at = now()?;
    let pod = get(&options, &policy, &pod_route)?;
    let observed_at = now()?;
    let confirmed: Value =
        serde_json::from_slice(&pod).map_err(|_| "invalid Kubernetes Pod response")?;
    if pod_revision(&confirmed, &policy)? != revision || confirmed != original {
        return reject("Kubernetes Pod changed during capture");
    }

    let inventory = PodScope {
        cluster: policy.cluster,
        namespace: policy.namespace,
        pod: policy.pod,
        pod_uid: policy.pod_uid,
        container: policy.container,
        service: policy.target.service,
        environment: policy.target.environment,
        region: policy.target.region,
        resource: policy.target.resource,
        owner: policy.owner,
        observed_at,
    };
    let deployment = ReplicaSetScope {
        replica_set_observed_at,
        deployment_name: policy.deployment_name,
        deployment_uid: policy.deployment_uid,
        repository: policy.repository,
    };
    let (record, rollout) = collect_deployment(&inventory, &deployment, &pod, &replica_set)
        .map_err(|error| error.to_string())?;
    let trace = trace_observations(
        record,
        rollout,
        None,
        None,
        observed_at,
        policy.max_age_seconds,
    )
    .map_err(|error| error.to_string())?;
    if !trace.deployment_verified() {
        return reject("Kubernetes capture could not establish a fresh deployment observation");
    }
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": 1, "as_of": observed_at, "max_age_seconds": policy.max_age_seconds,
        "kubernetes": {
            "pod": {"path": "pod.json", "sha256": sha256(&pod)},
            "replica_set": {"path": "replica-set.json", "sha256": sha256(&replica_set)},
            "inventory": inventory, "deployment": deployment
        }
    }))?;
    create_directory(&options.destination)?;
    write_new(&options.destination.join("pod.json"), &pod)?;
    write_new(&options.destination.join("replica-set.json"), &replica_set)?;
    let manifest_path = options.destination.join("sources.json");
    write_new(&manifest_path, &manifest)?;
    Ok(json!({
        "schema_version": 1, "manifest": manifest_path, "manifest_digest": sha256(&manifest),
        "capture_policy_digest": policy_digest, "observed_at": observed_at,
    }))
}

fn pod_revision<'a>(pod: &'a Value, policy: &Policy) -> Result<&'a str> {
    let metadata = &pod["metadata"];
    if pod["apiVersion"] != "v1"
        || pod["kind"] != "Pod"
        || metadata["name"] != policy.pod
        || metadata["namespace"] != policy.namespace
        || metadata["uid"] != policy.pod_uid
    {
        return reject("Kubernetes capture returned a different Pod identity");
    }
    let owners = metadata["ownerReferences"]
        .as_array()
        .ok_or("missing Pod owner references")?;
    let mut controllers = owners.iter().filter(|owner| owner["controller"] == true);
    if controllers.next().is_none_or(|owner| {
        owner["kind"] != "ReplicaSet"
            || owner["apiVersion"] != "apps/v1"
            || owner["name"] != policy.replica_set
    }) || controllers.next().is_some()
    {
        return reject("Pod controller differs from the approved ReplicaSet");
    }
    metadata["resourceVersion"]
        .as_str()
        .filter(|version| !version.is_empty())
        .ok_or_else(|| "Kubernetes Pod capture requires an object resource version".into())
}

fn resource_name(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value.split('.').all(|part| {
            part.as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn get(options: &Options, policy: &Policy, route: &str) -> Result<Vec<u8>> {
    let kubeconfig = read_evidence(&options.kubeconfig, 1024 * 1024)
        .map_err(|_| "could not read the approved kubeconfig")?;
    if sha256(&kubeconfig) != policy.kubeconfig_digest {
        return reject("kubeconfig does not match the approved digest");
    }
    drop(kubeconfig);
    let mut command = Command::new(&options.kubectl);
    command
        .arg("--kubeconfig")
        .arg(&options.kubeconfig)
        .arg("--context")
        .arg(&policy.context)
        .args(["--request-timeout=5s", "get", "--raw", route])
        .env("KUBECTL_KUBERC", "false")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("CRAFT_")
        {
            command.env_remove(name);
        }
    }
    let (status, bytes) = read_bounded(
        &mut command,
        "Kubernetes capture",
        Duration::from_secs(10),
        MAX_SNAPSHOT_BYTES,
    )?;
    if !status.success() {
        return reject("kubectl could not read the approved Kubernetes resource");
    }
    Ok(bytes)
}
