use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use clap::Args;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{Result, process_tree::read_bounded, read_json, reject, shared_runner};

const CONNECTOR_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Args)]
pub struct Options {
    /// Jira relay configuration with watches, discovery queries, or both.
    policy: PathBuf,
    /// Submit observations to this queue. Without a queue, print observations for review.
    #[arg(long)]
    queue: Option<String>,
    #[arg(long, requires = "queue")]
    ca: Option<PathBuf>,
    /// Read the queue intake token from this protected file.
    #[arg(long, requires = "queue")]
    intake_token_file: Option<PathBuf>,
    /// Atomically replace the last completed JSON report, including partial results.
    #[arg(long)]
    report_file: Option<PathBuf>,
    /// Stop starting new reads before the batch deadline and report deferred entries.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=300))]
    budget_seconds: Option<u64>,
    /// Keep the next watch position in a private SQLite file.
    #[arg(long, requires = "budget_seconds")]
    cursor_db: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    craft: PathBuf,
    craft_state: PathBuf,
    craft_profile: String,
    cloud_id: String,
    #[serde(default)]
    watches: Vec<Watch>,
    #[serde(default)]
    discoveries: Vec<Discovery>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
    rule: String,
    jql: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Watch {
    rule: String,
    issue: String,
    issue_id: String,
    approved_intent: Option<String>,
    work_order: Value,
}

pub fn poll(options: Options) -> Result<Value> {
    let budget = options.budget_seconds.map(Duration::from_secs);
    let deadline = budget.map(|budget| Instant::now() + budget);
    let policy: Policy = read_json(&options.policy)?;
    policy.validate()?;
    let roster = policy
        .watches
        .iter()
        .map(|watch| (&watch.rule, &watch.issue, &watch.issue_id))
        .collect::<Vec<_>>();
    let queries = policy
        .discoveries
        .iter()
        .map(|discovery| (&discovery.rule, &discovery.jql))
        .collect::<Vec<_>>();
    let entry_count = roster.len() + queries.len();
    let roster_key = shared_runner::sha256(&serde_json::to_vec(&(
        &policy.cloud_id,
        &options.queue,
        roster,
        queries,
    ))?);
    let mut cursor_db = options
        .cursor_db
        .as_deref()
        .map(Connection::open)
        .transpose()?;
    // Keep the original SQLite column for existing cursor files. Its index now spans both lists.
    let cursor = cursor_db.as_mut().map(|db| -> rusqlite::Result<_> {
        db.busy_timeout(Duration::ZERO)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS jira_relay_cursor (
            id INTEGER PRIMARY KEY CHECK (id = 1), roster_key TEXT NOT NULL, watch_index INTEGER NOT NULL
        );")?;
        Ok(tx)
    }).transpose()?;
    let start = if let Some(tx) = &cursor {
        tx.query_row(
            "SELECT watch_index FROM jira_relay_cursor WHERE id = 1 AND roster_key = ?1",
            [&roster_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < entry_count)
        .unwrap_or(0)
    } else {
        0
    };
    let mut observations = Vec::with_capacity(policy.watches.len());
    let mut errors = Vec::new();
    let mut first_deferred = None;
    let mut watch_budget = CONNECTOR_TIMEOUT;
    if options.queue.is_some() {
        watch_budget += shared_runner::QUEUE_TIMEOUT;
    }
    let mut discoveries = Vec::new();
    for offset in 0..entry_count {
        let index = (start + offset) % entry_count;
        let watch = policy.watches.get(index);
        let required = if watch.is_some() {
            watch_budget
        } else {
            CONNECTOR_TIMEOUT
        };
        if !fits(deadline, required) {
            if budget.is_some_and(|budget| budget > required) {
                first_deferred.get_or_insert(index);
            }
            let reason = "Jira relay batch budget exhausted";
            errors.push(match watch {
                Some(watch) => watch_error(watch, "budget", reason),
                None => json!({"rule": policy.discoveries[index - policy.watches.len()].rule,
                               "stage": "budget", "reason": reason}),
            });
            continue;
        }
        if let Some(watch) = watch {
            if let Some(observation) = poll_watch(&policy, &options, watch, &mut errors)? {
                observations.push(observation);
            }
            continue;
        }
        let discovery = &policy.discoveries[index - policy.watches.len()];
        match policy.reader().discover(&discovery.jql) {
            Ok((mut matches, truncated)) => {
                matches.retain(|found| {
                    !policy.watches.iter().any(|watch| {
                        watch.rule == discovery.rule
                            && (found["issue"] == watch.issue
                                || found["issue_id"] == watch.issue_id)
                    })
                });
                discoveries.push(
                    json!({"rule": discovery.rule, "has_more": truncated, "suggestions": matches}),
                );
            }
            Err(error) => errors.push(
                json!({"rule": discovery.rule, "stage": "discover", "reason": error.to_string()}),
            ),
        }
    }
    if let Some(tx) = cursor {
        let saved = tx.execute("INSERT INTO jira_relay_cursor (id, roster_key, watch_index) VALUES (1, ?1, ?2)
            ON CONFLICT(id) DO UPDATE SET roster_key = excluded.roster_key, watch_index = excluded.watch_index",
            rusqlite::params![roster_key, first_deferred.unwrap_or(0) as i64]).and_then(|_| tx.commit());
        if let Err(error) = saved {
            errors.push(json!({"stage": "cursor", "reason": error.to_string()}));
        }
    }
    let mut report = json!({"schema_version": 1, "submitted": options.queue.is_some() && !policy.watches.is_empty(),
                            "observations": observations, "errors": errors});
    if !policy.discoveries.is_empty() {
        report["discoveries"] = json!(discoveries);
    }
    if let Some(path) = &options.report_file
        && let Err(error) = save_report(path, &options, &report)
    {
        report["errors"]
            .as_array_mut()
            .expect("report has an error list")
            .push(json!({"stage": "report", "reason": error.to_string()}));
    }
    Ok(report)
}

fn poll_watch(
    policy: &Policy,
    options: &Options,
    watch: &Watch,
    errors: &mut Vec<Value>,
) -> Result<Option<Value>> {
    let mut snapshot = match policy.reader().observe(&watch.issue, &watch.issue_id) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            errors.push(watch_error(watch, "read", &error));
            return Ok(None);
        }
    };
    snapshot["intent_approved"] =
        json!(watch.approved_intent.as_deref() == snapshot["intent_digest"].as_str());
    let mut observation = json!({"rule": watch.rule, "snapshot": snapshot});
    if let Some(queue) = &options.queue {
        let snapshot = &observation["snapshot"];
        let mut order = watch.work_order.clone();
        let context = order["context"]
            .as_array_mut()
            .ok_or("Jira relay order is missing context pins")?;
        context.push(json!({
            "location": format!("jira-intent:{}/{}", policy.cloud_id, watch.issue_id),
            "revision": snapshot["intent_digest"],
        }));
        match shared_runner::submit_standing(
            queue,
            options.ca.as_deref(),
            options.intake_token_file.as_deref(),
            &watch.rule,
            snapshot,
            Some(&order),
        ) {
            Ok(reply) if reply["decision"]["state"].is_string() => {
                observation["decision"] = reply["decision"].clone();
            }
            Ok(_) => errors.push(watch_error(watch, "submit", "queue returned no decision")),
            Err(error) => errors.push(watch_error(watch, "submit", &error)),
        }
    }
    Ok(Some(observation))
}

fn save_report(path: &Path, options: &Options, report: &Value) -> Result<()> {
    if let Ok(output) = fs::canonicalize(path) {
        let inputs = [
            Some(options.policy.as_path()),
            options.cursor_db.as_deref(),
            options.ca.as_deref(),
            options.intake_token_file.as_deref(),
        ];
        if inputs
            .into_iter()
            .flatten()
            .any(|input| fs::canonicalize(input).is_ok_and(|input| input == output))
        {
            return reject("the Jira report path must be separate from its input and state files");
        }
    }
    crate::evidence_capture::replace_json(path, report)
}

fn fits(deadline: Option<Instant>, maximum: Duration) -> bool {
    deadline.is_none_or(|deadline| deadline.saturating_duration_since(Instant::now()) >= maximum)
}

fn watch_error(watch: &Watch, stage: &str, reason: impl std::fmt::Display) -> Value {
    json!({"rule": watch.rule, "issue": watch.issue, "stage": stage, "reason": reason.to_string()})
}

impl Policy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || self.watches.len() > 20
            || self.discoveries.len() > 5
            || (self.watches.is_empty() && self.discoveries.is_empty())
        {
            return reject("Jira relay requires up to 20 watches or 5 discovery rules");
        }
        self.reader().validate()?;
        let mut discovered_rules = HashSet::new();
        for discovery in &self.discoveries {
            if !rule_id(&discovery.rule)
                || !discovered_rules.insert(&discovery.rule)
                || discovery.jql.trim().is_empty()
                || discovery.jql.len() > 2048
                || discovery.jql.chars().any(char::is_control)
            {
                return reject("invalid Jira discovery rule or JQL");
            }
        }
        let mut unique = HashSet::new();
        for watch in &self.watches {
            if !issue_key(&watch.issue)
                || !positive_id(&watch.issue_id)
                || !rule_id(&watch.rule)
                || !unique.insert((&watch.rule, &watch.issue))
                || watch.approved_intent.as_ref().is_some_and(|value| {
                    !value
                        .strip_prefix("sha256:")
                        .is_some_and(|hash| hash.len() == 64 && crate::order::valid_oid(hash))
                })
                || watch.work_order["context"]
                    .as_array()
                    .is_none_or(|context| {
                        context.iter().any(|item| {
                            item["location"]
                                .as_str()
                                .is_some_and(|location| location.starts_with("jira-intent:"))
                        })
                    })
            {
                return reject(
                    "invalid Jira watch identity, approval digest, or work-order context",
                );
            }
        }
        Ok(())
    }

    fn reader(&self) -> JiraReader<'_> {
        JiraReader {
            craft: &self.craft,
            craft_state: &self.craft_state,
            craft_profile: &self.craft_profile,
            cloud_id: &self.cloud_id,
        }
    }
}

pub(crate) struct JiraReader<'a> {
    pub craft: &'a std::path::Path,
    pub craft_state: &'a std::path::Path,
    pub craft_profile: &'a str,
    pub cloud_id: &'a str,
}

impl JiraReader<'_> {
    pub fn validate(&self) -> Result<()> {
        if !self.craft.is_absolute()
            || !self.craft_state.is_absolute()
            || self.craft_profile.trim().is_empty()
            || !self.cloud_id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
            || !self
                .cloud_id
                .bytes()
                .all(|b| b == b'-' || b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
            return reject(
                "Jira capture requires an explicit Cloud ID, Craft paths, and a saved profile",
            );
        }
        #[cfg(windows)]
        if self
            .craft
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
        {
            return reject("the Jira Craft binary must be an executable file");
        }
        Ok(())
    }

    pub fn observe(&self, issue: &str, issue_id: &str) -> Result<Value> {
        if !issue_key(issue) || !positive_id(issue_id) {
            return reject("invalid Jira issue identity");
        }
        let response = self.read(
            "get",
            &["--issue", issue],
            "id,key,status.id,assignee.account_id,updated,labels,summary,description",
        )?;
        self.normalize(issue, issue_id, &response)
    }

    fn discover(&self, jql: &str) -> Result<(Vec<Value>, bool)> {
        let response = self.read(
            "search",
            &[
                &format!("--jql={jql}"),
                "--limit",
                "20",
                "--page-size",
                "20",
            ],
            "id,key",
        )?;
        self.assert_site(&response, "jira.issue.search")?;
        if response["data"]["cached"] != false || response["page"]["consistency"] != "live" {
            return reject("Jira discovery requires a live search result");
        }
        let has_more = response["page"]["has_more"]
            .as_bool()
            .ok_or("Jira search is missing its page status")?;
        let items = response["data"]["items"]
            .as_array()
            .filter(|items| items.len() <= 20)
            .ok_or("Jira search is missing a bounded list of issues")?;
        let mut keys = HashSet::new();
        let mut ids = HashSet::new();
        let mut matches = Vec::new();
        for issue in items {
            let key = issue["key"]
                .as_str()
                .filter(|key| issue_key(key) && keys.insert(*key))
                .ok_or("Jira search returned a missing or duplicate issue key")?;
            let id = issue["id"]
                .as_str()
                .filter(|id| positive_id(id) && ids.insert(*id))
                .ok_or("Jira search returned a missing or duplicate issue ID")?;
            matches.push(json!({"issue": key, "issue_id": id}));
        }
        Ok((matches, has_more))
    }

    fn read(&self, action: &str, arguments: &[&str], fields: &str) -> Result<Value> {
        let mut command = Command::new(self.craft);
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
            .args(["jira", "issue", action])
            .args(arguments)
            .args([
                "--cloud-id",
                self.cloud_id,
                "--profile",
                self.craft_profile,
                "--state-root",
            ])
            .arg(self.craft_state)
            .args([
                "--fields",
                fields,
                "--timeout-ms",
                "10000",
                "--max-bytes",
                "65536",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let (status, output) = read_bounded(
            &mut command,
            "Craft Jira connector",
            CONNECTOR_TIMEOUT,
            65536,
        )?;
        let response: Value = serde_json::from_slice(&output)?;
        if !status.success() || response["ok"] != true {
            let code = response["error"]["code"]
                .as_str()
                .filter(|code| {
                    code.len() <= 80
                        && code
                            .bytes()
                            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                })
                .unwrap_or("UNKNOWN");
            return reject(format!("Craft Jira {action} failed ({code})"));
        }
        Ok(response)
    }

    fn assert_site(&self, response: &Value, command: &str) -> Result<()> {
        let target = &response["data"]["target"];
        if response["schema_version"] != "1.0"
            || response["command"] != command
            || target["provider"] != "jira"
            || target["deployment"] != "cloud"
            || target["tenant"] != self.cloud_id
            || target["resource_type"] != "site"
            || target["id"] != self.cloud_id
        {
            return reject("Craft returned a different Jira command or Cloud site");
        }
        Ok(())
    }

    fn normalize(&self, key: &str, issue_id: &str, response: &Value) -> Result<Value> {
        self.assert_site(response, "jira.issue.get")?;
        let data = &response["data"];
        let issue = &data["issue"];
        if data["requested_issue"] != key
            || data["identity_resolution"] != "matching_key"
            || issue["key"] != key
            || issue["id"] != issue_id
        {
            return reject("Craft returned a different Jira issue or Cloud site");
        }
        let observed_at = data["observed_at"]
            .as_u64()
            .filter(|time| *time > 0)
            .ok_or("Craft Jira response has no observation time")?;
        let updated = issue["updated"]
            .as_str()
            .ok_or("Jira issue has no updated time")?;
        let source_revision = updated_millis(updated)?;
        let labels = issue["labels"]
            .as_array()
            .filter(|labels| {
                labels.len() <= 100
                    && labels
                        .iter()
                        .all(|label| label.as_str().is_some_and(|label| label.len() <= 100))
            })
            .ok_or("Jira did not return the requested labels")?;
        let summary = issue["summary"]
            .as_str()
            .filter(|summary| !summary.trim().is_empty())
            .ok_or("Jira did not return the requested summary")?;
        let description = issue
            .get("description")
            .ok_or("Jira did not return the requested description")?;
        let native_status = issue["status"]["id"]
            .as_str()
            .filter(|status| !status.is_empty())
            .ok_or("Jira did not return the native status ID")?;
        let assignee = match issue.get("assignee") {
            Some(Value::Null) => None,
            Some(user) => Some(
                user["account_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or("Jira did not return the assignee account ID")?,
            ),
            None => return reject("Jira did not return the assignee field"),
        };
        let intent_digest =
            shared_runner::sha256(&serde_json::to_vec(&json!([summary, description]))?);
        Ok(json!({
            "issue": key, "source": format!("jira:{}", self.cloud_id),
            "source_revision": source_revision, "observed_at": observed_at / 1000,
            "status": native_status, "assignee": assignee, "labels": labels,
            "intent_digest": intent_digest,
        }))
    }
}

fn rule_id(value: &str) -> bool {
    (1..=100).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn issue_key(value: &str) -> bool {
    value.split_once('-').is_some_and(|(project, number)| {
        project
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_uppercase)
            && project.len() <= 31
            && project
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            && positive_id(number)
    })
}

fn positive_id(id: &str) -> bool {
    !id.starts_with('0')
        && !id.is_empty()
        && id.len() <= 19
        && id.bytes().all(|b| b.is_ascii_digit())
}

fn updated_millis(raw: &str) -> Result<i64> {
    if !raw.is_ascii() || raw.len() > 64 {
        return reject("invalid Jira updated timestamp");
    }
    let mut normalized = raw.to_owned();
    if normalized.len() >= 5 {
        let offset = normalized.len() - 5;
        if matches!(normalized.as_bytes()[offset], b'+' | b'-')
            && normalized.as_bytes()[offset + 1..]
                .iter()
                .all(u8::is_ascii_digit)
        {
            normalized.insert(offset + 3, ':');
        }
    }
    let millis = OffsetDateTime::parse(&normalized, &Rfc3339)?.unix_timestamp_nanos() / 1_000_000;
    Ok(i64::try_from(millis)?)
}
