use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Args;
use craft_factory_evidence::{dsse, release_trace::read_evidence, sha256};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Result,
    order::{PinnedReference, WorkOrder, valid_oid},
    reject, repository,
};

pub(crate) const LOCKFILE: &str = "package-lock.json";
pub(crate) const MANIFEST: &str = "package.json";
const DEPENDENCIES: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];
pub(crate) const MAX_BYTES: usize = 2 * 1024 * 1024;
const SCAN_PAYLOAD_TYPE: &str = "application/vnd.craft.dependency-scan+json";
const MAX_SCAN_AGE_SECONDS: u64 = 7 * 24 * 60 * 60;
type Package = (String, String);
type Finding = (String, String, String);

#[derive(Args)]
pub struct IntakeOptions {
    /// Approved updater observation, scanner receipts, and work-order template.
    input: PathBuf,
    #[arg(long)]
    baseline_osv: PathBuf,
    #[arg(long)]
    candidate_osv: PathBuf,
    /// Replacement signed scan pair for the original approved intake and lockfiles.
    #[arg(long)]
    refresh_scans: Option<PathBuf>,
}

#[derive(Args)]
pub struct SealScanOptions {
    /// Complete OSV report from the protected scanner job.
    report: PathBuf,
    #[arg(long)]
    lockfile: PathBuf,
    #[arg(long)]
    repository: String,
    #[arg(long)]
    commit: String,
    #[arg(long)]
    issuer: String,
    #[arg(long)]
    reference: String,
    /// Scanner job completion time from protected CI, in UTC Unix seconds.
    #[arg(long)]
    completed_at: Option<u64>,
    #[arg(long)]
    public_key_base64: String,
    /// Protected Ed25519 PKCS#8 DER key file.
    #[arg(long)]
    signing_key: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    schema_version: u32,
    repository: String,
    updater_actor: String,
    scanner_issuer: String,
    /// Base64-encoded raw Ed25519 public key. When set, both scans must be signed.
    scanner_public_key_base64: Option<String>,
    /// Maximum signed scan age in seconds, from 1 to 604800.
    scanner_max_age_seconds: Option<u64>,
    proposal: Proposal,
    baseline_scan: ScanEvidence,
    candidate_scan: ScanEvidence,
    work_order: WorkOrder,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proposal {
    pub repository: String,
    pub pull_request: String,
    actor: String,
    pub base: String,
    pub head: String,
    pub package: String,
    pub from_version: String,
    pub to_version: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum ScanEvidence {
    Unsigned(ScannerReceipt),
    Signed(dsse::Envelope),
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanRefresh {
    schema_version: u32,
    baseline_scan: dsse::Envelope,
    candidate_scan: dsse::Envelope,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScanObservation {
    schema_version: u32,
    repository: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<u64>,
    receipt: ScannerReceipt,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScannerReceipt {
    issuer: String,
    reference: String,
    commit: String,
    lockfile_digest: String,
    report_digest: String,
    completed: bool,
}

#[derive(Deserialize)]
struct OsvReport {
    results: Vec<OsvSource>,
}

#[derive(Deserialize)]
struct OsvSource {
    source: OsvLocation,
    packages: Vec<OsvItem>,
}

#[derive(Deserialize)]
struct OsvLocation {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct OsvItem {
    package: OsvPackage,
    #[serde(default)]
    vulnerabilities: Vec<Advisory>,
}

#[derive(Deserialize)]
struct OsvPackage {
    name: String,
    version: String,
    ecosystem: String,
}

#[derive(Deserialize)]
struct Advisory {
    id: String,
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    status: &'static str,
    update_class: &'static str,
    pub(crate) proposal: Proposal,
    pub(crate) input_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scanner_signing_key_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scanner_fresh_through: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scanner_refresh_digest: Option<String>,
    introduced_findings: usize,
    candidate_package_findings: usize,
    pub(crate) work_order: Option<WorkOrder>,
}

fn identifier(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 500 && !value.chars().any(char::is_control)
}

fn package_name(name: &str) -> bool {
    let word = |value: &str| {
        !value.is_empty()
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
    };
    name.len() <= 214
        && match name.strip_prefix('@') {
            Some(scoped) => scoped
                .split_once('/')
                .is_some_and(|(scope, package)| word(scope) && word(package)),
            None => word(name),
        }
}

fn version(value: &str) -> Result<[u64; 3]> {
    let parts: Vec<_> = value.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else {
        return reject("dependency intake requires stable npm release versions");
    };
    let parse = |part: &str| -> Result<u64> {
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return reject("dependency intake requires canonical npm versions");
        }
        Ok(part.parse()?)
    };
    Ok([parse(major)?, parse(minor)?, parse(patch)?])
}

fn without_dependency(mut root: Value, package: &str) -> Result<(Value, Vec<&'static str>)> {
    let object = root
        .as_object_mut()
        .ok_or("npm manifest must be a JSON object")?;
    if object.contains_key("workspaces") {
        return reject("dependency intake requires a single npm project with a root lockfile");
    }
    let mut sections = Vec::new();
    for section in DEPENDENCIES {
        if let Some(values) = object.get_mut(section) {
            let dependencies = values
                .as_object_mut()
                .ok_or("npm dependency map must be an object")?;
            if let Some(spec) = dependencies.remove(package) {
                if !spec.as_str().is_some_and(identifier) {
                    return reject("npm dependency specification is missing");
                }
                sections.push(section);
            }
        }
    }
    Ok((root, sections))
}

fn parse_lock(data: &[u8]) -> Result<Value> {
    let lock: Value = serde_json::from_slice(data)?;
    if lock["lockfileVersion"] != 3 || lock["packages"][""].as_object().is_none() {
        return reject("dependency intake requires npm lockfile version 3 with a root package");
    }
    Ok(lock)
}

fn read_lock(data: &[u8], manifest: &Value) -> Result<Value> {
    let lock = parse_lock(data)?;
    if DEPENDENCIES
        .iter()
        .any(|section| manifest[section] != lock["packages"][""][section])
    {
        return reject("npm manifest and lockfile root disagree");
    }
    Ok(lock)
}

fn registry_source(entry: &Value) -> Result<String> {
    let url = entry["resolved"]
        .as_str()
        .ok_or("npm registry resolution is missing")?
        .parse::<ureq::http::Uri>()?;
    let authority = url
        .authority()
        .filter(|authority| !authority.as_str().contains('@'))
        .ok_or("npm registry authority is missing")?;
    if url.scheme_str() != Some("https")
        || url.path().is_empty()
        || !entry["integrity"].as_str().is_some_and(|integrity| {
            integrity.strip_prefix("sha512-").is_some_and(|digest| {
                digest.len() == 88 && STANDARD.decode(digest).is_ok_and(|bytes| bytes.len() == 64)
            })
        })
    {
        return reject("updated package requires an HTTPS registry source and SHA-512 integrity");
    }
    Ok(authority.as_str().to_ascii_lowercase())
}

fn locked_packages(lock: &Value) -> Result<BTreeSet<Package>> {
    let mut packages = BTreeSet::new();
    for (path, item) in lock["packages"]
        .as_object()
        .ok_or("npm lockfile has no packages")?
    {
        if path.is_empty() {
            continue;
        }
        let inferred = path
            .rsplit_once("node_modules/")
            .map(|(_, name)| name)
            .ok_or("npm lockfile contains a package outside node_modules")?;
        let name = item["name"].as_str().unwrap_or(inferred);
        let version = item["version"]
            .as_str()
            .filter(|value| identifier(value))
            .ok_or("npm lockfile contains an unresolved package")?;
        if !package_name(name) || item["link"] == true {
            return reject("dependency intake requires resolved registry packages");
        }
        packages.insert((name.to_owned(), version.to_owned()));
    }
    Ok(packages)
}

impl ScanEvidence {
    fn receipt(&self, repo: &str, key: Option<&[u8; 32]>) -> Result<(ScannerReceipt, Option<u64>)> {
        match (self, key) {
            (Self::Unsigned(receipt), None) => Ok((receipt.clone(), None)),
            (Self::Signed(envelope), Some(key)) => {
                let bytes = dsse::verify_envelope(envelope, SCAN_PAYLOAD_TYPE, key)
                    .map_err(|error| error.to_string())?;
                let observation: ScanObservation = serde_json::from_slice(&bytes)?;
                if observation.schema_version != 1 || observation.repository != repo {
                    return reject(
                        "signed scanner evidence belongs to a different repository or schema",
                    );
                }
                Ok((observation.receipt, observation.completed_at))
            }
            _ => reject("scanner evidence must match the intake signing policy"),
        }
    }
}

fn scan(
    file: &Path,
    receipt: &ScannerReceipt,
    issuer: &str,
    commit: &str,
    lock: &[u8],
    expected_packages: &BTreeSet<Package>,
) -> Result<BTreeSet<Finding>> {
    let bytes = read_evidence(file, MAX_BYTES as u64).map_err(|error| error.to_string())?;
    if !receipt.completed
        || receipt.issuer != issuer
        || !identifier(&receipt.reference)
        || receipt.commit != commit
        || receipt.lockfile_digest != sha256(lock)
        || receipt.report_digest != sha256(&bytes)
    {
        return reject("OSV scan receipt does not match its issuer, commit, lockfile, or report");
    }
    findings(&bytes, expected_packages)
}

fn findings(bytes: &[u8], expected_packages: &BTreeSet<Package>) -> Result<BTreeSet<Finding>> {
    let report: OsvReport = serde_json::from_slice(bytes)?;
    let [result] = report.results.as_slice() else {
        return reject("OSV scan must report one exact lockfile");
    };
    let path = result.source.path.replace('\\', "/");
    if result.source.kind != "lockfile"
        || (path != LOCKFILE && !path.ends_with("/package-lock.json"))
    {
        return reject("OSV scan did not identify the npm lockfile");
    }
    let mut covered = BTreeSet::new();
    let mut findings = BTreeSet::new();
    for item in &result.packages {
        if item.package.ecosystem != "npm"
            || !package_name(&item.package.name)
            || !identifier(&item.package.version)
        {
            return reject("OSV scan contains a non-npm or invalid package");
        }
        covered.insert((item.package.name.clone(), item.package.version.clone()));
        for advisory in &item.vulnerabilities {
            if !identifier(&advisory.id) {
                return reject("OSV scan has an invalid finding identity");
            }
            findings.insert((
                item.package.name.clone(),
                item.package.version.clone(),
                advisory.id.clone(),
            ));
        }
    }
    if &covered != expected_packages {
        return reject("OSV scan coverage differs from the full resolved npm package set");
    }
    Ok(findings)
}

pub fn seal_scan(options: SealScanOptions) -> Result<Value> {
    if !identifier(&options.repository)
        || !valid_oid(&options.commit)
        || !identifier(&options.issuer)
        || !identifier(&options.reference)
    {
        return reject("invalid trusted scanner job metadata");
    }
    if let Some(completed_at) = options.completed_at
        && (completed_at == 0 || completed_at > crate::evidence_capture::now()?)
    {
        return reject("invalid scanner completion time");
    }
    let public_key =
        dsse::public_key(&options.public_key_base64).map_err(|error| error.to_string())?;
    let read =
        |path: &Path| read_evidence(path, MAX_BYTES as u64).map_err(|error| error.to_string());
    let lockfile = read(&options.lockfile)?;
    let lock = parse_lock(&lockfile)?;
    let report = read(&options.report)?;
    findings(&report, &locked_packages(&lock)?)?;
    let observation = ScanObservation {
        schema_version: 1,
        repository: options.repository,
        completed_at: options.completed_at,
        receipt: ScannerReceipt {
            issuer: options.issuer,
            reference: options.reference,
            commit: options.commit,
            lockfile_digest: sha256(&lockfile),
            report_digest: sha256(&report),
            completed: true,
        },
    };
    let private_key = read_evidence(&options.signing_key, 1024)
        .map_err(|_| "could not read the protected scanner signing key file")?;
    let envelope = dsse::seal(
        &serde_json::to_vec(&observation)?,
        SCAN_PAYLOAD_TYPE,
        &private_key,
        &public_key,
        "scanner",
    )
    .map_err(|error| error.to_string())?;
    Ok(serde_json::from_slice(&envelope)?)
}

pub fn run(options: IntakeOptions) -> Result<Report> {
    run_in_checkout(options, None)
}

pub fn run_in_checkout(options: IntakeOptions, checkout: Option<&Path>) -> Result<Report> {
    let bytes = read_evidence(&options.input, 128 * 1024).map_err(|error| error.to_string())?;
    let mut input: Input = serde_json::from_slice(&bytes)?;
    let proposal = &input.proposal;
    let order = &mut input.work_order;
    if let Some(checkout) = checkout {
        order.repository = checkout.to_owned();
    }
    order.validate()?;
    if input.schema_version != 1
        || !identifier(&input.repository)
        || !identifier(&input.updater_actor)
        || !identifier(&input.scanner_issuer)
        || !identifier(&proposal.pull_request)
        || proposal.repository != input.repository
        || proposal.actor != input.updater_actor
        || proposal.base != order.base
        || !valid_oid(&proposal.head)
        || !package_name(&proposal.package)
        || order.ci_failure.is_some()
        || order.dependency_intake_digest.is_some()
    {
        return reject(
            "dependency proposal differs from the approved repository, updater, or base",
        );
    }
    let before_version = version(&proposal.from_version)?;
    let after_version = version(&proposal.to_version)?;
    if after_version <= before_version {
        return reject("dependency candidate must advance the resolved version");
    }
    let update_class = if before_version[0] != after_version[0] {
        "major"
    } else if before_version[1] != after_version[1] {
        "minor"
    } else {
        "patch"
    };
    repository::validate_base(order)?;
    let paths = repository::candidate_history_paths(order, &proposal.base, &proposal.head)?;
    if !paths.contains(LOCKFILE)
        || paths
            .iter()
            .any(|path| !matches!(path.as_str(), LOCKFILE | MANIFEST))
    {
        return reject("updater candidate must change only the npm manifest and lockfile");
    }

    let read = |head, path| repository::committed_blob(order, head, path, MAX_BYTES);
    let manifest_before: Value = serde_json::from_slice(&read(&proposal.base, MANIFEST)?)?;
    let manifest_after: Value = serde_json::from_slice(&read(&proposal.head, MANIFEST)?)?;
    let (before_without, before_sections) =
        without_dependency(manifest_before.clone(), &proposal.package)?;
    let (after_without, after_sections) =
        without_dependency(manifest_after.clone(), &proposal.package)?;
    if before_without != after_without
        || before_sections.is_empty()
        || before_sections != after_sections
    {
        return reject("npm manifest changes must stay within the declared direct dependency");
    }
    let before_bytes = read(&proposal.base, LOCKFILE)?;
    let after_bytes = read(&proposal.head, LOCKFILE)?;
    let before = read_lock(&before_bytes, &manifest_before)?;
    let after = read_lock(&after_bytes, &manifest_after)?;
    let key = format!("node_modules/{}", proposal.package);
    if before["packages"][&key]["version"] != proposal.from_version
        || after["packages"][&key]["version"] != proposal.to_version
    {
        return reject("committed lockfile versions differ from the updater observation");
    }
    if registry_source(&before["packages"][&key])? != registry_source(&after["packages"][&key])? {
        return reject("updated package changed its registry authority");
    }
    for section in DEPENDENCIES {
        if let Some(dependencies) = manifest_before[section].as_object() {
            for name in dependencies
                .keys()
                .filter(|name| *name != &proposal.package)
            {
                let key = format!("node_modules/{name}");
                let old = &before["packages"][&key];
                let new = &after["packages"][&key];
                if old["version"].as_str().is_none()
                    || ["version", "resolved", "integrity"]
                        .iter()
                        .any(|field| old[field] != new[field])
                {
                    return reject("another direct dependency changed its resolved identity");
                }
            }
        }
    }
    let key = input
        .scanner_public_key_base64
        .as_deref()
        .map(dsse::public_key)
        .transpose()
        .map_err(|error| error.to_string())?;
    let scanner_signing_key_digest = key.map(|value| sha256(&value));
    let (mut baseline_scan, baseline_completed_at) = input
        .baseline_scan
        .receipt(&input.repository, key.as_ref())?;
    let (mut candidate_scan, candidate_completed_at) = input
        .candidate_scan
        .receipt(&input.repository, key.as_ref())?;
    let mut scanner_refresh_digest = None;
    let scanner_fresh_through = if let Some(max_age) = input.scanner_max_age_seconds {
        let key = key
            .as_ref()
            .filter(|_| (1..=MAX_SCAN_AGE_SECONDS).contains(&max_age))
            .ok_or(
                "scanner freshness requires a signing key and a maximum age of 1 to 604800 seconds",
            )?;
        let now = crate::evidence_capture::now()?;
        let (mut baseline, mut candidate) =
            scan_completion_times(baseline_completed_at, candidate_completed_at, now)?;
        if let Some(path) = &options.refresh_scans {
            let raw = read_evidence(path, 128 * 1024).map_err(|error| error.to_string())?;
            let refresh: ScanRefresh = serde_json::from_slice(&raw)?;
            if refresh.schema_version != 1 {
                return reject("unsupported scanner refresh schema");
            }
            let before = ScanEvidence::Signed(refresh.baseline_scan)
                .receipt(&input.repository, Some(key))?;
            let after = ScanEvidence::Signed(refresh.candidate_scan)
                .receipt(&input.repository, Some(key))?;
            let (new_baseline, new_candidate) = scan_completion_times(before.1, after.1, now)?;
            if new_baseline <= baseline || new_candidate <= candidate {
                return reject("both refreshed scans must be newer than the original scans");
            }
            (baseline_scan, candidate_scan) = (before.0, after.0);
            (baseline, candidate) = (new_baseline, new_candidate);
            scanner_refresh_digest = Some(sha256(&raw));
        }
        let earliest = baseline.min(candidate);
        if now - earliest > max_age {
            return reject("signed scanner evidence is stale or has a future completion time");
        }
        Some(earliest + max_age)
    } else {
        if options.refresh_scans.is_some() {
            return reject("scanner refresh requires a signed intake with a freshness policy");
        }
        None
    };
    let baseline_findings = scan(
        &options.baseline_osv,
        &baseline_scan,
        &input.scanner_issuer,
        &proposal.base,
        &before_bytes,
        &locked_packages(&before)?,
    )?;
    let candidate_findings = scan(
        &options.candidate_osv,
        &candidate_scan,
        &input.scanner_issuer,
        &proposal.head,
        &after_bytes,
        &locked_packages(&after)?,
    )?;
    let introduced_findings = candidate_findings.difference(&baseline_findings).count();
    let candidate_package_findings = candidate_findings
        .iter()
        .filter(|(name, _, _)| name == &proposal.package)
        .count();
    let ready = introduced_findings == 0 && candidate_package_findings == 0;
    let input_digest = sha256(&bytes);
    if ready {
        order.require_native_checks = true;
        order.dependency_intake_digest = Some(input_digest.clone());
        order.acceptance.push(format!(
            "Run the approved compatibility checks for {} from {} to {} and record behavior at both pinned revisions.",
            proposal.package, proposal.from_version, proposal.to_version
        ));
        for (reference, revision) in [
            (&proposal.pull_request, &proposal.head),
            (&baseline_scan.reference, &baseline_scan.report_digest),
            (&candidate_scan.reference, &candidate_scan.report_digest),
        ] {
            order.context.push(PinnedReference {
                location: reference.clone(),
                revision: revision.clone(),
            });
        }
    }
    Ok(Report {
        schema_version: 1,
        status: if ready {
            "ready_for_supervised_verification"
        } else {
            "review_required"
        },
        update_class,
        proposal: input.proposal,
        input_digest,
        scanner_signing_key_digest,
        scanner_fresh_through,
        scanner_refresh_digest,
        introduced_findings,
        candidate_package_findings,
        work_order: ready.then_some(input.work_order),
    })
}

fn scan_completion_times(
    baseline: Option<u64>,
    candidate: Option<u64>,
    now: u64,
) -> Result<(u64, u64)> {
    let (Some(baseline), Some(candidate)) = (baseline, candidate) else {
        return reject("scanner freshness requires signed completion times for both scans");
    };
    if baseline.min(candidate) == 0 || baseline.max(candidate) > now {
        return reject("signed scanner evidence has an invalid completion time");
    }
    Ok((baseline, candidate))
}
