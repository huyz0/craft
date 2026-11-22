use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use clap::Args;
use craft_factory_evidence::{
    build_provenance::{
        BuildInput, MAX_ATTESTATION_BYTES, MAX_POLICY_BYTES, collect, seal, validate_policy,
    },
    release_trace::{read_evidence, valid_digest},
    sha256, valid_oid,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use ureq::{
    Agent,
    http::Uri,
    tls::{Certificate, RootCerts, TlsConfig},
};

use crate::{
    Result,
    evidence_capture::{create_directory, now, require_new_directory, write_new},
    reject,
};

#[derive(Args)]
pub struct Options {
    /// Approved capture policy selecting one immutable envelope.
    policy: PathBuf,
    /// SHA-256 digest of the exact capture policy from the approving channel.
    #[arg(long)]
    policy_digest: String,
    /// Signing policy named by the capture policy.
    #[arg(long)]
    signing_policy: PathBuf,
    /// PEM certificate for a private archive CA pinned by the capture policy.
    #[arg(long)]
    archive_ca: Option<PathBuf>,
    /// New directory for the original envelope, signing policy, and manifest build section.
    #[arg(long)]
    destination: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    schema_version: u32,
    /// Exact HTTPS URL on the approved evidence archive, with no query string.
    envelope_url: String,
    envelope_digest: String,
    signing_policy_digest: String,
    archive_ca_digest: Option<String>,
    artifact_digest: String,
    source_commit: String,
}

#[derive(Args)]
pub struct PublishOptions {
    /// Trusted build record with the invocation, exact commit, and produced digest.
    build: PathBuf,
    #[arg(long)]
    archive_policy: PathBuf,
    /// Digest of the archive policy from the approving owner.
    #[arg(long)]
    archive_policy_digest: String,
    #[arg(long)]
    signing_policy: PathBuf,
    /// Protected local file containing the Ed25519 private key in PKCS#8 DER format.
    #[arg(long)]
    signing_key: PathBuf,
    #[arg(long)]
    archive_ca: Option<PathBuf>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchivePolicy {
    schema_version: u32,
    /// Direct HTTPS prefix under which the publisher may create immutable objects.
    archive_prefix: String,
    signing_policy_digest: String,
    archive_ca_digest: Option<String>,
}

pub fn publish(options: PublishOptions) -> Result<Value> {
    let raw =
        read_evidence(&options.archive_policy, 32 * 1024).map_err(|error| error.to_string())?;
    if sha256(&raw) != options.archive_policy_digest {
        return reject("build archive policy does not match the approved digest");
    }
    let policy: ArchivePolicy = serde_json::from_slice(&raw)?;
    let prefix = archive_prefix(&policy.archive_prefix)?;
    if policy.schema_version != 1 {
        return reject("unsupported build archive policy version");
    }
    let ca = archive_ca(
        policy.archive_ca_digest.as_deref(),
        options.archive_ca.as_deref(),
    )?;
    let signing_policy = read_evidence(&options.signing_policy, MAX_POLICY_BYTES)
        .map_err(|error| error.to_string())?;
    let input = read_evidence(&options.build, 32 * 1024).map_err(|error| error.to_string())?;
    let build: BuildInput = serde_json::from_slice(&input)?;
    let key = read_evidence(&options.signing_key, 1024)
        .map_err(|_| "could not read the protected signing key file")?;
    let envelope = seal(
        &build,
        &signing_policy,
        &policy.signing_policy_digest,
        &key,
        now()?,
    )
    .map_err(|error| error.to_string())?;
    drop(key);
    let digest = sha256(&envelope);
    let endpoint = approved_endpoint(&format!("{prefix}{}.dsse.json", &digest[7..]))?;
    let token = archive_token("CRAFT_FACTORY_BUILD_PUBLISH_TOKEN")?
        .ok_or("CRAFT_FACTORY_BUILD_PUBLISH_TOKEN is required")?;
    let agent = archive_agent(ca);
    let response = agent
        .put(endpoint.clone())
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("If-None-Match", "*")
        .send(envelope.as_slice())
        .map_err(|_| "could not publish the signed build envelope")?;
    if !matches!(response.status().as_u16(), 200 | 201 | 204 | 412) {
        return reject("unexpected response to conditional build evidence publication");
    }
    drop(response);
    if get_object(&agent, endpoint.clone(), Some(&token))? != envelope {
        return reject("published build evidence does not match the signed envelope");
    }
    Ok(json!({
        "schema_version": 1, "archive_policy_digest": options.archive_policy_digest,
        "capture_policy": {
            "schema_version": 1, "envelope_url": endpoint.to_string(), "envelope_digest": digest,
            "signing_policy_digest": policy.signing_policy_digest, "archive_ca_digest": policy.archive_ca_digest,
            "artifact_digest": build.artifact_digest, "source_commit": build.source_commit,
        },
    }))
}

pub fn run(options: Options) -> Result<Value> {
    capture(options, get, now)
}

fn capture(
    options: Options,
    fetch: impl FnOnce(Uri, Option<Certificate<'static>>) -> Result<Vec<u8>>,
    time: impl FnOnce() -> Result<u64>,
) -> Result<Value> {
    require_new_directory(&options.destination)?;
    let raw = read_evidence(&options.policy, 32 * 1024).map_err(|error| error.to_string())?;
    let policy_digest = sha256(&raw);
    if policy_digest != options.policy_digest {
        return reject("build capture policy does not match the approved digest");
    }
    let policy: Policy = serde_json::from_slice(&raw)?;
    if policy.schema_version != 1
        || !valid_digest(&policy.envelope_digest)
        || !valid_digest(&policy.artifact_digest)
        || !valid_oid(&policy.source_commit)
    {
        return reject("invalid build capture scope");
    }
    let endpoint = approved_endpoint(&policy.envelope_url)?;
    let signing_policy = read_evidence(&options.signing_policy, MAX_POLICY_BYTES)
        .map_err(|error| error.to_string())?;
    validate_policy(&signing_policy, &policy.signing_policy_digest)
        .map_err(|error| error.to_string())?;
    let ca = archive_ca(
        policy.archive_ca_digest.as_deref(),
        options.archive_ca.as_deref(),
    )?;

    let envelope = fetch(endpoint, ca)?;
    if envelope.len() as u64 > MAX_ATTESTATION_BYTES || sha256(&envelope) != policy.envelope_digest
    {
        return reject(
            "retrieved build envelope does not match the approved digest and size limit",
        );
    }
    let observed_at = time()?;
    let build = collect(
        &envelope,
        &signing_policy,
        &policy.signing_policy_digest,
        &policy.artifact_digest,
        observed_at,
    )
    .map_err(|error| error.to_string())?;
    if build.source_commit != policy.source_commit {
        return reject("signed build does not match the approved source commit");
    }

    let source = serde_json::to_vec_pretty(&json!({
        "attestation": {"path": "build.dsse.json", "sha256": policy.envelope_digest},
        "policy": {"path": "signing-policy.json", "sha256": policy.signing_policy_digest},
        "observed_at": observed_at,
    }))?;
    create_directory(&options.destination)?;
    write_new(&options.destination.join("build.dsse.json"), &envelope)?;
    write_new(
        &options.destination.join("signing-policy.json"),
        &signing_policy,
    )?;
    let source_path = options.destination.join("build-source.json");
    write_new(&source_path, &source)?;
    Ok(json!({
        "schema_version": 1, "source": source_path, "source_digest": sha256(&source),
        "capture_policy_digest": policy_digest, "artifact_digest": build.artifact_digest,
        "source_commit": build.source_commit, "observed_at": observed_at,
    }))
}

fn archive_prefix(value: &str) -> Result<Uri> {
    let prefix = approved_endpoint(value)?;
    let path = prefix.path();
    let Some(path) = path
        .strip_prefix('/')
        .and_then(|path| path.strip_suffix('/'))
    else {
        return reject("the build archive prefix must end with a slash");
    };
    if path.split('/').any(|part| {
        part.is_empty()
            || matches!(part, "." | "..")
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    }) {
        return reject("invalid build archive path prefix");
    }
    Ok(prefix)
}

fn archive_ca(digest: Option<&str>, path: Option<&Path>) -> Result<Option<Certificate<'static>>> {
    match (digest, path) {
        (Some(digest), Some(path)) => {
            let bytes = read_evidence(path, 32 * 1024).map_err(|error| error.to_string())?;
            if sha256(&bytes) != digest {
                return reject("build archive CA does not match the approved digest");
            }
            Ok(Some(Certificate::from_pem(&bytes)?))
        }
        (None, None) => Ok(None),
        _ => reject("provide the archive CA exactly when the approved policy pins it"),
    }
}

fn approved_endpoint(value: &str) -> Result<Uri> {
    if value.len() > 2048 || value.contains(['@', '#', '?', '\\']) {
        return reject("expected a direct HTTPS evidence archive URL without credentials or query");
    }
    let uri: Uri = value
        .parse()
        .map_err(|_| "invalid build evidence archive URL")?;
    if uri.scheme_str() != Some("https") || uri.host().is_none() || uri.path() == "/" {
        return reject("expected a direct HTTPS evidence archive URL with an object path");
    }
    Ok(uri)
}

fn get(endpoint: Uri, ca: Option<Certificate<'static>>) -> Result<Vec<u8>> {
    let token = archive_token("CRAFT_FACTORY_BUILD_EVIDENCE_TOKEN")?;
    get_object(&archive_agent(ca), endpoint, token.as_deref())
}

fn archive_agent(ca: Option<Certificate<'static>>) -> Agent {
    let mut config = Agent::config_builder()
        .https_only(true)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(8)));
    if let Some(ca) = ca {
        config = config.tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::new_with_certs(&[ca]))
                .build(),
        );
    }
    config.build().into()
}

fn archive_token(variable: &str) -> Result<Option<String>> {
    match std::env::var(variable) {
        Ok(token)
            if !token.is_empty()
                && token.len() <= 4096
                && token.bytes().all(|byte| byte.is_ascii_graphic()) =>
        {
            Ok(Some(token))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        _ => reject("invalid build evidence archive credential"),
    }
}

fn get_object(agent: &Agent, endpoint: Uri, token: Option<&str>) -> Result<Vec<u8>> {
    let mut request = agent.get(endpoint).header("Accept", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let mut response = request
        .call()
        .map_err(|_| "could not read the approved build evidence archive")?;
    if response.status() != 200 {
        return reject("unexpected build evidence archive response");
    }
    response
        .body_mut()
        .with_config()
        .limit(MAX_ATTESTATION_BYTES)
        .read_to_vec()
        .map_err(|_| "could not read the bounded build evidence body".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::fs;

    #[test]
    fn only_direct_https_object_urls_are_accepted() {
        for url in [
            "http://archive.example/build.json",
            "https://archive.example",
            "https://user@archive.example/build.json",
            "https://archive.example/build.json?token=secret",
            "https://archive.example/build.json#fragment",
            r"https://archive.example\build.json",
        ] {
            assert!(approved_endpoint(url).is_err(), "accepted {url}");
        }
        assert!(approved_endpoint("https://archive.example/builds/abc/build.json").is_ok());
    }

    #[test]
    fn captures_a_signed_envelope_only_after_verifying_policy_artifact_and_commit() {
        let mut nonce = [0; 8];
        getrandom::getrandom(&mut nonce).unwrap();
        let root =
            std::env::temp_dir().join(format!("factory-build-{:x}", u64::from_ne_bytes(nonce)));
        create_directory(&root).unwrap();
        let capture_path = root.join("capture.json");
        let signing_path = root.join("approved-signing.json");
        let destination = root.join("archive");
        let sha = "a".repeat(64);
        let artifact = format!("sha256:{sha}");
        let commit = "b".repeat(40);
        let source = "git+https://bitbucket.org/team/service";
        let url = "https://archive.example/pipeline/run-1/build.dsse.json";
        let key = Ed25519KeyPair::from_seed_unchecked(&[19; 32]).unwrap();
        let signing = serde_json::to_vec(&json!({
            "schema_version": 1, "repository": "bitbucket:team/service", "source_uri": source,
            "artifact_name": "registry.example/api", "builder_id": "fixture:builder",
            "build_type": "fixture:build", "external_parameters": {},
            "public_key_base64": STANDARD.encode(key.public_key().as_ref()),
            "approval_reference": "fixture:signing-policy",
        }))
        .unwrap();
        let signing_digest = sha256(&signing);
        write_new(&signing_path, &signing).unwrap();
        let payload = serde_json::to_vec(&json!({
            "_type": "https://in-toto.io/Statement/v1",
            "predicateType": "https://slsa.dev/provenance/v1",
            "subject": [{"name": "registry.example/api", "digest": {"sha256": sha}}],
            "predicate": {
                "buildDefinition": {"buildType": "fixture:build", "externalParameters": {},
                    "resolvedDependencies": [{"uri": source, "digest": {"gitCommit": commit}}]},
                "runDetails": {"builder": {"id": "fixture:builder"},
                    "metadata": {"invocationId": "run-1", "finishedOn": "2026-11-21T05:14:20Z"}},
            },
        }))
        .unwrap();
        let mut pae =
            format!("DSSEv1 28 application/vnd.in-toto+json {} ", payload.len()).into_bytes();
        pae.extend_from_slice(&payload);
        let envelope = |signature: &[u8]| {
            serde_json::to_vec_pretty(&json!({
                "payloadType": "application/vnd.in-toto+json", "payload": STANDARD.encode(&payload),
                "signatures": [{"sig": STANDARD.encode(signature)}],
            }))
            .unwrap()
        };
        let approved = envelope(key.sign(&pae).as_ref());
        let invalid_signature = envelope(&[0; 64]);
        let options = |object: &[u8], expected_commit: &str, expected_signing: &str| {
            let policy = serde_json::to_vec(&json!({
                "schema_version": 1, "envelope_url": url, "envelope_digest": sha256(object),
                "signing_policy_digest": expected_signing, "artifact_digest": artifact,
                "source_commit": expected_commit,
            }))
            .unwrap();
            fs::write(&capture_path, &policy).unwrap();
            Options {
                policy: capture_path.clone(),
                policy_digest: sha256(&policy),
                signing_policy: signing_path.clone(),
                archive_ca: None,
                destination: destination.clone(),
            }
        };
        let fetch = |endpoint: Uri, ca: Option<Certificate<'static>>| {
            assert_eq!(endpoint.to_string(), url);
            assert!(ca.is_none());
            Ok(approved.clone())
        };
        let clock = || Ok(1795238090);
        let bad_pin = capture(
            options(&approved, &commit, &sha256(b"other policy")),
            |_, _| panic!("incorrect signing policy digest must not fetch"),
            clock,
        );
        assert!(bad_pin.unwrap_err().to_string().contains("approved digest"));
        let substituted = capture(
            options(&approved, &commit, &signing_digest),
            |_, _| Ok(invalid_signature.clone()),
            clock,
        );
        assert!(
            substituted
                .unwrap_err()
                .to_string()
                .contains("approved digest")
        );
        let unsigned = capture(
            options(&invalid_signature, &commit, &signing_digest),
            |_, _| Ok(invalid_signature),
            clock,
        );
        assert!(
            unsigned
                .unwrap_err()
                .to_string()
                .contains("no DSSE signature")
        );
        let wrong_commit = capture(
            options(&approved, &"c".repeat(40), &signing_digest),
            fetch,
            clock,
        );
        assert!(
            wrong_commit
                .unwrap_err()
                .to_string()
                .contains("approved source commit")
        );
        assert!(!destination.exists());

        let receipt = capture(options(&approved, &commit, &signing_digest), fetch, clock).unwrap();
        assert_eq!(receipt["artifact_digest"], artifact);
        assert_eq!(receipt["source_commit"], commit);
        assert_eq!(
            fs::read(destination.join("build.dsse.json")).unwrap(),
            approved
        );
        assert_eq!(
            fs::read(destination.join("signing-policy.json")).unwrap(),
            signing
        );
        let raw = fs::read(destination.join("build-source.json")).unwrap();
        assert_eq!(receipt["source_digest"], sha256(&raw));
        assert_eq!(
            serde_json::from_slice::<Value>(&raw).unwrap(),
            json!({
                "attestation": {"path": "build.dsse.json", "sha256": sha256(&approved)},
                "policy": {"path": "signing-policy.json", "sha256": signing_digest},
                "observed_at": 1795238090_u64,
            })
        );
        fs::remove_dir_all(root).unwrap();
    }
}
