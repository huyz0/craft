//! Published-contract conformance for envelopes that composed-host fixtures produce.
//!
//! Every observed envelope is validated against the response schema that the public
//! contract file publishes for its command, so a fixture cannot pass with a result shape
//! that agents would reject. When `CRAFT_CONFORMANCE_RECORD` names a directory, each
//! observation also appends the producing test's name to `<command>__<outcome>`, where
//! the outcome is `ok` or the error code. `tools/jira_conformance.py` reads that
//! directory to show which success and failure outcomes each command really reached.
#![allow(dead_code)]

use craft_cli::dispatch::CommandResult;
use craft_core::contracts::Envelope;
use craft_core::error::CraftError;
use craft_core::projection::ProjectionContract;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::sync::OnceLock;

/// Fixture setup failures are test bugs; workspace lints forbid `expect`, so they panic here.
trait Must<T> {
    fn must(self, what: &str) -> T;
}

impl<T, E: std::fmt::Display> Must<T> for Result<T, E> {
    fn must(self, what: &str) -> T {
        match self {
            Ok(value) => value,
            Err(error) => panic!("{what}: {error}"),
        }
    }
}

impl<T> Must<T> for Option<T> {
    fn must(self, what: &str) -> T {
        match self {
            Some(value) => value,
            None => panic!("{what}: missing"),
        }
    }
}

const CONTRACTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../schemas/local-contracts.json"
);

struct Published {
    response_schema: Value,
    projection: Option<ProjectionContract>,
}

fn published() -> &'static HashMap<String, Published> {
    static PUBLISHED: OnceLock<HashMap<String, Published>> = OnceLock::new();
    PUBLISHED.get_or_init(|| {
        let text = std::fs::read_to_string(CONTRACTS).must("published contract file");
        let contracts: Value = serde_json::from_str(&text).must("published contract JSON");
        let operations = contracts["operations"]
            .as_object()
            .must("published operations");
        operations
            .iter()
            .map(|(command, entry)| {
                let descriptor = &entry["descriptor"];
                let projection = match &descriptor["projection"] {
                    Value::Null => None,
                    contract => {
                        Some(serde_json::from_value(contract.clone()).must("published projection"))
                    }
                };
                (
                    command.clone(),
                    Published {
                        response_schema: descriptor["response_schema"].clone(),
                        projection,
                    },
                )
            })
            .collect()
    })
}

/// Validates a default-projection envelope against its command's published response
/// schema and records the outcome. Panics with the schema errors when it does not conform.
pub fn observe(envelope: &Envelope) {
    observe_selecting(envelope, &[]);
}

/// Validates an envelope produced for an explicit field selection against the schema the
/// published projection contract derives for that selection, exactly as `schema` would.
pub fn observe_selecting(envelope: &Envelope, fields: &[&str]) {
    let published = published()
        .get(&envelope.command)
        .unwrap_or_else(|| panic!("{} has no published descriptor", envelope.command));
    let schema = if fields.is_empty() {
        published.response_schema.clone()
    } else {
        let selected: Vec<String> = fields.iter().map(|field| (*field).into()).collect();
        published
            .projection
            .as_ref()
            .must("selection on a command without a published projection")
            .compile(&published.response_schema, &selected)
            .must("published projection compiles")
            .response_schema(&published.response_schema)
            .must("projected schema")
    };
    let value = serde_json::to_value(envelope).must("envelope serializes");
    let validator = jsonschema::validator_for(&schema).must("published schema compiles");
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{} envelope violates its published schema: {errors:?}",
        envelope.command
    );
    record(envelope);
}

fn record(envelope: &Envelope) {
    let Some(directory) = std::env::var_os("CRAFT_CONFORMANCE_RECORD") else {
        return;
    };
    let outcome = match &envelope.error {
        None => "ok".to_owned(),
        Some(error) => serde_json::to_value(error.code)
            .ok()
            .and_then(|code| code.as_str().map(String::from))
            .unwrap_or_else(|| "UNKNOWN".into()),
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).must("conformance record directory");
    let test = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(format!("{}__{outcome}", envelope.command)))
        .must("conformance record file");
    writeln!(file, "{test}").must("conformance record line");
}

/// Observes a command result in a call chain and hands it back unchanged.
pub trait Observed: Sized {
    fn observed(self) -> Self {
        self.observed_selecting(&[])
    }
    fn observed_selecting(self, fields: &[&str]) -> Self;
}

impl Observed for CommandResult {
    fn observed_selecting(self, fields: &[&str]) -> Self {
        observe_selecting(&self.envelope, fields);
        self
    }
}

impl<E> Observed for Result<CommandResult, E> {
    fn observed_selecting(self, fields: &[&str]) -> Self {
        if let Ok(result) = &self {
            observe_selecting(&result.envelope, fields);
        }
        self
    }
}

impl Observed for Envelope {
    fn observed_selecting(self, fields: &[&str]) -> Self {
        observe_selecting(&self, fields);
        self
    }
}

/// Observes a host-level `(data, error)` result as the envelope its public command builds:
/// failure with data when an error accompanies the evidence, success otherwise.
pub trait ObservedAs: Sized {
    fn observed_as(self, command: &str) -> Self;
}

impl<T: Serialize> ObservedAs for (T, Option<CraftError>) {
    fn observed_as(self, command: &str) -> Self {
        let data = serde_json::to_value(&self.0).must("host data serializes");
        let envelope = match &self.1 {
            Some(error) => {
                Envelope::failure_with_data(command, "observed".into(), error.clone(), data)
            }
            None => Envelope::success(command, "observed".into(), data),
        };
        observe(&envelope);
        self
    }
}

/// Observes a refusal raised before any host runs, as the public command reports it: a
/// failure envelope with no data.
pub fn observe_failure(command: &str, error: &CraftError) {
    observe(&Envelope::failure(
        command,
        "observed".into(),
        error.clone(),
    ));
}
