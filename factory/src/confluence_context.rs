use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use clap::Args;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Result, process_tree::read_bounded, read_json, reject, shared_runner::sha256};

const EXCERPT_LIMIT: u64 = 4096;
const CONNECTOR_LIMIT: u64 = 16384;

#[derive(Args)]
pub struct Options {
    /// Trusted Craft profile, Confluence page identity and repository destination.
    policy: PathBuf,
    /// Exact UTF-8 excerpt for the owner to approve.
    #[arg(long)]
    excerpt: Option<PathBuf>,
    /// Owner approval of this source, destination and exact excerpt.
    #[arg(long, requires = "excerpt")]
    approval: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    craft: PathBuf,
    craft_state: PathBuf,
    craft_profile: String,
    cloud_id: String,
    space_id: String,
    page_id: String,
    repository: String,
    location: String,
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    schema_version: u32,
    approval_digest: String,
    approval_reference: String,
}

pub(crate) fn prepare_release(
    policy: PathBuf,
    excerpt: PathBuf,
    approval: PathBuf,
) -> Result<Value> {
    let mut prepared = capture(Options {
        policy,
        excerpt: Some(excerpt),
        approval: Some(approval),
    })?;
    Ok(prepared["release"].take())
}

pub(crate) fn observe_source(path: &Path, report: Option<&Path>) -> Result<Value> {
    let policy: Policy = read_json(path)?;
    policy.validate()?;
    if let Some(report) = report {
        let parent = report
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if parent
            .canonicalize()?
            .starts_with(policy.craft_state.canonicalize()?)
        {
            return reject("the context check report must be outside the Craft state directory");
        }
        crate::evidence_capture::check_output_paths(
            &[Some(report)],
            [path, policy.craft.as_path()],
            "context check report path must be distinct from the inputs",
        )?;
    }
    capture_policy(policy, None, None)
}

pub(crate) fn assess_registered_source(observed: &Value, catalog: &Value) -> Result<Value> {
    let source = &observed["source"];
    let destination = &observed["destination"];
    let active = catalog["items"]
        .as_array()
        .ok_or("missing project context catalog items")?;
    let (status, registration) = match active.as_slice() {
        [] => ("unregistered", Value::Null),
        [item]
            if item["repository"] == destination["repository"]
                && item["location"] == destination["location"]
                && item["retired_at"].is_null() =>
        {
            let revision = format!(
                "{}/{}",
                source["version"]
                    .as_u64()
                    .ok_or("missing Confluence version")?,
                source["digest"]
                    .as_str()
                    .ok_or("missing Confluence fingerprint")?
            );
            let status = if item["source_reference"] != source["reference"]
                || item["kind"] != destination["kind"]
            {
                "different_binding"
            } else if item["source_revision"] != revision {
                "source_changed"
            } else {
                "current"
            };
            (
                status,
                json!({
                    "revision": item["revision"], "kind": item["kind"],
                    "source_reference": item["source_reference"], "source_revision": item["source_revision"],
                    "approval_reference": item["approval_reference"],
                }),
            )
        }
        _ => return reject("the active project context did not match the requested destination"),
    };
    Ok(json!({
        "schema_version": 1, "status": status, "source": source,
        "destination": destination, "registered": registration,
    }))
}

pub fn capture(options: Options) -> Result<Value> {
    let policy: Policy = read_json(options.policy)?;
    policy.validate()?;
    let excerpt = options.excerpt.map(read_excerpt).transpose()?;
    let approval: Option<Approval> = options.approval.map(read_json).transpose()?;
    capture_policy(policy, excerpt, approval)
}

fn capture_policy(
    policy: Policy,
    excerpt: Option<String>,
    approval: Option<Approval>,
) -> Result<Value> {
    if let Some(approval) = &approval
        && (approval.schema_version != 2
            || !digest(&approval.approval_digest)
            || !bounded_text(&approval.approval_reference, 500))
    {
        return reject("provide a version 2 Confluence excerpt approval receipt");
    }

    let response = policy.read()?;
    let data = &response["data"];
    let item = &data["item"];
    if response["schema_version"] != "1.0"
        || response["command"] != "confluence.page.get"
        || data["target"]["provider"] != "confluence"
        || data["target"]["deployment"] != "cloud"
        || data["target"]["resource_type"] != "site"
        || data["target"]["tenant"] != policy.cloud_id
        || data["target"]["id"] != policy.cloud_id
        || item["space_id"] != policy.space_id
        || item["id"] != policy.page_id
        || item["status"] != "current"
    {
        return reject("Craft returned a different Confluence site, space, or published page");
    }
    let version = item["revision"]["version"]
        .as_u64()
        .filter(|version| (1..=u64::from(u32::MAX)).contains(version))
        .ok_or("Craft returned no valid published Confluence version")?;
    let source_digest = item["revision"]["digest"]
        .as_str()
        .filter(|value| digest(value))
        .ok_or("Craft returned no valid published Confluence fingerprint")?;
    let title = item["title"]
        .as_str()
        .filter(|title| bounded_text(title, 1024))
        .ok_or("Craft returned no valid Confluence page title")?;
    if item["version"] != version
        || item["revision"]["representation"] != "storage"
        || item["fields_not_returned"] != json!([])
    {
        return reject("Craft did not return the complete published Confluence revision");
    }
    let source_reference = format!(
        "confluence:{}/spaces/{}/pages/{}",
        policy.cloud_id, policy.space_id, policy.page_id
    );
    let source_revision = format!("{version}/{source_digest}");
    let destination =
        json!({"repository": policy.repository, "location": policy.location, "kind": policy.kind});
    let excerpt_digest = excerpt.as_ref().map(|text| sha256(text.as_bytes()));
    let approval_digest = excerpt_digest
        .as_ref()
        .map(|digest| {
            serde_json::to_vec(&json!([
                "confluence-context-v2",
                source_reference,
                source_revision,
                destination,
                digest,
            ]))
            .map(|payload| sha256(&payload))
        })
        .transpose()?;
    let release = approval.map(|approval| {
        if Some(approval.approval_digest.as_str()) != approval_digest.as_deref() {
            return reject("the Confluence revision, repository destination, or excerpt changed after approval");
        }
        let excerpt = excerpt.ok_or("provide the approved Confluence excerpt")?;
        let revision = sha256(format!("{}\n{}\n{}\n{}\n{}", policy.kind, source_reference,
            source_revision, approval.approval_reference, excerpt).as_bytes());
        Ok(json!({"repository": policy.repository, "location": policy.location, "kind": policy.kind,
            "revision": revision, "source_reference": source_reference, "source_revision": source_revision,
            "approval_reference": approval.approval_reference, "excerpt": excerpt}))
    }).transpose()?;
    Ok(
        json!({"schema_version": 1, "source": {"reference": source_reference, "version": version,
        "title": title, "digest": source_digest}, "destination": destination,
        "excerpt_digest": excerpt_digest, "approval_digest": approval_digest, "release": release}),
    )
}

fn read_excerpt(path: PathBuf) -> Result<String> {
    let mut excerpt = String::new();
    File::open(path)?
        .take(EXCERPT_LIMIT + 1)
        .read_to_string(&mut excerpt)?;
    if excerpt.trim().is_empty()
        || excerpt.len() as u64 > EXCERPT_LIMIT
        || excerpt
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return reject("provide nonempty Confluence excerpt text up to 4 KiB");
    }
    Ok(excerpt)
}

impl Policy {
    fn validate(&self) -> Result<()> {
        let cloud = &self.cloud_id;
        let location = self.location.strip_prefix("project:").unwrap_or_default();
        if self.schema_version != 1
            || !self.craft.is_absolute()
            || !self.craft_state.is_absolute()
            || !bounded_text(&self.craft_profile, 200)
            || !bounded_text(&self.repository, 200)
            || !cloud.split('-').map(str::len).eq([8, 4, 4, 4, 12])
            || !cloud
                .bytes()
                .all(|b| b == b'-' || b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            || ![&self.space_id, &self.page_id].into_iter().all(|id| {
                !id.is_empty()
                    && id.len() <= 19
                    && !id.starts_with('0')
                    && id.bytes().all(|b| b.is_ascii_digit())
            })
            || location.len() > 80
            || !location
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !location
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
            || !matches!(self.kind.as_str(), "decision" | "runbook")
        {
            return reject("invalid Confluence context policy");
        }
        Ok(())
    }

    fn read(&self) -> Result<Value> {
        let mut command = Command::new(&self.craft);
        command.env_remove("CREDENTIALS_DIRECTORY");
        for (name, _) in std::env::vars_os() {
            if name
                .to_str()
                .is_some_and(|name| name.to_ascii_uppercase().starts_with("CRAFT_FACTORY_"))
            {
                command.env_remove(name);
            }
        }
        command
            .args([
                "confluence",
                "page",
                "get",
                "--id",
                &self.page_id,
                "--cloud-id",
                &self.cloud_id,
                "--profile",
                &self.craft_profile,
                "--state-root",
            ])
            .arg(&self.craft_state)
            .args([
                "--fields",
                "space_id,title,status,version,revision",
                "--timeout-ms",
                "10000",
                "--max-bytes",
                "16384",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let (status, output) = read_bounded(
            &mut command,
            "Craft Confluence connector",
            Duration::from_secs(12),
            CONNECTOR_LIMIT,
        )?;
        if !status.success() {
            return reject(
                "Craft could not read the published Confluence page with the configured profile",
            );
        }
        let response: Value = serde_json::from_slice(&output)?;
        if response["ok"] != true {
            return reject("Craft refused the published Confluence page read");
        }
        Ok(response)
    }
}

fn digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && crate::order::valid_oid(hex))
}

fn bounded_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
