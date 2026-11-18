use std::io::Write;
use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};

fn invoke(arguments: &[&str], input: Option<&[u8]>) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(craft_test_support::process::run(
        Path::new(env!("CARGO_BIN_EXE_craft")),
        arguments,
        input,
        Default::default(),
    )?)
}

fn envelope(output: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert!(
        output.stderr.is_empty(),
        "Unexpected diagnostics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn reviewed_setup_and_cloud_commands_are_discoverable_without_login()
-> Result<(), Box<dyn std::error::Error>> {
    let commands = [
        "artifact.get",
        "artifact.export",
        "auth.provision",
        "auth.provision-static",
        "trello.board.list",
        "trello.board.get",
        "trello.list.list",
        "trello.card.list",
        "trello.card.board-list",
        "trello.card.get",
        "trello.card.search",
        "trello.comment.list",
        "trello.label.list",
        "trello.checklist.list",
        "trello.attachment.list",
        "trello.member.list",
        "policy.get",
        "policy.set",
        "confluence.space.get",
        "confluence.space.list",
        "confluence.page.get",
        "confluence.page.list",
        "confluence.page.search",
        "confluence.page.create",
        "confluence.page.update",
        "confluence.page.trash",
        "confluence.comment.create",
        "confluence.comment.update",
        "confluence.comment.delete",
        "confluence.content-state.get",
        "confluence.content-state.available",
        "confluence.content-state.set",
        "confluence.content-state.clear",
        "confluence.attachment.get",
        "confluence.attachment.list",
        "confluence.attachment.trash",
        "confluence.folder.get",
        "confluence.folder.create",
        "confluence.folder.trash",
        "confluence.whiteboard.get",
        "confluence.whiteboard.create",
        "confluence.whiteboard.trash",
        "confluence.page.width.set",
        "confluence.space.create",
        "confluence.attachment.upload",
        "confluence.attachment.replace",
        "confluence.attachment.download",
        "jira.issue.get",
        "jira.issue.search",
        "jira.issue.create",
        "jira.project.list",
        "jira.project.get",
        "jira.project.statuses",
        "jira.field.list",
        "jira.field.search",
        "jira.metadata.edit",
        "jira.transition.list",
        "jira.permission.check",
        "jira.comment.get",
        "jira.comment.list",
        "jira.attachment.get",
        "jira.attachment.list",
        "jira.attachment.settings",
        "jira.attachment.download",
        "jira.attachment.upload",
        "jira.attachment.delete",
        "jira.comment.create",
        "jira.comment.update",
        "jira.comment.delete",
        "jira.issue.update",
        "jira.issue.assign",
        "jira.issue.unassign",
        "jira.issue.delete",
        "jira.transition.apply",
        "operation.reconcile",
        "operation.list",
        "operation.preview",
        "plan.page-create",
        "design.page-create",
        "plan.page-get",
        "design.page-get",
        "confluence.comment.get",
        "confluence.comment.list",
        "confluence.comment.replies",
        "review.policy.get",
        "review.policy.set",
        "review.prepare",
        "review.verify",
        "plan.tickets-preview",
        "plan.tickets-create",
        "plan.tickets-resume",
        "plan.tickets-record-completion",
        "jira.link.create",
        "jira.link.delete",
        "jira.link.get",
        "jira.link.list",
        "jira.link.types",
        "jira.link.reconcile",
        "operation.wait",
        "scan.start",
        "scan.resume",
        "scan.export",
        "whiteboard.encode",
        "whiteboard.decode",
        "whiteboard.layout",
        "whiteboard.check",
        "whiteboard.preview",
    ];
    for command in commands {
        let body = envelope(&invoke(&["schema", "--command", command], None)?)?;
        assert_eq!(body["ok"], true, "{command}: {body}");
    }
    let body = envelope(&invoke(&["help", "--json"], None)?)?;
    let listed = body["data"]["commands"].as_array().ok_or("commands")?;
    assert_eq!(listed.len(), 20 + commands.len());
    for command in commands {
        assert!(listed.iter().any(|c| c["id"] == command), "{command}");
    }
    let providers = envelope(&invoke(&["provider", "list"], None)?)?;
    assert_eq!(
        providers["data"]["items"]
            .as_array()
            .ok_or("providers")?
            .len(),
        3
    );
    let capability = envelope(&invoke(
        &["capabilities", "--command", "confluence.page.create"],
        None,
    )?)?;
    let item = &capability["data"]["items"][0];
    assert_eq!(item["authentication"], "service_oauth");
    for field in [
        "policy_enabled",
        "credential_scoped",
        "tenant_supported",
        "verified",
    ] {
        assert_eq!(item["availability"][field], "unknown", "{field}");
    }
    let update = [
        "schema",
        "--command",
        "confluence.page.update",
        "--view",
        "full",
    ];
    let input = envelope(&invoke(&update, None)?)?["data"]["schema"].to_string();
    assert!(
        input.contains("native_reconciliation_may_overwrite"),
        "{input}"
    );
    let response = [&update[..], &["--response", "--max-bytes", "65536"]].concat();
    let output = envelope(&invoke(&response, None)?)?["data"]["schema"].to_string();
    assert!(
        output.contains("editor_draft_preservation_verified"),
        "{output}"
    );
    let body = envelope(&invoke(
        &["schema", "--command", "jira.project.delete"],
        None,
    )?)?;
    assert_eq!(body["error"]["code"], "COMMAND_UNKNOWN");
    Ok(())
}

#[test]
fn public_cloud_and_setup_inputs_fail_before_state_or_secret_access()
-> Result<(), Box<dyn std::error::Error>> {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    let root = craft_test_support::state::fixture_root(
        &parent,
        "public-input-refusal",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let root_text = root.to_str().ok_or("root")?;
    for arguments in [
        vec!["confluence", "page", "create"],
        vec!["confluence", "comment", "get", "--id", "201"],
        vec![
            "confluence",
            "comment",
            "get",
            "--id",
            "201",
            "--page-id",
            "bad-id",
        ],
        vec!["jira", "issue", "create"],
        vec!["jira", "project", "list", "--cursor", "invalid"],
        vec!["jira", "project", "get", "--cloud-id", "invalid"],
        vec!["jira", "project", "statuses", "--cursor", "invalid"],
        vec!["jira", "field", "list", "--cursor", "invalid"],
        vec!["jira", "field", "search", "--cursor", "invalid"],
        vec!["jira", "metadata", "edit"],
        vec!["jira", "transition", "list"],
        vec!["jira", "permission", "check"],
        vec!["jira", "issue", "update"],
        vec!["jira", "issue", "assign"],
        vec!["jira", "issue", "unassign"],
        vec!["jira", "issue", "delete"],
        vec!["jira", "transition", "apply"],
        vec!["operation", "reconcile"],
        vec!["policy", "set"],
        vec!["review", "prepare"],
    ] {
        let mut args = vec!["--state-root", root_text, "--profile", "factory"];
        args.extend(arguments);
        let output = invoke(&args, None)?;
        let body = envelope(&output)?;
        assert!(!output.status.success());
        assert_eq!(body["error"]["code"], "INPUT_INVALID");
        assert!(!root.exists());
    }
    let output = invoke(
        &[
            "--state-root",
            root_text,
            "auth",
            "provision",
            "--input",
            "-",
        ],
        Some(br#"{"profiles":["factory"],"expected_actor":"actor","secret_stdin":true}"#),
    )?;
    let body = envelope(&output)?;
    assert_eq!(body["error"]["code"], "INPUT_INVALID");
    assert!(!root.exists());
    let output=invoke(&["input","validate","--command","auth.provision","--file","-"],Some(br#"{"profiles":["factory"],"expected_actor":"actor","secret_stdin":true,"client_secret":"synthetic-secret"}"#))?;
    let body = envelope(&output)?;
    assert_eq!(body["ok"], false);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret"));
    Ok(())
}

#[test]
fn issue_action_and_recovery_inputs_refuse_before_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "public-issue-actions",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let registry = craft_cli::catalog::registry()?;
    for (command, path) in [
        ("jira.issue.update", vec!["jira", "issue", "update"]),
        ("jira.issue.assign", vec!["jira", "issue", "assign"]),
        ("jira.issue.unassign", vec!["jira", "issue", "unassign"]),
        ("jira.issue.delete", vec!["jira", "issue", "delete"]),
        ("jira.transition.apply", vec!["jira", "transition", "apply"]),
        ("operation.reconcile", vec!["operation", "reconcile"]),
    ] {
        let descriptor = registry.descriptor(command)?;
        let is_recovery = command == "operation.reconcile";
        for mode in ["unknown", "missing", "null", "fields", "budget", "intent"] {
            let mut input = descriptor.example.clone();
            let required = if is_recovery { "original" } else { "expected" };
            let expected = match mode {
                "unknown" => {
                    input["unexpected"] = json!("private-input-sentinel");
                    "INPUT_INVALID"
                }
                "missing" => {
                    input.as_object_mut().ok_or("input")?.remove(required);
                    "INPUT_INVALID"
                }
                "null" => {
                    input[required] = json!(null);
                    "INPUT_INVALID"
                }
                "fields" => "FIELD_UNKNOWN",
                "budget" => "OUTPUT_BUDGET_TOO_SMALL",
                _ if is_recovery => {
                    input["candidate_issue_ids"] = json!(["1", "1"]);
                    "INPUT_INVALID"
                }
                _ => {
                    input["apply"] = json!(false);
                    "APPLY_REQUIRED"
                }
            };
            let mut arguments = vec![
                "--state-root",
                root.to_str().ok_or("root")?,
                "--profile",
                "factory",
                "--max-bytes",
                if mode == "budget" { "512" } else { "16777216" },
            ];
            if mode == "fields" {
                arguments.extend(["--fields", "not_a_response_field"]);
            }
            arguments.extend(path.iter().copied());
            arguments.extend(["--input", "-"]);
            let output = invoke(&arguments, Some(&serde_json::to_vec(&input)?))?;
            let body = envelope(&output)?;
            assert!(!output.status.success(), "{command}/{mode}");
            assert_eq!(body["error"]["code"], expected, "{command}/{mode}: {body}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
            assert!(!root.exists(), "{command}/{mode}");
        }
        for field in ["expected_assignee", "subtasks"] {
            if !descriptor
                .example
                .as_object()
                .ok_or("input")?
                .contains_key(field)
            {
                continue;
            }
            let mut input = descriptor.example.clone();
            input.as_object_mut().ok_or("input")?.remove(field);
            let mut arguments = vec![
                "--state-root",
                root.to_str().ok_or("root")?,
                "--profile",
                "factory",
            ];
            arguments.extend(path.iter().copied());
            arguments.extend(["--input", "-"]);
            let body = envelope(&invoke(&arguments, Some(&serde_json::to_vec(&input)?))?)?;
            assert_eq!(
                body["error"]["code"], "INPUT_INVALID",
                "{command}/{field}: {body}"
            );
            assert!(!root.exists());
        }
    }
    Ok(())
}

#[test]
fn issue_action_discovery_preserves_effect_and_unverified_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let contracts = craft_cli::contracts::snapshot()?;
    for (command, effect) in [
        ("jira.issue.update", "write"),
        ("jira.issue.assign", "write"),
        ("jira.issue.unassign", "write"),
        ("jira.issue.delete", "permanent"),
        ("jira.transition.apply", "write"),
        ("operation.reconcile", "read"),
    ] {
        for control in ["profile", "state_root"] {
            assert_eq!(
                contracts["operations"][command]["controls"][control], "supported",
                "{command}/{control}"
            );
        }
        let body = envelope(&invoke(
            &[
                "capabilities",
                "--command",
                command,
                "--fields",
                "command,effect,authentication,availability",
            ],
            None,
        )?)?;
        let item = &body["data"]["items"][0];
        assert_eq!(item["effect"], effect, "{command}");
        assert_eq!(item["authentication"], "service_oauth", "{command}");
        for field in [
            "policy_enabled",
            "credential_scoped",
            "tenant_supported",
            "verified",
        ] {
            assert_eq!(item["availability"][field], "unknown", "{command}/{field}");
        }
    }
    Ok(())
}

#[test]
fn saved_recovery_selector_is_strict_and_validated_without_loading_state()
-> Result<(), Box<dyn std::error::Error>> {
    let original = craft_cli::catalog::registry()?
        .descriptor("operation.reconcile")?
        .example["original"]
        .clone();
    for (input, valid) in [
        (
            json!({"operation_id":"existing-operation","saved":true}),
            true,
        ),
        (json!({"operation_id":"existing-operation"}), false),
        (
            json!({"operation_id":"existing-operation","saved":false}),
            false,
        ),
        (
            json!({"operation_id":"existing-operation","saved":true,"original":original}),
            false,
        ),
        (
            json!({"operation_id":"existing-operation","saved":true,"original":null}),
            false,
        ),
        (
            json!({"operation_id":"existing-operation","saved":"true"}),
            false,
        ),
    ] {
        let body = envelope(&invoke(
            &[
                "input",
                "validate",
                "--command",
                "operation.reconcile",
                "--file",
                "-",
            ],
            Some(&serde_json::to_vec(&input)?),
        )?)?;
        assert_eq!(body["ok"], valid, "{input}: {body}");
    }
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "saved-recovery-controls",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    for extra in [
        vec!["--candidate-issue-ids", "bad-id"],
        vec!["--fields", "absent"],
        vec!["--max-bytes", "512"],
    ] {
        let mut args = vec![
            "--state-root",
            root.to_str().ok_or("root")?,
            "--profile",
            "factory",
            "operation",
            "reconcile",
            "--operation-id",
            "existing-operation",
            "--saved",
        ];
        args.extend(extra);
        let body = envelope(&invoke(&args, None)?)?;
        assert_eq!(body["ok"], false);
        assert_ne!(body["error"]["code"], "COMMAND_UNKNOWN");
        assert!(!root.exists());
    }
    Ok(())
}

#[test]
fn review_preparation_controls_refuse_before_creating_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    let root = craft_test_support::state::fixture_root(
        &parent,
        "public-review-preparation",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let root_text = root.to_str().ok_or("root")?;
    for (mode, expected) in [
        ("budget", "OUTPUT_BUDGET_TOO_SMALL"),
        ("apply", "APPLY_REQUIRED"),
        ("profile", "INPUT_INVALID"),
        ("duplicate", "INPUT_INVALID"),
        ("custom", "INPUT_INVALID"),
        ("unknown", "INPUT_INVALID"),
    ] {
        let mut input = json!({"review_id":"contract-review","apply":mode!="apply","page_id":"101","space_id":"100","tasks":[{"task_id":"define-contract","profile":"jira-test","project_id":"10000","input":{"issue_type_id":"10001","summary":"Define contract"}}]});
        if mode == "duplicate" {
            let task = input["tasks"][0].clone();
            input["tasks"].as_array_mut().ok_or("tasks")?.push(task);
        }
        if mode == "custom" {
            input["tasks"][0]["input"]["custom_fields"] = json!({"unexpected":1});
        }
        if mode == "unknown" {
            input["unknown"] = json!(true);
        }
        let mut args = vec!["--state-root", root_text];
        if mode != "profile" {
            args.extend(["--profile", "factory"]);
        }
        if mode != "budget" {
            args.extend(["--max-bytes", "1048576"]);
        }
        args.extend(["review", "prepare", "--input", "-"]);
        let bytes = serde_json::to_vec(&input)?;
        let output = invoke(&args, Some(&bytes))?;
        let body = envelope(&output)?;
        assert!(!output.status.success(), "{mode}");
        assert_eq!(body["error"]["code"], expected, "{mode}: {body}");
        assert!(!root.exists(), "{mode}");
    }
    Ok(())
}

#[test]
fn review_verification_controls_refuse_before_creating_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    review_public_controls(["review", "verify"])
}
#[test]
fn ticket_preview_controls_refuse_before_creating_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    review_public_controls(["plan", "tickets-preview"])
}
fn review_public_controls(command: [&str; 2]) -> Result<(), Box<dyn std::error::Error>> {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    let root = craft_test_support::state::fixture_root(
        &parent,
        "public-review-verification",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let root_text = root.to_str().ok_or("root")?;
    for (mode, expected) in [
        ("budget", "OUTPUT_BUDGET_TOO_SMALL"),
        ("apply", "APPLY_REQUIRED"),
        ("profile", "INPUT_INVALID"),
        ("identity", "INPUT_INVALID"),
        ("fields", "INPUT_INVALID"),
        ("unknown", "INPUT_INVALID"),
    ] {
        let mut input = json!({"review_id":"contract-review","apply":mode!="apply"});
        if mode == "identity" {
            input["review_id"] = json!("../other");
        }
        if mode == "unknown" {
            input["unknown"] = json!(true);
        }
        let mut args = vec!["--state-root", root_text];
        if mode != "profile" {
            args.extend(["--profile", "factory"]);
        }
        if mode != "budget" {
            args.extend(["--max-bytes", "1048576"]);
        }
        if mode == "fields" {
            args.extend(["--fields", "review_verified"]);
        }
        args.extend(command);
        args.extend(["--input", "-"]);
        let bytes = serde_json::to_vec(&input)?;
        let output = invoke(&args, Some(&bytes))?;
        let body = envelope(&output)?;
        assert!(!output.status.success(), "{mode}");
        assert_eq!(body["error"]["code"], expected, "{mode}: {body}");
        assert!(!root.exists(), "{mode}");
    }
    Ok(())
}

#[test]
fn directed_link_controls_and_input_refuse_before_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "public-directed-link",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let root_text = root.to_str().ok_or("root")?;
    let example = craft_cli::catalog::registry()?
        .descriptor("jira.link.create")?
        .example
        .clone();
    for (mode, expected) in [
        ("budget", "OUTPUT_BUDGET_TOO_SMALL"),
        ("apply", "APPLY_REQUIRED"),
        ("profile", "INPUT_INVALID"),
        ("identity", "INPUT_INVALID"),
        ("self", "INPUT_INVALID"),
        ("fields", "INPUT_INVALID"),
        ("comment", "INPUT_INVALID"),
        ("null_type", "INPUT_INVALID"),
    ] {
        let mut input = example.clone();
        match mode {
            "apply" => input["apply"] = json!(false),
            "identity" => input["inward_issue_id"] = json!("PAY-1"),
            "self" => input["inward_issue_id"] = input["outward_issue_id"].clone(),
            "comment" => input["comment"] = json!("private-input-sentinel"),
            "null_type" => input["link_type"] = json!(null),
            _ => {}
        }
        let mut args = vec!["--state-root", root_text];
        if mode != "profile" {
            args.extend(["--profile", "factory"]);
        }
        if mode != "budget" {
            args.extend(["--max-bytes", "8388608"]);
        }
        if mode == "fields" {
            args.extend(["--fields", "receipt"]);
        }
        args.extend(["jira", "link", "create", "--input", "-"]);
        let output = invoke(&args, Some(&serde_json::to_vec(&input)?))?;
        let body = envelope(&output)?;
        assert!(!output.status.success(), "{mode}");
        assert_eq!(body["error"]["code"], expected, "{mode}: {body}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
        assert!(!root.exists(), "{mode}");
    }
    Ok(())
}

#[test]
fn link_lifecycle_unknown_input_is_rejected_before_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "public-link-lifecycle",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let registry = craft_cli::catalog::registry()?;
    for verb in ["get", "list", "types", "delete", "reconcile"] {
        let command = format!("jira.link.{verb}");
        let mut input = registry.descriptor(&command)?.example.clone();
        input["unexpected"] = json!("private-input-sentinel");
        let output = invoke(
            &[
                "--state-root",
                root.to_str().ok_or("root")?,
                "--profile",
                "factory",
                "--max-bytes",
                "8388608",
                "jira",
                "link",
                verb,
                "--input",
                "-",
            ],
            Some(&serde_json::to_vec(&input)?),
        )?;
        let body = envelope(&output)?;
        assert!(!output.status.success(), "{verb}");
        assert_eq!(body["command"], command);
        assert_eq!(body["error"]["code"], "INPUT_INVALID", "{verb}: {body}");
        assert!(!root.exists(), "{verb}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
    }
    Ok(())
}

#[test]
fn ticket_application_refuses_invalid_controls_before_opening_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "public-ticket-application",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let registry = craft_cli::catalog::registry()?;
    for verb in [
        "tickets-create",
        "tickets-resume",
        "tickets-record-completion",
    ] {
        for (mode, expected) in [
            ("apply", "APPLY_REQUIRED"),
            ("profile", "INPUT_INVALID"),
            ("consistency", "INPUT_INVALID"),
            ("budget", "OUTPUT_BUDGET_TOO_SMALL"),
            ("fields", "INPUT_INVALID"),
            ("path", "INPUT_INVALID"),
            ("unknown", "INPUT_INVALID"),
        ] {
            let mut input = registry
                .descriptor(&format!("plan.{verb}"))?
                .example
                .clone();
            match mode {
                "apply" => input["apply"] = json!(false),
                "consistency" => input["consistency"] = json!(null),
                "path" => input["preview_artifact"] = json!("../preview.json"),
                "unknown" => input["comment"] = json!("private-input-sentinel"),
                _ => {}
            }
            let mut arguments = vec!["--state-root", root.to_str().ok_or("root")?];
            if mode != "profile" {
                arguments.extend(["--profile", "factory"]);
            }
            if mode != "budget" {
                arguments.extend(["--max-bytes", "16777216"]);
            }
            if mode == "fields" {
                arguments.extend(["--fields", "complete"]);
            }
            arguments.extend(["plan", verb, "--input", "-"]);
            let output = invoke(&arguments, Some(&serde_json::to_vec(&input)?))?;
            let body = envelope(&output)?;
            assert!(!output.status.success(), "{verb}: {mode}");
            assert_eq!(body["error"]["code"], expected, "{verb}: {mode}: {body}");
            assert!(!root.exists(), "{verb}: {mode}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
        }
        let mut arguments = vec![
            "--state-root",
            root.to_str().ok_or("root")?,
            "--profile",
            "factory",
            "--max-bytes",
            "16777216",
            "plan",
            verb,
            "--run-id",
            "delivery",
            "--preview-artifact",
            "tickets-preview.json",
            "--result-title",
            "Results",
            "--consistency",
            "revalidate_each_batch",
        ];
        if verb == "tickets-record-completion" {
            arguments.extend([
                "--task-id",
                "implement",
                "--completion-file",
                "retained-completion.json",
            ]);
        }
        let output = invoke(&arguments, None)?;
        assert_eq!(envelope(&output)?["error"]["code"], "APPLY_REQUIRED");
        assert!(!root.exists());
    }
    Ok(())
}

#[test]
fn local_commands_return_typed_single_envelopes() -> Result<(), Box<dyn std::error::Error>> {
    for (arguments, command, input) in [
        (vec!["version"], "version", None),
        (
            vec!["command", "find", "--query", "schema"],
            "command.find",
            None,
        ),
        (vec!["provider", "list"], "provider.list", None),
        (
            vec!["capabilities", "--command", "version"],
            "capabilities",
            None,
        ),
        (vec!["help", "--json"], "help", None),
        (vec!["schema", "--command", "version"], "schema", None),
        (
            vec!["input", "validate", "--command", "version", "--file", "-"],
            "input.validate",
            Some(b"{}".as_slice()),
        ),
    ] {
        let output = invoke(&arguments, input)?;
        let result = envelope(&output)?;
        assert!(output.status.success(), "{result}");
        assert_eq!(result["ok"], true);
        assert_eq!(result["command"], command);
        assert_eq!(result["schema_version"], "1.0");
        let selected_schema = envelope(&invoke(
            &[
                "schema",
                "--command",
                command,
                "--response",
                "--view",
                "full",
                "--max-bytes",
                "65536",
            ],
            None,
        )?)?;
        assert_eq!(selected_schema["ok"], true, "{selected_schema}");
        assert_eq!(selected_schema["data"]["contract_version"], "1.0");
        assert_eq!(
            selected_schema["data"]["provider_version"],
            craft_core::VERSION
        );
        let validator = jsonschema::validator_for(&selected_schema["data"]["schema"])?;
        assert!(validator.is_valid(&result), "{command}: {result}");
        let mut invalid = arguments;
        invalid.extend(["--input", "-"]);
        let failure = envelope(&invoke(&invalid, Some(br#"{"unexpected":true}"#))?)?;
        assert_eq!(failure["ok"], false);
        assert!(validator.is_valid(&failure), "{command}: {failure}");
    }
    Ok(())
}

#[test]
fn reports_invalid_calls_without_echoing_input_secrets() -> Result<(), Box<dyn std::error::Error>> {
    for (arguments, input) in [
        (vec!["absent-command", "private-sentinel"], None),
        (
            vec!["input", "validate", "--command", "version", "--file", "-"],
            Some(br#"{"secret":"private-sentinel"}"#.as_slice()),
        ),
        (
            vec!["input", "validate", "--command", "version", "--file", "-"],
            Some(br#"{"id":1,"id":2}"#.as_slice()),
        ),
    ] {
        let output = invoke(&arguments, input)?;
        let result = envelope(&output)?;
        assert_eq!(output.status.code(), Some(10));
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["details"]["write_state"], "not_sent");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn generated_response_schema_accepts_actual_command_output()
-> Result<(), Box<dyn std::error::Error>> {
    let output = invoke(
        &[
            "schema",
            "--command",
            "version",
            "--response",
            "--view",
            "full",
        ],
        None,
    )?;
    let result = envelope(&output)?;
    assert!(output.status.success(), "{result}");
    let validator = jsonschema::validator_for(&result["data"]["schema"])?;
    let actual = envelope(&invoke(&["version"], None)?)?;
    assert!(validator.is_valid(&actual));
    let compact = envelope(&invoke(&["schema", "--command", "version"], None)?)?;
    assert_eq!(compact["data"]["complete"], false);
    let fragment = envelope(&invoke(
        &[
            "schema",
            "--command",
            "command.find",
            "--path",
            "/properties/query",
        ],
        None,
    )?)?;
    assert_eq!(fragment["data"]["complete"], false);
    assert_eq!(fragment["data"]["view"], "fragment");
    assert_eq!(fragment["data"]["schema"]["type"], "string");
    let example = envelope(&invoke(
        &[
            "schema",
            "--command",
            "version",
            "--response",
            "--example",
            "minimal",
        ],
        None,
    )?)?;
    assert!(validator.is_valid(&example["data"]["schema"]));
    Ok(())
}

#[test]
fn output_limit_keeps_errors_complete_and_bounded() -> Result<(), Box<dyn std::error::Error>> {
    let output = invoke(
        &[
            "schema",
            "--command",
            "version",
            "--response",
            "--view",
            "full",
            "--max-bytes",
            "512",
        ],
        None,
    )?;
    let result = envelope(&output)?;
    assert_eq!(output.status.code(), Some(10));
    assert!(output.stdout.len() <= 512);
    assert_eq!(result["error"]["code"], "OUTPUT_BUDGET_TOO_SMALL");
    Ok(())
}

#[test]
fn validates_complete_input_without_claiming_live_checks() -> Result<(), Box<dyn std::error::Error>>
{
    let output = invoke(
        &["input", "validate", "--command", "version", "--file", "-"],
        Some(b"{}"),
    )?;
    let result = envelope(&output)?;
    assert!(output.status.success(), "{result}");
    assert_eq!(result["data"]["valid"], true);
    assert_eq!(result["data"]["live_conditions_checked"], false);
    assert_eq!(
        result["data"]["digest"],
        craft_core::input::canonical_digest(&json!({}))?
    );
    Ok(())
}

#[test]
fn selects_nested_fields_and_matches_the_selected_response_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let actual = envelope(&invoke(
        &[
            "capabilities",
            "--command",
            "version",
            "--fields",
            "availability.implemented",
        ],
        None,
    )?)?;
    assert_eq!(actual["ok"], true);
    assert_eq!(
        actual["data"]["items"][0],
        json!({"command":"version","availability":{"implemented":true}})
    );
    let schema = envelope(&invoke(
        &[
            "schema",
            "--command",
            "capabilities",
            "--response",
            "--fields",
            "availability.implemented",
            "--view",
            "full",
        ],
        None,
    )?)?;
    assert_eq!(schema["ok"], true, "{schema}");
    let validator = jsonschema::validator_for(&schema["data"]["schema"])?;
    assert!(validator.is_valid(&actual));
    assert!(
        schema["data"]["selectable_fields"]
            .as_array()
            .ok_or("Expected fields")?
            .contains(&json!("availability.verified"))
    );
    assert_eq!(schema["data"]["projection"]["mode"], "local");
    let search = envelope(&invoke(
        &[
            "command",
            "find",
            "--query",
            "schema",
            "--fields",
            "description",
        ],
        None,
    )?)?;
    let record = &search["data"]["matches"][0];
    assert!(record.get("id").is_some());
    assert!(record.get("description").is_some());
    assert!(record.get("effect").is_none());
    Ok(())
}

#[test]
fn rejects_bad_projection_and_input_schema_field_selection()
-> Result<(), Box<dyn std::error::Error>> {
    for arguments in [
        vec!["version", "--fields", "private-sentinel"],
        vec!["version", "--fields", "version,version"],
        vec!["schema", "--command", "version", "--fields", "version"],
        vec!["version", "--fields", "*"],
        vec!["schema", "--command", "version", "--path", "not-a-pointer"],
        vec![
            "schema",
            "--command",
            "version",
            "--path",
            "/properties/missing",
        ],
        vec!["schema", "--command", "version", "--path", "/properties/~3"],
        vec![
            "schema",
            "--command",
            "version",
            "--path",
            "/properties/product",
            "--example",
            "minimal",
        ],
    ] {
        let output = invoke(&arguments, None)?;
        assert_eq!(output.status.code(), Some(10));
        let result = envelope(&output)?;
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["details"]["write_state"], "not_sent");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn structured_stdin_uses_the_same_typed_command_contract_as_flags()
-> Result<(), Box<dyn std::error::Error>> {
    let flags = envelope(&invoke(
        &["command", "find", "--query", "schema", "--limit", "1"],
        None,
    )?)?;
    let input = envelope(&invoke(
        &["command", "find", "--input", "-"],
        Some(br#"{"query":"schema","limit":1}"#),
    )?)?;
    assert_eq!(input["ok"], true, "{input}");
    assert_eq!(flags["data"], input["data"]);
    let schema = envelope(&invoke(
        &["schema", "--input", "-"],
        Some(br#"{"command":"version","response":true,"fields":["version"],"view":"full"}"#),
    )?)?;
    assert_eq!(schema["ok"], true, "{schema}");
    let actual = envelope(&invoke(
        &["version", "--input", "-", "--fields", "version"],
        Some(b"{}"),
    )?)?;
    assert_eq!(actual["ok"], true);
    assert!(jsonschema::validator_for(&schema["data"]["schema"])?.is_valid(&actual));
    let help = envelope(&invoke(
        &["help", "--input", "-"],
        Some(br#"{"json":true}"#),
    )?)?;
    assert_eq!(help["ok"], true);
    Ok(())
}

#[test]
fn rejects_input_source_conflicts_and_malformed_structured_arguments()
-> Result<(), Box<dyn std::error::Error>> {
    for (arguments, input) in [
        (
            vec!["command", "find", "--input", "-", "--query", ""],
            br#"{"query":"private-sentinel"}"#.as_slice(),
        ),
        (
            vec!["command", "find", "--input", "-"],
            br#"{"limit":"private-sentinel"}"#.as_slice(),
        ),
        (
            vec!["command", "find", "--input", "-"],
            br#"{"query":"private-sentinel","query":"other"}"#.as_slice(),
        ),
        (
            vec!["schema", "--input", "-", "--fields", "version"],
            br#"{"command":"version","response":true,"fields":["product"]}"#.as_slice(),
        ),
        (
            vec!["input", "validate", "--input", "-"],
            br#"{"command":"version","file":"-"}"#.as_slice(),
        ),
        (vec!["schema"], b"".as_slice()),
        (vec!["--input", "-"], b"{}".as_slice()),
    ] {
        let output = invoke(&arguments, Some(input))?;
        let result = envelope(&output)?;
        assert_eq!(output.status.code(), Some(10), "{result}");
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["details"]["write_state"], "not_sent");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn bounds_structured_stdin_and_reports_nested_parse_paths() -> Result<(), Box<dyn std::error::Error>>
{
    for bytes in [
        vec![b' '; craft_core::input::MAX_INPUT_BYTES + 1],
        format!("{{\"unknown\":{}0{}}}", "[".repeat(66), "]".repeat(66)).into_bytes(),
        b"{} {}".to_vec(),
        vec![0xff],
    ] {
        let output = invoke(&["version", "--input", "-"], Some(&bytes))?;
        assert_eq!(output.status.code(), Some(10));
        let result = envelope(&output)?;
        assert_eq!(result["error"]["details"]["write_state"], "not_sent");
    }
    let duplicate = envelope(&invoke(
        &["version", "--input", "-"],
        Some(br#"{"nested":[{"id":1,"id":2}]}"#),
    )?)?;
    assert_eq!(
        duplicate["error"]["details"]["issues"][0]["path"],
        "/nested/0/id"
    );
    Ok(())
}

struct InputFile(std::path::PathBuf);

#[test]
fn waiting_for_stdin_still_returns_one_deadline_envelope() -> Result<(), Box<dyn std::error::Error>>
{
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(env!("CARGO_BIN_EXE_craft"))
        .args(["version", "--input", "-", "--timeout-ms", "500"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let held_input = child.stdin.take().ok_or("Missing input pipe")?;
    let started = Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() > Duration::from_secs(5) {
            child.kill()?;
            let _ = child.wait();
            return Err("Input deadline did not terminate the child".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(held_input);
    let output = child.wait_with_output()?;
    let result = envelope(&output)?;
    assert_eq!(output.status.code(), Some(60));
    assert_eq!(result["error"]["code"], "DEADLINE_EXCEEDED");
    assert_eq!(result["error"]["details"]["write_state"], "not_sent");
    Ok(())
}

#[test]
fn a_closed_output_pipe_does_not_emit_a_panic_or_stderr_trace()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_craft"))
        .args([
            "schema",
            "--command",
            "schema",
            "--response",
            "--view",
            "full",
            "--max-bytes",
            "131072",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stdout.take());
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn input_validation_and_execution_share_strict_numeric_decoding()
-> Result<(), Box<dyn std::error::Error>> {
    for arguments in [
        vec![
            "input",
            "validate",
            "--command",
            "command.find",
            "--file",
            "-",
        ],
        vec!["command", "find", "--input", "-"],
    ] {
        let result = envelope(&invoke(&arguments, Some(br#"{"limit":1.0}"#))?)?;
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["details"]["issues"][0]["path"], "/limit");
        assert_eq!(
            result["error"]["details"]["issues"][0]["rule"],
            "typed_input_mismatch"
        );
    }
    Ok(())
}

impl InputFile {
    fn create(root: &Path, bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = root.join(format!(".craft-input-{}-{stamp}.json", std::process::id()));
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let guard = Self(path);
        output.write_all(bytes)?;
        Ok(guard)
    }
    fn argument(&self) -> Result<&str, Box<dyn std::error::Error>> {
        self.0
            .to_str()
            .ok_or_else(|| "Expected UTF-8 fixture path".into())
    }
}

impl Drop for InputFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn file_input_preserves_types_limits_roots_and_stdin_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::current_dir()?;
    let file = InputFile::create(&root, br#"{"command":"version","file":"-"}"#)?;
    let validated = envelope(&invoke(
        &["input", "validate", "--input", file.argument()?],
        Some(b"{}"),
    )?)?;
    assert_eq!(validated["ok"], true, "{validated}");
    assert_eq!(validated["data"]["live_conditions_checked"], false);
    let relative = file
        .0
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("Missing relative input")?;
    let validated = envelope(&invoke(
        &["input", "validate", "--input", relative],
        Some(b"{}"),
    )?)?;
    assert_eq!(validated["ok"], true, "{validated}");
    #[cfg(windows)]
    {
        let plain = file
            .argument()?
            .strip_prefix(r"\\?\")
            .unwrap_or(file.argument()?);
        for spelling in [format!(r"\\?\{plain}"), plain.to_ascii_lowercase()] {
            let validated = envelope(&invoke(
                &["input", "validate", "--input", &spelling],
                Some(b"{}"),
            )?)?;
            assert_eq!(validated["ok"], true, "{validated}");
        }
    }
    let oversized = InputFile::create(&root, &vec![b' '; craft_core::input::MAX_INPUT_BYTES + 1])?;
    let refused = envelope(&invoke(
        &["version", "--input", oversized.argument()?],
        None,
    )?)?;
    assert_eq!(
        refused["error"]["details"]["issues"][0]["rule"],
        "input_byte_limit_exceeded"
    );
    let outside = InputFile::create(root.parent().ok_or("Missing parent")?, b"{}")?;
    let refused = envelope(&invoke(&["version", "--input", outside.argument()?], None)?)?;
    assert_eq!(refused["error"]["code"], "POLICY_DENIED");
    let conflict = envelope(&invoke(
        &[
            "command",
            "find",
            "--query",
            "schema",
            "--input",
            "does-not-exist.json",
        ],
        None,
    )?)?;
    assert_eq!(
        conflict["error"]["details"]["issues"][0]["rule"],
        "input_and_action_flags_are_exclusive"
    );
    Ok(())
}

#[test]
fn public_file_input_refuses_hard_links_before_decoding() -> Result<(), Box<dyn std::error::Error>>
{
    let root = std::env::current_dir()?;
    let source = InputFile::create(
        root.parent().ok_or("Missing parent")?,
        br#"{"query":"private-input-sentinel"}"#,
    )?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let alias = InputFile(root.join(format!(
        ".craft-linked-input-{}-{stamp}.json",
        std::process::id()
    )));
    std::fs::hard_link(&source.0, &alias.0)?;
    for arguments in [
        vec!["command", "find", "--input", alias.argument()?],
        vec!["plan", "validate", "--file", alias.argument()?],
        vec![
            "plan",
            "tickets-record-completion",
            "--profile",
            "factory",
            "--max-bytes",
            "16777216",
            "--run-id",
            "delivery",
            "--preview-artifact",
            "tickets-example.json",
            "--result-title",
            "Results",
            "--consistency",
            "revalidate_each_batch",
            "--task-id",
            "implement",
            "--apply",
            "--completion-file",
            alias.argument()?,
        ],
    ] {
        let output = invoke(&arguments, None)?;
        let result = envelope(&output)?;
        assert_eq!(result["error"]["code"], "INPUT_INVALID");
        assert!(
            result["error"]["message"]
                .as_str()
                .ok_or("Missing error")?
                .contains("without links or traversal")
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
    }
    assert_eq!(
        std::fs::read(&source.0)?,
        br#"{"query":"private-input-sentinel"}"#
    );
    Ok(())
}

#[test]
fn public_comment_inputs_and_controls_fail_before_creating_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "public-comment-validation",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let registry = craft_cli::catalog::registry()?;
    for verb in ["get", "list"] {
        for mode in [
            "unknown", "missing", "null", "id", "fields", "budget", "cursor",
        ] {
            let mut input = registry
                .descriptor(&format!("jira.comment.{verb}"))?
                .example
                .clone();
            match mode {
                "unknown" => input["unexpected"] = json!("private-input-sentinel"),
                "missing" => {
                    input.as_object_mut().ok_or("input")?.remove("issue_id");
                }
                "null" => input["issue_id"] = Value::Null,
                "id" => input["issue_id"] = json!("0100"),
                "cursor" => input["cursor"] = json!("not-an-opaque-cursor"),
                _ => {}
            }
            let mut arguments = vec![
                "--state-root",
                root.to_str().ok_or("root")?,
                "--profile",
                "factory",
                "--max-bytes",
                if mode == "budget" { "512" } else { "16384" },
            ];
            if mode == "fields" {
                arguments.extend(["--fields", "not_a_comment_field"]);
            }
            arguments.extend(["jira", "comment", verb, "--input", "-"]);
            let output = invoke(&arguments, Some(&serde_json::to_vec(&input)?))?;
            let result = envelope(&output)?;
            assert!(!result["ok"].as_bool().ok_or("ok")?, "{verb}/{mode}");
            assert!(!root.exists(), "{verb}/{mode}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
        }
    }
    Ok(())
}

#[test]
fn public_attachment_metadata_inputs_refuse_before_private_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "attachment-input-refusal",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let registry = craft_cli::catalog::registry()?;
    for verb in ["get", "list", "settings"] {
        let descriptor = registry.descriptor(&format!("jira.attachment.{verb}"))?;
        registry.validate(&descriptor.id, &descriptor.example)?;
        assert!(jsonschema::validator_for(&descriptor.input_schema)?.is_valid(&descriptor.example));
        assert!(
            jsonschema::validator_for(&descriptor.response_schema)?
                .is_valid(&descriptor.response_example)
        );
        for mode in [
            "unknown", "id", "cloud", "cursor", "limit", "fields", "budget",
        ] {
            if verb == "settings" && matches!(mode, "id" | "limit")
                || verb == "get" && mode == "limit"
            {
                continue;
            }
            let mut input = descriptor.example.clone();
            match mode {
                "unknown" => input["url"] = json!("private-input-sentinel"),
                "id" => input[if verb == "get" { "id" } else { "issue_id" }] = json!("0101"),
                "cloud" => input["cloud_id"] = json!("not-a-cloud-id"),
                "cursor" => input["cursor"] = json!("not-an-opaque-cursor"),
                "limit" => input["limit"] = json!(0),
                _ => {}
            }
            let mut arguments = vec![
                "--state-root",
                root.to_str().ok_or("root")?,
                "--profile",
                "factory",
                "--max-bytes",
                if mode == "budget" { "512" } else { "16384" },
            ];
            if mode == "fields" {
                arguments.extend(["--fields", "not_a_metadata_field"]);
            }
            arguments.extend(["jira", "attachment", verb, "--input", "-"]);
            let output = invoke(&arguments, Some(&serde_json::to_vec(&input)?))?;
            let body = envelope(&output)?;
            assert_eq!(body["ok"], false, "{verb}/{mode}: {body}");
            assert!(!root.exists(), "{verb}/{mode}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
        }
    }
    use clap::Parser;
    let parsed = craft_cli::args::Cli::try_parse_from([
        "craft",
        "jira",
        "attachment",
        "list",
        "--issue-id",
        "100",
        "--limit",
        "1",
    ])?;
    let action = parsed.action.ok_or("action")?;
    assert_eq!(
        craft_cli::dispatch::command_id(&action),
        "jira.attachment.list"
    );
    let craft_cli::args::Action::Jira {
        action:
            craft_cli::args::JiraAction::Attachment {
                action: craft_cli::args::JiraAttachmentAction::List(args),
            },
    } = action
    else {
        return Err("expected attachment list".into());
    };
    let value = serde_json::to_value(args)?;
    assert_eq!(value["issue_id"], "100");
    assert_eq!(value["limit"], 1);
    registry.validate("jira.attachment.list", &value)?;
    Ok(())
}

#[test]
fn public_comment_writes_refuse_malformed_or_incomplete_intent_before_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = craft_test_support::state::fixture_root(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp"),
        "comment-write-refusal",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
    );
    let registry = craft_cli::catalog::registry()?;
    for verb in ["create", "update", "delete"] {
        let descriptor = registry.descriptor(&format!("jira.comment.{verb}"))?;
        registry.validate(&descriptor.id, &descriptor.example)?;
        assert!(jsonschema::validator_for(&descriptor.input_schema)?.is_valid(&descriptor.example));
        assert!(
            jsonschema::validator_for(&descriptor.response_schema)?
                .is_valid(&descriptor.response_example)
        );
        for mode in [
            "unknown",
            "id",
            "missing_apply",
            "missing_operation",
            "body_null",
            "visibility_null",
            "bad_ownership",
            "budget",
            "fields",
            "profile",
        ] {
            if verb == "delete" && matches!(mode, "body_null" | "visibility_null") {
                continue;
            }
            let mut input = descriptor.example.clone();
            match mode {
                "unknown" => input["unexpected"] = json!("private-input-sentinel"),
                "id" => input["issue_id"] = json!("0100"),
                "missing_apply" => input["apply"] = json!(false),
                "missing_operation" => input["operation_id"] = json!(""),
                "body_null" => input["body"] = Value::Null,
                "visibility_null" => input["visibility"] = Value::Null,
                "bad_ownership" => input["ownership"] = json!("all"),
                _ => {}
            }
            let mut arguments = vec![
                "--state-root",
                root.to_str().ok_or("root")?,
                "--max-bytes",
                if mode == "budget" { "16384" } else { "8388608" },
            ];
            if mode != "profile" {
                arguments.extend(["--profile", "factory"]);
            }
            if mode == "fields" {
                arguments.extend(["--fields", "receipt"]);
            }
            arguments.extend(["jira", "comment", verb, "--input", "-"]);
            let output = invoke(&arguments, Some(&serde_json::to_vec(&input)?))?;
            let result = envelope(&output)?;
            assert_eq!(result["ok"], false, "{verb}/{mode}: {result}");
            assert!(!root.exists(), "{verb}/{mode}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private-input-sentinel"));
        }
    }
    Ok(())
}

#[test]
fn malformed_download_flags_and_json_leave_state_unopened_and_stdout_as_one_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/tmp/download-pure-{stamp}"));
    assert!(!root.exists());
    let state = root.to_str().ok_or("state path")?;
    let output = if cfg!(windows) {
        "D:/exports"
    } else {
        "/tmp/exports"
    };
    let invalid = invoke(
        &[
            "--state-root",
            state,
            "jira",
            "attachment",
            "download",
            "--id",
            "101",
            "--artifact-name",
            "valid.bin",
            "--output-root",
            output,
            "--output",
            "../escape.bin",
        ],
        None,
    )?;
    assert_eq!(envelope(&invalid)?["ok"], false);
    assert!(!root.exists());
    let projected = invoke(
        &[
            "--state-root",
            state,
            "--fields",
            "artifact",
            "jira",
            "attachment",
            "download",
            "--id",
            "101",
            "--artifact-name",
            "valid.bin",
            "--output-root",
            output,
            "--output",
            "file.bin",
        ],
        None,
    )?;
    assert_eq!(envelope(&projected)?["ok"], false);
    assert!(!root.exists());
    let input = serde_json::to_vec(
        &json!({"id":"101","artifact_name":"valid.bin","output_root":output,"output":"file.bin","content_url":"https://untrusted.invalid"}),
    )?;
    let unknown = invoke(
        &[
            "--state-root",
            state,
            "jira",
            "attachment",
            "download",
            "--input",
            "-",
        ],
        Some(&input),
    )?;
    assert_eq!(envelope(&unknown)?["ok"], false);
    assert!(!root.exists());
    Ok(())
}

#[test]
fn whiteboard_scenes_encode_and_decode_through_stdin_without_network()
-> Result<(), Box<dyn std::error::Error>> {
    let scene = br#"{"format":"craft.whiteboard.scene.v1","elements":[
        {"kind":"sticky","id":"a","x":0,"y":0,"text":"Idea"},
        {"kind":"shape","id":"b","shape":"diamond","x":300,"y":0,"w":160,"h":160,"fill":"yellow"},
        {"kind":"connector","from":"a","to":"b"}]}"#;
    let encoded = envelope(&invoke(
        &["whiteboard", "encode", "--file", "-"],
        Some(scene),
    )?)?;
    assert_eq!(encoded["ok"], true, "{encoded}");
    assert_eq!(encoded["data"]["board_written"], false);
    assert_eq!(encoded["data"]["connector_count"], 1);
    let html = encoded["data"]["clipboard_html"].as_str().ok_or("html")?;
    let decoded = envelope(&invoke(
        &["whiteboard", "decode", "--file", "-"],
        Some(html.as_bytes()),
    )?)?;
    assert_eq!(decoded["ok"], true, "{decoded}");
    let elements = decoded["data"]["scene"]["elements"]
        .as_array()
        .ok_or("elements")?;
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[2]["kind"], "connector");
    assert_eq!(
        (elements[2]["from"].clone(), elements[2]["to"].clone()),
        (json!("e1"), json!("e2"))
    );
    let refused = invoke(
        &["whiteboard", "encode", "--file", "-"],
        Some(br#"{"format":"craft.whiteboard.scene.v1","elements":[{"kind":"sticky","x":0,"y":0,"color":"blue-bold"}]}"#),
    )?;
    assert_eq!(refused.status.code(), Some(10));
    assert_eq!(envelope(&refused)?["ok"], false);
    Ok(())
}
