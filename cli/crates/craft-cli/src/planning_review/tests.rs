use super::*;
use clap::Parser;
type TestResult = Result<(), Box<dyn std::error::Error>>;
#[test]
fn structured_task_flags_preserve_exact_json_and_reject_duplicate_members() -> TestResult {
    let registry = crate::catalog::registry()?;
    let example = registry.descriptor(command::PREPARE)?.example.clone();
    let task = serde_json::to_string(&example["tasks"][0])?;
    let cli = crate::args::Cli::try_parse_from([
        "craft",
        "review",
        "prepare",
        "--review-id",
        "payment-plan",
        "--apply",
        "--page-id",
        "101",
        "--space-id",
        "100",
        "--task",
        &task,
    ])?;
    let action = cli.action.ok_or("action")?;
    let actual = crate::dispatch::argument_value(&action)?;
    let expected: command::PrepareArgs = serde_json::from_value(example)?;
    assert_eq!(actual["tasks"], serde_json::to_value(expected)?["tasks"]);
    assert_eq!(crate::dispatch::command_id(&action), command::PREPARE);
    assert!(parse_task(r#"{"task_id":"one","task_id":"two"}"#).is_err());
    assert!(parse_design_profiles(r#"{"site":"one","site":"two"}"#).is_err());
    assert!(parse_task(r#"{"task_id":"one","unknown":true}"#).is_err());
    Ok(())
}
#[test]
fn review_preparation_control_and_proposal_errors_are_local() -> TestResult {
    let registry = crate::catalog::registry()?;
    for mode in [
        "missing_apply",
        "implicit_profile",
        "small_budget",
        "bad_page",
        "duplicate_task",
        "bad_custom_field",
        "fields",
        "link_type_only",
        "link_direction_only",
    ] {
        let mut value = registry.descriptor(command::PREPARE)?.example.clone();
        if mode == "link_type_only" {
            value["dependency_link_type_id"] = json!("10000");
        }
        if mode == "link_direction_only" {
            value["dependency_link_direction"] = json!("inward");
        }
        if mode == "missing_apply" {
            value["apply"] = json!(false);
        }
        if mode == "bad_page" {
            value["page_id"] = json!("01");
        }
        if mode == "duplicate_task" {
            let duplicate = value["tasks"][0].clone();
            value["tasks"]
                .as_array_mut()
                .ok_or("tasks")?
                .push(duplicate);
        }
        if mode == "bad_custom_field" {
            value["tasks"][0]["input"]["custom_fields"] = json!({"unexpected":1});
        }
        let args: command::PrepareArgs = serde_json::from_value(value)?;
        let options = crate::profiles::InvocationOptions {
            state_root: None,
            profile: if mode == "implicit_profile" {
                None
            } else {
                Some("factory")
            },
            request_id: "review-test",
            max_bytes: if mode == "small_budget" {
                16_384
            } else {
                1_048_576
            },
        };
        assert!(
            args.validate(
                &if mode == "fields" {
                    vec!["statement".into()]
                } else {
                    vec![]
                },
                &options
            )
            .is_err(),
            "{mode}"
        );
    }
    Ok(())
}
