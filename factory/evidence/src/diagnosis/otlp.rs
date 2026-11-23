use std::{collections::HashSet, path::PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{Log, Metric, MetricKey, Severity, Telemetry, Window, approved_name};
use crate::{
    Result, reject,
    release_trace::{
        KubernetesContainer, Observation, Target, Version, present, read_evidence, valid_digest,
    },
    sha256,
};

const MAX_EXPORT_BYTES: u64 = 2 * 1024 * 1024;
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NO_RECORDED_VALUE: u64 = 1;

#[derive(Args)]
pub struct Options {
    /// Approved collection and redaction policy.
    policy: PathBuf,
    #[arg(long)]
    policy_digest: String,
    #[arg(long)]
    logs: PathBuf,
    #[arg(long)]
    metrics: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Archive {
    pub policy: PathBuf,
    pub logs: PathBuf,
    pub metrics: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    target: Target,
    version: Version,
    service_instance_id: String,
    #[serde(default)]
    kubernetes_container: Option<KubernetesContainer>,
    log_scope: String,
    metric_scope: String,
    coverage: Window,
    observed_at: u64,
    approval_reference: String,
    logs_digest: String,
    metrics_digest: String,
    max_records: usize,
    log_codes: Vec<String>,
    metrics: Vec<MetricKey>,
}

pub(crate) struct Binding {
    pub version: Version,
    pub kubernetes_container: Option<KubernetesContainer>,
}

impl Policy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !self.target.valid()
            || !self.version.valid()
            || !present(&self.service_instance_id)
            || self
                .kubernetes_container
                .as_ref()
                .is_some_and(|container| !container.valid())
            || !present(&self.log_scope)
            || !present(&self.metric_scope)
            || !self.coverage.valid()
            || self.coverage.to - self.coverage.from > 3600
            || self.coverage.to.checked_mul(NANOS_PER_SECOND).is_none()
            || self.observed_at < self.coverage.to
            || !present(&self.approval_reference)
            || !valid_digest(&self.logs_digest)
            || !valid_digest(&self.metrics_digest)
            || !(1..=500).contains(&self.max_records)
            || self.log_codes.len() > 100
            || self.metrics.len() > 100
            || self.log_codes.iter().any(|code| !approved_name(code))
            || self
                .metrics
                .iter()
                .any(|key| !approved_name(&key.name) || !approved_name(&key.unit))
            || self.log_codes.iter().collect::<HashSet<_>>().len() != self.log_codes.len()
            || self.metrics.iter().collect::<HashSet<_>>().len() != self.metrics.len()
        {
            return reject("invalid diagnostic OTLP import policy");
        }
        Ok(())
    }

    fn contains_resource(&self, resource: &Value) -> Result<bool> {
        let required = [
            ("service.name", &self.target.service),
            ("service.instance.id", &self.service_instance_id),
            ("deployment.environment.name", &self.target.environment),
            ("cloud.region", &self.target.region),
            ("craft.resource.id", &self.target.resource),
            ("craft.deployment.id", &self.version.deployment_id),
            ("craft.config.revision", &self.version.config_revision),
            ("craft.artifact.digest", &self.version.artifact_digest),
        ];
        let container_attributes = self.kubernetes_container.iter().flat_map(|container| {
            [
                ("k8s.pod.uid", &container.pod_uid),
                ("k8s.container.name", &container.container_name),
                ("container.id", &container.container_id),
            ]
        });
        let attributes = repeated(&resource["attributes"])?;
        let mut matches = true;
        for (key, expected) in required.into_iter().chain(container_attributes) {
            let mut entries = attributes
                .iter()
                .filter(|attribute| attribute["key"] == key);
            matches &= entries
                .next()
                .is_some_and(|entry| entry["value"]["stringValue"] == *expected);
            if entries.next().is_some() {
                return reject("OTLP resource repeats a release binding attribute");
            }
        }
        Ok(matches)
    }

    fn timestamp(&self, sample: &Value) -> Result<Option<u64>> {
        let field = &sample["timeUnixNano"];
        let value = field
            .as_u64()
            .or_else(|| field.as_str()?.parse().ok())
            .filter(|value| *value > 0)
            .ok_or("approved OTLP record is missing a valid event or sample time")?;
        let from = self.coverage.from * NANOS_PER_SECOND;
        let to = self.coverage.to * NANOS_PER_SECOND;
        Ok((from..=to)
            .contains(&value)
            .then_some(value / NANOS_PER_SECOND))
    }
}

pub fn run(options: Options) -> Result<Telemetry> {
    let archive = Archive {
        policy: options.policy,
        logs: options.logs,
        metrics: options.metrics,
    };
    load(&archive, &options.policy_digest).map(|(telemetry, _)| telemetry)
}

pub(crate) fn load(archive: &Archive, expected_digest: &str) -> Result<(Telemetry, Binding)> {
    let raw = read_evidence(&archive.policy, 32 * 1024)?;
    let policy_digest = sha256(&raw);
    if policy_digest != expected_digest {
        return reject("OTLP import policy does not match the approved digest");
    }
    let policy: Policy =
        serde_json::from_slice(&raw).map_err(|_| "invalid diagnostic OTLP import policy format")?;
    policy.validate()?;
    let logs = read_export(&archive.logs, &policy.logs_digest, "resourceLogs", &policy)?;
    let metrics = read_export(
        &archive.metrics,
        &policy.metrics_digest,
        "resourceMetrics",
        &policy,
    )?;
    if logs.is_empty() && metrics.is_empty() {
        return reject("OTLP archives do not contain the approved resource instance and release");
    }
    let logs = collect_logs(&policy, &logs)?;
    let metrics = collect_metrics(&policy, &metrics, policy.max_records - logs.len())?;
    Ok((
        Telemetry {
            schema_version: 1,
            target: policy.target,
            coverage: policy.coverage,
            observation: Observation {
                source: "opentelemetry-otlp-json".to_owned(),
                reference: format!("otlp:policy:{policy_digest}"),
                observed_at: policy.observed_at,
            },
            logs,
            metrics,
        },
        Binding {
            version: policy.version,
            kubernetes_container: policy.kubernetes_container,
        },
    ))
}

fn read_export(
    path: &std::path::Path,
    digest: &str,
    field: &str,
    policy: &Policy,
) -> Result<Vec<Value>> {
    let raw = read_evidence(path, MAX_EXPORT_BYTES)?;
    if sha256(&raw) != digest {
        return reject("OTLP export does not match the approved digest");
    }
    let mut body: Value = serde_json::from_slice(&raw).map_err(|_| "invalid OTLP JSON export")?;
    let Some(Value::Array(entries)) = body.get_mut(field).map(Value::take) else {
        return reject("expected an OTLP export with the approved signal type");
    };
    let mut matching = Vec::new();
    for entry in entries {
        if policy.contains_resource(&entry["resource"])? {
            matching.push(entry);
        }
    }
    Ok(matching)
}

fn repeated(value: &Value) -> Result<&[Value]> {
    match value {
        Value::Null => Ok(&[]),
        Value::Array(items) => Ok(items),
        _ => reject("invalid OTLP repeated field"),
    }
}

fn collect_logs(policy: &Policy, resources: &[Value]) -> Result<Vec<Log>> {
    let mut logs = Vec::new();
    for resource in resources {
        for scope in repeated(&resource["scopeLogs"])? {
            if scope["scope"]["name"] != policy.log_scope {
                continue;
            }
            for record in repeated(&scope["logRecords"])? {
                let Some(code) = record["eventName"]
                    .as_str()
                    .filter(|code| policy.log_codes.iter().any(|allowed| allowed == *code))
                else {
                    continue;
                };
                let Some(timestamp) = policy.timestamp(record)? else {
                    continue;
                };
                let severity = match record["severityNumber"].as_u64() {
                    Some(1..=8) => Severity::Debug,
                    Some(9..=12) => Severity::Info,
                    Some(13..=16) => Severity::Warn,
                    Some(17..=20) => Severity::Error,
                    Some(21..=24) => Severity::Critical,
                    _ => return reject("approved OTLP event is missing a valid severity number"),
                };
                if logs.len() == policy.max_records {
                    return reject("diagnostic OTLP output exceeds the approved record limit");
                }
                logs.push(Log {
                    timestamp,
                    version: policy.version.clone(),
                    code: code.to_owned(),
                    severity,
                });
            }
        }
    }
    logs.sort_by_key(|log| log.timestamp);
    Ok(logs)
}

fn collect_metrics(policy: &Policy, resources: &[Value], remaining: usize) -> Result<Vec<Metric>> {
    let mut output = Vec::new();
    let mut samples = HashSet::new();
    for resource in resources {
        for scope in repeated(&resource["scopeMetrics"])? {
            if scope["scope"]["name"] != policy.metric_scope {
                continue;
            }
            for metric in repeated(&scope["metrics"])? {
                let Some(key) = policy
                    .metrics
                    .iter()
                    .find(|key| metric["name"] == key.name && metric["unit"] == key.unit)
                else {
                    continue;
                };
                if !metric["gauge"].is_object()
                    || ["sum", "histogram", "exponentialHistogram", "summary"]
                        .iter()
                        .any(|kind| !metric[*kind].is_null())
                {
                    return reject("approved OTLP metric must be a gauge");
                }
                for point in repeated(&metric["gauge"]["dataPoints"])? {
                    let flags = match &point["flags"] {
                        Value::Null => 0,
                        value => value
                            .as_u64()
                            .filter(|flags| *flags <= u32::MAX.into())
                            .ok_or("approved OTLP gauge has invalid flags")?,
                    };
                    if flags & NO_RECORDED_VALUE != 0 {
                        continue;
                    }
                    let Some(timestamp) = policy.timestamp(point)? else {
                        continue;
                    };
                    if !repeated(&point["attributes"])?.is_empty()
                        || !samples.insert((key, timestamp))
                    {
                        return reject("approved OTLP gauge contains labeled or ambiguous samples");
                    }
                    let value = gauge_value(point)?;
                    if output.len() == remaining {
                        return reject("diagnostic OTLP output exceeds the approved record limit");
                    }
                    output.push(Metric {
                        timestamp,
                        version: policy.version.clone(),
                        metric: key.clone(),
                        value,
                    });
                }
            }
        }
    }
    output.sort_by_key(|sample| sample.timestamp);
    Ok(output)
}

fn gauge_value(point: &Value) -> Result<f64> {
    if point["asInt"].is_null() {
        return point["asDouble"]
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| "approved OTLP gauge requires one finite numeric sample".into());
    }
    if !point["asDouble"].is_null() {
        return reject("approved OTLP gauge contains multiple numeric values");
    }
    point["asInt"]
        .as_i64()
        .or_else(|| point["asInt"].as_str()?.parse().ok())
        .filter(|value| value.unsigned_abs() <= 1u64 << 53)
        .map(|value| value as f64)
        .ok_or_else(|| "approved OTLP integer sample is invalid or would lose precision".into())
}
