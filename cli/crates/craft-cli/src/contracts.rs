//! Deterministic development export. This module parses examples without executing operations.

use std::collections::{BTreeMap, BTreeSet};

use clap::{Arg, Command, CommandFactory, FromArgMatches};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::canonical_digest;
use craft_core::registry::{OperationDescriptor, Registry};
use serde_json::{Value, json};

use crate::args::{Cli, is_invocation_control};
use crate::{catalog, dispatch};

fn invalid() -> CraftError {
    CraftError::new(
        ErrorCode::Internal,
        "Parser and operation contracts disagree.",
    )
}

fn flag(arg: &Arg) -> Value {
    json!({
        "long":arg.get_long(),
        "short":arg.get_short(),
        "action":format!("{:?}",arg.get_action()),
        "required":arg.is_required_set(),
        "arity":arg.get_num_args().map(|range|json!({"min":range.min_values(),"max":range.max_values()})),
        "defaults":arg.get_default_values().iter().map(|value|value.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "choices":arg.get_value_parser().possible_values().map(|values|values.map(|value|value.get_name().to_owned()).collect::<Vec<_>>()),
        "delimiter":arg.get_value_delimiter(),
        "long_aliases":arg.get_all_aliases(),
        "short_aliases":arg.get_all_short_aliases(),
    })
}

fn leaf_commands<'a>(
    command: &'a Command,
    path: Vec<String>,
    output: &mut Vec<(Vec<String>, &'a Command)>,
) {
    if command.get_subcommands().next().is_none() {
        output.push((path, command));
    } else {
        for child in command.get_subcommands() {
            let mut nested = path.clone();
            nested.push(child.get_name().to_owned());
            leaf_commands(child, nested, output);
        }
    }
}

fn parse_action(parser: &Command, arguments: &[String]) -> Result<crate::args::Action, CraftError> {
    let matches = parser
        .clone()
        .try_get_matches_from(arguments)
        .map_err(|_| invalid())?;
    Cli::from_arg_matches(&matches)
        .map_err(|_| invalid())?
        .action
        .ok_or_else(invalid)
}

pub(crate) fn example_arguments(
    path: &[String],
    command: &Command,
    example: &Value,
) -> Result<Vec<String>, CraftError> {
    let mut arguments = vec!["craft".to_owned()];
    arguments.extend_from_slice(path);
    for arg in command.get_arguments().filter(|arg| !arg.is_global_set()) {
        let id = arg.get_id().as_str();
        let Some(value) = example.get(id) else {
            continue;
        };
        let name = arg.get_long().ok_or_else(invalid)?;
        match value {
            Value::Null => {}
            Value::Bool(value) if matches!(arg.get_action(), clap::ArgAction::Set) => {
                arguments.push(format!("--{name}={value}"));
            }
            Value::Bool(false) => {}
            Value::Bool(true) => arguments.push(format!("--{name}")),
            Value::String(value) => arguments.push(format!("--{name}={value}")),
            Value::Number(value) => arguments.push(format!("--{name}={value}")),
            Value::Array(values) => {
                for value in values {
                    let value = match value {
                        Value::String(value) => value.clone(),
                        Value::Number(value) => value.to_string(),
                        Value::Array(_) | Value::Object(_) => value.to_string(),
                        _ => return Err(invalid()),
                    };
                    arguments.push(format!("--{name}={value}"));
                }
            }
            Value::Object(_) => arguments.push(format!("--{name}={value}")),
        }
    }
    Ok(arguments)
}

fn example_agrees(
    parser: &Command,
    path: &[String],
    command: &Command,
    descriptor: &OperationDescriptor,
    registry: &Registry,
    example: &Value,
) -> Result<(), CraftError> {
    let arguments = example_arguments(path, command, example)?;
    let actual = parse_action(parser, &arguments)?;
    if dispatch::command_id(&actual) != descriptor.id {
        return Err(invalid());
    }
    let actual = dispatch::argument_value(&actual)?;
    registry.validate(&descriptor.id, &actual)?;
    let prototype = parse_action(
        parser,
        &std::iter::once("craft".to_owned())
            .chain(path.iter().cloned())
            .collect::<Vec<_>>(),
    )?;
    registry.validate(&descriptor.id, example)?;
    let expected = dispatch::decode_action(prototype, example.clone())?;
    let mut expected = dispatch::argument_value(&expected)?;
    let mut actual = actual;
    // Structured-only values intentionally have no scalar-flag representation.
    for (name, schema) in descriptor.input_schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if schema["x-craft-input-channel"] == "json_only" {
            expected.as_object_mut().ok_or_else(invalid)?.remove(name);
            actual.as_object_mut().ok_or_else(invalid)?.remove(name);
        }
    }
    if actual != expected {
        return Err(invalid());
    }
    Ok(())
}

fn minimal_example(
    descriptor: &OperationDescriptor,
    registry: &Registry,
) -> Result<Value, CraftError> {
    registry.validate(&descriptor.id, &descriptor.example)?;
    let required: BTreeSet<_> = descriptor.input_schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut minimal = descriptor.example.clone();
    let optional: Vec<_> = minimal
        .as_object()
        .ok_or_else(invalid)?
        .keys()
        .filter(|name| !required.contains(name.as_str()))
        .cloned()
        .collect();
    for name in optional {
        let mut candidate = minimal.clone();
        candidate.as_object_mut().ok_or_else(invalid)?.remove(&name);
        if registry.validate(&descriptor.id, &candidate).is_ok() {
            minimal = candidate;
        }
    }
    Ok(minimal)
}

fn operation(
    parser: &Command,
    path: &[String],
    leaf: &Command,
    descriptor: &OperationDescriptor,
    registry: &Registry,
) -> Result<Value, CraftError> {
    let properties = descriptor.input_schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut flags = BTreeMap::new();
    for arg in leaf.get_arguments().filter(|arg| !arg.is_global_set()) {
        let id = arg.get_id().as_str();
        if matches!(id, "help" | "version") {
            continue;
        }
        if is_invocation_control(id) || arg.get_long().is_none() || !properties.contains_key(id) {
            return Err(invalid());
        }
        flags.insert(id.to_owned(), flag(arg));
    }
    for (name, schema) in &properties {
        match schema.get("x-craft-input-channel").and_then(Value::as_str) {
            Some("json_only") if !flags.contains_key(name) => {}
            Some("output_control") if is_invocation_control(name) => {}
            None if flags.contains_key(name) => {}
            _ => return Err(invalid()),
        }
    }
    example_agrees(
        parser,
        path,
        leaf,
        descriptor,
        registry,
        &descriptor.example,
    )?;
    let minimal = minimal_example(descriptor, registry)?;
    example_agrees(parser, path, leaf, descriptor, registry, &minimal)?;
    let mut field_digests = BTreeMap::new();
    let mut default_digest = None;
    if let Some(projection) = &descriptor.projection {
        for field in projection.selectable_fields(&descriptor.response_schema)? {
            let compiled =
                projection.compile(&descriptor.response_schema, std::slice::from_ref(&field))?;
            let schema = compiled.response_schema(&descriptor.response_schema)?;
            let mut example = descriptor.response_example.clone();
            example["data"] = compiled.data(&example["data"])?;
            validate_selected(descriptor, schema.clone(), example)?;
            field_digests.insert(field, canonical_digest(&schema)?);
        }
        default_digest = Some(canonical_digest(
            &projection
                .compile(&descriptor.response_schema, &[])?
                .response_schema(&descriptor.response_schema)?,
        )?);
    }
    let controls: BTreeMap<_, _> = leaf
        .get_arguments()
        .filter(|arg| arg.is_global_set())
        .map(|arg| {
            let id = arg.get_id().as_str();
            let applies = match id {
                "state_root" => crate::args::uses_state_context(&descriptor.id),
                "profile" => crate::args::accepts_profile_selection(&descriptor.id),
                "fields" => descriptor.projection.is_some() || descriptor.id == "schema",
                "input" | "max_bytes" | "timeout_ms" => true,
                _ => false,
            };
            (
                id.to_owned(),
                if applies { "supported" } else { "rejected" },
            )
        })
        .collect();
    Ok(
        json!({"path":path,"descriptor":descriptor,"flags":flags,"controls":controls,
        "selectable_fields":field_digests,"default_projection_digest":default_digest}),
    )
}

fn validate_selected(
    source: &OperationDescriptor,
    schema: Value,
    example: Value,
) -> Result<(), CraftError> {
    let mut registry = Registry::default();
    let mut descriptor = source.clone();
    descriptor.response_schema = schema;
    descriptor.response_example = example;
    descriptor.projection = None;
    registry.register(descriptor)
}

/// Generates non-secret contracts from the actual parser and compiled registry, without I/O.
pub fn snapshot() -> Result<Value, CraftError> {
    let registry = catalog::registry()?;
    let mut parser = Cli::command();
    parser.build();
    snapshot_from(&registry, &parser)
}

fn snapshot_from(registry: &Registry, parser: &Command) -> Result<Value, CraftError> {
    let mut leaves = Vec::new();
    leaf_commands(parser, Vec::new(), &mut leaves);
    let expected: BTreeSet<_> = registry
        .descriptors()
        .map(|descriptor| descriptor.id.as_str())
        .collect();
    let actual: BTreeSet<_> = leaves.iter().map(|(path, _)| path.join(".")).collect();
    if expected != actual.iter().map(String::as_str).collect() {
        return Err(invalid());
    }
    let mut controls = BTreeMap::new();
    for arg in parser.get_arguments() {
        let id = arg.get_id().as_str();
        if matches!(id, "help" | "version") {
            continue;
        }
        if !arg.is_global_set() || !is_invocation_control(id) {
            return Err(invalid());
        }
        controls.insert(id.to_owned(), flag(arg));
    }
    let mut operations = BTreeMap::new();
    for (path, leaf) in leaves {
        let id = path.join(".");
        let descriptor = registry.descriptor(&id)?;
        operations.insert(
            id.clone(),
            operation(parser, &path, leaf, descriptor, registry).map_err(|mut error| {
                error
                    .details
                    .issues
                    .push(craft_core::error::FieldIssue::new(
                        format!("/operations/{id}"),
                        "parser_contract_mismatch",
                    ));
                error
            })?,
        );
    }
    Ok(
        json!({"format_version":1,"contract_version":craft_core::contracts::SCHEMA_VERSION,
        "controls":controls,"providers":registry.providers().collect::<Vec<_>>(),"operations":operations}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_examples_preserve_an_alternative_required_secret_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut registry = Registry::default();
        crate::provision::register_candidate(&mut registry)?;
        let mut descriptor = registry.descriptor("auth.provision")?.clone();
        descriptor.example["interactive"] = json!(false);
        descriptor.example["replace"] = json!(false);
        let minimal = minimal_example(&descriptor, &registry)?;
        assert_eq!(
            minimal,
            json!({"profiles":["factory"],"expected_actor":"example-service-actor","secret_stdin":true})
        );
        registry.validate(&descriptor.id, &minimal)?;
        Ok(())
    }

    #[test]
    fn unrepresented_flags_and_unimplemented_shared_controls_fail_export()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = catalog::registry()?;
        for mut parser in [
            Cli::command().mut_subcommand("version", |command| {
                command.arg(Arg::new("unused").long("unused"))
            }),
            Cli::command().arg(
                Arg::new("unused_control")
                    .long("unused-control")
                    .global(true),
            ),
        ] {
            parser.build();
            assert!(snapshot_from(&registry, &parser).is_err());
        }
        Ok(())
    }

    #[test]
    fn parser_default_changes_cannot_silently_disagree_with_json_defaults()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = catalog::registry()?;
        let mut parser = Cli::command().mut_subcommand("command", |command| {
            command.mut_subcommand("find", |command| {
                command.mut_arg("limit", |argument| argument.default_value("7"))
            })
        });
        parser.build();
        let error = snapshot_from(&registry, &parser)
            .err()
            .ok_or("Expected default mismatch")?;
        assert!(
            error
                .details
                .issues
                .iter()
                .any(|issue| issue.path == "/operations/command.find")
        );
        Ok(())
    }

    #[test]
    fn typed_response_and_projection_changes_require_consistent_examples()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = catalog::registry()?;
        let mut response = source.descriptor("version")?.clone();
        response.response_schema["properties"]["data"]["properties"]["version"]["type"] =
            json!("integer");
        assert!(Registry::default().register(response).is_err());
        let mut projection = source.descriptor("version")?.clone();
        projection
            .projection
            .as_mut()
            .ok_or("Expected projection")?
            .default_selected
            .push("absent".into());
        assert!(Registry::default().register(projection).is_err());
        Ok(())
    }

    #[test]
    fn export_uses_source_flags_schemas_and_only_explicit_json_only_exceptions()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let operation = &snapshot["operations"]["profile.create"];
        assert_eq!(
            operation["descriptor"]["input_schema"]["properties"]["defaults"]["x-craft-input-channel"],
            "json_only"
        );
        assert!(operation["flags"].get("defaults").is_none());
        assert_eq!(operation["flags"]["auth_mode"]["long"], "auth-mode");
        assert_eq!(operation["controls"]["profile"], "rejected");
        assert_eq!(
            snapshot["operations"]["profile.show"]["controls"]["profile"],
            "supported"
        );
        for command in [
            "review.prepare",
            "review.verify",
            "plan.tickets-preview",
            "plan.tickets-create",
            "plan.tickets-resume",
            "plan.tickets-record-completion",
            "jira.link.create",
            "jira.link.delete",
            "jira.link.get",
            "jira.comment.get",
            "jira.comment.list",
            "jira.link.list",
            "jira.link.types",
            "jira.link.reconcile",
            "confluence.comment.list",
            "confluence.comment.replies",
        ] {
            assert_eq!(
                snapshot["operations"][command]["controls"]["profile"], "supported",
                "{command}"
            );
            assert_eq!(
                snapshot["operations"][command]["controls"]["state_root"], "supported",
                "{command}"
            );
        }
        assert!(
            snapshot["operations"]["version"]["selectable_fields"]
                .get("version")
                .is_some()
        );
        Ok(())
    }
}
