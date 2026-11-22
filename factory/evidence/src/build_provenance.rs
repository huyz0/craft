use std::path::PathBuf;

use crate::{
    Result, dsse, reject,
    release_trace::{Build, Bundle, Observation, read_evidence, valid_digest},
    sha256, valid_oid,
};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
pub const MAX_ATTESTATION_BYTES: u64 = 1024 * 1024;
pub const MAX_POLICY_BYTES: u64 = 32 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildInput {
    schema_version: u32,
    pub artifact_digest: String,
    pub source_commit: String,
    invocation_id: String,
    started_on: Option<String>,
    finished_on: String,
}

#[derive(Args)]
pub struct Options {
    /// Existing release evidence bundle.
    bundle: PathBuf,
    #[arg(long)]
    attestation: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    /// Approved SHA-256 digest of the exact policy file.
    #[arg(long)]
    policy_digest: String,
    /// Exact SHA-256 digest reported by the inventory or deployment evidence.
    #[arg(long)]
    artifact_digest: String,
    /// UTC Unix seconds when the attestation was collected.
    #[arg(long)]
    observed_at: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    repository: String,
    source_uri: String,
    artifact_name: String,
    builder_id: String,
    build_type: String,
    external_parameters: Value,
    /// Base64 encoding of the raw 32-byte Ed25519 public key.
    public_key_base64: String,
    approval_reference: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Statement {
    #[serde(rename = "_type")]
    statement_type: String,
    predicate_type: String,
    subject: Vec<Subject>,
    predicate: Predicate,
}

#[derive(Deserialize)]
struct Subject {
    name: Option<String>,
    digest: SubjectDigest,
}

#[derive(Deserialize)]
struct SubjectDigest {
    sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Predicate {
    build_definition: Definition,
    run_details: RunDetails,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Definition {
    build_type: String,
    external_parameters: Value,
    resolved_dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    uri: Option<String>,
    digest: Option<DependencyDigest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DependencyDigest {
    git_commit: Option<String>,
}

#[derive(Deserialize)]
struct RunDetails {
    builder: Builder,
    metadata: Metadata,
}

#[derive(Deserialize)]
struct Builder {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    invocation_id: String,
    started_on: Option<String>,
    finished_on: String,
}

pub fn run(options: Options) -> Result<Bundle> {
    let policy =
        read_evidence(&options.policy, MAX_POLICY_BYTES).map_err(|error| error.to_string())?;
    let archive = read_evidence(&options.attestation, MAX_ATTESTATION_BYTES)
        .map_err(|error| error.to_string())?;
    let build = collect(
        &archive,
        &policy,
        &options.policy_digest,
        &options.artifact_digest,
        options.observed_at,
    )?;
    Bundle::with_build(&options.bundle, build).map_err(|error| error.to_string().into())
}

pub fn collect(
    archive: &[u8],
    policy_bytes: &[u8],
    policy_digest: &str,
    artifact_digest: &str,
    observed_at: u64,
) -> Result<Build> {
    if !valid_digest(artifact_digest) || observed_at == 0 {
        return reject("provide a SHA-256 artifact digest and a positive UTC observation time");
    }
    if archive.len() as u64 > MAX_ATTESTATION_BYTES {
        return reject("build attestation exceeds the permitted byte limit");
    }
    let (policy, public_key) = approved_policy(policy_bytes, policy_digest)?;
    let statement = serde_json::from_slice(&dsse::verify(archive, PAYLOAD_TYPE, &public_key)?)?;
    match_statement(
        &policy,
        statement,
        archive,
        policy_digest,
        artifact_digest,
        observed_at,
    )
}

pub fn validate_policy(policy_bytes: &[u8], policy_digest: &str) -> Result<()> {
    approved_policy(policy_bytes, policy_digest).map(|_| ())
}

pub fn seal(
    input: &BuildInput,
    policy_bytes: &[u8],
    policy_digest: &str,
    private_key: &[u8],
    observed_at: u64,
) -> Result<Vec<u8>> {
    let (policy, public_key) = approved_policy(policy_bytes, policy_digest)?;
    if input.schema_version != 1
        || !valid_oid(&input.source_commit)
        || !valid_digest(&input.artifact_digest)
    {
        return reject("invalid trusted build record");
    }
    let mut metadata =
        json!({"invocationId": input.invocation_id, "finishedOn": input.finished_on});
    if let Some(started) = &input.started_on {
        metadata["startedOn"] = json!(started);
    }
    let payload = serde_json::to_vec(&json!({
        "_type": "https://in-toto.io/Statement/v1", "predicateType": "https://slsa.dev/provenance/v1",
        "subject": [{"name": policy.artifact_name, "digest": {"sha256": &input.artifact_digest[7..]}}],
        "predicate": {
            "buildDefinition": {
                "buildType": policy.build_type, "externalParameters": policy.external_parameters,
                "resolvedDependencies": [{"uri": policy.source_uri, "digest": {"gitCommit": input.source_commit}}],
            },
            "runDetails": {
                "builder": {"id": policy.builder_id},
                "metadata": metadata,
            },
        },
    }))?;
    let envelope = dsse::seal(&payload, PAYLOAD_TYPE, private_key, &public_key, "build")?;
    collect(
        &envelope,
        policy_bytes,
        policy_digest,
        &input.artifact_digest,
        observed_at,
    )?;
    Ok(envelope)
}

fn approved_policy(policy_bytes: &[u8], policy_digest: &str) -> Result<(Policy, [u8; 32])> {
    if policy_bytes.len() as u64 > MAX_POLICY_BYTES {
        return reject("build provenance policy exceeds the permitted byte limit");
    }
    if sha256(policy_bytes) != policy_digest {
        return reject("build provenance policy does not match the approved digest");
    }
    let policy: Policy = serde_json::from_slice(policy_bytes)?;
    if policy.schema_version != 1
        || !policy.external_parameters.is_object()
        || [
            &policy.repository,
            &policy.source_uri,
            &policy.artifact_name,
            &policy.builder_id,
            &policy.build_type,
            &policy.approval_reference,
        ]
        .iter()
        .any(|value| !identifier_text(value))
    {
        return reject("invalid build provenance policy");
    }
    let public_key = dsse::public_key(&policy.public_key_base64)?;
    Ok((policy, public_key))
}

fn match_statement(
    policy: &Policy,
    statement: Statement,
    archive: &[u8],
    policy_digest: &str,
    artifact_digest: &str,
    observed_at: u64,
) -> Result<Build> {
    let definition = statement.predicate.build_definition;
    let run = statement.predicate.run_details;
    if statement.statement_type != "https://in-toto.io/Statement/v1"
        || statement.predicate_type != "https://slsa.dev/provenance/v1"
        || run.builder.id != policy.builder_id
        || definition.build_type != policy.build_type
        || definition.external_parameters != policy.external_parameters
    {
        return reject(
            "signed provenance does not match the approved statement, builder, or build inputs",
        );
    }
    let mut subjects = statement
        .subject
        .iter()
        .filter(|subject| subject.name.as_deref() == Some(&policy.artifact_name));
    let expected_hash = artifact_digest.strip_prefix("sha256:");
    if subjects
        .next()
        .is_none_or(|subject| subject.digest.sha256.as_deref() != expected_hash)
        || subjects.next().is_some()
    {
        return reject(
            "signed provenance does not identify one approved subject with the requested artifact digest",
        );
    }
    let mut dependencies = definition
        .resolved_dependencies
        .iter()
        .filter(|dependency| dependency.uri.as_deref() == Some(&policy.source_uri));
    let source_commit = dependencies
        .next()
        .and_then(|dependency| dependency.digest.as_ref()?.git_commit.as_ref())
        .filter(|commit| valid_oid(commit))
        .ok_or("signed provenance is missing the approved source URI and full Git commit")?;
    if dependencies.next().is_some() {
        return reject("signed provenance has multiple records for the approved source URI");
    }

    let built_at = timestamp(&run.metadata.finished_on)?;
    let started_at = run
        .metadata
        .started_on
        .as_deref()
        .map(timestamp)
        .transpose()?;
    if !identifier_text(&run.metadata.invocation_id)
        || started_at.is_some_and(|time| time > built_at)
        || built_at > observed_at
    {
        return reject(
            "signed provenance has inconsistent build timing or an invalid invocation ID",
        );
    }
    Ok(Build {
        artifact_digest: artifact_digest.to_owned(),
        repository: policy.repository.clone(),
        source_commit: source_commit.clone(),
        built_at,
        observation: Observation {
            source: "verified-slsa-dsse".to_owned(),
            reference: format!(
                "dsse:{}#{}#policy:{}",
                run.metadata.invocation_id,
                sha256(archive),
                policy_digest
            ),
            observed_at,
        },
    })
}

fn timestamp(text: &str) -> Result<u64> {
    OffsetDateTime::parse(text, &Rfc3339)
        .ok()
        .and_then(|time| u64::try_from(time.unix_timestamp()).ok())
        .filter(|time| *time > 0)
        .ok_or_else(|| {
            "signed provenance requires a valid positive RFC 3339 build timestamp".into()
        })
}

fn identifier_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 500 && !value.chars().any(char::is_control)
}
