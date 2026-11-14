use super::*;
use clap::CommandFactory;
#[test]
fn structured_reviewer_input_contains_only_invocation_controls()
-> Result<(), Box<dyn std::error::Error>> {
    let mut command = crate::args::Cli::command();
    let matches = command.try_get_matches_from_mut([
        "craft",
        "--state-root",
        "unreachable",
        "--profile",
        "factory",
        "review",
        "policy",
        "set",
        "--input",
        "-",
    ])?;
    assert!(!crate::args::has_action_flags(&command, &matches));
    let matches = command.try_get_matches_from_mut([
        "craft",
        "--profile",
        "factory",
        "review",
        "policy",
        "set",
        "--input",
        "-",
        "--quorum",
        "2",
    ])?;
    assert!(crate::args::has_action_flags(&command, &matches));
    Ok(())
}

#[test]
fn uncertain_reviewer_commit_retains_proposal_without_claiming_observed_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let target = space("00000000-1111-2222-3333-444444444444", "100");
    let snapshot = Snapshot {
        revision: 2,
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        target: target.clone(),
        expected_actor: "worker-agent".into(),
        enabled: true,
        reviewers: vec!["alice".into()],
        excluded_actors: vec![],
        quorum: 1,
    };
    let options = InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "reviewer-policy-request",
        max_bytes: 16384,
    };
    let error = craft_runtime::profiles::commit_outcome_unknown();
    let result = finish_set(
        &options,
        data("factory", target),
        Some(snapshot),
        Err(error),
    );
    let value = serde_json::to_value(&result.envelope)?;
    assert_eq!(value["data"]["configuration_write_state"], "sent_unknown");
    assert!(value["data"]["policy"].is_null());
    assert_eq!(value["data"]["proposed_policy"]["revision"], 2);
    assert_eq!(value["data"]["review_verified"], false);
    assert_eq!(value["data"]["live_verified"], false);
    assert!(jsonschema::validator_for(&response_schema::<PolicyData>()?)?.is_valid(&value));
    let encoded = crate::output::encode(result.envelope, options.max_bytes, result.output_policy)?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded.bytes)?,
        value
    );
    Ok(())
}
