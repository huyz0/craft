use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn doctor_reports_login_and_times_out_a_stalled_probe() {
    let fixture = Fixture::new();
    let executable = fixture.fixture_executable();
    let home = fixture.root.join("doctor codex home");
    fs::create_dir(&home).unwrap();
    let run = || {
        fixture
            .command()
            .args(["doctor", "--codex"])
            .arg(&executable)
            .arg("--codex-home")
            .arg(&home)
            .arg("--gh")
            .arg(&executable)
            .output()
            .unwrap()
    };

    let logged_out = run();
    assert!(!logged_out.status.success());
    let report: Value = serde_json::from_slice(&logged_out.stdout).unwrap();
    assert_eq!(report["ready"], false);
    assert_eq!(report["codex_authenticated"], false);
    assert_eq!(report["github_authenticated"], true);

    fs::write(home.join("logged-in"), "").unwrap();
    let report = success(run());
    assert_eq!(report["ready"], true);
    assert_eq!(report["codex_authenticated"], true);
    assert_eq!(report["github_authenticated"], true);
    assert_eq!(report["sqlite_writable"], true);

    fs::write(home.join("hang"), "").unwrap();
    let stalled = run();
    assert!(stalled.stdout.is_empty());
    fails(stalled, "Codex authentication probe timed out");
}

#[cfg(windows)]
#[test]
fn offline_smoke_accepts_a_canonicalized_windows_destination() {
    let fixture = Fixture::new();
    let destination = fixture.root.canonicalize().unwrap().join("canonical smoke");
    let report = success(
        fixture
            .command()
            .arg("smoke")
            .arg("--destination")
            .arg(&destination)
            .output()
            .unwrap(),
    );
    assert_eq!(report["passed"], true);
    assert_eq!(report["state"], "closed");
    assert_ne!(report["initial_candidate"], report["repaired_candidate"]);
}

#[test]
fn coordinator_review_brief_highlights_changes_and_blocks_constraint_drift() {
    let fixture = Fixture::new();
    let seed_bytes = include_bytes!("../examples/series-proposal.json");
    let seed_digest = craft_factory_evidence::sha256(seed_bytes);
    let seed = fixture.root.join("seed.json");
    let draft = fixture.root.join("draft.json");
    fs::write(&seed, seed_bytes).unwrap();
    let mut candidate: Value = serde_json::from_slice(seed_bytes).unwrap();
    candidate["members"].as_array_mut().unwrap().swap(0, 1);
    candidate["members"][0]["work_order"]["intent"] =
        json!("Read [untrusted](#link) <script> safely\nwith fallback.");
    candidate["members"][0]["work_order"]["acceptance"] = json!([
        "Old contract passes",
        "New contract passes",
        "Flag-off behavior passes",
        "Hidden fourth criterion"
    ]);
    candidate["compatibility"] = json!("Allow old and new payloads before rollout.");
    fs::write(&draft, serde_json::to_vec(&candidate).unwrap()).unwrap();
    let review = |brief: bool| {
        let mut command = fixture.command();
        command
            .arg("review-series-proposal")
            .arg(&draft)
            .arg("--against-seed")
            .arg(&seed)
            .args(["--seed-digest", &seed_digest]);
        if brief {
            command.arg("--brief");
        }
        command.output().unwrap()
    };
    let report = success(review(false));
    assert_eq!(report["seed_review"]["seed_compatible"], true);
    assert_eq!(report["seed_review"]["members"][0]["previous_position"], 2);
    assert_eq!(report["seed_review"]["compatibility_changed"], true);
    let brief = review(true);
    assert!(brief.status.success());
    let text = String::from_utf8(brief.stdout).unwrap();
    assert!(text.contains("consumer-read-optional-field` in `example/consumer` (was 2)"));
    assert!(text.contains("\\[untrusted\\]") && text.contains("\\<script\\>"));
    assert!(
        !text.contains("Hidden fourth criterion") && text.contains("1 more acceptance criteria")
    );

    candidate["members"][0]["token_limit"] = json!(9000);
    candidate["members"][0]["work_order"]["owner"] = json!("Other owner");
    candidate["members"][0]["work_order"]["scope"] = json!(["src", "tests"]);
    let changed_bytes = serde_json::to_vec(&candidate).unwrap();
    fs::write(&draft, &changed_bytes).unwrap();
    let blocked = review(false);
    let report: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(report["seed_review"]["seed_compatible"], false);
    assert_eq!(
        report["seed_review"]["members"][0]["locked_fields"],
        json!(["token_limit", "work_order.owner", "work_order.scope"])
    );
    fails(blocked, "proposal changes approved constraints");
    let blocked = fixture
        .command()
        .args([
            "shared-series",
            "--queue",
            "unused-queue",
            "register-proposal",
        ])
        .arg(&draft)
        .args([
            "--proposal-digest",
            &craft_factory_evidence::sha256(&changed_bytes),
            "--approval-reference",
            "fixture:review",
        ])
        .arg("--against-seed")
        .arg(&seed)
        .args(["--seed-digest", &seed_digest])
        .output()
        .unwrap();
    fails(blocked, "proposal changes approved constraints");
    assert!(!fixture.root.join("state.db").exists());
}

#[test]
fn coordinator_profile_comparison_counts_attempts_and_binds_reviews() {
    let fixture = Fixture::new();
    let write_pin = |relative: &str, value: &Value| {
        let bytes = serde_json::to_vec(value).unwrap();
        let path = fixture.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &bytes).unwrap();
        json!({"path":relative, "digest":craft_factory_evidence::sha256(&bytes)})
    };
    let base = json!({"id":"base", "model":"fixture-routine", "reasoning_effort":"low",
                      "timeout_seconds":30, "max_attempts":2, "approval_reference":"fixture:baseline-profile"});
    let challenger = json!({"id":"challenger", "model":"fixture-complex", "reasoning_effort":"high",
                            "timeout_seconds":30, "max_attempts":1, "approval_reference":"fixture:challenger-profile"});
    let base_pin = write_pin("baseline-profile.json", &base);
    let challenger_pin = write_pin("challenger-profile.json", &challenger);
    let seed_digest = craft_factory_evidence::sha256(b"approved fixture seed");
    let draft_bytes = include_bytes!("../examples/series-proposal.json");
    let proposal_digest = craft_factory_evidence::sha256(draft_bytes);
    let attempt = |name: &str, profile: &Value, pin: &Value, usage: Value, state: &str| {
        let mut receipt = json!({"schema_version":1, "seed_digest":seed_digest, "state":state,
            "profile":profile, "profile_digest":pin["digest"], "timeout_seconds":25, "elapsed_millis":500,
            "usage": usage});
        if state == "awaiting_approval" {
            receipt["proposal_digest"] = json!(proposal_digest);
        }
        let pin = write_pin(&format!("{name}/run.json"), &receipt);
        if state == "awaiting_approval" {
            fs::write(fixture.root.join(name).join("proposal.json"), draft_bytes).unwrap();
        }
        pin
    };
    let usage = |input, output| json!({"input_tokens":input, "cached_input_tokens":2, "output_tokens":output});
    let base_failed = attempt("baseline-failed", &base, &base_pin, usage(3, 7), "failed");
    let base_draft = attempt(
        "baseline-draft",
        &base,
        &base_pin,
        usage(11, 9),
        "awaiting_approval",
    );
    let challenger_timeout = attempt(
        "challenger-timeout",
        &challenger,
        &challenger_pin,
        Value::Null,
        "failed",
    );
    let challenger_draft = attempt(
        "challenger-draft",
        &challenger,
        &challenger_pin,
        usage(70, 30),
        "awaiting_approval",
    );
    let mut manifest = json!({"schema_version":1, "baseline_profile":base_pin, "challenger_profile":challenger_pin,
        "cases":[{"id":"case-one", "task_class":"cross-repo", "seed_digest":seed_digest,
            "baseline":{"attempts":[base_failed, base_draft], "review":{"proposal_digest":proposal_digest,
                "reviewer":"Release reviewer", "evidence":"fixture:review-one", "decision":"accepted"}},
            "challenger":{"attempts":[challenger_timeout, challenger_draft]}}]});
    let compare = |manifest: &Value| {
        write_pin("comparison.json", manifest);
        fixture
            .command()
            .arg("compare-series-profiles")
            .arg(fixture.root.join("comparison.json"))
            .output()
            .unwrap()
    };
    let output = success(compare(&manifest));
    let baseline = &output["overall"]["baseline"];
    let challenger = &output["overall"]["challenger"];
    assert_eq!(baseline["totals"]["failed"], 1);
    assert_eq!(baseline["totals"]["reported_tokens"], 30);
    assert_eq!(baseline["tokens_per_accepted_case"], 30.0);
    assert_eq!(challenger["totals"]["reported_tokens"], 100);
    assert_eq!(challenger["totals"]["attempts_without_usage"], 1);
    assert_eq!(challenger["totals"]["cases_over_attempt_limit"], 1);
    assert_eq!(challenger["totals"]["awaiting_review"], 1);
    assert!(challenger["tokens_per_accepted_case"].is_null());
    assert_eq!(output["by_task_class"]["cross-repo"]["baseline"], *baseline);

    let original = manifest.clone();
    manifest["cases"][0]["seed_digest"] = json!(craft_factory_evidence::sha256(b"different seed"));
    fails(compare(&manifest), "does not match the case seed");
    manifest = original.clone();
    manifest["cases"][0]["baseline"]["review"]["proposal_digest"] =
        json!(craft_factory_evidence::sha256(b"old proposal"));
    fails(compare(&manifest), "review must identify an exact proposal");
    manifest = original.clone();
    let duplicate = fixture.root.join("copy");
    fs::create_dir(&duplicate).unwrap();
    fs::copy(
        fixture.root.join("baseline-failed/run.json"),
        duplicate.join("run.json"),
    )
    .unwrap();
    manifest["cases"][0]["baseline"]["attempts"][1] = base_failed;
    manifest["cases"][0]["baseline"]["attempts"][1]["path"] = json!("copy/run.json");
    fails(compare(&manifest), "may only be counted once");
    let mut changed = draft_bytes.to_vec();
    changed.push(b'\n');
    fs::write(fixture.root.join("baseline-draft/proposal.json"), changed).unwrap();
    fails(
        compare(&original),
        "saved proposal does not match its coordinator receipt",
    );
}

#[test]
fn coordinator_drafts_only_reviewable_work_with_the_approved_constraints() {
    let fixture = Fixture::new();
    let agent = fixture.fixture_executable();
    let home = fixture.root.join("coordinator-home");
    fs::create_dir(&home).unwrap();
    let seed_path = fixture.root.join("series-seed.json");
    let seed: Value =
        serde_json::from_str(include_str!("../examples/series-proposal.json")).unwrap();
    let profile_path = fixture.root.join("planning-profile.json");
    let profile = json!({"id":"fixture-plan", "model":"fixture-planning", "reasoning_effort":"high",
                         "timeout_seconds":2, "max_attempts":1, "approval_reference":"fixture:approved-profile"});
    let profile_bytes = serde_json::to_vec(&profile).unwrap();
    let profile_digest = craft_factory_evidence::sha256(&profile_bytes);
    fs::write(&profile_path, profile_bytes).unwrap();
    let run = |seed: &Value,
               digest: Option<&str>,
               destination: &Path,
               requested_profile: Option<&str>| {
        let bytes = serde_json::to_vec(seed).unwrap();
        fs::write(&seed_path, &bytes).unwrap();
        let actual = craft_factory_evidence::sha256(&bytes);
        let mut command = fixture.command();
        command
            .arg("draft-series-proposal")
            .arg(&seed_path)
            .arg("--seed-digest")
            .arg(digest.unwrap_or(&actual))
            .arg("--codex")
            .arg(&agent)
            .arg("--codex-home")
            .arg(&home)
            .arg("--destination")
            .arg(destination)
            .args(["--timeout-seconds", "5"])
            .env("CRAFT_FACTORY_OPERATOR_TOKEN", "fixture-only")
            .env("BITBUCKET_TOKEN", "fixture-only");
        if let Some(digest) = requested_profile {
            command
                .arg("--profile")
                .arg(&profile_path)
                .args(["--profile-digest", digest]);
        }
        command.output().unwrap()
    };
    let destination = fixture.root.join("draft");
    fails(
        run(&seed, Some("wrong-seed"), &destination, None),
        "does not match the approved digest",
    );
    assert!(!destination.exists());
    fails(
        run(&seed, None, &destination, Some("wrong-profile-digest")),
        "profile does not match the approved digest",
    );
    assert!(!destination.exists());
    let inside = fixture.repo.join("draft");
    fails(run(&seed, None, &inside, None), "outside a Git checkout");
    assert!(!inside.exists());

    let mut profiled = seed.clone();
    profiled["compatibility"] = json!("Coordinator profile fixture");
    let result = success(run(&profiled, None, &destination, Some(&profile_digest)));
    assert_eq!(result["state"], "awaiting_approval");
    assert_eq!(result["profile"], profile);
    assert_eq!(result["profile_digest"], profile_digest);
    assert_eq!(result["timeout_seconds"], 2);
    let saved: Value =
        serde_json::from_slice(&fs::read(destination.join("run.json")).unwrap()).unwrap();
    assert_eq!(result, saved);
    assert_eq!(
        result["usage"],
        json!({"input_tokens":11,"cached_input_tokens":2,"output_tokens":9})
    );
    let proposal_path = destination.join("proposal.json");
    let review = success(
        fixture
            .command()
            .arg("review-series-proposal")
            .arg(&proposal_path)
            .output()
            .unwrap(),
    );
    assert_eq!(review["proposal_digest"], result["proposal_digest"]);
    assert_eq!(review["source"], seed["source"]);
    assert_eq!(review["owner"], seed["owner"]);
    let mut expected_member = seed["members"][1].clone();
    expected_member["work_order"]["intent"] = json!("Accept both payload versions.");
    expected_member["work_order"]["acceptance"] = json!(["The client accepts both fixtures."]);
    assert_eq!(review["members"][0]["member"], expected_member);
    assert_eq!(
        review["members"][1]["waits_for"],
        "consumer-read-optional-field"
    );
    assert!(!fixture.root.join("state.db").exists());
    fails(
        run(&seed, None, &destination, None),
        "destination already exists",
    );

    for (title, diagnostic, expected_state) in [
        (
            "Coordinator unknown ID fixture",
            "unknown or repeated work ID",
            "failed",
        ),
        (
            "Coordinator blocked fixture",
            "needs more information",
            "blocked",
        ),
        (
            "Coordinator timeout fixture",
            "coordinator timed out",
            "failed",
        ),
    ] {
        let failed = fixture.root.join(title);
        let mut invalid = seed.clone();
        invalid["compatibility"] = json!(title);
        let result = run(&invalid, None, &failed, Some(&profile_digest));
        if title == "Coordinator blocked fixture" {
            let report: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["state"], "blocked");
            assert_eq!(report["summary"], "Need the compatibility contract.");
        }
        fails(result, diagnostic);
        assert!(!failed.join("proposal.json").exists());
        let saved: Value =
            serde_json::from_slice(&fs::read(failed.join("run.json")).unwrap()).unwrap();
        assert_eq!(saved["state"], expected_state);
        assert_eq!(saved["profile_digest"], profile_digest);
        assert_eq!(saved["timeout_seconds"], 2);
        if title == "Coordinator timeout fixture" {
            assert!(saved["usage"].is_null());
        } else {
            assert_eq!(saved["usage"]["input_tokens"], 11);
        }
    }
}

#[test]
fn jira_capture_binds_reviewed_intent_and_prepares_the_supervised_order() {
    let fixture = Fixture::new();
    let order = fixture.order("jira-captured", "Change the greeting");
    let mut template: Value = serde_json::from_slice(&fs::read(&order).unwrap()).unwrap();
    template["context"] = json!([]);
    fs::write(&order, template.to_string()).unwrap();
    let policy = fixture.root.join("jira-policy.json");
    fs::write(
        &policy,
        json!({"schema_version":1,"craft":fixture.fixture_executable(),"craft_state":fixture.root,
        "craft_profile":"fixture-jira","cloud_id":"11111111-2222-3333-4444-555555555555",
        "issue_id":"1042","issue_url":"https://team.atlassian.net/browse/PILOT-1"})
        .to_string(),
    )
    .unwrap();
    let jira_command = || {
        let mut command = fixture.command();
        command
            .env("CRAFT_FACTORY_OPERATOR_TOKEN", "fixture-only")
            .env("FACTORY_FIXTURE_JIRA_OBSERVED", "1725494401000")
            .env("FACTORY_FIXTURE_JIRA_UPDATED", "2024-09-05T00:00:00.002Z");
        command
    };
    let capture = || {
        let mut command = jira_command();
        command
            .arg("capture-jira-snapshot")
            .arg(&policy)
            .arg(&order);
        command
    };
    let preview = success(capture().output().unwrap());
    assert_eq!(preview["assignee"], "relay-human");
    assert!(preview["prepared"].is_null());
    let unassigned = success(
        capture()
            .env("FACTORY_FIXTURE_JIRA_UNASSIGNED", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(unassigned.get("assignee"), Some(&Value::Null));
    assert_ne!(unassigned["approval_digest"], preview["approval_digest"]);
    fails(
        capture()
            .env("FACTORY_FIXTURE_JIRA_MISSING_ASSIGNEE", "1")
            .output()
            .unwrap(),
        "Jira did not return the assignee field",
    );
    let destination = fixture.root.join("approved-issue");
    let prepare = || {
        let mut command = capture();
        command
            .args([
                "--approval-digest",
                preview["approval_digest"].as_str().unwrap(),
                "--destination",
            ])
            .arg(&destination);
        command
    };
    for flag in ["CHANGED", "REASSIGNED", "UNASSIGNED"] {
        fails(
            prepare()
                .env(format!("FACTORY_FIXTURE_JIRA_{flag}"), "1")
                .output()
                .unwrap(),
            "changed after review",
        );
    }
    fails(
        prepare()
            .env("FACTORY_FIXTURE_JIRA_WRONG_ID", "1")
            .output()
            .unwrap(),
        "different Jira issue",
    );
    let mut unreviewed = template.clone();
    unreviewed["intent"] = json!("Unreviewed order edit");
    fs::write(&order, unreviewed.to_string()).unwrap();
    fails(prepare().output().unwrap(), "changed after review");
    assert!(!destination.exists());
    fs::write(&order, template.to_string()).unwrap();
    let prepared = success(
        prepare()
            .env("FACTORY_FIXTURE_JIRA_UPDATED", "2024-09-05T00:01:00.123Z")
            .output()
            .unwrap(),
    );
    let receipt = success(fixture.run(&[
        "delegate",
        prepared["prepared"]["work_order"].as_str().unwrap(),
        "--jira-snapshot",
        prepared["prepared"]["jira_snapshot"].as_str().unwrap(),
    ]));
    assert_eq!(
        receipt["work_order"]["context"][0],
        prepared["prepared"]["pin"]
    );
    let replay_directory = fixture.root.join("replayed-issue");
    let replay = success(
        capture()
            .args([
                "--approval-digest",
                preview["approval_digest"].as_str().unwrap(),
                "--destination",
            ])
            .arg(&replay_directory)
            .env("FACTORY_FIXTURE_JIRA_OBSERVED", "1725494403000")
            .output()
            .unwrap(),
    );
    assert_eq!(replay["prepared"]["pin"], prepared["prepared"]["pin"]);
    assert_eq!(
        receipt["events"].as_array().unwrap().last().unwrap()["kind"],
        "jira_snapshot_verified"
    );
    let live_check = || {
        let mut command = jira_command();
        command
            .args(["check-jira-snapshot", "jira-captured"])
            .arg(&policy);
        command
    };
    assert_eq!(
        success(live_check().output().unwrap())["decision"],
        "current"
    );
    fails(
        live_check()
            .env("FACTORY_FIXTURE_JIRA_REASSIGNED", "1")
            .output()
            .unwrap(),
        "changed since approval",
    );
    let attempt = fixture.root.join("blocked-jira-attempt");
    let codex = fixture.fixture_executable();
    fails(
        jira_command()
            .args(["run", "jira-captured", "--destination"])
            .arg(&attempt)
            .arg("--codex")
            .arg(&codex)
            .arg("--codex-home")
            .arg(&fixture.root)
            .arg("--jira-policy")
            .arg(&policy)
            .env("FACTORY_FIXTURE_JIRA_CHANGED", "1")
            .output()
            .unwrap(),
        "changed since approval",
    );
    assert!(!attempt.exists());
    let head = fixture.commit("src/hello", "Approved update");
    success(fixture.run(&[
        "submit",
        "jira-captured",
        "--commit",
        &head,
        "--note",
        "Fixture",
    ]));
    let bind = || {
        let mut command = jira_command();
        command
            .args([
                "bind-draft",
                "jira-captured",
                "--head",
                &head,
                "--reference",
                "https://example.invalid/pr/42",
                "--jira-policy",
            ])
            .arg(&policy);
        command
    };
    fails(
        bind()
            .env("FACTORY_FIXTURE_JIRA_CHANGED", "1")
            .output()
            .unwrap(),
        "changed since approval",
    );
    assert!(success(fixture.run(&["status", "jira-captured"]))["draft"].is_null());
    assert_eq!(success(bind().output().unwrap())["draft"]["head"], head);
}

#[test]
fn review_brief_ranks_a_bounded_list_and_counts_binary_changes() {
    let fixture = Fixture::new();
    let order = fixture.order("diff-review", "Make and verify a focused edit");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(order)
            .output()
            .unwrap(),
    );
    fs::write(
        fixture.repo.join("src/large.txt"),
        "tracked line\n".repeat(20),
    )
    .unwrap();
    fs::write(fixture.repo.join("src/opaque.bin"), [0_u8, 1, 2, 3]).unwrap();
    for index in 0..8 {
        fs::write(
            fixture.repo.join(format!("src/small-{index}.txt")),
            "one line\n",
        )
        .unwrap();
    }
    git(&fixture.repo, &["add", "src"]);
    git(
        &fixture.repo,
        &["commit", "-qm", "Review the measured diff"],
    );
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    success(fixture.run(&[
        "submit",
        "diff-review",
        "--commit",
        &head,
        "--note",
        "fixture",
    ]));
    let output = fixture.run(&["brief", "diff-review"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let brief = String::from_utf8(output.stdout).unwrap();
    assert!(brief.contains("10 files changed · +28 / -0 lines · 1 binary file"));
    assert!(brief.contains("- src/opaque.bin (binary)\n- src/large.txt (+20 / -0)"));
    assert!(brief.contains("- 2 more in the full diff."));
    assert!(!brief.contains("src/small-7.txt"));
}

#[test]
fn confluence_context_requires_approval_for_the_same_source_destination_and_excerpt() {
    let fixture = Fixture::new();
    let policy_path = fixture.root.join("confluence-policy.json");
    let mut policy = json!({"schema_version":1, "craft":fixture.fixture_executable(),
        "craft_state":fixture.root, "craft_profile":"fixture-confluence",
        "cloud_id":"11111111-2222-3333-4444-555555555555", "space_id":"200", "page_id":"101",
        "repository":"team/service", "location":"project:routing", "kind":"decision"});
    fs::write(&policy_path, policy.to_string()).unwrap();
    let capture = || {
        let mut command = fixture.command();
        command.arg("capture-confluence-context").arg(&policy_path);
        command.env(
            "CRAFT_FACTORY_OPERATOR_TOKEN",
            "fixture-token-never-inherit",
        );
        command.env("CREDENTIALS_DIRECTORY", &fixture.root);
        command
    };
    let preview = success(capture().output().unwrap());
    assert!(preview["release"].is_null() && preview["approval_digest"].is_null());
    assert_eq!(preview["source"]["version"], 5);
    let excerpt = fixture.root.join("reviewed-excerpt.txt");
    fs::write(&excerpt, "Use the regional endpoint.\r\n").unwrap();
    let with_excerpt = || {
        let mut command = capture();
        command.arg("--excerpt").arg(&excerpt);
        command
    };
    let review = success(with_excerpt().output().unwrap());
    assert!(review["release"].is_null() && review["excerpt_digest"].is_string());
    let approval = fixture.root.join("confluence-approval.json");
    fs::write(
        &approval,
        json!({"schema_version":2, "approval_digest":review["approval_digest"],
        "approval_reference":"fixture:owner-reviewed"})
        .to_string(),
    )
    .unwrap();
    let approved = || {
        let mut command = with_excerpt();
        command.arg("--approval").arg(&approval);
        command
    };
    let prepared = success(approved().output().unwrap());
    assert_eq!(prepared["release"]["repository"], "team/service");
    assert_eq!(
        prepared["release"]["excerpt"],
        "Use the regional endpoint.\r\n"
    );
    fs::write(&excerpt, "Use the regional endpoint.\n").unwrap();
    fails(
        approved().output().unwrap(),
        "excerpt changed after approval",
    );
    fs::write(&excerpt, "Use the regional endpoint.\r\n").unwrap();
    fails(
        approved()
            .env("FACTORY_FIXTURE_CONFLUENCE_CHANGED", "1")
            .output()
            .unwrap(),
        "changed after approval",
    );
    fails(
        approved()
            .env("FACTORY_FIXTURE_CONFLUENCE_DIFF_BODY", "1")
            .output()
            .unwrap(),
        "changed after approval",
    );
    fails(
        capture()
            .env("FACTORY_FIXTURE_CONFLUENCE_WRONG_SPACE", "1")
            .output()
            .unwrap(),
        "different Confluence site, space, or published page",
    );
    fails(
        capture()
            .env("FACTORY_FIXTURE_CONFLUENCE_OVERSIZE", "1")
            .output()
            .unwrap(),
        "output exceeded its byte limit",
    );
    policy["repository"] = json!("team/other");
    fs::write(&policy_path, policy.to_string()).unwrap();
    fails(approved().output().unwrap(), "changed after approval");
}

#[test]
fn shadow_merge_binds_policy_reviews_checks_and_git_history() {
    let fixture = Fixture::new();
    let work = fixture.order("shadow", "Update routine text");
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://bitbucket.org/team/service",
        ],
    );
    let head = fixture.commit("src/hello", "revised");
    let policy_path = fixture.root.join("shadow-policy.json");
    let observation_path = fixture.root.join("shadow-observation.json");
    let policy = json!({
        "schema_version": 1, "repository": "team/service", "base_branch": "main",
        "protection_revision": "branch-rules-7", "observation_issuer": "fixture:forge-exporter",
        "approval_reference": "fixture:lane-approval", "allowed_paths": ["src"], "protected_paths": [],
        "max_changed_paths": 3, "max_snapshot_age_seconds": 120, "max_review_age_seconds": 3600,
        "minimum_approvals": 1, "eligible_reviewers": ["reviewer-uuid", "author-uuid"],
        "required_checks": [{"name": "Verify", "issuer": "pipeline:trusted"}]
    });
    let policy_bytes = serde_json::to_vec(&policy).unwrap();
    let digest = craft_factory_evidence::sha256(&policy_bytes);
    fs::write(&policy_path, policy_bytes).unwrap();
    let observation = json!({
        "schema_version": 1, "issuer": "fixture:forge-exporter", "evidence": "fixture:observation/101",
        "policy_digest": digest, "repository": "team/service", "base_branch": "main",
        "draft_url": "https://bitbucket.org/team/service/pull-requests/101",
        "base": fixture.base, "head": head, "author": "author-uuid",
        "open": true, "draft": false, "mergeable": true, "protected": true,
        "protection_revision": "branch-rules-7", "observed_at": 10000,
        "checks": [{"name": "Verify", "issuer": "pipeline:trusted", "head": head, "passed": true,
                    "evidence": "fixture:checks/101"}],
        "reviews": [{"actor": "reviewer-uuid", "head": head, "decision": "approved",
                     "decided_at": 9900, "evidence": "fixture:approval/101"}]
    });
    let evaluate = |observed: &Value, expected: &str, approved: &str| {
        fs::write(&observation_path, serde_json::to_vec(observed).unwrap()).unwrap();
        fixture
            .command()
            .arg("shadow-merge")
            .arg(&work)
            .args([
                "--head",
                expected,
                "--approved-policy-digest",
                approved,
                "--as-of",
                "10020",
                "--require-eligible",
            ])
            .arg("--policy")
            .arg(&policy_path)
            .arg("--observation")
            .arg(&observation_path)
            .output()
            .unwrap()
    };
    let accepted = evaluate(&observation, &head, &digest);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let report: Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(report["shadow_eligible"], true);
    assert_eq!(report["counted_reviewers"], json!(["reviewer-uuid"]));
    assert_eq!(report["changed_path_count"], 1);
    assert!(
        report["observation_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(
        report["approval_evidence"][0]["evidence"],
        "fixture:approval/101"
    );
    let mut veto = observation.clone();
    veto["reviews"].as_array_mut().unwrap().push(json!({
        "actor": "additional-reviewer", "head": head, "decision": "changes_requested",
        "decided_at": 9950, "evidence": "fixture:changes-requested"
    }));
    let denied = evaluate(&veto, &head, &digest);
    let report: Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert_eq!(report["blocking_reasons"], json!(["changes_requested"]));
    let rejected = evaluate(&observation, &head, "sha256:unapproved");
    assert!(!rejected.status.success() && rejected.stdout.is_empty());
    let mut moved = observation.clone();
    moved["head"] = json!(fixture.base);
    moved["checks"][0]["head"] = json!(fixture.base);
    moved["reviews"][0]["head"] = json!(fixture.base);
    let denied = evaluate(&moved, &head, &digest);
    let report: Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert!(!denied.status.success());
    assert_eq!(report["passing_checks"], json!([]));
    assert_eq!(report["counted_reviewers"], json!([]));

    let mut adverse = observation.clone();
    adverse["head"] = json!(fixture.base);
    adverse["base"] = json!("b".repeat(40));
    adverse["observed_at"] = json!(9800);
    adverse["checks"][0]["issuer"] = json!("pipeline:untrusted");
    adverse["reviews"][0]["actor"] = json!("author-uuid");
    adverse["reviews"][0]["decided_at"] = json!(9700);
    adverse["protection_revision"] = json!("branch-rules-8");
    let denied = evaluate(&adverse, &head, &digest);
    assert!(!denied.status.success());
    let report: Value = serde_json::from_slice(&denied.stdout).unwrap();
    for reason in [
        "wrong_repository_or_revision",
        "stale_observation",
        "branch_protection_mismatch",
        "required_checks_missing_or_failed",
        "insufficient_current_approvals",
    ] {
        assert!(
            report["blocking_reasons"]
                .as_array()
                .unwrap()
                .contains(&json!(reason)),
            "{report}"
        );
    }

    fixture.commit("src/CODEOWNERS", "* @someone");
    git(&fixture.repo, &["rm", "src/CODEOWNERS"]);
    git(
        &fixture.repo,
        &["commit", "-qm", "Remove changed policy file"],
    );
    let later = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let mut history = observation.clone();
    history["head"] = json!(later);
    history["checks"][0]["head"] = json!(later);
    let stale = evaluate(&history, &later, &digest);
    let report: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert!(
        report["blocking_reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("insufficient_current_approvals"))
    );
    history["reviews"][0]["head"] = json!(later);
    let denied = evaluate(&history, &later, &digest);
    assert!(!denied.status.success());
    let report: Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert_eq!(report["blocking_reasons"], json!(["restricted_paths"]));
    assert_eq!(report["restricted_paths"], json!(["src/CODEOWNERS"]));

    let mut native_policy = policy;
    native_policy["native_standard_checks_digest"] = json!(format!("sha256:{}", "1".repeat(64)));
    let policy_bytes = serde_json::to_vec(&native_policy).unwrap();
    let digest = craft_factory_evidence::sha256(&policy_bytes);
    fs::write(&policy_path, policy_bytes).unwrap();
    let mut current = observation;
    current["policy_digest"] = json!(digest);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    current["observed_at"] = json!(now);
    current["reviews"][0]["decided_at"] = json!(now);
    fs::write(&observation_path, serde_json::to_vec(&current).unwrap()).unwrap();
    let denied = fixture
        .command()
        .arg("shadow-merge")
        .arg(&work)
        .args([
            "--head",
            &head,
            "--approved-policy-digest",
            &digest,
            "--require-eligible",
        ])
        .arg("--policy")
        .arg(&policy_path)
        .arg("--observation")
        .arg(&observation_path)
        .env_remove("CRAFT_FACTORY_BITBUCKET_SHADOW_TOKEN")
        .output()
        .unwrap();
    assert!(!denied.status.success());
    let report: Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert_eq!(
        report["blocking_reasons"],
        json!(["native_preflight_unavailable"])
    );
    assert_eq!(report["native_preflight_requested"], true);
}

#[test]
fn kubernetes_capture_runs_only_approved_reads_and_pins_a_stable_archive() {
    use craft_factory_evidence::sha256;

    let fixture = Fixture::new();
    let kubectl = fixture.fixture_executable();
    let input = fixture.root.join("kube-fixture");
    fs::create_dir(&input).unwrap();
    let kubeconfig = input.join("config");
    let config = b"fixture-only kubeconfig bytes";
    fs::write(&kubeconfig, config).unwrap();
    let policy_path = fixture.root.join("capture-policy.json");
    let destination = fixture.root.join("captured-release");
    let artifact = format!("sha256:{}", "a".repeat(64));
    let annotations = json!({
        "craft.factory/deployment-id":"rollout-17","craft.factory/config-revision":"config-1",
        "craft.factory/source-repository":"bitbucket:example/api"
    });
    let labels = json!({"pod-template-hash":"fixture"});
    let containers = json!([{"name":"api","image":format!("registry.example/api@{artifact}")}]);
    let pod = json!({
        "apiVersion":"v1","kind":"Pod",
        "metadata":{"namespace":"stage","name":"api-live","uid":"pod-uid","resourceVersion":"101",
            "creationTimestamp":"2024-02-28T20:00:01Z","annotations":annotations,"labels":labels,
            "ownerReferences":[{"apiVersion":"apps/v1","kind":"ReplicaSet","name":"api-fixture","uid":"rs-uid","controller":true}]},
        "spec":{"containers":containers},
        "status":{"phase":"Running","containerStatuses":[
            {"name":"api","imageID":artifact,"state":{"running":{"startedAt":"2024-02-28T20:00:02Z"}}}
        ]}
    });
    let stable = serde_json::to_vec(&pod).unwrap();
    fs::write(input.join("pod.json"), &stable).unwrap();
    let replica_set = json!({
        "apiVersion":"apps/v1","kind":"ReplicaSet",
        "metadata":{"namespace":"stage","name":"api-fixture","uid":"rs-uid","creationTimestamp":"2024-02-28T20:00:00Z",
            "ownerReferences":[{"apiVersion":"apps/v1","kind":"Deployment","name":"api","uid":"deployment-uid","controller":true}]},
        "spec":{"template":{"metadata":{"annotations":annotations,"labels":labels},"spec":{"containers":containers}}}
    });
    let rs_bytes = serde_json::to_vec(&replica_set).unwrap();
    fs::write(input.join("replica-set.json"), &rs_bytes).unwrap();
    let policy = json!({
        "schema_version":1,"context":"stage-context","kubeconfig_digest":sha256(config),"cluster":"stage-one",
        "namespace":"stage","pod":"api-live","pod_uid":"pod-uid","replica_set":"api-fixture","container":"api",
        "deployment_name":"api","deployment_uid":"deployment-uid","repository":"bitbucket:example/api",
        "target":{"service":"api","environment":"staging","region":"au-east","resource":"api-blue"},
        "owner":"Service owners","max_age_seconds":120
    });
    let run = |policy: &Value, dest: &Path| {
        let raw = serde_json::to_vec(policy).unwrap();
        fs::write(&policy_path, &raw).unwrap();
        fixture
            .command()
            .arg("collect-kubernetes-release")
            .arg(&policy_path)
            .arg("--policy-digest")
            .arg(sha256(&raw))
            .arg("--kubectl")
            .arg(&kubectl)
            .arg("--kubeconfig")
            .arg(&kubeconfig)
            .arg("--destination")
            .arg(dest)
            .env(
                "CRAFT_FACTORY_RUNNER_TOKEN",
                "must stay outside the collector",
            )
            .output()
            .unwrap()
    };
    let output = run(&policy, &destination);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let capture: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fs::read(destination.join("pod.json")).unwrap(), stable);
    assert_eq!(
        fs::read(destination.join("replica-set.json")).unwrap(),
        rs_bytes
    );
    let manifest = destination.join("sources.json");
    assert_eq!(
        capture["manifest_digest"],
        sha256(&fs::read(&manifest).unwrap())
    );
    let trace = fixture
        .command()
        .arg("trace-release-sources")
        .arg(&manifest)
        .arg("--manifest-digest")
        .arg(capture["manifest_digest"].as_str().unwrap())
        .output()
        .unwrap();
    assert!(
        trace.status.success(),
        "{}",
        String::from_utf8_lossy(&trace.stderr)
    );
    let report: Value = serde_json::from_slice(&trace.stdout).unwrap();
    assert_eq!(
        report["trace"]["gap"],
        json!({"stage":"build","reason":"missing"})
    );
    assert_eq!(
        report["trace"]["deployment"]["version"]["artifact_digest"],
        artifact
    );
    let calls = input.join("calls");
    let expected_routes = [
        "/api/v1/namespaces/stage/pods/api-live",
        "/apis/apps/v1/namespaces/stage/replicasets/api-fixture",
        "/api/v1/namespaces/stage/pods/api-live",
    ];
    assert_eq!(
        fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        expected_routes
    );

    let missing = fixture.root.join("failed-capture");
    let rejected = |output: Output, reason: &str| {
        assert!(!output.status.success() && output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(reason), "{error}");
        assert!(!missing.exists());
    };
    rejected(run(&policy, &destination), "already exists");
    fs::write(&calls, "").unwrap();
    let mut invalid = policy.clone();
    invalid["pod"] = json!("../different");
    rejected(run(&invalid, &missing), "invalid Kubernetes capture scope");
    fs::write(&kubeconfig, "changed config").unwrap();
    rejected(run(&policy, &missing), "kubeconfig does not match");
    fs::write(&kubeconfig, config).unwrap();
    assert!(fs::read_to_string(&calls).unwrap().is_empty());

    invalid = policy.clone();
    invalid["replica_set"] = json!("not-approved-by-pod");
    rejected(run(&invalid, &missing), "approved ReplicaSet");
    assert_eq!(
        fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [expected_routes[0]]
    );
    fs::write(&calls, "").unwrap();
    let mut moved = pod;
    moved["metadata"]["resourceVersion"] = json!("102");
    fs::write(
        input.join("pod-next.json"),
        serde_json::to_vec(&moved).unwrap(),
    )
    .unwrap();
    rejected(run(&policy, &missing), "changed during capture");
    fs::write(&calls, "").unwrap();
    fs::write(input.join("replica-set.json"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    rejected(run(&policy, &missing), "byte limit");
}

#[test]
fn otlp_import_redacts_and_bounds_telemetry_before_release_diagnosis() {
    use craft_factory_evidence::sha256;

    let fixture = Fixture::new();
    let release_path = fixture.root.join("release.json");
    let logs_path = fixture.root.join("otlp-logs.json");
    let metrics_path = fixture.root.join("otlp-metrics.json");
    let policy_path = fixture.root.join("otlp-policy.json");
    let telemetry_path = fixture.root.join("telemetry.json");
    let diagnostic_path = fixture.root.join("diagnostic-policy.json");
    let release = include_bytes!("../examples/release-evidence.json");
    fs::write(&release_path, release).unwrap();
    let evidence: Value = serde_json::from_slice(release).unwrap();
    let target = &evidence["inventory"][0]["target"];
    let version = &evidence["inventory"][0]["running"];
    let attr = |key: &str, value: &Value| json!({"key":key,"value":{"stringValue":value}});
    let attrs = json!([
        attr("service.name", &target["service"]),
        attr("service.instance.id", &json!("instance-a")),
        attr("deployment.environment.name", &target["environment"]),
        attr("cloud.region", &target["region"]),
        attr("craft.resource.id", &target["resource"]),
        attr("craft.deployment.id", &version["deployment_id"]),
        attr("craft.config.revision", &version["config_revision"]),
        attr("craft.artifact.digest", &version["artifact_digest"]),
        attr("private-label", &json!("unexported resource label")),
    ]);
    let log = json!({
        "timeUnixNano":"1795238350000000000","severityNumber":13,
        "eventName":"UPSTREAM_TIMEOUT","body":{"stringValue":"unexported raw log"},
        "traceId":"unexported trace ID","attributes":[attr("user.id", &json!("unexported user"))]
    });
    let mut outside = log.clone();
    outside["timeUnixNano"] = json!("1795238400000000001");
    let mut denied = log.clone();
    denied["eventName"] = json!("INTERNAL_PRIVATE");
    let mut other_instance = attrs.clone();
    other_instance[1]["value"]["stringValue"] = json!("instance-b");
    let logs = json!({"resourceLogs":[
        {"resource":{"attributes":attrs},"scopeLogs":[
            {"scope":{"name":"api.events"},"logRecords":[log,outside,denied]},
            {"scope":{"name":"other.scope"},"logRecords":[log]}
        ]},
        {"resource":{"attributes":other_instance},"scopeLogs":[
            {"scope":{"name":"api.events"},"logRecords":[log]}
        ]}
    ]});
    let metrics = json!({"resourceMetrics":[{"resource":{"attributes":attrs},"scopeMetrics":[
        {"scope":{"name":"api.metrics"},"metrics":[
            {"name":"request_latency_p95","unit":"ms","description":"unexported description",
             "gauge":{"dataPoints":[
                {"timeUnixNano":1795238360000000000u64,"asDouble":425.5,
                 "exemplars":[{"spanId":"unexported exemplar"}]},
                {"timeUnixNano":"1795238361000000000","asInt":"420"},
                {"timeUnixNano":"1795238362000000000","asInt":"99","flags":1}
             ]}},
            {"name":"request_latency_p95","unit":"s","sum":{"dataPoints":[]}}
        ]}
    ]}]});
    let policy = json!({
        "schema_version":1,"target":target,"version":version,"service_instance_id":"instance-a",
        "log_scope":"api.events","metric_scope":"api.metrics",
        "coverage":{"from":1795238300,"to":1795238400},"observed_at":1795238405,
        "approval_reference":"fixture:telemetry-approval","max_records":3,
        "log_codes":["UPSTREAM_TIMEOUT"],"metrics":[{"name":"request_latency_p95","unit":"ms"}],
    });
    let import = |logs: &Value, metrics: &Value, policy: &Value, wrong_digest: bool| {
        let logs = serde_json::to_vec(logs).unwrap();
        let metrics = serde_json::to_vec(metrics).unwrap();
        fs::write(&logs_path, &logs).unwrap();
        fs::write(&metrics_path, &metrics).unwrap();
        let mut policy = policy.clone();
        policy["logs_digest"] = json!(if wrong_digest {
            sha256(b"wrong file")
        } else {
            sha256(&logs)
        });
        policy["metrics_digest"] = json!(sha256(&metrics));
        let raw = serde_json::to_vec(&policy).unwrap();
        fs::write(&policy_path, &raw).unwrap();
        fixture
            .command()
            .arg("import-diagnostic-otlp")
            .arg(&policy_path)
            .arg("--policy-digest")
            .arg(sha256(&raw))
            .arg("--logs")
            .arg(&logs_path)
            .arg("--metrics")
            .arg(&metrics_path)
            .output()
            .unwrap()
    };
    let imported = import(&logs, &metrics, &policy, false);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let telemetry: Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert!(!String::from_utf8_lossy(&imported.stdout).contains("unexported"));
    assert!(!String::from_utf8_lossy(&imported.stdout).contains("INTERNAL_PRIVATE"));
    assert_eq!(
        telemetry["logs"],
        json!([{"timestamp":1795238350,"version":version,
        "code":"UPSTREAM_TIMEOUT","severity":"warn"}])
    );
    assert_eq!(telemetry["metrics"].as_array().unwrap().len(), 2);
    assert_eq!(telemetry["metrics"][0]["value"], 425.5);
    assert_eq!(telemetry["metrics"][1]["value"], 420.0);
    assert_eq!(
        telemetry["observation"]["reference"],
        format!("otlp:policy:{}", sha256(&fs::read(&policy_path).unwrap()))
    );

    fs::write(&telemetry_path, &imported.stdout).unwrap();
    fs::write(&diagnostic_path, serde_json::to_vec(&json!({
        "schema_version":1,"target":target,"approval_reference":"fixture:diagnosis",
        "release_digest":sha256(release),"telemetry_digest":sha256(&imported.stdout),
        "authorized_window":policy["coverage"],"max_window_seconds":100,"release_max_age_seconds":120,
        "max_records":3,"log_codes":policy["log_codes"],"metrics":policy["metrics"],
    })).unwrap()).unwrap();
    let report = fixture
        .command()
        .arg("diagnose-release")
        .arg(&diagnostic_path)
        .arg("--release")
        .arg(&release_path)
        .arg("--telemetry")
        .arg(&telemetry_path)
        .args(["--from", "1795238330", "--to", "1795238400"])
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let report: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert!(report["release"]["gap"].is_null());
    assert_eq!(report["logs"], telemetry["logs"]);
    assert_eq!(report["metrics"], telemetry["metrics"]);

    let reject = |output: Output, reason: &str| {
        assert!(!output.status.success() && output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(reason) && !error.contains("unexported"),
            "{error}"
        );
    };
    reject(import(&logs, &metrics, &policy, true), "approved digest");
    reject(
        import(&json!(true), &metrics, &policy, false),
        "signal type",
    );
    let mut wrong_instance = policy.clone();
    wrong_instance["service_instance_id"] = json!("no-such-instance");
    reject(
        import(&logs, &metrics, &wrong_instance, false),
        "approved resource instance",
    );
    let mut capped = policy.clone();
    capped["max_records"] = json!(2);
    reject(import(&logs, &metrics, &capped, false), "record limit");
    let mut unexpected = logs.clone();
    unexpected["resourceLogs"][0]["resource"]["attributes"]
        .as_array_mut()
        .unwrap()
        .push(attr("service.instance.id", &json!("ambiguous")));
    reject(
        import(&unexpected, &metrics, &policy, false),
        "repeats a release binding",
    );
    let mut labeled = metrics.clone();
    labeled["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][0]["attributes"] =
        json!([attr("user.id", &json!("unexported label"))]);
    reject(
        import(&logs, &labeled, &policy, false),
        "labeled or ambiguous",
    );
    let mut imprecise = metrics.clone();
    imprecise["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][1]["asInt"] =
        json!("9007199254740993");
    reject(import(&logs, &imprecise, &policy, false), "lose precision");
}

#[test]
fn release_source_manifest_traces_raw_archives_without_trusting_a_normalized_bundle() {
    exercise_release_source_manifest(false);
}

#[test]
fn github_release_source_manifest_traces_the_reviewed_commit_to_the_running_build() {
    exercise_release_source_manifest(true);
}

fn exercise_release_source_manifest(github: bool) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use craft_factory_evidence::sha256;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    let fixture = Fixture::new();
    let archive = fixture.root.join("evidence");
    fs::create_dir(&archive).unwrap();
    let manifest_path = fixture.root.join("sources.json");
    let save = |name: &str, value: &Value| {
        let raw = serde_json::to_vec(value).unwrap();
        fs::write(archive.join(name), &raw).unwrap();
        json!({"path": format!("evidence/{name}"), "sha256": sha256(&raw)})
    };
    let artifact = "a".repeat(64);
    let source_head = "b".repeat(40);
    let merge_commit = "c".repeat(40);
    let (repository, host) = if github {
        ("github:team/service", "github.com")
    } else {
        ("bitbucket:team/service", "bitbucket.org")
    };
    let containers =
        json!([{"name":"api","image":format!("registry.example/api@sha256:{artifact}")}]);
    let annotations = json!({
        "craft.factory/deployment-id":"rollout-1", "craft.factory/config-revision":"cfg-7",
        "craft.factory/source-repository":repository
    });
    let labels = json!({"pod-template-hash":"releasehash"});
    let owner = |kind: &str, name: &str, uid: &str| {
        json!([
            {"apiVersion":"apps/v1","kind":kind,"name":name,"uid":uid,"controller":true}
        ])
    };
    let pod = save(
        "pod.json",
        &json!({
            "apiVersion":"v1","kind":"Pod",
            "metadata":{
                "namespace":"stage","name":"api-releasehash-x","uid":"pod-uid",
                "creationTimestamp":"2026-11-21T05:15:00Z","labels":labels,"annotations":annotations,
                "ownerReferences":owner("ReplicaSet","api-releasehash","rs-uid")
            },
            "spec":{"containers":containers},
            "status":{"phase":"Running","containerStatuses":[
                {"name":"api","imageID":format!("sha256:{artifact}"),
                 "state":{"running":{"startedAt":"2026-11-21T05:16:30Z"}}}
            ]}
        }),
    );
    let replica_set = save(
        "replica-set.json",
        &json!({
            "apiVersion":"apps/v1","kind":"ReplicaSet",
            "metadata":{
                "namespace":"stage","name":"api-releasehash","uid":"rs-uid",
                "creationTimestamp":"2026-11-21T05:14:55Z",
                "ownerReferences":owner("Deployment","api","deployment-uid")
            },
            "spec":{"template":{"metadata":{"labels":labels,"annotations":annotations},"spec":{"containers":containers}}}
        }),
    );
    let signer = Ed25519KeyPair::from_seed_unchecked(&[25; 32]).unwrap();
    let builder = "https://ci.example/builders/release";
    let build_type = "https://ci.example/types/container";
    let source_uri = format!("git+https://{host}/team/service@refs/heads/main");
    let policy = save(
        "build-policy.json",
        &json!({
            "schema_version":1,"repository":repository,"source_uri":source_uri,
            "artifact_name":"registry.example/api","builder_id":builder,"build_type":build_type,
            "external_parameters":{},"public_key_base64":STANDARD.encode(signer.public_key().as_ref()),
            "approval_reference":"fixture:builder-approval"
        }),
    );
    let statement = serde_json::to_vec(&json!({
        "_type":"https://in-toto.io/Statement/v1","predicateType":"https://slsa.dev/provenance/v1",
        "subject":[{"name":"registry.example/api","digest":{"sha256":artifact}}],
        "predicate":{
            "buildDefinition":{"buildType":build_type,"externalParameters":{},
                "resolvedDependencies":[{"uri":source_uri,"digest":{"gitCommit":merge_commit}}]},
            "runDetails":{"builder":{"id":builder},"metadata":{"invocationId":"release-1","finishedOn":"2026-11-21T05:14:20Z"}}
        }
    })).unwrap();
    let mut pae = format!(
        "DSSEv1 28 application/vnd.in-toto+json {} ",
        statement.len()
    )
    .into_bytes();
    pae.extend_from_slice(&statement);
    let envelope = json!({
        "payloadType":"application/vnd.in-toto+json","payload":STANDARD.encode(statement),
        "signatures":[{"sig":STANDARD.encode(signer.sign(&pae).as_ref())}]
    });
    let attestation = save("build.dsse.json", &envelope);
    let uuid = "12345678-4321-1234-abcd-fedcba987654";
    let pull_request = save(
        "pr.json",
        &json!({
            "type":"pullrequest","id":2,"state":"MERGED","source":{"commit":{"hash":source_head}},
            "destination":{"branch":{"name":"main"},"repository":{"full_name":"team/service","uuid":uuid}},
            "merge_commit":{"hash":merge_commit},
            "created_on":"2026-11-21T05:11:00Z","updated_on":"2026-11-21T05:13:25Z"
        }),
    );
    let approval = save(
        "approval.json",
        &json!({
            "schema_version":1,"workspace":"team","repository":"service","repository_uuid":uuid,
            "destination_branch":"main","pull_request_id":2,"approved_source_commit":source_head,
            "result_commit":merge_commit,"work_item":"FIXTURE-2","intent_reference":"fixture:intent",
            "intent_revision":"r3","approval_reference":"fixture:owner-approval","approved_at":1795237990,
            "merge_reference":"fixture:merge-event","merged_at":1795238000
        }),
    );
    let mut manifest = json!({
        "schema_version":1,"as_of":1795238400,"max_age_seconds":120,
        "kubernetes":{
            "pod":pod,"replica_set":replica_set,
            "inventory":{"cluster":"stage-one","namespace":"stage","pod":"api-releasehash-x","pod_uid":"pod-uid",
                "container":"api","service":"api","environment":"staging","region":"au-east","resource":"api-blue",
                "owner":"Fixture owners","observed_at":1795238340},
            "deployment":{"replica_set_observed_at":1795238320,"deployment_name":"api","deployment_uid":"deployment-uid",
                "repository":repository}
        },
        "build":{"attestation":attestation,"policy":policy,"observed_at":1795238090},
        "change":{"pull_request":pull_request,"approval":approval,"observed_at":1795238030}
    });
    if github {
        let approval = save(
            "github-approval.json",
            &json!({
                "schema_version": 1, "repository": "team/service", "repository_id": 100,
                "destination_branch": "main", "pull_request_number": 42,
                "approved_source_commit": source_head, "result_commit": merge_commit,
                "work_item": "FIXTURE-2", "intent_reference": "fixture:intent", "intent_revision": "r3",
                "approval_reference": "fixture:owner-approval", "approved_at": 1735689590_u64,
                "merged_at": 1735689600_u64,
            }),
        );
        let destination = fixture.root.join("github");
        let gh = fixture.fixture_executable();
        let collect = |head: &str, repository_id: bool| {
            let mut command = fixture.command();
            command
                .arg("collect-github-change")
                .arg(archive.join("github-approval.json"))
                .args(["--approval-digest", approval["sha256"].as_str().unwrap()])
                .arg("--gh")
                .arg(&gh)
                .arg("--destination")
                .arg(&destination)
                .env("FACTORY_FIXTURE_SHA", head)
                .env("FACTORY_FIXTURE_BASE", &fixture.base)
                .env("FACTORY_FIXTURE_MERGED", "1")
                .env("FACTORY_FIXTURE_MERGE_TIME", "2025-01-01T00:00:00Z");
            if repository_id {
                command.env("FACTORY_FIXTURE_WRONG_GITHUB_REPO_ID", "1");
            }
            command.output().unwrap()
        };
        assert!(!collect(&source_head, true).status.success() && !destination.exists());
        assert!(!collect(&merge_commit, false).status.success() && !destination.exists());
        let collected = success(collect(&source_head, false));
        assert_eq!(
            collected["pull_request"],
            "https://github.com/team/service/pull/42"
        );
        let source = fs::read(destination.join("change-source.json")).unwrap();
        assert_eq!(collected["source_digest"], sha256(&source));
        let mut change: Value = serde_json::from_slice(&source).unwrap();
        for kind in ["pull_request", "approval"] {
            change[kind]["path"] =
                json!(format!("github/{}", change[kind]["path"].as_str().unwrap()));
        }
        let as_of = collected["observed_at"].as_u64().unwrap().max(1795238400);
        manifest["as_of"] = json!(as_of);
        manifest["kubernetes"]["inventory"]["observed_at"] = json!(as_of - 60);
        manifest["kubernetes"]["deployment"]["replica_set_observed_at"] = json!(as_of - 80);
        manifest["change"] = change;
    }
    let trace = |manifest: &Value, pin: Option<&str>, strict: bool| {
        let raw = serde_json::to_vec(manifest).unwrap();
        fs::write(&manifest_path, &raw).unwrap();
        let digest = sha256(&raw);
        let mut command = fixture.command();
        command
            .arg("trace-release-sources")
            .arg(&manifest_path)
            .args(["--manifest-digest", pin.unwrap_or(&digest)]);
        if strict {
            command.arg("--require-complete");
        }
        command.output().unwrap()
    };
    let output = trace(&manifest, None, true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["manifest_digest"],
        sha256(&fs::read(&manifest_path).unwrap())
    );
    assert!(report["trace"]["gap"].is_null());
    assert_eq!(report["trace"]["build"]["source_commit"], merge_commit);
    assert_eq!(
        report["trace"]["change"]["approved_source_commit"],
        source_head
    );
    assert_eq!(
        report["trace"]["change"]["approval_reference"],
        "fixture:owner-approval"
    );
    assert!(!fixture.root.join("state.db").exists());
    if github {
        assert_eq!(report["trace"]["change"]["repository"], repository);
        assert_eq!(
            report["trace"]["change"]["pull_request"],
            "https://github.com/team/service/pull/42"
        );
        exercise_release_assembly(&fixture, &manifest, &report["trace"]);
        let mut misclassified = manifest.clone();
        misclassified["change"]["provider"] = json!("bitbucket");
        assert!(!trace(&misclassified, None, false).status.success());
        fs::write(fixture.root.join("github/pull-request.json"), b"{}").unwrap();
        assert!(!trace(&manifest, None, false).status.success());
        return;
    }
    exercise_source_diagnosis(&fixture, &manifest_path, &manifest, &report["trace"]);

    let rejected = |output: Output, reason: &str| {
        assert!(!output.status.success() && output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(reason), "{error}");
    };
    let mut reviewed_pr: Value =
        serde_json::from_slice(&fs::read(archive.join("pr.json")).unwrap()).unwrap();
    reviewed_pr["author"] = json!({"uuid": "12345678-4321-1234-abcd-fedcba987655"});
    let reviewer = "12345678-4321-1234-abcd-fedcba987656";
    let review_policy = save(
        "review-policy.json",
        &json!({
            "schema_version":1,"workspace":"team","repository":"service","repository_uuid":uuid,
            "destination_branch":"main","pull_request_id":2,"approval_comment_id":51,"approver_uuid":reviewer,
            "approved_source_commit":source_head,"result_commit":merge_commit,"work_item":"FIXTURE-2",
            "intent_reference":"fixture:intent","intent_revision":"r3",
        }),
    );
    let decision = json!({"decision":"approve","source_commit":source_head,"work_item":"FIXTURE-2",
                          "intent_reference":"fixture:intent","intent_revision":"r3"});
    let comment = json!({
        "type":"pullrequest_comment","id":51,"deleted":false,"pullrequest":{"id":2},
        "user":{"uuid": reviewer},"content":{"raw":format!("Craft release approval v1\n{decision}")},
        "created_on":"2026-11-21T05:13:10.100Z","updated_on":"2026-11-21T05:13:20.050Z",
    });
    let comment_file = save("review-comment.json", &comment);
    let reviewed_pr_file = save("reviewed-pr.json", &reviewed_pr);
    let review_policy_bytes = fs::read(archive.join("review-policy.json")).unwrap();
    let policy = craft_factory_evidence::bitbucket_review::Policy::parse(
        &review_policy_bytes,
        review_policy["sha256"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        policy.endpoint(),
        "https://api.bitbucket.org/2.0/repositories/team/service/pullrequests/2"
    );
    assert_eq!(
        policy.comment_endpoint(),
        format!("{}/comments/51", policy.endpoint())
    );
    let activity_endpoint = policy.activity_endpoint().unwrap();
    let next = format!("{activity_endpoint}?page=2");
    let page_one = json!({"values":[{"update":{"state":"OPEN"}}],"next":next});
    let page_two = json!({"values":[{"update":{"state":"MERGED","date":"2026-11-21T05:13:20.100Z",
        "source":{"commit":{"hash":source_head}},"destination":reviewed_pr["destination"]},
        "pull_request":{"id":2}}]});
    let first = save("activity-1.json", &page_one);
    let second = save("activity-2.json", &page_two);
    let mut requests = Vec::new();
    let activity = policy
        .collect_activity(|url| {
            requests.push(url.to_owned());
            let page = if url == activity_endpoint {
                &page_one
            } else {
                assert_eq!(url, next);
                &page_two
            };
            Ok(serde_json::to_vec(page).unwrap())
        })
        .unwrap();
    assert_eq!(requests, [activity_endpoint.clone(), next]);
    let foreign = policy.collect_activity(|url| {
        assert_eq!(url, activity_endpoint);
        Ok(serde_json::to_vec(&json!({"values":[],"next":"https://api.bitbucket.org/2.0/repositories/team/other/pullrequests/2/activity?page=2"})).unwrap())
    }).unwrap_err();
    assert!(
        foreign
            .to_string()
            .contains("left the approved pull request")
    );
    let (derived, _) = policy
        .verify(
            &fs::read(archive.join("reviewed-pr.json")).unwrap(),
            &fs::read(archive.join("review-comment.json")).unwrap(),
            &activity,
            1795238030,
        )
        .unwrap();
    fs::write(archive.join("derived-approval.json"), &derived).unwrap();
    let mut native = manifest.clone();
    native["change"] = json!({
        "pull_request":reviewed_pr_file,"approval":{"path":"evidence/derived-approval.json","sha256":sha256(&derived)},
        "review":{"policy":review_policy,"comment":comment_file,"activity":[first,second]},"observed_at":1795238030,
    });
    let verified = trace(&native, None, true);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(report["trace"]["change"]["merged_at"], 1795238000_u64);
    assert_eq!(
        report["trace"]["change"]["merge_reference"],
        format!("{activity_endpoint}#{}", sha256(&activity[1]))
    );
    exercise_release_assembly(&fixture, &native, &report["trace"]);
    assert!(
        report["trace"]["change"]["approval_reference"]
            .as_str()
            .unwrap()
            .contains("#comment-51#sha256:")
    );
    let mut substituted = native.clone();
    substituted["change"]["approval"] = manifest["change"]["approval"].clone();
    rejected(
        trace(&substituted, None, false),
        "differs from the original review evidence",
    );
    let mut missing = native.clone();
    missing["change"].as_object_mut().unwrap().remove("review");
    rejected(
        trace(&missing, None, false),
        "requires its original comment and policy",
    );
    let mut self_approved_pr = reviewed_pr.clone();
    self_approved_pr["author"]["uuid"] = json!(reviewer);
    let mut self_approval = native.clone();
    self_approval["change"]["pull_request"] = save("self-approved-pr.json", &self_approved_pr);
    rejected(trace(&self_approval, None, false), "ineligible author");
    let mut incomplete_activity = native.clone();
    incomplete_activity["change"]["review"]["activity"] = json!([first]);
    rejected(
        trace(&incomplete_activity, None, false),
        "incomplete archived Bitbucket activity",
    );
    let mut wrong_activity = native.clone();
    let mut other_head = page_two.clone();
    other_head["values"][0]["update"]["source"]["commit"]["hash"] = json!("d".repeat(40));
    wrong_activity["change"]["review"]["activity"][1] = save("wrong-activity.json", &other_head);
    rejected(
        trace(&wrong_activity, None, false),
        "approved PR, head, and destination",
    );
    for (page, reason) in [
        (json!({"values":[]}), "does not contain a merge event"),
        (
            json!({"values":[page_two["values"][0],page_two["values"][0]]}),
            "lists multiple merge events",
        ),
    ] {
        wrong_activity["change"]["review"]["activity"][1] = save("wrong-activity.json", &page);
        rejected(trace(&wrong_activity, None, false), reason);
    }

    let mut after_merge = comment.clone();
    after_merge["updated_on"] = json!("2026-11-21T05:13:20.101Z");
    let mut other_author = comment.clone();
    other_author["user"]["uuid"] = json!("12345678-4321-1234-abcd-fedcba987657");
    let mut stale_decision = decision.clone();
    stale_decision["source_commit"] = json!("e".repeat(40));
    let mut stale = comment;
    stale["content"]["raw"] = json!(format!("Craft release approval v1\n{stale_decision}"));
    for (wrong, reason) in [
        (after_merge, "must predate the merge"),
        (other_author, "ineligible author"),
        (stale, "exact source commit and intent"),
    ] {
        native["change"]["review"]["comment"] = save("wrong-comment.json", &wrong);
        rejected(trace(&native, None, false), reason);
    }

    let mut partial = manifest.clone();
    partial.as_object_mut().unwrap().remove("change");
    let incomplete = trace(&partial, None, true);
    assert!(!incomplete.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&incomplete.stdout).unwrap()["trace"]["gap"],
        json!({"stage":"change","reason":"missing"})
    );
    partial.as_object_mut().unwrap().remove("build");
    let incomplete = trace(&partial, None, false);
    assert!(incomplete.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&incomplete.stdout).unwrap()["trace"]["gap"],
        json!({"stage":"build","reason":"missing"})
    );

    let mut other_pr: Value =
        serde_json::from_slice(&fs::read(archive.join("pr.json")).unwrap()).unwrap();
    let mut other_approval: Value =
        serde_json::from_slice(&fs::read(archive.join("approval.json")).unwrap()).unwrap();
    other_pr["merge_commit"]["hash"] = json!("d".repeat(40));
    other_approval["result_commit"] = json!("d".repeat(40));
    let mut mismatched = manifest.clone();
    mismatched["change"]["pull_request"] = save("other-pr.json", &other_pr);
    mismatched["change"]["approval"] = save("other-approval.json", &other_approval);
    let incomplete = trace(&mismatched, None, true);
    assert!(!incomplete.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&incomplete.stdout).unwrap()["trace"]["gap"],
        json!({"stage":"change","reason":"missing"})
    );

    let wrong_pin = trace(&manifest, Some(&sha256(b"other manifest")), false);
    assert!(!wrong_pin.status.success() && wrong_pin.stdout.is_empty());
    assert!(String::from_utf8_lossy(&wrong_pin.stderr).contains("approved digest"));
    let mut escape = manifest.clone();
    escape["kubernetes"]["pod"]["path"] = json!("../pod.json");
    let outside = trace(&escape, None, false);
    assert!(!outside.status.success() && outside.stdout.is_empty());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("inside the archive"));
    fs::write(archive.join("build.dsse.json"), b"{}").unwrap();
    let tampered = trace(&manifest, None, false);
    assert!(!tampered.status.success() && tampered.stdout.is_empty());
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("pinned digest"));
}

fn exercise_release_assembly(fixture: &Fixture, source: &Value, expected_trace: &Value) {
    use craft_factory_evidence::sha256;

    let package = fixture.root.join("collected");
    fs::create_dir(&package).unwrap();
    let stage = |directory: &str, name: &str, mut fragment: Value, references: &[&str]| {
        let target = package.join(directory);
        fs::create_dir(&target).unwrap();
        for pointer in references {
            let path = fragment.pointer_mut(pointer).unwrap();
            let original = Path::new(path.as_str().unwrap());
            let file = original.file_name().unwrap();
            fs::copy(fixture.root.join(original), target.join(file)).unwrap();
            *path = json!(file.to_str().unwrap());
        }
        let bytes = serde_json::to_vec(&fragment).unwrap();
        let path = target.join(name);
        fs::write(&path, &bytes).unwrap();
        (path, sha256(&bytes))
    };
    let mut deployment = source.clone();
    deployment.as_object_mut().unwrap().remove("build");
    deployment.as_object_mut().unwrap().remove("change");
    let (kube, kube_pin) = stage(
        "runtime",
        "sources.json",
        deployment,
        &["/kubernetes/pod/path", "/kubernetes/replica_set/path"],
    );
    let (build, build_pin) = stage(
        "build",
        "build-source.json",
        source["build"].clone(),
        &["/attestation/path", "/policy/path"],
    );
    let mut change_files = vec!["/pull_request/path", "/approval/path"];
    let github = source["change"]["provider"] == "github";
    if !github {
        change_files.extend([
            "/review/policy/path",
            "/review/comment/path",
            "/review/activity/0/path",
            "/review/activity/1/path",
        ]);
    }
    let (change, change_pin) = stage(
        "change",
        "change-source.json",
        source["change"].clone(),
        &change_files,
    );
    let manifest = package.join("assembled.json");
    let assemble = |pin: &str, as_of: Option<&str>, output: &Path| {
        let mut command = fixture.command();
        command
            .arg("assemble-release-sources")
            .arg("--kubernetes")
            .arg(&kube)
            .args(["--kubernetes-digest", &kube_pin])
            .arg("--build")
            .arg(&build)
            .args(["--build-digest", pin])
            .arg("--change")
            .arg(&change)
            .args(["--change-digest", &change_pin])
            .arg("--destination")
            .arg(output);
        if let Some(as_of) = as_of {
            command.args(["--as-of", as_of]);
        }
        command.output().unwrap()
    };
    let wrong = assemble(&sha256(b"substituted"), None, &manifest);
    assert!(!wrong.status.success() && !manifest.exists());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("collector digest"));
    let unrelated = fixture.root.join("other-archive");
    fs::create_dir(&unrelated).unwrap();
    let outside = assemble(&build_pin, None, &unrelated.join("sources.json"));
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("inside the destination archive"));
    let later = (source["as_of"].as_u64().unwrap() + 600).to_string();
    let stale = assemble(&build_pin, Some(&later), &manifest);
    assert!(!stale.status.success() && !manifest.exists());
    assert_eq!(
        serde_json::from_slice::<Value>(&stale.stdout).unwrap()["trace"]["gap"]["reason"],
        "stale"
    );
    let output = assemble(&build_pin, None, &manifest);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let bytes = fs::read(&manifest).unwrap();
    let assembled: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(assembled["kubernetes"]["pod"]["path"], "runtime/pod.json");
    assert_eq!(
        assembled["build"]["attestation"]["path"],
        "build/build.dsse.json"
    );
    if github {
        assert_eq!(assembled["change"]["provider"], "github");
        assert_eq!(
            assembled["change"]["pull_request"]["path"],
            "change/pull-request.json"
        );
    } else {
        assert_eq!(
            assembled["change"]["review"]["activity"][1]["path"],
            "change/activity-2.json"
        );
    }
    let digest = sha256(&bytes);
    assert_eq!(report["manifest_digest"], digest);
    assert_eq!(&report["trace"], expected_trace);
    let output = fixture
        .command()
        .arg("trace-release-sources")
        .arg(&manifest)
        .args(["--manifest-digest", &digest, "--require-complete"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        report
    );
    let duplicate = assemble(&build_pin, None, &manifest);
    assert!(!duplicate.status.success());
    assert_eq!(fs::read(&manifest).unwrap(), bytes);
    if !github {
        exercise_archived_rollout(fixture, &package, &assembled, &report["trace"], &digest);
    }
}

fn exercise_archived_rollout(
    fixture: &Fixture,
    archive: &Path,
    sources: &Value,
    trace: &Value,
    digest: &str,
) {
    use craft_factory_evidence::sha256;

    let mut west = sources.clone();
    let kube = &mut west["kubernetes"];
    let mut pod: Value =
        serde_json::from_slice(&fs::read(archive.join("runtime/pod.json")).unwrap()).unwrap();
    let mut replica_set: Value =
        serde_json::from_slice(&fs::read(archive.join("runtime/replica-set.json")).unwrap())
            .unwrap();
    pod["metadata"]["uid"] = json!("pod-uid-west");
    pod["metadata"]["ownerReferences"][0]["uid"] = json!("rs-uid-west");
    replica_set["metadata"]["uid"] = json!("rs-uid-west");
    replica_set["metadata"]["ownerReferences"][0]["uid"] = json!("deployment-uid-west");
    kube["inventory"]["pod_uid"] = json!("pod-uid-west");
    kube["inventory"]["cluster"] = json!("stage-west");
    kube["inventory"]["region"] = json!("au-west");
    kube["deployment"]["deployment_uid"] = json!("deployment-uid-west");
    let save = |path: &str, value: &Value| {
        let raw = serde_json::to_vec(value).unwrap();
        fs::write(archive.join(path), &raw).unwrap();
        json!({"path":path,"sha256":sha256(&raw)})
    };
    kube["pod"] = save("runtime/west-pod.json", &pod);
    kube["replica_set"] = save("runtime/west-replica-set.json", &replica_set);
    let west_source = save("west.json", &west);
    let east_source = json!({"path":"assembled.json","sha256":digest});
    let change = &trace["change"];
    let mut west_target = trace["target"].clone();
    west_target["region"] = json!("au-west");
    let policy = save(
        "remediation.json",
        &json!({
            "schema_version":1, "change": {
                "repository":change["repository"],"source_commit":change["result_commit"],
                "pull_request":change["pull_request"],"work_item":change["work_item"],
                "intent_reference":change["intent_reference"],"intent_revision":change["intent_revision"],
                "approval_reference":change["approval_reference"],
            },
            "artifact_digest":trace["build"]["artifact_digest"],"release_max_age_seconds":120,
            "required_targets":[{"target":trace["target"],"config_revision":"cfg-7"},
                                {"target":west_target,"config_revision":"cfg-7"}],
        }),
    );
    let rollout = |releases: Value, as_of: &str| {
        let index = save(
            "rollout.json",
            &json!({"schema_version":1,"releases":releases}),
        );
        fixture
            .command()
            .arg("verify-remediation-sources")
            .arg(archive.join("remediation.json"))
            .args(["--policy-digest", policy["sha256"].as_str().unwrap()])
            .arg("--sources")
            .arg(archive.join("rollout.json"))
            .args([
                "--sources-digest",
                index["sha256"].as_str().unwrap(),
                "--as-of",
                as_of,
                "--require-complete",
            ])
            .output()
            .unwrap()
    };
    let partial = rollout(json!([east_source]), "1795238400");
    assert!(!partial.status.success());
    let report: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(report["verified_targets"], 1);
    assert_eq!(report["targets"][1]["release"]["gap"]["reason"], "missing");
    let complete = rollout(json!([west_source, east_source]), "1795238401");
    assert!(
        complete.status.success(),
        "{}",
        String::from_utf8_lossy(&complete.stderr)
    );
    let report: Value = serde_json::from_slice(&complete.stdout).unwrap();
    assert_eq!(report["verified_targets"], 2);
    assert_eq!(
        report["evidence_digest"],
        sha256(&fs::read(archive.join("rollout.json")).unwrap())
    );
    let index_path = archive.join("prepared-index.json");
    let head = change["approved_source_commit"].as_str().unwrap();
    let prepare = |head: &str, releases: &[&Value]| {
        let mut command = fixture.command();
        command
            .arg("prepare-rollout-archive")
            .arg(archive.join("remediation.json"))
            .args([
                "--policy-digest",
                policy["sha256"].as_str().unwrap(),
                "--as-of",
                "1795238401",
                "--work-id",
                "fixture-rollout",
                "--reviewed-head",
                head,
            ])
            .arg("--destination")
            .arg(&index_path);
        for release in releases {
            command
                .arg("--release")
                .arg(archive.join(release["path"].as_str().unwrap()))
                .arg(release["sha256"].as_str().unwrap());
        }
        command.output().unwrap()
    };
    let missing = prepare(head, &[&east_source]);
    assert!(!missing.status.success() && !index_path.exists());
    assert_eq!(
        serde_json::from_slice::<Value>(&missing.stdout).unwrap()["verified_targets"],
        1
    );
    let wrong_head = prepare(&"f".repeat(40), &[&east_source, &west_source]);
    assert!(!wrong_head.status.success() && !index_path.exists());
    assert!(String::from_utf8_lossy(&wrong_head.stderr).contains("reviewed candidate head"));
    let prepared = prepare(head, &[&west_source, &east_source]);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let bytes = fs::read(&index_path).unwrap();
    let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(
        prepared["catalog_entry"],
        json!({"work_id":"fixture-rollout", "policy":"remediation.json",
        "policy_digest":policy["sha256"], "sources":"prepared-index.json", "sources_digest":sha256(&bytes)})
    );
    assert_eq!(prepared["verification"]["verified_targets"], 2);
    assert!(
        !prepare(head, &[&west_source, &east_source])
            .status
            .success()
    );
    assert_eq!(fs::read(&index_path).unwrap(), bytes);
    let duplicate = rollout(json!([east_source, east_source]), "1795238401");
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("same target"));
    let stale = rollout(json!([west_source, east_source]), "1795239000");
    assert!(!stale.status.success());
    let report: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(report["verified_targets"], 0);
    assert_eq!(report["targets"][0]["release"]["gap"]["reason"], "stale");
    let mut tampered = west_source;
    tampered["sha256"] = json!(sha256(b"another manifest"));
    let rejected = rollout(json!([east_source, tampered]), "1795238401");
    assert!(!rejected.status.success() && rejected.stdout.is_empty());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("approved digest"));
}

fn exercise_source_diagnosis(
    fixture: &Fixture,
    manifest_path: &Path,
    manifest: &Value,
    release: &Value,
) {
    use craft_factory_evidence::sha256;

    let telemetry_path = fixture.root.join("telemetry.json");
    let policy_path = fixture.root.join("diagnostic-policy.json");
    let target = &release["target"];
    let version = &release["inventory"]["running"];
    let mut previous_rollout = version.clone();
    previous_rollout["deployment_id"] = json!("older-rollout");
    let event = |timestamp: u64, release: &Value| {
        json!({
            "timestamp":timestamp, "version":release, "code":"UPSTREAM_TIMEOUT", "severity":"warn"
        })
    };
    let metric = json!({"name":"request_latency_p95","unit":"ms"});
    let telemetry = json!({
        "schema_version":1, "target":target, "coverage":{"from":1795238130,"to":1795238400},
        "observation":{"source":"fixture telemetry","reference":"fixture:incident","observed_at":1795238405},
        "logs":[event(1795238189, version), event(1795238350, &previous_rollout), event(1795238360, version)],
        "metrics":[{"timestamp":1795238361,"version":version,"metric":metric,"value":425.0}]
    });
    let raw = serde_json::to_vec(&telemetry).unwrap();
    fs::write(&telemetry_path, &raw).unwrap();
    let manifest_bytes = fs::read(manifest_path).unwrap();
    let policy = json!({
        "schema_version":1,"target":target,"approval_reference":"fixture:diagnosis-approved",
        "release_basis":"source_manifest","release_digest":sha256(&manifest_bytes),"telemetry_digest":sha256(&raw),
        "authorized_window":{"from":1795238130,"to":1795238400},"max_window_seconds":300,
        "release_max_age_seconds":90,"max_records":2,"log_codes":["UPSTREAM_TIMEOUT"],"metrics":[metric]
    });
    let diagnose = |policy: &Value, pin: Option<&str>, from: &str, to: &str| {
        let raw = serde_json::to_vec(policy).unwrap();
        let digest = sha256(&raw);
        fs::write(&policy_path, raw).unwrap();
        fixture
            .command()
            .arg("diagnose-release-sources")
            .arg(&policy_path)
            .arg("--manifest")
            .arg(manifest_path)
            .arg("--telemetry")
            .arg(&telemetry_path)
            .args([
                "--policy-digest",
                pin.unwrap_or(&digest),
                "--from",
                from,
                "--to",
                to,
            ])
            .output()
            .unwrap()
    };
    let output = diagnose(&policy, None, "1795238130", "1795238400");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["release_basis"], "source_manifest");
    assert_eq!(report["release_digest"], policy["release_digest"]);
    assert!(report["release"]["gap"].is_null());
    assert_eq!(report["telemetry_correlated"], true);
    assert_eq!(report["logs"], json!([event(1795238360, version)]));
    assert_eq!(report["metrics"].as_array().unwrap().len(), 1);
    let legacy = fixture
        .command()
        .arg("diagnose-release")
        .arg(&policy_path)
        .arg("--release")
        .arg(manifest_path)
        .arg("--telemetry")
        .arg(&telemetry_path)
        .args(["--from", "1795238130", "--to", "1795238400"])
        .output()
        .unwrap();
    assert!(!legacy.status.success() && legacy.stdout.is_empty());
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("release evidence format"));

    let mut stale_policy = policy.clone();
    stale_policy["release_max_age_seconds"] = json!(30);
    let stale = diagnose(&stale_policy, None, "1795238130", "1795238400");
    assert!(stale.status.success());
    let stale: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(
        stale["release"]["gap"],
        json!({"stage":"inventory","reason":"stale"})
    );
    assert_eq!(stale["telemetry_correlated"], false);
    assert_eq!(stale["logs"], json!([]));

    let rejected = |result: Output, error: &str| {
        assert!(!result.status.success() && result.stdout.is_empty());
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(message.contains(error), "{message}");
    };
    rejected(
        diagnose(
            &policy,
            Some(&sha256(b"wrong policy")),
            "1795238130",
            "1795238400",
        ),
        "approved digest",
    );
    rejected(
        diagnose(&policy, None, "1795238129", "1795238400"),
        "window exceeds",
    );
    rejected(
        diagnose(&policy, None, "1795238130", "1795238399"),
        "approved diagnostic target, time",
    );
    let mut wrong_basis = policy.clone();
    wrong_basis["release_basis"] = json!("bundle");
    rejected(
        diagnose(&wrong_basis, None, "1795238130", "1795238400"),
        "release evidence format",
    );

    let mut partial = manifest.clone();
    partial.as_object_mut().unwrap().remove("change");
    let partial_bytes = serde_json::to_vec(&partial).unwrap();
    fs::write(manifest_path, &partial_bytes).unwrap();
    rejected(
        diagnose(&policy, None, "1795238130", "1795238400"),
        "manifest does not match",
    );
    let mut partial_policy = policy;
    partial_policy["release_digest"] = json!(sha256(&partial_bytes));
    let diagnosed = diagnose(&partial_policy, None, "1795238130", "1795238400");
    assert!(diagnosed.status.success());
    let diagnosed: Value = serde_json::from_slice(&diagnosed.stdout).unwrap();
    assert_eq!(
        diagnosed["release"]["gap"],
        json!({"stage":"change","reason":"missing"})
    );
    assert_eq!(diagnosed["logs"], report["logs"]);
    fs::write(manifest_path, manifest_bytes).unwrap();
}

#[test]
fn bitbucket_change_import_joins_reviewed_head_and_merge_result_to_the_build() {
    use craft_factory_evidence::sha256;

    let fixture = Fixture::new();
    let bundle_path = fixture.root.join("release-evidence.json");
    let pr_path = fixture.root.join("pull-request.json");
    let approval_path = fixture.root.join("approval.json");
    let mut bundle: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    let result_commit = bundle["builds"][0]["source_commit"]
        .as_str()
        .unwrap()
        .to_owned();
    bundle["changes"] = json!([]);
    fs::write(&bundle_path, serde_json::to_vec(&bundle).unwrap()).unwrap();
    let source_head = "c".repeat(40);
    let repo_uuid = "a9876543-4321-1234-abcd-fedcba987654";
    let approval = json!({
        "schema_version": 1,
        "workspace": "example",
        "repository": "sample-api",
        "repository_uuid": repo_uuid,
        "destination_branch": "main",
        "pull_request_id": 16,
        "approved_source_commit": source_head,
        "result_commit": result_commit,
        "work_item": "SAMPLE-42",
        "intent_reference": "fixture:decision/42",
        "intent_revision": "7",
        "approval_reference": "fixture:approval/42",
        "approved_at": 1795237990,
        "merge_reference": "fixture:merge-events/16",
        "merged_at": 1795238000,
    });
    let pr = json!({
        "type": "pullrequest", "id": 16, "state": "MERGED",
        "source": {"commit": {"hash": source_head}},
        "destination": {
            "branch": {"name": "main"},
            "repository": {"full_name": "example/sample-api", "uuid": format!("{{{repo_uuid}}}")}
        },
        "merge_commit": {"hash": result_commit},
        "created_on": "2026-11-21T05:10:00Z",
        "updated_on": "2026-11-21T05:13:30Z",
        "description": "private description outside the release trace",
    });
    let import =
        |snapshot: &Value, record: &Value, supplied_digest: Option<&str>, observed_at: &str| {
            let bytes = serde_json::to_vec(record).unwrap();
            let digest = sha256(&bytes);
            fs::write(&approval_path, &bytes).unwrap();
            fs::write(&pr_path, serde_json::to_vec(snapshot).unwrap()).unwrap();
            fixture
                .command()
                .arg("import-bitbucket-change")
                .arg(&bundle_path)
                .arg("--pull-request-snapshot")
                .arg(&pr_path)
                .arg("--approval")
                .arg(&approval_path)
                .args([
                    "--approval-digest",
                    supplied_digest.unwrap_or(&digest),
                    "--observed-at",
                    observed_at,
                ])
                .output()
                .unwrap()
        };
    let output = import(&pr, &approval, None, "1795238030");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let enriched: Value = serde_json::from_slice(&output.stdout).unwrap();
    let change = &enriched["changes"][0];
    assert_eq!(
        change["pull_request"],
        "https://bitbucket.org/example/sample-api/pull-requests/16"
    );
    assert_eq!(change["approved_source_commit"], source_head);
    assert_eq!(change["result_commit"], result_commit);
    assert_eq!(change["merge_reference"], "fixture:merge-events/16");
    assert_eq!(change["merged_at"], 1795238000);
    assert_eq!(
        change["observation"]["reference"],
        format!(
            "bitbucket:example/sample-api/pull-requests/16#{}#approval:{}",
            sha256(&fs::read(&pr_path).unwrap()),
            sha256(&fs::read(&approval_path).unwrap())
        )
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private description"));
    fs::write(&bundle_path, &output.stdout).unwrap();
    let duplicate = import(&pr, &approval, None, "1795238030");
    assert!(duplicate.status.success());
    assert_eq!(duplicate.stdout, output.stdout);
    let trace = fixture
        .command()
        .arg("trace-release")
        .arg(&bundle_path)
        .args([
            "--service",
            "sample-api",
            "--environment",
            "staging",
            "--region",
            "au-east",
            "--resource",
            "sample-api-blue",
            "--as-of",
            "1795238400",
            "--max-age-seconds",
            "120",
        ])
        .output()
        .unwrap();
    assert!(trace.status.success());
    let report: Value = serde_json::from_slice(&trace.stdout).unwrap();
    assert!(report["gap"].is_null());
    assert_eq!(
        report["change"]["approval_reference"],
        "fixture:approval/42"
    );

    let rejected = |result: Output, error: &str| {
        assert!(!result.status.success() && result.stdout.is_empty());
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(message.contains(error), "{message}");
    };
    for (path, value, error) in [
        ("/state", json!("OPEN"), "approved merged pull request"),
        ("/id", json!(17), "approved merged pull request"),
        (
            "/destination/repository/uuid",
            json!("b9876543-4321-1234-abcd-fedcba987654"),
            "approved merged pull request",
        ),
        (
            "/destination/branch/name",
            json!("other"),
            "approved merged pull request",
        ),
        (
            "/source/commit/hash",
            json!("d".repeat(40)),
            "approved source head",
        ),
        ("/merge_commit", Value::Null, "exact merge result"),
        (
            "/merge_commit/hash",
            json!("d".repeat(40)),
            "exact merge result",
        ),
        (
            "/updated_on",
            json!("2026-11-21T05:10:10Z"),
            "inconsistent times",
        ),
    ] {
        let mut changed = pr.clone();
        *changed.pointer_mut(path).unwrap() = value;
        rejected(import(&changed, &approval, None, "1795238030"), error);
    }
    rejected(
        import(
            &pr,
            &approval,
            Some(&sha256(b"other approval")),
            "1795238030",
        ),
        "approved digest",
    );
    let mut late = approval.clone();
    late["approved_at"] = json!(1795238010);
    rejected(
        import(&pr, &late, None, "1795238030"),
        "invalid approved Bitbucket change",
    );
    let mut conflict = approval;
    conflict["intent_revision"] = json!("8");
    rejected(
        import(&pr, &conflict, None, "1795238030"),
        "conflicting evidence",
    );
}

#[test]
fn signed_build_provenance_binds_the_artifact_repository_and_commit() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use craft_factory_evidence::sha256;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    let fixture = Fixture::new();
    let release_path = fixture.root.join("release.json");
    let policy_path = fixture.root.join("policy.json");
    let envelope_path = fixture.root.join("attestation.json");
    let mut evidence: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    let repository = evidence["builds"][0]["repository"]
        .as_str()
        .unwrap()
        .to_owned();
    let commit = evidence["builds"][0]["source_commit"]
        .as_str()
        .unwrap()
        .to_owned();
    let artifact = evidence["builds"][0]["artifact_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    evidence["builds"] = json!([]);
    fs::write(&release_path, serde_json::to_vec(&evidence).unwrap()).unwrap();

    let signer = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
    let other_signer = Ed25519KeyPair::from_seed_unchecked(&[8; 32]).unwrap();
    let policy = json!({
        "schema_version": 1,
        "repository": repository,
        "source_uri": "git+https://bitbucket.org/example/sample-api@refs/heads/main",
        "artifact_name": "registry.example/sample-api",
        "builder_id": "https://ci.example/builders/release",
        "build_type": "https://ci.example/build-types/oci",
        "external_parameters": {"branch": "main", "workflow": "release"},
        "public_key_base64": URL_SAFE_NO_PAD.encode(signer.public_key().as_ref()),
        "approval_reference": "fixture:build-policy/1"
    });
    let policy_bytes = serde_json::to_vec(&policy).unwrap();
    fs::write(&policy_path, &policy_bytes).unwrap();
    let digest = sha256(&policy_bytes);

    let statement = json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{"name": policy["artifact_name"], "digest": {"sha256": &artifact[7..]}}],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": policy["build_type"],
                "externalParameters": policy["external_parameters"],
                "resolvedDependencies": [{"uri": policy["source_uri"], "digest": {"gitCommit": commit}}]
            },
            "runDetails": {
                "builder": {"id": policy["builder_id"]},
                "metadata": {
                    "invocationId": "fixture-build-78",
                    "startedOn": "2026-11-21T05:14:00Z",
                    "finishedOn": "2026-11-21T05:14:20Z"
                }
            }
        }
    });
    let envelope = |payload: &Value, key: &Ed25519KeyPair| {
        let body = serde_json::to_vec(payload).unwrap();
        let mut message =
            format!("DSSEv1 28 application/vnd.in-toto+json {} ", body.len()).into_bytes();
        message.extend_from_slice(&body);
        json!({
            "payloadType": "application/vnd.in-toto+json",
            "payload": URL_SAFE_NO_PAD.encode(body),
            "signatures": [{"keyid":"an unsigned hint", "sig":URL_SAFE_NO_PAD.encode(key.sign(&message).as_ref())}]
        })
    };
    let import = |signed: &Value, policy_digest: &str, artifact_digest: &str, time: &str| {
        fs::write(&envelope_path, serde_json::to_vec(signed).unwrap()).unwrap();
        fixture
            .command()
            .arg("import-build-provenance")
            .arg(&release_path)
            .arg("--attestation")
            .arg(&envelope_path)
            .arg("--policy")
            .arg(&policy_path)
            .args([
                "--policy-digest",
                policy_digest,
                "--artifact-digest",
                artifact_digest,
                "--observed-at",
                time,
            ])
            .output()
            .unwrap()
    };
    let signed = envelope(&statement, &signer);
    let output = import(&signed, &digest, &artifact, "1795238090");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let assembled: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(assembled["builds"][0]["source_commit"], commit);
    assert_eq!(assembled["builds"][0]["repository"], repository);
    assert_eq!(assembled["builds"][0]["built_at"], 1795238060);
    assert_eq!(
        assembled["builds"][0]["observation"]["source"],
        "verified-slsa-dsse"
    );
    assert_eq!(
        assembled["builds"][0]["observation"]["reference"],
        format!(
            "dsse:fixture-build-78#{}#policy:{digest}",
            sha256(&fs::read(&envelope_path).unwrap())
        )
    );
    fs::write(&release_path, &output.stdout).unwrap();
    let repeated = import(&signed, &digest, &artifact, "1795238090");
    assert!(repeated.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&repeated.stdout).unwrap(),
        assembled
    );
    let trace = fixture
        .command()
        .arg("trace-release")
        .arg(&release_path)
        .args([
            "--service",
            "sample-api",
            "--environment",
            "staging",
            "--region",
            "au-east",
            "--resource",
            "sample-api-blue",
            "--as-of",
            "1795238400",
            "--max-age-seconds",
            "120",
        ])
        .output()
        .unwrap();
    assert!(trace.status.success());
    let report: Value = serde_json::from_slice(&trace.stdout).unwrap();
    assert!(report["gap"].is_null());
    assert_eq!(
        report["change"]["approval_reference"],
        "fixture:approvals/42"
    );

    let rejected = |result: Output, error: &str| {
        assert!(!result.status.success() && result.stdout.is_empty());
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(message.contains(error), "{message}");
    };
    rejected(
        import(
            &signed,
            &sha256(b"unapproved policy"),
            &artifact,
            "1795238090",
        ),
        "approved digest",
    );
    rejected(
        import(
            &signed,
            &digest,
            &format!("sha256:{}", "e".repeat(64)),
            "1795238090",
        ),
        "requested artifact",
    );
    rejected(
        import(&signed, &digest, &artifact, "1795238040"),
        "inconsistent build timing",
    );
    rejected(
        import(
            &envelope(&statement, &other_signer),
            &digest,
            &artifact,
            "1795238090",
        ),
        "approved Ed25519",
    );
    let mut other_input = statement.clone();
    other_input["predicate"]["buildDefinition"]["externalParameters"]["branch"] =
        json!("unapproved");
    let mut tampered = signed.clone();
    tampered["payload"] = envelope(&other_input, &signer)["payload"].clone();
    rejected(
        import(&tampered, &digest, &artifact, "1795238090"),
        "approved Ed25519",
    );
    rejected(
        import(
            &envelope(&other_input, &signer),
            &digest,
            &artifact,
            "1795238090",
        ),
        "build inputs",
    );
    let mut moved_source = statement.clone();
    moved_source["predicate"]["buildDefinition"]["resolvedDependencies"][0]["uri"] =
        json!("git+https://example.com/another-repo");
    rejected(
        import(
            &envelope(&moved_source, &signer),
            &digest,
            &artifact,
            "1795238090",
        ),
        "approved source URI",
    );
    let mut changed_commit = statement;
    changed_commit["predicate"]["buildDefinition"]["resolvedDependencies"][0]["digest"]["gitCommit"] =
        json!("c".repeat(40));
    rejected(
        import(
            &envelope(&changed_commit, &signer),
            &digest,
            &artifact,
            "1795238090",
        ),
        "conflicting build evidence",
    );
}

#[test]
fn kubernetes_deployment_import_requires_current_owner_chain_and_matching_template() {
    let fixture = Fixture::new();
    let bundle_path = fixture.root.join("evidence.json");
    let pod_path = fixture.root.join("pod.json");
    let rs_path = fixture.root.join("replica-set.json");
    let mut bundle: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    let artifact = bundle["builds"][0]["artifact_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let repository = bundle["builds"][0]["repository"]
        .as_str()
        .unwrap()
        .to_owned();
    bundle["inventory"] = json!([]);
    bundle["deployments"] = json!([]);
    fs::write(&bundle_path, serde_json::to_vec(&bundle).unwrap()).unwrap();

    let annotations = json!({
        "craft.factory/deployment-id": "sample-rollout-17",
        "craft.factory/config-revision": "sample-config-42",
        "craft.factory/source-repository": repository,
    });
    let labels = json!({"pod-template-hash": "84fd4"});
    let containers = json!([{"name": "api", "image": format!("registry.example/api@{artifact}")}]);
    let pod = json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": {
            "namespace": "sample-stage", "name": "api-84fd4-dz", "uid": "pod-uid",
            "creationTimestamp": "2026-11-21T05:15:00Z",
            "annotations": annotations, "labels": labels,
            "ownerReferences": [{"apiVersion": "apps/v1", "kind": "ReplicaSet", "controller": true,
                                 "name": "api-84fd4", "uid": "rs-uid"}]
        },
        "spec": {"containers": containers},
        "status": {"phase": "Running", "containerStatuses": [
            {"name": "api", "imageID": artifact, "state": {"running": {"startedAt": "2026-11-21T05:16:30Z"}}}
        ]}
    });
    let replica_set = json!({
        "apiVersion": "apps/v1", "kind": "ReplicaSet",
        "metadata": {
            "namespace": "sample-stage", "name": "api-84fd4", "uid": "rs-uid",
            "creationTimestamp": "2026-11-21T05:14:55Z",
            "ownerReferences": [{"apiVersion": "apps/v1", "kind": "Deployment", "controller": true,
                                 "name": "api", "uid": "deployment-uid"}]
        },
        "spec": {"template": {
            "metadata": {"annotations": annotations, "labels": labels},
            "spec": {"containers": containers}
        }}
    });
    fs::write(&pod_path, serde_json::to_vec(&pod).unwrap()).unwrap();
    let import = |snapshot: &Value, rs_observed_at: &str| {
        fs::write(&rs_path, serde_json::to_vec(snapshot).unwrap()).unwrap();
        fixture
            .command()
            .arg("import-kubernetes-deployment")
            .arg(&bundle_path)
            .arg("--pod-snapshot")
            .arg(&pod_path)
            .arg("--replica-set-snapshot")
            .arg(&rs_path)
            .args([
                "--cluster",
                "staging-one",
                "--namespace",
                "sample-stage",
                "--pod",
                "api-84fd4-dz",
                "--pod-uid",
                "pod-uid",
                "--container",
                "api",
                "--service",
                "sample-api",
                "--environment",
                "staging",
                "--region",
                "au-east",
                "--resource",
                "sample-api-blue",
                "--owner",
                "Sample API team",
                "--observed-at",
                "1795238340",
                "--deployment-name",
                "api",
                "--deployment-uid",
                "deployment-uid",
                "--repository",
                &repository,
                "--replica-set-observed-at",
                rs_observed_at,
            ])
            .output()
            .unwrap()
    };
    let output = import(&replica_set, "1795238320");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let enriched: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(enriched["deployments"][0]["deployed_at"], 1795238100);
    assert_eq!(
        enriched["deployments"][0]["version"],
        enriched["inventory"][0]["running"]
    );
    assert_eq!(
        enriched["deployments"][0]["observation"]["source"],
        "kubernetes-replicaset"
    );
    assert_eq!(
        enriched["deployments"][0]["observation"]["reference"],
        format!(
            "kubernetes:staging-one/sample-stage/api-84fd4/rs-uid#{}",
            craft_factory_evidence::sha256(&fs::read(&rs_path).unwrap())
        )
    );
    fs::write(&bundle_path, &output.stdout).unwrap();
    let duplicate = import(&replica_set, "1795238320");
    assert!(duplicate.status.success());
    assert_eq!(duplicate.stdout, output.stdout);
    let trace = fixture
        .command()
        .arg("trace-release")
        .arg(&bundle_path)
        .args([
            "--service",
            "sample-api",
            "--environment",
            "staging",
            "--region",
            "au-east",
            "--resource",
            "sample-api-blue",
            "--as-of",
            "1795238400",
            "--max-age-seconds",
            "120",
        ])
        .output()
        .unwrap();
    assert!(trace.status.success());
    let report: Value = serde_json::from_slice(&trace.stdout).unwrap();
    assert!(report["gap"].is_null());
    assert_eq!(
        report["change"]["approval_reference"],
        "fixture:approvals/42"
    );

    for (path, value, error) in [
        (
            "/metadata/namespace",
            json!("unrelated"),
            "controlling ReplicaSet",
        ),
        (
            "/metadata/uid",
            json!("same-name-replacement"),
            "controlling ReplicaSet",
        ),
        (
            "/metadata/ownerReferences/0/uid",
            json!("unrelated"),
            "expected Deployment",
        ),
        (
            "/metadata/ownerReferences/0/controller",
            json!(false),
            "missing a controller",
        ),
        (
            "/spec/template/metadata/labels/pod-template-hash",
            json!("unrelated"),
            "template hash",
        ),
        (
            "/spec/template/metadata/annotations/craft.factory~1source-repository",
            json!("bitbucket:other/repo"),
            "approved repository",
        ),
        (
            "/spec/template/metadata/annotations/craft.factory~1config-revision",
            json!("unrelated"),
            "approved repository",
        ),
        (
            "/spec/template/spec/containers/0/image",
            json!("registry.example/api:latest"),
            "approved repository",
        ),
        (
            "/metadata/creationTimestamp",
            json!("2026-11-21T05:15:02Z"),
            "inconsistent creation",
        ),
    ] {
        let mut changed = replica_set.clone();
        *changed.pointer_mut(path).unwrap() = value;
        let rejected = import(&changed, "1795238320");
        assert!(!rejected.status.success() && rejected.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(error),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }
    let stale = import(&replica_set, "1795238099");
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("inconsistent creation"));
}

#[test]
fn kubernetes_snapshot_binds_one_running_container_to_release_evidence() {
    let fixture = Fixture::new();
    let input = fixture.root.join("release-evidence.json");
    let snapshot = fixture.root.join("pod.json");
    let mut evidence: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    evidence["inventory"] = json!([]);
    let artifact = evidence["builds"][0]["artifact_digest"].as_str().unwrap();
    let pod = json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": {
            "namespace": "sample-stage", "name": "api-7c6-94k", "uid": "fixture-pod-uid",
            "deletionTimestamp": null,
            "annotations": {
                "craft.factory/deployment-id": "sample-rollout-17",
                "craft.factory/config-revision": "sample-config-42",
                "fixture.io/private": "omitted snapshot metadata"
            },
        },
        "spec": {"containers": [
            {"name": "api", "image": format!("registry.example/api@{artifact}"),
             "env": [{"name": "PRIVATE", "value": "omitted container environment"}]},
            {"name": "sidecar", "image": "registry.example/sidecar:latest"},
        ]},
        "status": {"phase": "Running", "containerStatuses": [
            {"name": "sidecar", "ready": true, "imageID": "unrelated"},
            {"name": "api", "ready": false,
             "imageID": format!("docker-pullable://registry.example/api@{artifact}"),
             "containerID": "containerd://fixture-instance",
             "state": {"running": {"startedAt": "2026-11-21T05:16:30Z"}}},
        ]},
    });
    fs::write(&input, serde_json::to_vec(&evidence).unwrap()).unwrap();
    let import = |pod: &Value, capture: &str| {
        fs::write(&snapshot, serde_json::to_vec(pod).unwrap()).unwrap();
        fixture
            .command()
            .arg("import-kubernetes-inventory")
            .arg(&input)
            .arg("--pod-snapshot")
            .arg(&snapshot)
            .args([
                "--cluster",
                "staging-one",
                "--namespace",
                "sample-stage",
                "--pod",
                "api-7c6-94k",
                "--pod-uid",
                "fixture-pod-uid",
                "--container",
                "api",
                "--service",
                "sample-api",
                "--environment",
                "staging",
                "--region",
                "au-east",
                "--resource",
                "sample-api-blue",
                "--owner",
                "Sample API team",
                "--observed-at",
                capture,
            ])
            .output()
            .unwrap()
    };
    let output = import(&pod, "1795238340");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let enriched: Value = serde_json::from_slice(&output.stdout).unwrap();
    let record = &enriched["inventory"][0];
    assert_eq!(record["running"]["artifact_digest"], artifact);
    assert_eq!(record["running_since"], 1795238190);
    assert_eq!(
        record["kubernetes_container"],
        json!({"pod_uid": "fixture-pod-uid", "container_name": "api", "container_id": "fixture-instance"})
    );
    assert_eq!(record["observation"]["source"], "kubernetes-pod");
    assert_eq!(
        record["observation"]["reference"],
        format!(
            "kubernetes:staging-one/sample-stage/api-7c6-94k/fixture-pod-uid/api#{}",
            craft_factory_evidence::sha256(&fs::read(&snapshot).unwrap())
        )
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("omitted"));
    let mut previous_bundle = enriched.clone();
    previous_bundle["inventory"][0]
        .as_object_mut()
        .unwrap()
        .remove("kubernetes_container");
    fs::write(&input, serde_json::to_vec(&previous_bundle).unwrap()).unwrap();
    let upgraded_replay = import(&pod, "1795238340");
    assert!(upgraded_replay.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&upgraded_replay.stdout).unwrap(),
        enriched
    );
    fs::write(&input, &output.stdout).unwrap();
    let replay = import(&pod, "1795238340");
    assert!(replay.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&replay.stdout).unwrap(),
        enriched
    );

    let trace = |as_of: &str| {
        let result = fixture
            .command()
            .arg("trace-release")
            .arg(&input)
            .args([
                "--service",
                "sample-api",
                "--environment",
                "staging",
                "--region",
                "au-east",
                "--resource",
                "sample-api-blue",
                "--as-of",
                as_of,
                "--max-age-seconds",
                "120",
            ])
            .output()
            .unwrap();
        assert!(result.status.success());
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    };
    let report = trace("1795238400");
    assert!(report["gap"].is_null());
    assert_eq!(
        report["change"]["approval_reference"],
        "fixture:approvals/42"
    );
    assert_eq!(
        trace("1795238600")["gap"],
        json!({"stage":"inventory","reason":"stale"})
    );
    let mut contradictory = enriched.clone();
    contradictory["deployments"][0]["deployed_at"] = json!(1795238200);
    fs::write(&input, serde_json::to_vec(&contradictory).unwrap()).unwrap();
    assert_eq!(
        trace("1795238400")["gap"],
        json!({"stage":"deployment","reason":"inconsistent_time"})
    );
    fs::write(&input, &output.stdout).unwrap();

    for (field, value, expected) in [
        (
            "/metadata/uid",
            json!("same-name-replacement"),
            "expected Pod",
        ),
        ("/metadata/namespace", json!("other-stage"), "expected Pod"),
        (
            "/spec/containers/0/image",
            json!("registry.example/api:latest"),
            "not pinned",
        ),
        (
            "/metadata/deletionTimestamp",
            json!("2026-11-21T03:57:00Z"),
            "non-terminating",
        ),
        (
            "/status/containerStatuses/1/imageID",
            json!(format!("sha256:{}", "c".repeat(64))),
            "digest does not match",
        ),
        (
            "/status/containerStatuses/1/state/running/startedAt",
            json!("2026-11-22T03:55:30Z"),
            "started after",
        ),
        (
            "/metadata/annotations/craft.factory~1config-revision",
            json!("different config"),
            "conflicting inventory",
        ),
    ] {
        let mut altered = pod.clone();
        *altered.pointer_mut(field).unwrap() = value;
        let rejected = import(&altered, "1795238340");
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }
}

#[test]
fn release_trace_uses_observed_artifact_and_reports_evidence_gaps() {
    let fixture = Fixture::new();
    let path = fixture.root.join("release-evidence.json");
    let baseline: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    let trace = |bundle: &Value| -> Value {
        fs::write(&path, serde_json::to_vec(bundle).unwrap()).unwrap();
        let output = fixture
            .command()
            .args(["trace-release"])
            .arg(&path)
            .args([
                "--service",
                "sample-api",
                "--environment",
                "staging",
                "--region",
                "au-east",
                "--resource",
                "sample-api-blue",
                "--as-of",
                "1795238400",
                "--max-age-seconds",
                "120",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let result = trace(&baseline);
    assert_eq!(result["inventory"]["owner"], "Sample API team");
    assert_eq!(
        result["deployment"]["version"]["deployment_id"],
        "sample-rollout-17"
    );
    assert_eq!(
        result["build"]["source_commit"],
        baseline["builds"][0]["source_commit"]
    );
    assert_eq!(result["change"]["intent_revision"], "7");
    assert_eq!(
        result["change"]["approval_reference"],
        "fixture:approvals/42"
    );
    assert!(result["gap"].is_null());
    assert!(!fixture.root.join("state.db").exists());

    let mut future = baseline["inventory"][0].clone();
    future["observation"]["observed_at"] = json!(1795238401);
    future["running"]["deployment_id"] = json!("sample-rollout-18");
    let mut bundle = baseline.clone();
    bundle["inventory"].as_array_mut().unwrap().push(future);
    assert!(trace(&bundle)["gap"].is_null());

    let mut bundle = baseline.clone();
    bundle["inventory"][0]["observation"]["observed_at"] = json!(1795238200);
    assert_eq!(
        trace(&bundle)["gap"],
        json!({"stage":"inventory","reason":"stale"})
    );

    let mut bundle = baseline.clone();
    bundle["deployments"][0]["target"]["region"] = json!("different-region");
    assert_eq!(
        trace(&bundle)["gap"],
        json!({"stage":"deployment","reason":"missing"})
    );
    bundle["deployments"][0]["target"]["region"] = json!("au-east");
    bundle["deployments"][0]["version"]["config_revision"] = json!("different-config");
    let mismatch = trace(&bundle);
    assert_eq!(
        mismatch["gap"],
        json!({"stage":"deployment","reason":"mismatch"})
    );
    assert!(mismatch["build"].is_null());

    let mut bundle = baseline.clone();
    let mut competing = baseline["builds"][0].clone();
    competing["source_commit"] = json!("c".repeat(40));
    bundle["builds"].as_array_mut().unwrap().push(competing);
    assert_eq!(
        trace(&bundle)["gap"],
        json!({"stage":"build","reason":"ambiguous"})
    );

    let mut bundle = baseline.clone();
    bundle["changes"][0]["result_commit"] = json!("c".repeat(40));
    assert_eq!(
        trace(&bundle)["gap"],
        json!({"stage":"change","reason":"missing"})
    );
    bundle["changes"][0]["result_commit"] = baseline["changes"][0]["result_commit"].clone();
    bundle["changes"][0]["merged_at"] = json!(1795238300);
    assert_eq!(
        trace(&bundle)["gap"],
        json!({"stage":"change","reason":"inconsistent_time"})
    );
}

#[test]
fn diagnosis_binds_redacted_telemetry_to_the_observed_deployment() {
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new();
    let release_path = fixture.root.join("release.json");
    let telemetry_path = fixture.root.join("telemetry.json");
    let policy_path = fixture.root.join("policy.json");
    let release: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    let telemetry: Value =
        serde_json::from_str(include_str!("../examples/diagnostic-telemetry.json")).unwrap();

    let write = |path: &Path, value: &Value| {
        let bytes = serde_json::to_vec(value).unwrap();
        fs::write(path, &bytes).unwrap();
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("sha256:{digest}")
    };
    let authorize = |release: &Value, telemetry: &Value| {
        json!({
            "schema_version": 1, "target": telemetry["target"],
            "approval_reference": "fixture:diagnostic-approval/17",
            "release_digest": write(&release_path, release),
            "telemetry_digest": write(&telemetry_path, telemetry),
            "authorized_window": {"from":1795238300,"to":1795238400},
            "max_window_seconds": 100, "release_max_age_seconds": 120, "max_records": 10,
            "log_codes":["UPSTREAM_TIMEOUT"],
            "metrics":[{"name":"request_latency_p95","unit":"ms"}]
        })
    };
    let run = |policy: &Value, from: &str, to: &str| {
        fs::write(&policy_path, serde_json::to_vec(policy).unwrap()).unwrap();
        fixture
            .command()
            .arg("diagnose-release")
            .arg(&policy_path)
            .arg("--release")
            .arg(&release_path)
            .arg("--telemetry")
            .arg(&telemetry_path)
            .args(["--from", from, "--to", to])
            .output()
            .unwrap()
    };
    let full_window = |policy: &Value| run(policy, "1795238300", "1795238400");
    let success = |output: Output| -> Value {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let denied = |output: Output, reason: &str| {
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(reason), "{error}");
        assert!(!error.contains("private-raw-log-sentinel"));
    };

    let policy = authorize(&release, &telemetry);
    let report = success(full_window(&policy));
    assert_eq!(report["release"]["change"]["work_item"], "SAMPLE-42");
    assert_eq!(report["telemetry_correlated"], true);
    assert_eq!(report["logs"].as_array().unwrap().len(), 1);
    assert_eq!(report["logs"][0]["severity"], "warn");
    assert_eq!(report["metrics"][0]["value"], 425.0);
    assert_eq!(
        report["approval_reference"],
        "fixture:diagnostic-approval/17"
    );
    assert!(!fixture.root.join("state.db").exists());
    assert!(
        success(run(&policy, "1795238355", "1795238400"))["logs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    denied(run(&policy, "1795238299", "1795238400"), "approved scope");
    let mut limited = policy.clone();
    limited["max_records"] = json!(1);
    denied(full_window(&limited), "approved record limit");

    let mut injected = telemetry.clone();
    injected["logs"][0]["message"] = json!("private-raw-log-sentinel");
    write(&telemetry_path, &injected);
    denied(full_window(&policy), "approved digest");
    denied(
        full_window(&authorize(&release, &injected)),
        "structured redacted telemetry",
    );
    let mut unapproved = telemetry.clone();
    unapproved["metrics"][0]["metric"]["name"] = json!("internal_user_count");
    denied(
        full_window(&authorize(&release, &unapproved)),
        "unapproved event, metric",
    );
    let mut other_environment = authorize(&release, &telemetry);
    other_environment["target"]["environment"] = json!("prod");
    denied(
        full_window(&other_environment),
        "differs from the approved scope",
    );

    let mut changed_release = release.clone();
    changed_release["deployments"][0]["version"]["config_revision"] = json!("different-config");
    let mismatch = success(full_window(&authorize(&changed_release, &telemetry)));
    assert_eq!(mismatch["release"]["gap"]["reason"], "mismatch");
    assert_eq!(mismatch["telemetry_correlated"], false);
    assert!(mismatch["logs"].as_array().unwrap().is_empty());
    let mut missing_change = release.clone();
    missing_change["changes"] = json!([]);
    let partial = success(full_window(&authorize(&missing_change, &telemetry)));
    assert_eq!(partial["release"]["gap"]["stage"], "change");
    assert_eq!(partial["telemetry_correlated"], true);
}

#[test]
fn remediation_stays_open_until_every_target_runs_the_approved_change() {
    let fixture = Fixture::new();
    let policy_path = fixture.root.join("remediation-policy.json");
    let evidence_path = fixture.root.join("release-evidence.json");
    let mut policy: Value =
        serde_json::from_str(include_str!("../examples/remediation-policy.json")).unwrap();
    let mut evidence: Value =
        serde_json::from_str(include_str!("../examples/release-evidence.json")).unwrap();
    let mut second = policy["required_targets"][0].clone();
    second["target"]["region"] = json!("au-west");
    second["target"]["resource"] = json!("sample-api-west");
    second["config_revision"] = json!("sample-config-43");
    policy["required_targets"]
        .as_array_mut()
        .unwrap()
        .push(second.clone());

    let verify = |policy: &Value, evidence: &Value, require_complete| {
        fs::write(&policy_path, serde_json::to_vec(policy).unwrap()).unwrap();
        fs::write(&evidence_path, serde_json::to_vec(evidence).unwrap()).unwrap();
        let mut command = fixture.command();
        command
            .arg("verify-remediation")
            .arg(&policy_path)
            .arg("--release")
            .arg(&evidence_path)
            .args(["--as-of", "1795238400"]);
        if require_complete {
            command.arg("--require-complete");
        }
        command.output().unwrap()
    };
    let report = |out: &Output| -> Value {
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("no report: {}", String::from_utf8_lossy(&out.stderr)))
    };

    let missing = verify(&policy, &evidence, true);
    assert!(!missing.status.success());
    assert_eq!(report(&missing)["targets"][1]["status"], "evidence_gap");
    assert_eq!(report(&missing)["remaining_targets"], 1);

    let mut inventory = evidence["inventory"][0].clone();
    let mut deployment = evidence["deployments"][0].clone();
    inventory["target"] = second["target"].clone();
    deployment["target"] = second["target"].clone();
    inventory["running"]["deployment_id"] = json!("sample-west-rollout");
    inventory["running"]["artifact_digest"] = json!(format!("sha256:{}", "e".repeat(64)));
    deployment["version"] = inventory["running"].clone();
    evidence["inventory"]
        .as_array_mut()
        .unwrap()
        .push(inventory);
    evidence["deployments"]
        .as_array_mut()
        .unwrap()
        .push(deployment);
    let partial = verify(&policy, &evidence, false);
    assert!(partial.status.success());
    let partial = report(&partial);
    assert_eq!(partial["targets"][0]["status"], "verified");
    assert_eq!(partial["targets"][1]["status"], "artifact_pending");
    assert_eq!(partial["verified_targets"], 1);
    assert_eq!(partial["rollout_verified"], false);

    for (section, version_key) in [("inventory", "running"), ("deployments", "version")] {
        evidence[section][1][version_key]["artifact_digest"] = policy["artifact_digest"].clone();
    }
    assert_eq!(
        report(&verify(&policy, &evidence, true))["targets"][1]["status"],
        "configuration_pending"
    );
    for (section, version_key) in [("inventory", "running"), ("deployments", "version")] {
        evidence[section][1][version_key]["config_revision"] = second["config_revision"].clone();
    }
    let complete = verify(&policy, &evidence, true);
    assert!(
        complete.status.success(),
        "{}",
        String::from_utf8_lossy(&complete.stderr)
    );
    let completed = report(&complete);
    assert_eq!(completed["rollout_verified"], true);
    assert_eq!(completed["verified_targets"], 2);
    assert_eq!(completed["remaining_targets"], 0);
    assert!(
        completed["evidence_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    evidence["changes"][0]["approval_reference"] = json!("fixture:approval-for-different-intent");
    let wrong_approval = verify(&policy, &evidence, true);
    assert!(!wrong_approval.status.success());
    assert_eq!(
        report(&wrong_approval)["targets"][1]["status"],
        "change_mismatch"
    );
    policy["required_targets"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let duplicate = verify(&policy, &evidence, false);
    assert!(!duplicate.status.success() && duplicate.stdout.is_empty());
    assert!(!fixture.root.join("state.db").exists());
}

#[test]
fn dependency_intake_binds_renovate_and_osv_to_the_committed_npm_update() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new();
    let integrity = format!("sha512-{}", STANDARD.encode([0u8; 64]));
    let manifest = |version| json!({"name":"fixture-npm","version":"1.0.0","dependencies":{"sample-lib":version}});
    let lockfile = |version| {
        json!({
            "name":"fixture-npm", "version":"1.0.0", "lockfileVersion":3,
            "packages":{
                "":{"name":"fixture-npm","version":"1.0.0","dependencies":{"sample-lib":version}},
                "node_modules/sample-lib":{"version":version,"resolved":"https://registry.npmjs.org/sample-lib/example.tgz","integrity":integrity},
                "node_modules/sample-lib/node_modules/sample-leaf":{"version":"1.0.0"}
            }
        })
    };
    let write_pair = |version| {
        fs::write(
            fixture.repo.join("package.json"),
            serde_json::to_vec(&manifest(version)).unwrap(),
        )
        .unwrap();
        let lock = serde_json::to_vec(&lockfile(version)).unwrap();
        fs::write(fixture.repo.join("package-lock.json"), &lock).unwrap();
        git(&fixture.repo, &["add", "package.json", "package-lock.json"]);
        git(&fixture.repo, &["commit", "-qm", version]);
        (git(&fixture.repo, &["rev-parse", "HEAD"]), lock)
    };
    let (base, baseline_lock) = write_pair("1.0.1");
    let (head, candidate_lock) = write_pair("1.0.2");
    let osv = |version, findings: Value, leaf_findings: Value| {
        json!({
            "results":[{
                "source":{"path":"fixture/package-lock.json","type":"lockfile"},
                "packages":[
                    {"package":{"name":"sample-lib","version":version,"ecosystem":"npm"},"vulnerabilities":findings},
                    {"package":{"name":"sample-leaf","version":"1.0.0","ecosystem":"npm"},"vulnerabilities":leaf_findings}
                ]
            }]
        })
    };
    let digest = |bytes: &[u8]| {
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("sha256:{hex}")
    };
    let write_scan = |file: &Path, commit: &str, lock: &[u8], report: &Value| {
        let bytes = serde_json::to_vec(report).unwrap();
        fs::write(file, &bytes).unwrap();
        json!({
            "issuer":"fixture-ci", "reference":format!("fixture:osv/{commit}"), "commit":commit,
            "lockfile_digest":digest(lock), "report_digest":digest(&bytes), "completed":true
        })
    };
    let input_path = fixture.root.join("dependency-update.json");
    let before_path = fixture.root.join("before-osv.json");
    let after_path = fixture.root.join("after-osv.json");
    let template = fixture.order(
        "SAMPLE-UPDATE",
        "Verify application behavior after the proposed update",
    );
    let mut work_order: Value = serde_json::from_slice(&fs::read(template).unwrap()).unwrap();
    work_order["base"] = json!(base);
    work_order["skill"]["revision"] = json!(base);
    work_order["scope"] = json!(["src", "package.json", "package-lock.json"]);
    let mut input = json!({
        "schema_version":1,"repository":"team/service","updater_actor":"fixture-renovate","scanner_issuer":"fixture-ci",
        "proposal":{"repository":"team/service","pull_request":"fixture:pr/42","actor":"fixture-renovate",
            "base":base,"head":head,"package":"sample-lib","from_version":"1.0.1","to_version":"1.0.2"},
        "baseline_scan":write_scan(&before_path, &base, &baseline_lock, &osv("1.0.1", json!([{"id":"FIXTURE-BASELINE"}]), json!([]))),
        "candidate_scan":write_scan(&after_path, &head, &candidate_lock, &osv("1.0.2", json!([]), json!([]))),
        "work_order":work_order
    });
    let run = |input: &Value| {
        fs::write(&input_path, serde_json::to_vec(input).unwrap()).unwrap();
        fixture
            .command()
            .arg("prepare-dependency-update")
            .arg(&input_path)
            .arg("--baseline-osv")
            .arg(&before_path)
            .arg("--candidate-osv")
            .arg(&after_path)
            .output()
            .unwrap()
    };
    let prepared = run(&input);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(prepared["status"], "ready_for_supervised_verification");
    assert_eq!(prepared["proposal"]["head"], head);
    assert_eq!(prepared["update_class"], "patch");
    assert_eq!(prepared["work_order"]["base"], base);
    assert_eq!(prepared["work_order"]["require_native_checks"], true);
    assert!(
        prepared["work_order"]["context"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pin| pin["location"] == "fixture:pr/42" && pin["revision"] == head)
    );
    assert!(!fixture.root.join("state.db").exists());
    let order_file = fixture.root.join("prepared-order.json");
    fs::write(
        &order_file,
        serde_json::to_vec(&prepared["work_order"]).unwrap(),
    )
    .unwrap();
    let delegated = fixture
        .command()
        .arg("delegate")
        .arg(&order_file)
        .output()
        .unwrap();
    assert!(
        delegated.status.success(),
        "{}",
        String::from_utf8_lossy(&delegated.stderr)
    );
    let submitted = fixture
        .command()
        .args([
            "submit",
            "SAMPLE-UPDATE",
            "--commit",
            &head,
            "--note",
            "Observed updater candidate",
        ])
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    verify_dependency_compatibility_fixture(
        &fixture,
        &input,
        [&input_path, &before_path, &after_path],
        &prepared,
    );

    let mut wrong_receipt = input.clone();
    wrong_receipt["candidate_scan"]["commit"] = json!(base);
    let rejected = run(&wrong_receipt);
    assert!(
        !rejected.status.success()
            && String::from_utf8_lossy(&rejected.stderr).contains("OSV scan receipt")
    );
    input["candidate_scan"] = write_scan(
        &after_path,
        &head,
        &candidate_lock,
        &osv("1.0.2", json!([]), json!([{"id":"FIXTURE-NEW-FINDING"}])),
    );
    let blocked = run(&input);
    assert!(blocked.status.success());
    let blocked: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(blocked["status"], "review_required");
    assert_eq!(blocked["introduced_findings"], 1);
    assert!(blocked["work_order"].is_null());

    let mut incomplete = osv("1.0.2", json!([]), json!([]));
    incomplete["results"][0]["packages"]
        .as_array_mut()
        .unwrap()
        .pop();
    input["candidate_scan"] = write_scan(&after_path, &head, &candidate_lock, &incomplete);
    let rejected = run(&input);
    assert!(
        !rejected.status.success()
            && String::from_utf8_lossy(&rejected.stderr).contains("coverage differs")
    );
    input["proposal"]["head"] = json!(fixture.commit("src/hello", "unrelated edit"));
    let rejected = run(&input);
    assert!(
        !rejected.status.success()
            && String::from_utf8_lossy(&rejected.stderr).contains("must change only")
    );
}

fn verify_dependency_compatibility_fixture(
    fixture: &Fixture,
    intake: &Value,
    [input_path, baseline_scan, candidate_scan]: [&Path; 3],
    prepared: &Value,
) {
    let policy_path = fixture.root.join("compatibility-policy.json");
    let baseline_path = fixture.root.join("baseline-run.json");
    let candidate_path = fixture.root.join("candidate-run.json");
    let follow_up = fixture.commit(
        "src/hello",
        "preserves ordinary behavior and fixes the fixture regression",
    );
    let harness = format!("sha256:{}", "3".repeat(64));
    let mut policy = json!({
        "schema_version":1, "intake_digest":prepared["input_digest"],
        "candidate_head":follow_up, "verifier_issuer":"fixture-native-ci",
        "harness_digest":harness, "approval_reference":"fixture:approved-compatibility/3",
        "checks":[{"id":"preserves-old-behavior","expectation":"preserve"},
                  {"id":"regression","expectation":"regression"}]
    });
    let receipt = |mode: &str, commit: &Value, regression: &str| {
        json!({
            "schema_version":1, "repository":intake["repository"], "commit":commit,
            "issuer":"fixture-native-ci", "reference":format!("fixture:compatibility/{mode}"),
            "harness_digest":harness, "lockfile_digest":intake[format!("{mode}_scan")]["lockfile_digest"],
            "completed":true,
            "cases":[{"id":"preserves-old-behavior","outcome":"passed"},{"id":"regression","outcome":regression}]
        })
    };
    let baseline = receipt("baseline", &intake["proposal"]["base"], "assertion_failed");
    let candidate = receipt("candidate", &json!(follow_up), "passed");
    let run = |policy: &Value, before: &Value, after: &Value| {
        for (path, data) in [
            (&policy_path, policy),
            (&baseline_path, before),
            (&candidate_path, after),
        ] {
            fs::write(path, serde_json::to_vec(data).unwrap()).unwrap();
        }
        fixture
            .command()
            .arg("verify-dependency-compatibility")
            .arg(input_path)
            .arg("--baseline-osv")
            .arg(baseline_scan)
            .arg("--candidate-osv")
            .arg(candidate_scan)
            .arg("--policy")
            .arg(&policy_path)
            .arg("--baseline-run")
            .arg(&baseline_path)
            .arg("--candidate-run")
            .arg(&candidate_path)
            .arg("--require-complete")
            .output()
            .unwrap()
    };
    let verified = run(&policy, &baseline, &candidate);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(report["compatibility_verified"], true);
    assert_eq!(report["updater_head"], intake["proposal"]["head"]);
    assert_eq!(report["candidate"]["commit"], follow_up);
    assert_eq!(report["baseline"]["commit"], intake["proposal"]["base"]);

    let mut bad_baseline = baseline.clone();
    bad_baseline["cases"][1]["outcome"] = json!("error");
    let blocked = run(&policy, &bad_baseline, &candidate);
    assert!(!blocked.status.success());
    let report: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(report["compatibility_verified"], false);
    assert_eq!(report["checks"][1]["matched"], false);
    bad_baseline["cases"][1]["outcome"] = json!("passed");
    assert!(!run(&policy, &bad_baseline, &candidate).status.success());

    let mut wrong_candidate = candidate.clone();
    wrong_candidate["commit"] = intake["proposal"]["head"].clone();
    let blocked = run(&policy, &baseline, &wrong_candidate);
    assert!(!blocked.status.success() && blocked.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("does not match the approved issuer")
    );
    wrong_candidate = candidate.clone();
    wrong_candidate["cases"][1]["id"] = json!("preserves-old-behavior");
    let blocked = run(&policy, &baseline, &wrong_candidate);
    assert!(
        !blocked.status.success()
            && String::from_utf8_lossy(&blocked.stderr).contains("duplicate or unapproved")
    );

    let lock = fixture.repo.join("package-lock.json");
    let mut changed = fs::read_to_string(lock).unwrap();
    changed.push('\n');
    let changed_head = fixture.commit("package-lock.json", &changed);
    policy["candidate_head"] = json!(changed_head);
    let blocked = run(&policy, &baseline, &candidate);
    assert!(
        !blocked.status.success()
            && String::from_utf8_lossy(&blocked.stderr)
                .contains("changed the scanned dependency inputs")
    );
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "craft-factory-{}-{time}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "user.name", "Factory fixture"]);
        git(&repo, &["config", "commit.gpgSign", "false"]);
        fs::create_dir(repo.join("src")).unwrap();
        fs::create_dir_all(repo.join("skill")).unwrap();
        fs::write(repo.join("skill/SKILL.md"), "Pinned skill for fixture").unwrap();
        fs::write(repo.join("src/hello"), "base").unwrap();
        fs::write(repo.join("outside"), "original").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "Base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        Self { root, repo, base }
    }

    fn order(&self, id: &str, intent: &str) -> PathBuf {
        let path = self.root.join(format!("{id}.json"));
        fs::write(
            &path,
            json!({
                "schema_version": 1,
                "id": id,
                "issue": "PILOT-1",
                "owner": "Developer",
                "repository": self.repo,
                "base": self.base,
                "intent": intent,
                "acceptance": ["The fixture file has the requested contents."],
                "scope": ["src"],
                "context": [{"location": "jira:PILOT-1", "revision": "fixture-v1"}],
                "skill": {"location": "repo:skill/SKILL.md", "revision": self.base}
            })
            .to_string(),
        )
        .unwrap();
        path
    }

    fn command(&self) -> Command {
        let launcher = std::env::var_os("CRAFT_FACTORY_TEST_LAUNCHER")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_craft-factory")));
        assert!(launcher.is_absolute() && launcher.is_file());
        let mut cmd = Command::new(launcher);
        cmd.arg("--state").arg(self.root.join("state.db"));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn commit(&self, file: &str, contents: &str) -> String {
        fs::write(self.repo.join(file), contents).unwrap();
        git(&self.repo, &["add", file]);
        git(&self.repo, &["commit", "-qm", "Change"]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn review(&self, head: &str, pr: &str, evidence: &str) -> PathBuf {
        let path = self.root.join("review.json");
        fs::write(
            &path,
            json!({
                "schema_version": 1,
                "draft": {"reference": pr, "head": head},
                "actor": "Independent reviewer fixture",
                "evidence": evidence,
                "note": "Simulated evidence"
            })
            .to_string(),
        )
        .unwrap();
        path
    }

    fn attest(&self, action: &str, head: &str, pr: &str, evidence: &str) -> Output {
        self.command()
            .args([action, "sample"])
            .arg(self.review(head, pr, evidence))
            .output()
            .unwrap()
    }

    fn fixture_executable(&self) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent.rs");
        let executable = self
            .root
            .join(format!("agent{}", std::env::consts::EXE_SUFFIX));
        let build = Command::new("rustc")
            .arg(source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        executable
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn success(result: Output) -> Value {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

fn fails(result: Output, reason: &str) {
    assert!(!result.status.success(), "{result:?}");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains(reason),
        "{result:?}"
    );
}

#[test]
fn intake_is_idempotent_and_candidate_survives_restart() {
    let fixture = Fixture::new();
    let order = fixture.order("sample", "Change the greeting");
    let initial = success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    let repeated = success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    assert_eq!(initial, repeated);
    let commit = fixture.commit("src/hello", "hello");
    let candidate = success(fixture.run(&[
        "submit",
        "sample",
        "--commit",
        &commit,
        "--note",
        "Ready for checks",
    ]));
    assert_eq!(candidate["candidate"], commit);
    assert_eq!(success(fixture.run(&["status", "sample"])), candidate);
    let modified = fixture.order("sample", "Changes approved intent");
    fails(
        fixture
            .command()
            .arg("delegate")
            .arg(modified)
            .output()
            .unwrap(),
        "different inputs",
    );
}

#[test]
fn github_issue_capture_guards_supervised_delegation_and_draft_binding() {
    let fixture = Fixture::new();
    let write_json = |path: &Path, value: &Value| {
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    };
    let gh = fixture.fixture_executable();
    let native_path = fixture.root.join("native issue.json");
    let policy = fixture.root.join("issue-policy.json");
    write_json(
        &policy,
        &json!({"schema_version": 1, "gh": gh, "repository": "team/service", "number": 42, "issue_id": 707}),
    );
    let template = fixture.order("sample", "Change a greeting");
    let mut order: Value = serde_json::from_slice(&fs::read(&template).unwrap()).unwrap();
    order["issue"] = json!("team/service#42");
    order["context"].as_array_mut().unwrap().push(json!({
        "location": "https://github.com/team/service/issues/43", "revision": format!("sha256:{}", "a".repeat(64))
    }));
    write_json(&template, &order);
    let native = json!({"repository_url": "https://api.github.com/repos/team/service",
        "html_url": "https://github.com/team/service/issues/42", "number": 42, "id": 707,
        "state": "open", "title": "Improve greeting", "body": "Private source description",
        "updated_at": "2026-11-01T00:00:00Z", "assignees": [{"id": 92}, {"id": 36}]});
    let command = || {
        let mut launcher = fixture.command();
        launcher
            .env("FACTORY_FIXTURE_GITHUB_ISSUE_JSON", &native_path)
            .env("CRAFT_FACTORY_OPERATOR_TOKEN", "synthetic credential")
            .env("CREDENTIALS_DIRECTORY", &fixture.root);
        launcher
    };
    let capture = || {
        let mut launcher = command();
        launcher
            .arg("capture-github-issue-snapshot")
            .arg(&policy)
            .arg(&template);
        launcher
    };
    write_json(&native_path, &native);
    let preview = success(capture().output().unwrap());
    assert_eq!(preview["source"]["assignees"], json!([36, 92]));
    assert!(!preview.to_string().contains("Private source description"));
    let digest = preview["approval_digest"].as_str().unwrap();
    let prepared_path = fixture.root.join("approved issue");
    let mut changed = native.clone();
    changed["body"] = json!("Edited without changing the timestamp");
    write_json(&native_path, &changed);
    fails(
        capture()
            .args(["--approval-digest", digest, "--destination"])
            .arg(&prepared_path)
            .output()
            .unwrap(),
        "changed after review",
    );
    assert!(!prepared_path.exists());
    changed["pull_request"] = json!({"url": "fixture"});
    write_json(&native_path, &changed);
    fails(capture().output().unwrap(), "pinned issue identity");

    let mut reordered = native.clone();
    reordered["assignees"] = json!([{"id": 36}, {"id": 92}]);
    reordered["updated_at"] = json!("2026-11-01T01:00:00Z");
    write_json(&native_path, &reordered);
    let prepared = success(
        capture()
            .args(["--approval-digest", digest, "--destination"])
            .arg(&prepared_path)
            .output()
            .unwrap(),
    );
    let frozen = prepared["prepared"]["issue_snapshot"].as_str().unwrap();
    let pinned_order = prepared["prepared"]["work_order"].as_str().unwrap();
    assert!(
        !fs::read_to_string(frozen)
            .unwrap()
            .contains("Private source description")
    );
    fails(
        fixture.run(&["delegate", pinned_order]),
        "digest-pinned issue snapshot",
    );
    let admitted = success(fixture.run(&["delegate", pinned_order, "--issue-snapshot", frozen]));
    assert!(
        admitted["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "github_issue_snapshot_verified")
    );
    let check = || {
        command()
            .args(["check-github-issue-snapshot", "sample"])
            .arg(&policy)
            .output()
            .unwrap()
    };
    assert_eq!(success(check())["decision"], "current");
    write_json(&native_path, &changed);
    let attempt = fixture.root.join("blocked attempt");
    fails(
        command()
            .args(["run", "sample", "--destination"])
            .arg(&attempt)
            .arg("--codex")
            .arg(&gh)
            .arg("--codex-home")
            .arg(&fixture.root)
            .arg("--github-issue-policy")
            .arg(&policy)
            .output()
            .unwrap(),
        "pinned issue identity",
    );
    assert!(!attempt.exists());
    write_json(&native_path, &native);

    let head = fixture.commit("src/hello", "hello");
    success(fixture.run(&[
        "submit",
        "sample",
        "--commit",
        &head,
        "--note",
        "Review requested",
    ]));
    let bind = || {
        command()
            .args([
                "bind-draft",
                "sample",
                "--reference",
                "https://github.com/team/service/pull/22",
                "--head",
                &head,
                "--github-issue-policy",
            ])
            .arg(&policy)
            .output()
            .unwrap()
    };
    let mut moved = native.clone();
    moved["repository_url"] = json!("https://api.github.com/repos/team/other");
    write_json(&native_path, &moved);
    fails(check(), "pinned issue identity");
    let mut reassigned = native.clone();
    reassigned["assignees"] = json!([{"id": 14}]);
    write_json(&native_path, &reassigned);
    fails(bind(), "changed since approval");
    assert!(success(fixture.run(&["status", "sample"]))["draft"].is_null());
    let mut closed = native.clone();
    closed["state"] = json!("closed");
    write_json(&native_path, &closed);
    fails(check(), "no longer open");
    write_json(&native_path, &native);
    assert_eq!(success(bind())["draft"]["head"], head);
}

#[test]
fn digest_pinned_jira_snapshot_blocks_drift_before_binding_a_pr() {
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new();
    let order_path = fixture.order("sample", "Change a greeting");
    let mut order: Value = serde_json::from_slice(&fs::read(&order_path).unwrap()).unwrap();
    let url = "https://team.atlassian.net/browse/PILOT-1";
    let snapshot_path = fixture.root.join("jira-snapshot.json");
    let contents = json!({
        "key": "PILOT-1", "url": url, "intent": order["intent"], "acceptance": order["acceptance"]
    })
    .to_string();
    let revision: String = Sha256::digest(contents.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    order["context"] = json!([{"location": url, "revision": format!("sha256:{revision}")}]);
    fs::write(&order_path, order.to_string()).unwrap();
    let delegate = || {
        let mut command = fixture.command();
        command.arg("delegate").arg(&order_path);
        command
    };
    fails(delegate().output().unwrap(), "attach the digest-pinned");
    fs::write(&snapshot_path, &contents).unwrap();
    let pinned = success(
        delegate()
            .arg("--jira-snapshot")
            .arg(&snapshot_path)
            .output()
            .unwrap(),
    );
    assert_eq!(
        pinned["events"].as_array().unwrap().last().unwrap()["kind"],
        "jira_snapshot_verified"
    );
    let head = fixture.commit("src/hello", "hi");
    success(fixture.run(&["submit", "sample", "--commit", &head, "--note", "Candidate"]));
    let bind = || {
        fixture.run(&[
            "bind-draft",
            "sample",
            "--head",
            &head,
            "--reference",
            "https://example.invalid/pr/1",
        ])
    };
    fs::write(
        &snapshot_path,
        contents.replace("Change a greeting", "Expanded intent"),
    )
    .unwrap();
    fails(bind(), "Jira snapshot differs from the approved digest");
    assert!(success(fixture.run(&["status", "sample"]))["draft"].is_null());
    fs::write(&snapshot_path, &contents).unwrap();
    success(bind());
    let pr = "https://example.invalid/pr/1";
    let verified = success(fixture.attest("attest-checks", &head, pr, "independent-check"));
    assert_eq!(verified["review_ready"], true);
    fs::write(
        &snapshot_path,
        contents.replace("Change a greeting", "Expanded intent"),
    )
    .unwrap();
    let drifted = success(fixture.run(&["status", "sample"]));
    assert_eq!(drifted["review_ready"], false);
    assert!(
        drifted["snapshot_problem"]
            .as_str()
            .unwrap()
            .contains("approved digest")
    );
    let brief = fixture.run(&["brief", "sample"]);
    assert!(brief.status.success());
    assert!(
        String::from_utf8(brief.stdout)
            .unwrap()
            .contains("Restore or review the pinned Jira snapshot")
    );
    fails(
        fixture.attest("close", &head, pr, "outcome"),
        "approved digest",
    );
    fs::write(&snapshot_path, &contents).unwrap();
    assert_eq!(
        success(fixture.run(&["status", "sample"]))["review_ready"],
        true
    );
}

#[test]
fn concurrent_delegation_reserves_one_slot_and_cancel_releases_it() {
    let fixture = Fixture::new();
    let first = fixture.order("first", "First issue");
    let second = fixture.order("second", "Second issue");
    let mut processes: Vec<_> = [&first, &second]
        .iter()
        .map(|order| {
            fixture
                .command()
                .args([OsStr::new("delegate"), order.as_os_str()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    let results: Vec<_> = processes
        .iter_mut()
        .map(|child| child.wait().unwrap().success())
        .collect();
    assert_ne!(results[0], results[1]);
    let (winner, other) = if results[0] {
        ("first", second)
    } else {
        ("second", first)
    };
    assert_eq!(
        success(fixture.run(&["cancel", winner, "--reason", "Stopped"]))["state"],
        "cancelled"
    );
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(other)
            .output()
            .unwrap(),
    );
}

#[test]
fn rejects_out_of_scope_change_and_late_submission_after_cancel() {
    let fixture = Fixture::new();
    let order = fixture.order("sample", "Change the greeting");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    let commit = fixture.commit("outside", "scope violation");
    fails(
        fixture.run(&[
            "submit",
            "sample",
            "--commit",
            &commit,
            "--note",
            "Candidate",
        ]),
        "out-of-scope",
    );
    git(&fixture.repo, &["reset", "--hard", &fixture.base]);
    let commit = fixture.commit("src/hello", "hello");
    success(fixture.run(&["cancel", "sample", "--reason", "Stopped"]));
    fails(
        fixture.run(&["submit", "sample", "--commit", &commit, "--note", "Late"]),
        "active delegated",
    );
}

#[test]
fn repair_preserves_pr_and_requires_checks_for_the_replacement_head() {
    let fixture = Fixture::new();
    let order = fixture.order("sample", "Change the greeting");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(order)
            .output()
            .unwrap(),
    );
    let a = fixture.commit("src/hello", "hello");
    let pr = "https://forge.example.invalid/team/repo/pull/42";
    let different_pr = "https://forge.example.invalid/team/repo/pull/43";
    success(fixture.run(&["submit", "sample", "--commit", &a, "--note", "Initial"]));
    fails(
        fixture.attest("attest-checks", &a, pr, "checks-a"),
        "current pull request",
    );
    success(fixture.run(&["bind-draft", "sample", "--reference", pr, "--head", &a]));
    success(fixture.attest("attest-checks", &a, pr, "checks-a"));
    let feedback_file = fixture.review(&a, pr, "feedback-a");
    let mut feedback: Value = serde_json::from_slice(&fs::read(&feedback_file).unwrap()).unwrap();
    feedback["source"] = json!("simulated");
    fs::write(&feedback_file, feedback.to_string()).unwrap();
    let request_changes = || {
        fixture
            .command()
            .args(["request-changes", "sample"])
            .arg(&feedback_file)
            .output()
            .unwrap()
    };
    let request = success(request_changes());
    assert_eq!(request["attestations"][1]["record"]["source"], "simulated");
    assert_eq!(request, success(request_changes()));
    feedback["source"] = json!("operator_observed");
    fs::write(&feedback_file, feedback.to_string()).unwrap();
    fails(request_changes(), "already bound to different inputs");
    git(&fixture.repo, &["switch", "--detach", &fixture.base]);
    let sibling = fixture.commit("src/hello", "Changes from the original base");
    fails(
        fixture.run(&[
            "submit",
            "sample",
            "--commit",
            &sibling,
            "--replaces",
            &a,
            "--note",
            "Wrong history",
        ]),
        "must descend from the expected prior head",
    );
    git(&fixture.repo, &["switch", "--detach", &a]);
    fixture.commit("outside", "Unexpected change in history");
    fixture.commit("outside", "original");
    let reverted = fixture.commit("src/hello", "Net difference looks in scope");
    fails(
        fixture.run(&[
            "submit",
            "sample",
            "--commit",
            &reverted,
            "--replaces",
            &a,
            "--note",
            "Out of scope in history",
        ]),
        "out-of-scope path",
    );
    git(&fixture.repo, &["switch", "--detach", &a]);
    fixture.commit("src/hello", "First repair commit");
    let b = fixture.commit("src/hello", "Hello!");
    fails(
        fixture.run(&[
            "submit",
            "sample",
            "--commit",
            &b,
            "--note",
            "Missing parent",
        ]),
        "expected prior head",
    );
    success(fixture.run(&[
        "submit",
        "sample",
        "--commit",
        &b,
        "--replaces",
        &a,
        "--note",
        "Repaired",
    ]));
    fails(
        fixture.run(&[
            "bind-draft",
            "sample",
            "--reference",
            different_pr,
            "--head",
            &b,
        ]),
        "different pull request",
    );
    success(fixture.run(&["bind-draft", "sample", "--reference", pr, "--head", &b]));
    let destination = fixture.root.join("verifier");
    let prepare_verification = |head| {
        fixture
            .command()
            .args([
                "prepare-verification",
                "sample",
                "--head",
                head,
                "--destination",
            ])
            .arg(&destination)
            .output()
            .unwrap()
    };
    fails(prepare_verification(&a), "not the current candidate");
    fs::write(
        fixture.repo.join("src/hello"),
        "Uncommitted input\nSecond working-tree line\nThird working-tree line\n",
    )
    .unwrap();
    let verification = success(prepare_verification(&b));
    assert_eq!(verification["head"], b);
    let checkout = Path::new(verification["checkout"].as_str().unwrap());
    assert_eq!(git(checkout, &["rev-parse", "HEAD"]), b);
    assert!(git(checkout, &["remote"]).is_empty());
    assert_eq!(
        fs::read_to_string(checkout.join("src/hello")).unwrap(),
        "Hello!"
    );
    let brief = fixture.run(&["brief", "sample"]);
    assert!(brief.status.success());
    let brief = String::from_utf8(brief.stdout).unwrap();
    assert!(brief.contains("includes a simulated change request"));
    assert!(brief.contains(&format!("Revision `{b}` against `{}`", fixture.base)));
    assert!(brief.contains(&format!(
        "Changes since reviewed revision `{a}`\n\n1 file changed · +1 / -1 lines"
    )));
    assert!(brief.contains("### Review feedback\n\nSimulated evidence"));
    assert!(brief.contains("- src/hello (+1 / -1)"));
    assert!(!brief.contains("Uncommitted input"));
    fails(
        fixture.attest("close", &a, pr, "stale-decision"),
        "current pull request",
    );
    let repeated = success(fixture.attest("attest-checks", &a, pr, "checks-a"));
    assert_eq!(repeated["state"], "candidate");
    fails(
        fixture.attest("close", &b, pr, "outcome-b"),
        "current state",
    );
    let verified = success(fixture.attest("attest-checks", &b, pr, "checks-b"));
    assert_eq!(verified["state"], "verification_attested");
    let closed = success(fixture.attest("close", &b, pr, "outcome-b"));
    assert_eq!(closed["state"], "closed");
    assert_eq!(closed["attestations"].as_array().unwrap().len(), 4);
}

#[test]
fn ci_retry_requires_a_content_identical_child_and_new_pr_head_binding() {
    let fixture = Fixture::new();
    let order = fixture.order("sample", "Change the greeting");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(order)
            .output()
            .unwrap(),
    );
    let a = fixture.commit("src/hello", "hello");
    let pr = "https://bitbucket.org/team/service/pull-requests/42";
    success(fixture.run(&["submit", "sample", "--commit", &a, "--note", "Initial"]));
    success(fixture.run(&["bind-draft", "sample", "--reference", pr, "--head", &a]));
    let changed = fixture.commit("src/hello", "different");
    fails(
        fixture.run(&["retry-ci", "sample", "--previous", &a, "--commit", &changed]),
        "preserve the candidate",
    );
    let tree = git(&fixture.repo, &["rev-parse", &format!("{a}^{{tree}}")]);
    let retry = git(
        &fixture.repo,
        &["commit-tree", &tree, "-p", &a, "-m", "Trigger PR checks"],
    );
    let pending =
        success(fixture.run(&["retry-ci", "sample", "--previous", &a, "--commit", &retry]));
    assert_eq!(pending["candidate"], retry);
    assert_eq!(pending["draft"]["head"], a);
    fails(
        fixture.attest("attest-checks", &a, pr, "old-check"),
        "current pull request",
    );
    success(fixture.run(&["bind-draft", "sample", "--reference", pr, "--head", &retry]));
    success(fixture.attest("attest-checks", &retry, pr, "retry-check"));
}

#[test]
fn prepared_checkout_keeps_source_and_publication_remote_separate() {
    let fixture = Fixture::new();
    let order = fixture.order("sample", "Change the greeting");
    let unrelated = fixture.commit("outside", "Separate branch contents");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(order)
            .output()
            .unwrap(),
    );
    let prepare = |destination: &Path| {
        fixture
            .command()
            .args(["prepare", "sample", "--destination"])
            .arg(destination)
            .output()
            .unwrap()
    };
    fails(prepare(&fixture.repo.join("inside")), "outside the source");
    let workspace = fixture.root.join("worker");
    let result = success(prepare(&workspace));
    let checkout = PathBuf::from(result["checkout"].as_str().unwrap());
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), fixture.base);
    assert_eq!(git(&checkout, &["remote"]), "");
    let unrelated_object = Command::new("git")
        .current_dir(&checkout)
        .args(["cat-file", "-e", &unrelated])
        .output()
        .unwrap();
    assert!(!unrelated_object.status.success());
    assert!(!checkout.join(".git/objects/info/alternates").exists());
    fs::write(checkout.join("src/hello"), "worker change").unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("src/hello")).unwrap(),
        "base"
    );
    let occupied = prepare(&workspace);
    assert!(!occupied.status.success());
    assert_eq!(
        fs::read_to_string(checkout.join("src/hello")).unwrap(),
        "worker change"
    );
}

#[test]
fn interactive_handoff_reuses_pinned_skill_and_imports_scoped_repair_history() {
    let fixture = Fixture::new();
    let order = fixture.order("sample", "Change the greeting");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(order)
            .output()
            .unwrap(),
    );
    fs::write(
        fixture.repo.join("skill/SKILL.md"),
        "Unapproved working copy",
    )
    .unwrap();
    let prepare = |dir: &str| {
        let report = success(
            fixture
                .command()
                .args(["prepare", "sample", "--handoff", "--destination"])
                .arg(fixture.root.join(dir))
                .output()
                .unwrap(),
        );
        let bytes = fs::read(report["handoff"].as_str().unwrap()).unwrap();
        assert_eq!(
            report["handoff_digest"],
            craft_factory_evidence::sha256(&bytes)
        );
        let content: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(content["pinned_skill"], "Pinned skill for fixture");
        assert_eq!(content["work"]["allowed_paths"], json!(["src"]));
        assert_eq!(content["work"]["skill_reference"]["revision"], fixture.base);
        assert!(content["work"].get("repository").is_none());
        (PathBuf::from(report["checkout"].as_str().unwrap()), content)
    };
    let commit = |checkout: &Path, path: &str, text: &str| {
        fs::write(checkout.join(path), text).unwrap();
        git(checkout, &["add", path]);
        git(
            checkout,
            &[
                "-c",
                "user.name=Editor fixture",
                "-c",
                "user.email=editor@example.invalid",
                "-c",
                "commit.gpgSign=false",
                "commit",
                "-qm",
                "Editor change",
            ],
        );
        git(checkout, &["rev-parse", "HEAD"])
    };
    let submit = |checkout: &Path, head: &str, previous: Option<&str>| {
        let mut command = fixture.command();
        command
            .args(["submit", "sample", "--commit", head, "--checkout"])
            .arg(checkout)
            .args(["--note", "Fixture editor 1.0 in interactive mode"]);
        if let Some(previous) = previous {
            command.args(["--replaces", previous]);
        }
        command.output().unwrap()
    };
    let (checkout, handoff) = prepare("first interactive checkout");
    #[cfg(windows)]
    assert_eq!(
        git(&checkout, &["config", "--local", "--get", "core.longpaths"]),
        "true"
    );
    assert_eq!(handoff["head"], fixture.base);
    assert!(handoff["replaces"].is_null() && handoff["work"]["repair_feedback"].is_null());
    commit(&checkout, "outside", "Out of scope");
    commit(&checkout, "outside", "original");
    let invalid = commit(&checkout, "src/hello", "Looks in scope in the final diff");
    fails(submit(&checkout, &invalid, None), "out-of-scope path");
    assert!(
        !Command::new("git")
            .current_dir(&fixture.repo)
            .args(["cat-file", "-e", &invalid])
            .status()
            .unwrap()
            .success()
    );
    git(&checkout, &["reset", "--hard", &fixture.base]);
    commit(&checkout, "src/hello", "First approved edit");
    let initial = commit(&checkout, "src/hello", "Second approved edit");
    let recorded = success(submit(&checkout, &initial, None));
    assert_eq!(recorded["candidate"], initial);
    assert_eq!(success(submit(&checkout, &initial, None)), recorded);
    assert_eq!(git(&fixture.repo, &["rev-parse", "HEAD"]), fixture.base);

    let pr = "https://forge.example.invalid/team/repo/pull/42";
    success(fixture.run(&[
        "bind-draft",
        "sample",
        "--reference",
        pr,
        "--head",
        &initial,
    ]));
    success(fixture.attest("attest-checks", &initial, pr, "check-fixture"));
    success(fixture.attest("request-changes", &initial, pr, "feedback-fixture"));
    let (repair, handoff) = prepare("interactive repair");
    assert_eq!(handoff["head"], initial);
    assert_eq!(handoff["replaces"], initial);
    assert_eq!(
        handoff["work"]["repair_feedback"]["evidence"],
        "feedback-fixture"
    );
    assert_eq!(git(&repair, &["rev-parse", "HEAD"]), initial);
    let repaired = commit(&repair, "src/hello", "Reviewed greeting");
    fails(submit(&repair, &repaired, None), "expected prior head");
    let status = success(submit(&repair, &repaired, Some(&initial)));
    assert_eq!(status["candidate"], repaired);
    assert_eq!(status["draft"]["reference"], pr);
    assert_eq!(status["review_ready"], false);
    assert_eq!(
        git(&fixture.repo, &["show", &format!("{repaired}:src/hello")]),
        "Reviewed greeting"
    );
}

#[test]
fn headless_attempt_imports_candidate_and_timeout_leaves_order_recoverable() {
    let fixture = Fixture::new();
    let agent = fixture.fixture_executable();
    let home = fixture.root.join("agent-home");
    fs::create_dir(&home).unwrap();
    let run = |id: &str, workspace: &str, timeout: &str| {
        fixture
            .command()
            .env("FACTORY_TEST_SECRET", "only-in-launcher")
            .args(["run", id, "--destination"])
            .arg(fixture.root.join(workspace))
            .arg("--codex")
            .arg(&agent)
            .arg("--codex-home")
            .arg(&home)
            .args(["--timeout-seconds", timeout])
            .output()
            .unwrap()
    };

    let order = fixture.order("sample", "Change the greeting");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    let receipt = success(run("sample", "worker", "15"));
    let commit = receipt["candidate"].as_str().unwrap();
    assert_eq!(
        git(
            &fixture.repo,
            &[
                "rev-parse",
                &format!("refs/craft-factory/candidates/sample/{commit}")
            ]
        ),
        commit
    );
    assert_eq!(
        git(&fixture.repo, &["show", &format!("{commit}:src/hello")]),
        "changed by fixture agent"
    );
    assert_eq!(git(&fixture.repo, &["rev-parse", "HEAD"]), fixture.base);
    let events = receipt["events"].as_array().unwrap();
    let observed = events
        .iter()
        .find(|event| event["kind"] == "runner_observed")
        .unwrap();
    let usage: Value = serde_json::from_str(observed["note"].as_str().unwrap()).unwrap();
    assert_eq!(usage["input_tokens"], 21);
    success(fixture.run(&["cancel", "sample", "--reason", "End first test attempt"]));

    let malformed = fixture.order("malformed", "Return an invalid final result");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(malformed)
            .output()
            .unwrap(),
    );
    fails(
        run("malformed", "malformed-worker", "15"),
        "valid final candidate or blocker result",
    );
    let rejected = success(fixture.run(&["status", "malformed"]));
    assert_eq!(rejected["state"], "delegated");
    assert!(rejected["candidate"].is_null());
    assert!(
        rejected["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "runner_observed")
    );
    success(fixture.run(&[
        "cancel",
        "malformed",
        "--reason",
        "End malformed test attempt",
    ]));

    let order = fixture.order("slow", "Wait for timeout");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    fails(run("slow", "slow-worker", "1"), "worker timed out");
    let status = success(fixture.run(&["status", "slow"]));
    assert_eq!(status["state"], "delegated");
    assert_eq!(
        status["events"].as_array().unwrap().last().unwrap()["kind"],
        "runner_stopped"
    );
}

#[test]
fn publication_hold_survives_restart_and_forge_observation_refreshes_brief() {
    let fixture = Fixture::new();
    let gh = fixture.fixture_executable();
    let policy = fixture.root.join("policy.json");
    fs::write(
        &policy,
        json!({
            "schema_version": 1, "repository": "team/service", "base_branch": "main",
            "check_definitions": ["scripts"], "required_checks": [{"name":"Checks", "app_id":42}]
        })
        .to_string(),
    )
    .unwrap();
    let order = fixture.order("sample", "Change the greeting");
    let mut required_order: Value = serde_json::from_slice(&fs::read(&order).unwrap()).unwrap();
    required_order["require_native_checks"] = json!(true);
    fs::write(&order, required_order.to_string()).unwrap();
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    let head = fixture.commit("src/hello", "hi");
    let pr = "https://github.com/team/service/pull/42";
    success(fixture.run(&["submit", "sample", "--commit", &head, "--note", "Candidate"]));
    success(fixture.run(&[
        "hold-publication",
        "sample",
        "--head",
        &head,
        "--reason",
        "Publisher did not see a response",
    ]));
    fails(
        fixture.run(&["cancel", "sample", "--reason", "Premature release"]),
        "unresolved publication",
    );
    fails(
        fixture.attest("attest-checks", &head, pr, "check"),
        "unresolved publication",
    );
    let bound = success(fixture.run(&["bind-draft", "sample", "--reference", pr, "--head", &head]));
    assert!(bound["publication_hold"].is_null());
    assert_eq!(bound["native_ci"], "required");
    fails(
        fixture.attest("attest-checks", &head, pr, "operator-only"),
        "native CI must pass",
    );
    assert!(
        bound["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "publication_reconciled")
    );
    let check = |failed: bool, base: &str| {
        let mut command = fixture.command();
        command
            .args(["check-github", "sample"])
            .arg(&policy)
            .arg("--gh")
            .arg(&gh)
            .env("FACTORY_FIXTURE_SHA", &head)
            .env("FACTORY_FIXTURE_BASE", base);
        if failed {
            command.env("FACTORY_FIXTURE_FAILED_CHECK", "1");
        }
        command
    };
    fails(
        check(false, &head).output().unwrap(),
        "base, state, or head differs",
    );
    let observation = success(check(false, &fixture.base).output().unwrap());
    assert_eq!(
        observation["events"].as_array().unwrap().last().unwrap()["kind"],
        "github_checks_observed"
    );
    let brief = String::from_utf8(fixture.run(&["brief", "sample"]).stdout).unwrap();
    assert!(brief.contains("Checks (run 7)"));
    assert!(brief.contains("Independent verification is pending."));
    let reviewed = success(fixture.attest("attest-checks", &head, pr, "native-checked"));
    assert_eq!(reviewed["review_ready"], true);
    let older = check(false, &fixture.base)
        .env("FACTORY_FIXTURE_CHECK_GATE", &fixture.root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !fixture.root.join("waiting").exists() {
        assert!(
            std::time::Instant::now() < until,
            "provider check never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let pending = success(fixture.run(&["status", "sample"]));
    assert_eq!(pending["native_ci"], "pending");
    assert_eq!(pending["review_ready"], false);
    fails(check(true, &fixture.base).output().unwrap(), "unsuccessful");
    fs::write(fixture.root.join("release"), "").unwrap();
    fails(
        older.wait_with_output().unwrap(),
        "newer verification superseded",
    );
    let held = success(fixture.run(&["status", "sample"]));
    assert_eq!(held["native_ci"], "unavailable");
    assert_eq!(held["review_ready"], false);
    fails(
        fixture.attest("close", &head, pr, "attempted-close"),
        "native CI must pass",
    );
    let brief = String::from_utf8(fixture.run(&["brief", "sample"]).stdout).unwrap();
    assert!(brief.contains("Current provider checks are pending or unavailable"));
    assert!(!brief.contains("Checks (run 7)"));
    assert_eq!(
        success(check(false, &fixture.base).output().unwrap())["review_ready"],
        true
    );
}

#[test]
fn bitbucket_observation_uses_saved_profile_and_rejects_incomplete_steps() {
    let fixture = Fixture::new();
    let craft = fixture.fixture_executable();
    let policy = fixture.root.join("bitbucket-policy.json");
    fs::write(&policy, json!({
        "schema_version": 1, "workspace": "team", "repository": "service", "base_branch": "main",
        "craft": craft, "craft_state": fixture.root.join("separate-provider-state"),
        "craft_profile": "saved-profile", "required_steps": ["Verify candidate"], "check_definitions": []
    }).to_string()).unwrap();
    let order = fixture.order("sample", "Change the greeting");
    success(
        fixture
            .command()
            .arg("delegate")
            .arg(&order)
            .output()
            .unwrap(),
    );
    let head = fixture.commit("src/hello", "hi");
    success(fixture.run(&["submit", "sample", "--commit", &head, "--note", "Candidate"]));
    success(fixture.run(&[
        "bind-draft",
        "sample",
        "--reference",
        "https://bitbucket.org/team/service/pull-requests/42",
        "--head",
        &head,
    ]));
    let check = |truncated: bool| {
        let mut request = fixture.command();
        request
            .args(["check-bitbucket", "sample"])
            .arg(&policy)
            .args(["--pipeline", "cafe0000-0000-0000-0000-000000000042"])
            .env("FACTORY_FIXTURE_SHA", &head);
        if truncated {
            request.env("FACTORY_FIXTURE_TRUNCATED", "1");
        }
        request.output().unwrap()
    };
    let original = fs::read(&policy).unwrap();
    let mut protected: Value = serde_json::from_slice(&original).unwrap();
    protected["check_definitions"] = json!(["src/hello"]);
    fs::write(&policy, protected.to_string()).unwrap();
    fails(check(false), "protected check definition");
    assert_eq!(
        success(fixture.run(&["status", "sample"]))["native_ci"],
        "unavailable"
    );
    fs::write(&policy, original).unwrap();
    let observation = success(check(false));
    assert_eq!(
        observation["events"].as_array().unwrap().last().unwrap()["kind"],
        "bitbucket_checks_observed"
    );
    let brief = String::from_utf8(fixture.run(&["brief", "sample"]).stdout).unwrap();
    assert!(brief.contains("Verify candidate"));
    assert!(brief.contains("Run check-bitbucket-twg to verify the live PR target"));
    assert_eq!(observation["native_ci"], "unavailable");
    fails(
        fixture.attest(
            "attest-checks",
            &head,
            "https://bitbucket.org/team/service/pull-requests/42",
            "partial-check",
        ),
        "native CI must pass",
    );
    fails(check(true), "one fresh complete page");
}

#[test]
fn github_failure_pins_the_selected_attempt_through_native_verification() {
    let fixture = Fixture::new();
    let gh = fixture.fixture_executable();
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/team/service.git",
        ],
    );
    let policy = fixture.root.join("github-policy.json");
    fs::write(&policy, json!({
        "schema_version": 1, "repository": "team/service", "base_branch": "main", "check_definitions": [],
        "required_checks": [{"name":"Checks","app_id":42}],
        "reviewer_ids": [77, 88],
        "ci_repair": {"workflow_id":9,"job_name":"Checks"}
    }).to_string()).unwrap();
    let order = fixture.order("sample", "Repair the approved failed job");
    let request = |order: &Path| {
        let mut command = fixture.command();
        command
            .arg("delegate-github-failure")
            .arg(order)
            .arg(&policy)
            .args(["--run", "73", "--attempt", "2", "--gh"])
            .arg(&gh)
            .env("FACTORY_FIXTURE_SHA", &fixture.base)
            .env("FACTORY_FIXTURE_BASE", &fixture.base);
        command
    };
    fails(
        request(&order)
            .env("FACTORY_FIXTURE_RERUN", "1")
            .output()
            .unwrap(),
        "not the current failed push attempt",
    );
    fails(
        request(&order)
            .env("FACTORY_FIXTURE_TRUNCATE_JOBS", "1")
            .output()
            .unwrap(),
        "incomplete job list",
    );
    fails(
        request(&order)
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
        "superseded the selected failure",
    );
    fails(
        request(&order)
            .env("FACTORY_FIXTURE_BRANCH", "0".repeat(40))
            .output()
            .unwrap(),
        "branch has moved",
    );
    let initial = success(request(&order).output().unwrap());
    assert_eq!(initial["work_order"]["require_native_checks"], true);
    assert_eq!(initial["work_order"]["ci_failure"]["job_id"], 21);
    assert_eq!(initial["work_order"]["ci_failure"]["commit"], fixture.base);
    assert_eq!(initial, success(request(&order).output().unwrap()));
    fails(
        request(&fixture.order("duplicate", "Duplicate repair"))
            .output()
            .unwrap(),
        "already delegated as sample",
    );

    let candidate = fixture.commit("src/hello", "repair");
    success(fixture.run(&[
        "submit",
        "sample",
        "--commit",
        &candidate,
        "--note",
        "CI repair",
    ]));
    let pr = "https://github.com/team/service/pull/42";
    fails(
        fixture.run(&[
            "bind-draft",
            "sample",
            "--reference",
            "https://bitbucket.org/team/service/pull-requests/42",
            "--head",
            &candidate,
        ]),
        "repository that failed",
    );
    success(fixture.run(&[
        "bind-draft",
        "sample",
        "--reference",
        pr,
        "--head",
        &candidate,
    ]));
    fails(
        fixture.attest("attest-checks", &candidate, pr, "operator-only"),
        "native CI must pass",
    );
    let check = || {
        let mut command = fixture.command();
        command
            .args(["check-github", "sample"])
            .arg(&policy)
            .arg("--gh")
            .arg(&gh)
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", &candidate);
        command
    };
    fails(
        check().env("FACTORY_FIXTURE_RERUN", "1").output().unwrap(),
        "not the current failed push attempt",
    );
    fails(
        check().env("FACTORY_FIXTURE_NEWER", "1").output().unwrap(),
        "superseded the selected failure",
    );
    success(check().output().unwrap());
    let verified = success(fixture.attest("attest-checks", &candidate, pr, "independent fixture"));
    assert_eq!(verified["state"], "verification_attested");
    let brief = fixture.run(&["brief", "sample"]);
    assert!(brief.status.success());
    assert!(
        String::from_utf8(brief.stdout)
            .unwrap()
            .contains("https://github.com/team/service/actions/runs/73/job/21")
    );
}

#[test]
fn github_failure_cycle_pins_inputs_and_saves_the_last_report() {
    let fixture = Fixture::new();
    let gh = fixture.fixture_executable();
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/team/service.git",
        ],
    );
    let policy = fixture.root.join("github-policy.json");
    let policy_bytes = serde_json::to_vec(&json!({
        "schema_version": 1, "repository": "team/service", "base_branch": "main", "check_definitions": [],
        "required_checks": [{"name":"Checks","app_id":42}], "reviewer_ids": [77, 88],
        "minimum_merge_approvals": 1, "ci_repair": {"workflow_id":9,"job_name":"Checks"}
    })).unwrap();
    fs::write(&policy, &policy_bytes).unwrap();
    let order = fixture.order("pinned-ci", "Repair the approved GitHub job");
    let order_bytes = fs::read(&order).unwrap();
    let order_digest = craft_factory_evidence::sha256(&order_bytes);
    let policy_digest = craft_factory_evidence::sha256(&policy_bytes);
    let home = fixture.root.join("ci-home");
    let attempts = fixture.root.join("ci-attempts");
    let report = fixture.root.join("last-report.json");
    let state = fixture.root.join("state.db");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&attempts).unwrap();
    let cycle = |destination: &Path| {
        let mut command = fixture.command();
        command
            .arg("run-ci-cycle")
            .arg(&order)
            .args([
                "--order-digest",
                &order_digest,
                "--policy-digest",
                &policy_digest,
            ])
            .arg("--github-policy")
            .arg(&policy)
            .arg("--gh")
            .arg(&gh)
            .arg("--codex")
            .arg(&gh)
            .arg("--codex-home")
            .arg(&home)
            .arg("--attempt-root")
            .arg(&attempts)
            .arg("--report-file")
            .arg(destination)
            .args(["--timeout-seconds", "15"])
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", &fixture.base);
        command
    };
    fails(
        cycle(&state).output().unwrap(),
        "report must be separate from the state database",
    );
    fs::write(&report, b"unrelated file").unwrap();
    fails(cycle(&report).output().unwrap(), "is not a CI cycle report");
    assert_eq!(fs::read(&report).unwrap(), b"unrelated file");
    fs::remove_file(&report).unwrap();
    fs::write(&order, [order_bytes.as_slice(), b"\n"].concat()).unwrap();
    fails(
        cycle(&report).output().unwrap(),
        "CI work order does not match the approved digest",
    );
    assert!(!state.exists() && !report.exists());
    fs::write(&order, order_bytes).unwrap();

    let shadow = success(cycle(&report).output().unwrap());
    assert_eq!(shadow["poll"]["decision"], "would_admit");
    assert_eq!(
        shadow["input_digests"],
        json!({"work_order":order_digest,"policy":policy_digest})
    );
    assert!(shadow["dispatch"].is_null());
    assert!(shadow["poll"].get("branch").is_none());

    // A state database from before branch observations must retain its receipts.
    rusqlite::Connection::open(&state)
        .unwrap()
        .execute_batch("DROP TABLE ci_poll_branch_observations")
        .unwrap();

    let head = fixture.commit("src/existing-change", "Approved branch advanced");
    let follow = |branch_head: &str| {
        let mut command = cycle(&report);
        command
            .arg("--follow-branch")
            .env("FACTORY_FIXTURE_BASE", branch_head)
            .env("FACTORY_FIXTURE_SHA", branch_head);
        command
    };
    let unknown = success(follow("invalid").arg("--admit").output().unwrap());
    assert_eq!(unknown["poll"]["decision"], "blocked");
    assert_eq!(
        unknown["poll"]["branch"],
        json!({"name":"main","anchor":fixture.base,"observed_head":null})
    );
    let missing = "d".repeat(40);
    let absent = success(follow(&missing).arg("--admit").output().unwrap());
    assert_eq!(absent["poll"]["decision"], "blocked");
    assert!(
        absent["poll"]["reason"]
            .as_str()
            .unwrap()
            .contains("unavailable in the checkout")
    );
    assert_eq!(absent["poll"]["branch"]["observed_head"], missing);
    let foreign = git(
        &fixture.repo,
        &[
            "commit-tree",
            &format!("{head}^{{tree}}"),
            "-m",
            "Unrelated history",
        ],
    );
    let rewritten = success(follow(&foreign).arg("--admit").output().unwrap());
    assert_eq!(rewritten["poll"]["decision"], "blocked");
    assert!(
        rewritten["poll"]["reason"]
            .as_str()
            .unwrap()
            .contains("must descend")
    );
    let moved = success(
        follow(&head)
            .arg("--admit")
            .env(
                "FACTORY_FIXTURE_BRANCH_TIP_MARKER",
                fixture.root.join("branch-observed"),
            )
            .env("FACTORY_FIXTURE_BRANCH_AFTER", &foreign)
            .output()
            .unwrap(),
    );
    assert_eq!(moved["poll"]["decision"], "blocked");
    assert!(
        moved["poll"]["reason"]
            .as_str()
            .unwrap()
            .contains("GitHub branch has moved")
    );
    assert!(moved["dispatch"].is_null());

    let active = success(
        follow(&head)
            .arg("--admit")
            .env("FACTORY_FIXTURE_REPLACE_CI_POLICY", &policy)
            .output()
            .unwrap(),
    );
    assert_eq!(active["dispatch"]["decision"], "candidate_ready");
    assert_eq!(
        active["poll"]["branch"],
        json!({"name":"main","anchor":fixture.base,"observed_head":head})
    );
    assert_eq!(active["poll"]["ci_failure"]["commit"], head);
    let candidate = active["dispatch"]["candidate"].as_str().unwrap();
    assert_eq!(
        git(&fixture.repo, &["rev-parse", &format!("{candidate}^")]),
        head
    );
    assert_eq!(active["kind"], "ci_cycle_report");
    let last = fs::read(&report).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&last).unwrap(), active);
    assert_eq!(
        fs::read(&policy).unwrap(),
        b"policy changed after the cycle read it"
    );
    fails(
        cycle(&report).arg("--admit").output().unwrap(),
        "CI policy does not match the approved digest",
    );
    assert_eq!(fs::read(&report).unwrap(), last);

    let history = success(fixture.run(&["ci-poll-history", "--limit", "10"]));
    let history = history.as_array().unwrap();
    let polls = [&active, &moved, &rewritten, &absent, &unknown, &shadow];
    assert_eq!(history.len(), polls.len());
    for (record, cycle) in history.iter().zip(polls) {
        let mut recorded_poll = record.clone();
        assert!(
            recorded_poll
                .as_object_mut()
                .unwrap()
                .remove("observed_at")
                .is_some()
        );
        assert_eq!(recorded_poll, cycle["poll"]);
    }
}

#[test]
fn github_failure_poll_reuses_pause_admission_limit_and_receipts() {
    let fixture = Fixture::new();
    let gh = fixture.fixture_executable();
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/team/service.git",
        ],
    );
    let policy = fixture.root.join("github-policy.json");
    fs::write(&policy, json!({
        "schema_version": 1, "repository": "team/service", "base_branch": "main", "check_definitions": [],
        "required_checks": [{"name":"Checks","app_id":42}],
        "reviewer_ids": [77, 88], "minimum_merge_approvals": 1,
        "ci_repair": {"workflow_id":9,"job_name":"Checks"}
    }).to_string()).unwrap();
    let order = fixture.order("watched", "Repair the approved GitHub job");
    let poll = || {
        let mut command = fixture.command();
        command
            .arg("poll-github-failure")
            .arg(&order)
            .arg(&policy)
            .arg("--gh")
            .arg(&gh)
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", &fixture.base);
        command
    };
    let home = fixture.root.join("ci-codex-home");
    let work = fixture.root.join("ci-attempts");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&work).unwrap();
    let repair_command = |cycle: bool, timeout: &str| {
        let mut command = fixture.command();
        if cycle {
            command.arg("run-ci-cycle").arg(&order);
        } else {
            command.arg("run-ci-repair");
        }
        command
            .arg("--github-policy")
            .arg(&policy)
            .arg("--gh")
            .arg(&gh)
            .arg("--codex")
            .arg(&gh)
            .arg("--codex-home")
            .arg(&home)
            .arg("--attempt-root")
            .arg(&work)
            .args(["--timeout-seconds", timeout])
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", &fixture.base);
        command
    };
    let dispatch = |timeout: &str| repair_command(false, timeout);
    let standard_run = |id: &str| {
        let mut command = fixture.command();
        command
            .args(["run", id, "--destination"])
            .arg(work.join("unguarded"))
            .arg("--codex")
            .arg(&gh)
            .arg("--codex-home")
            .arg(&home);
        command
    };
    for flag in ["FACTORY_FIXTURE_NEWER", "FACTORY_FIXTURE_OTHER_JOB"] {
        let ignored = success(poll().arg("--admit").env(flag, "1").output().unwrap());
        assert_eq!(ignored["decision"], "no_eligible_failure");
        assert_eq!(ignored["repository"], "github.com/team/service");
    }
    let incomplete = success(
        poll()
            .arg("--admit")
            .env("FACTORY_FIXTURE_TRUNCATE_WORKFLOWS", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(incomplete["decision"], "blocked");
    assert!(
        incomplete["reason"]
            .as_str()
            .unwrap()
            .contains("incomplete workflow history")
    );
    let shadow = success(poll().output().unwrap());
    assert_eq!(shadow["decision"], "would_admit");
    let id = shadow["work_order_id"].as_str().unwrap();
    fails(fixture.run(&["status", id]), "unknown work order");
    assert_eq!(
        success(dispatch("15").output().unwrap())["decision"],
        "idle"
    );

    success(fixture.run(&["set-ci-intake", "pause"]));
    assert_eq!(
        success(poll().arg("--admit").output().unwrap())["decision"],
        "paused"
    );
    success(fixture.run(&["set-ci-intake", "resume"]));
    let start = || {
        poll()
            .arg("--admit")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let (first, second) = (start(), start());
    let mut observed = [
        success(first.wait_with_output().unwrap()),
        success(second.wait_with_output().unwrap()),
    ];
    observed.sort_by_key(|receipt| receipt["decision"].as_str().unwrap().to_owned());
    assert_eq!(observed[0]["decision"], "admitted");
    assert_eq!(observed[1]["decision"], "already_admitted");
    assert!(
        observed
            .iter()
            .all(|receipt| receipt["work_order_id"] == id)
    );
    assert_eq!(success(fixture.run(&["status", id]))["state"], "delegated");
    fails(standard_run(id).output().unwrap(), "run-ci-repair");
    let premature = success(dispatch("15").args(["--retry", id]).output().unwrap());
    assert_eq!(premature["decision"], "blocked");
    assert!(
        premature["reason"]
            .as_str()
            .unwrap()
            .contains("not waiting for a retry")
    );
    let shadow = success(repair_command(true, "15").output().unwrap());
    assert_eq!(shadow["poll"]["decision"], "already_admitted");
    assert_eq!(shadow["poll"]["mode"], "shadow");
    assert!(shadow["dispatch"].is_null());
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
    fs::remove_dir(&work).unwrap();
    let misconfigured = success(repair_command(true, "15").arg("--admit").output().unwrap());
    assert_eq!(misconfigured["poll"]["decision"], "already_admitted");
    assert_eq!(misconfigured["dispatch"]["decision"], "blocked");
    assert!(
        misconfigured["dispatch"]["reason"]
            .as_str()
            .unwrap()
            .contains("attempt root")
    );
    fs::create_dir(&work).unwrap();
    success(fixture.run(&["set-ci-intake", "pause"]));
    fails(standard_run(id).output().unwrap(), "run-ci-repair");
    assert_eq!(
        success(dispatch("15").output().unwrap())["decision"],
        "paused"
    );
    success(fixture.run(&["set-ci-intake", "resume"]));
    let obsolete = success(
        dispatch("15")
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(obsolete["decision"], "blocked");
    assert!(obsolete["reason"].as_str().unwrap().contains("superseded"));
    let mismatched = success(
        repair_command(true, "15")
            .arg("--admit")
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(mismatched["poll"]["decision"], "no_eligible_failure");
    assert!(mismatched["dispatch"].is_null());
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
    let start_dispatch = || {
        repair_command(true, "15")
            .arg("--admit")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let (first, second) = (start_dispatch(), start_dispatch());
    let outcomes = [
        success(first.wait_with_output().unwrap()),
        success(second.wait_with_output().unwrap()),
    ];
    assert!(
        outcomes
            .iter()
            .all(|cycle| cycle["poll"]["decision"] == "already_admitted")
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome["dispatch"]["decision"] == "candidate_ready")
            .count(),
        1,
        "{outcomes:?}"
    );
    let stored = success(fixture.run(&["status", id]));
    assert_eq!(stored["state"], "candidate");
    let candidate = stored["candidate"].as_str().unwrap();
    assert_eq!(
        git(&fixture.repo, &["show", &format!("{candidate}:src/hello")]),
        "changed by fixture agent"
    );
    assert!(stored["draft"].is_null());
    assert_eq!(
        stored["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "runner_started")
            .count(),
        1
    );
    assert_eq!(
        success(dispatch("15").output().unwrap())["decision"],
        "idle"
    );
    verify_scheduled_github_candidate(&fixture, id, candidate, &policy, &gh);
    let check_shadow = |protection: &str| {
        let mut command = fixture.command();
        command
            .args(["shadow-merge-github", id])
            .arg(&policy)
            .arg("--gh")
            .arg(&gh)
            .args([
                "--approved-policy-digest",
                &craft_factory_evidence::sha256(&fs::read(&policy).unwrap()),
                "--approved-protection-digest",
                protection,
            ])
            .env("FACTORY_FIXTURE_GITHUB_SHADOW", "1")
            .env("FACTORY_FIXTURE_SHA", candidate)
            .env("FACTORY_FIXTURE_BASE", &fixture.base);
        command
    };
    let preview = success(
        check_shadow(&format!("sha256:{}", "0".repeat(64)))
            .output()
            .unwrap(),
    );
    assert_eq!(preview["shadow_eligible"], false);
    let protection = preview["observed_protection_digest"].as_str().unwrap();
    let eligible = success(
        check_shadow(protection)
            .arg("--require-eligible")
            .output()
            .unwrap(),
    );
    assert_eq!(eligible["shadow_eligible"], true);
    assert_eq!(eligible["head"], candidate);
    assert_eq!(
        eligible["evidence"]["approvals"][0]["actor"],
        "github:user:88"
    );
    for (flag, value) in [
        ("FACTORY_FIXTURE_PROTECTION_DRIFT", "1"),
        ("FACTORY_FIXTURE_APPROVAL_STALE", "1"),
        ("FACTORY_FIXTURE_MERGE_UNKNOWN", "1"),
        ("FACTORY_FIXTURE_BRANCH", candidate),
    ] {
        let blocked = success(check_shadow(protection).env(flag, value).output().unwrap());
        assert_eq!(blocked["shadow_eligible"], false, "{flag}: {blocked}");
        assert!(blocked["evidence"].is_null());
    }
    let active = |pin: &str, mode: &str| {
        success(
            check_shadow(pin)
                .env("FACTORY_FIXTURE_ACTIVE_RULES", mode)
                .output()
                .unwrap(),
        )
    };
    let with_rules = active(protection, "supported");
    assert_eq!(with_rules["active_branch_rule_count"], 3);
    assert_eq!(with_rules["shadow_eligible"], false);
    let rules_pin = with_rules["observed_protection_digest"].as_str().unwrap();
    assert_ne!(rules_pin, protection);
    assert_eq!(active(rules_pin, "reordered")["shadow_eligible"], true);

    for mode in ["merge_queue", "wrong_app"] {
        let preview = active(rules_pin, mode);
        let pin = preview["observed_protection_digest"].as_str().unwrap();
        let blocked = active(pin, mode);
        assert_eq!(blocked["shadow_eligible"], false);
        assert_eq!(
            blocked["blocking_reasons"][0],
            "GitHub has an active branch rule outside the supported shadow checks"
        );
    }
    let incomplete = active(rules_pin, "limit");
    assert_eq!(incomplete["shadow_eligible"], false);
    assert!(incomplete["observed_protection_digest"].is_null());

    let policy_bytes = fs::read(&policy).unwrap();
    let mut queue_policy: Value = serde_json::from_slice(&policy_bytes).unwrap();
    queue_policy["require_merge_queue"] = json!(true);
    fs::write(&policy, serde_json::to_vec(&queue_policy).unwrap()).unwrap();
    let queue_preview = active(protection, "queue");
    let queue_pin = queue_preview["observed_protection_digest"]
        .as_str()
        .unwrap();
    let ready = active(queue_pin, "queue_reordered");
    assert_eq!(ready["shadow_eligible"], true, "{ready}");
    assert_eq!(ready["merge_queue_required"], true);
    assert_eq!(
        ready["evidence"]["merge_queue"],
        json!({
            "pull_request_node_id": "PR_fixture_42",
            "merge_queue_ruleset_id": 56,
            "active_ruleset_ids": [56, 78],
        })
    );
    for mode in ["supported", "queue_headgreen", "queue_duplicate"] {
        let preview = active(queue_pin, mode);
        let pin = preview["observed_protection_digest"].as_str().unwrap();
        assert_eq!(
            active(pin, mode)["blocking_codes"],
            json!(["github_protection_policy"]),
            "{mode}"
        );
    }
    let check_queue = |key: &str, value: &str| {
        success(
            check_shadow(queue_pin)
                .env("FACTORY_FIXTURE_ACTIVE_RULES", "queue")
                .env(key, value)
                .output()
                .unwrap(),
        )
    };
    for mode in ["present", "hidden"] {
        assert_eq!(
            check_queue("FACTORY_FIXTURE_QUEUE_BYPASS", mode)["blocking_codes"],
            json!(["github_queue_rulesets_unavailable"])
        );
    }
    for mode in ["disabled", "queued", "moved", "auto_merge", "stacked"] {
        assert_eq!(
            check_queue("FACTORY_FIXTURE_QUEUE_PR", mode)["blocking_codes"],
            json!(["github_queue_pr_unavailable"])
        );
    }
    let marker = work.join("queue-read-marker");
    assert_eq!(
        check_queue(
            "FACTORY_FIXTURE_QUEUE_READ_MARKER",
            marker.to_str().unwrap()
        )["blocking_codes"],
        json!(["github_evidence_changed"])
    );
    fs::write(&policy, policy_bytes).unwrap();

    import_github_change_request(&fixture, id, candidate, &policy, &gh);
    let repaired = success(
        fixture
            .command()
            .args(["run", id, "--destination"])
            .arg(work.join("repair"))
            .arg("--codex")
            .arg(&gh)
            .arg("--codex-home")
            .arg(&home)
            .args(["--timeout-seconds", "15"])
            .output()
            .unwrap(),
    );
    let replacement = repaired["candidate"].as_str().unwrap();
    assert_ne!(replacement, candidate);
    assert_eq!(
        repaired["draft"]["reference"],
        "https://github.com/team/service/pull/42"
    );
    assert_eq!(repaired["draft"]["head"], candidate);
    assert_eq!(
        git(
            &fixture.repo,
            &["show", &format!("{replacement}:src/hello")]
        ),
        "repaired from reviewed revision"
    );
    assert_eq!(repaired["review_ready"], false);
    verify_scheduled_github_candidate(&fixture, id, replacement, &policy, &gh);
    record_scheduled_github_merge(&fixture, id, replacement, &policy, &gh);

    let capped = success(
        poll()
            .arg("--admit")
            .env("FACTORY_FIXTURE_RERUN", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(capped["decision"], "budget_exhausted");
    assert_eq!(capped["ci_failure"]["run_attempt"], 3);
    fails(
        fixture.run(&["status", capped["work_order_id"].as_str().unwrap()]),
        "unknown work order",
    );
    let history = success(fixture.run(&["ci-poll-history", "--limit", "1"]));
    assert_eq!(history[0]["decision"], "budget_exhausted");
    assert_eq!(history[0]["ci_failure"]["job_id"], 22);

    let later = success(
        poll()
            .args(["--admit", "--daily-limit", "2"])
            .env("FACTORY_FIXTURE_RERUN", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(later["decision"], "admitted");
    let later_id = later["work_order_id"].as_str().unwrap();
    fs::write(home.join("fixture-wait"), "").unwrap();
    let timed = success(
        dispatch("1")
            .env("FACTORY_FIXTURE_RERUN", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(timed["decision"], "blocked");
    assert!(timed["reason"].as_str().unwrap().contains("timed out"));
    let repeat = success(
        dispatch("15")
            .env("FACTORY_FIXTURE_RERUN", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(repeat["decision"], "needs_operator");
    assert_eq!(repeat["work_order_id"], later_id);
    let retry = |requested: &str| {
        let mut command = dispatch("15");
        command
            .args(["--retry", requested])
            .env("FACTORY_FIXTURE_RERUN", "1");
        command
    };
    fails(standard_run(later_id).output().unwrap(), "run-ci-repair");
    let wrong = success(retry(id).output().unwrap());
    assert_eq!(wrong["decision"], "blocked");
    assert_eq!(wrong["work_order_id"], id);
    success(fixture.run(&["set-ci-intake", "pause"]));
    assert_eq!(
        success(retry(later_id).output().unwrap())["decision"],
        "paused"
    );
    success(fixture.run(&["set-ci-intake", "resume"]));
    let stale = success(
        retry(later_id)
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(stale["decision"], "blocked");
    assert!(stale["reason"].as_str().unwrap().contains("superseded"));
    assert!(!work.join("unguarded").exists());
    let stored = success(fixture.run(&["status", later_id]));
    assert_eq!(stored["state"], "delegated");
    assert!(stored["candidate"].is_null());
    assert_eq!(
        stored["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "runner_started")
            .count(),
        1
    );
    assert!(
        stored["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "ci_dispatch_blocked")
    );
    fs::remove_file(home.join("fixture-wait")).unwrap();
    let retried = success(retry(later_id).output().unwrap());
    assert_eq!(retried["decision"], "candidate_ready");
    assert_eq!(retried["work_order_id"], later_id);
    let stored = success(fixture.run(&["status", later_id]));
    assert_eq!(stored["candidate"], retried["candidate"]);
    assert_eq!(
        stored["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "runner_started")
            .count(),
        2
    );
    assert_eq!(
        success(retry(later_id).output().unwrap())["decision"],
        "blocked"
    );
}

fn record_scheduled_github_merge(
    fixture: &Fixture,
    id: &str,
    head: &str,
    policy: &Path,
    gh: &Path,
) {
    let close = |selected_policy: &Path| {
        let mut command = fixture.command();
        command
            .args(["close-github", id])
            .arg(selected_policy)
            .arg("--gh")
            .arg(gh)
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", head);
        command
    };
    fails(close(policy).output().unwrap(), "has not reported a merge");
    fails(
        close(policy)
            .env("FACTORY_FIXTURE_MERGED", "1")
            .env("FACTORY_FIXTURE_SHA", &fixture.base)
            .output()
            .unwrap(),
        "exact reviewed head",
    );
    for flag in [
        "FACTORY_FIXTURE_APPROVAL_SELF",
        "FACTORY_FIXTURE_APPROVAL_STALE",
        "FACTORY_FIXTURE_APPROVAL_DISMISSED",
        "FACTORY_FIXTURE_APPROVAL_LATE",
    ] {
        fails(
            close(policy)
                .env("FACTORY_FIXTURE_MERGED", "1")
                .env(flag, "1")
                .output()
                .unwrap(),
            "lacks the required independent approvals",
        );
    }
    fails(
        close(policy)
            .env("FACTORY_FIXTURE_MERGED", "1")
            .env("FACTORY_FIXTURE_APPROVAL_OBJECTED", "1")
            .output()
            .unwrap(),
        "still had a request for changes",
    );
    let mut weakened: Value = serde_json::from_slice(&fs::read(policy).unwrap()).unwrap();
    weakened["minimum_merge_approvals"] = json!(0);
    let weakened_policy = fixture.root.join("weakened-policy.json");
    fs::write(&weakened_policy, weakened.to_string()).unwrap();
    fails(
        close(&weakened_policy)
            .env("FACTORY_FIXTURE_MERGED", "1")
            .output()
            .unwrap(),
        "approval policy differs",
    );
    fails(
        fixture
            .command()
            .args(["close", id])
            .arg(fixture.review(
                head,
                "https://github.com/team/service/pull/42",
                "fixture:manual-outcome",
            ))
            .output()
            .unwrap(),
        "requires native approval evidence",
    );
    assert_eq!(success(fixture.run(&["status", id]))["review_ready"], true);
    let outcome = success(
        close(policy)
            .env("FACTORY_FIXTURE_MERGED", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(outcome["state"], "closed");
    let native = &outcome["native_merge"];
    assert_eq!(native["provider"], "github");
    assert_eq!(native["result_commit"], "c".repeat(40));
    assert_eq!(native["draft"]["head"], head);
    assert_eq!(native["pinned_base"], fixture.base);
    assert_eq!(native["actor"], "github:user:88");
    assert_eq!(native["approvals"].as_array().unwrap().len(), 1);
    assert_eq!(native["approvals"][0]["actor"], "github:user:88");
    assert_eq!(
        native["approvals"][0]["reference"],
        "https://github.com/team/service/pull/42#pullrequestreview-603"
    );
    assert!(
        native["approvals"][0]["approved_at"].as_i64().unwrap()
            < native["merged_at"].as_i64().unwrap()
    );
    assert_eq!(outcome, success(fixture.run(&["status", id])));
    assert_eq!(
        outcome,
        success(
            close(policy)
                .env("FACTORY_FIXTURE_MERGED", "1")
                .output()
                .unwrap()
        )
    );
    fails(
        close(policy)
            .env("FACTORY_FIXTURE_MERGED", "1")
            .env("FACTORY_FIXTURE_MERGE_TIME", "2026-11-21T04:01:00Z")
            .output()
            .unwrap(),
        "different merge outcome",
    );
}

fn import_github_change_request(fixture: &Fixture, id: &str, head: &str, policy: &Path, gh: &Path) {
    let summary = fixture.root.join("repair-summary.txt");
    let approved_note = "Preserve the reviewed change and say repaired";
    fs::write(&summary, approved_note).unwrap();
    let request = || {
        let mut command = fixture.command();
        command
            .args(["request-changes-github", id])
            .arg(policy)
            .arg("--gh")
            .arg(gh)
            .args(["--review", "601", "--summary"])
            .arg(&summary)
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", head);
        command
    };
    fails(
        request()
            .env("FACTORY_FIXTURE_REVIEW_SELF", "1")
            .output()
            .unwrap(),
        "other than the PR author",
    );
    fails(
        request()
            .env("FACTORY_FIXTURE_REVIEW_HEAD", &fixture.base)
            .output()
            .unwrap(),
        "exact pull request head",
    );
    fails(
        request()
            .env("FACTORY_FIXTURE_REVIEW_DISMISSED", "1")
            .output()
            .unwrap(),
        "active change request",
    );
    fails(
        request()
            .env("FACTORY_FIXTURE_REVIEW_SUPERSEDED", "1")
            .output()
            .unwrap(),
        "superseded this change request",
    );
    assert_eq!(success(fixture.run(&["status", id]))["review_ready"], true);

    let receipt = success(request().output().unwrap());
    assert_eq!(receipt["state"], "repair_requested");
    assert_eq!(receipt["candidate"], head);
    let feedback = receipt["attestations"].as_array().unwrap().last().unwrap();
    assert_eq!(feedback["record"]["actor"], "github:user:88");
    assert_eq!(
        feedback["record"]["evidence"],
        "https://github.com/team/service/pull/42#pullrequestreview-601"
    );
    assert_eq!(feedback["record"]["note"], approved_note);
    assert_eq!(receipt, success(request().output().unwrap()));
}

fn verify_scheduled_github_candidate(
    fixture: &Fixture,
    id: &str,
    head: &str,
    policy: &Path,
    gh: &Path,
) {
    let pr = "https://github.com/team/service/pull/42";
    let revision = &head[..8];
    success(fixture.run(&["bind-draft", id, "--reference", pr, "--head", head]));
    let prepare = |name: &str| {
        fixture
            .command()
            .args([
                "prepare-verification",
                id,
                "--head",
                head,
                "--handoff",
                "--destination",
            ])
            .arg(fixture.root.join(format!("{name}-{revision}")))
            .output()
            .unwrap()
    };
    let checks_with_policy = |selected_policy: &Path| {
        fixture
            .command()
            .args(["check-github", id])
            .arg(selected_policy)
            .arg("--gh")
            .arg(gh)
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_SHA", head)
            .output()
            .unwrap()
    };
    let checks = || success(checks_with_policy(policy));
    let attest = |evidence: &str, packet: Option<&Path>| {
        let mut command = fixture.command();
        command
            .args(["attest-checks", id])
            .arg(fixture.review(head, pr, evidence));
        if let Some(packet) = packet {
            command.arg("--handoff").arg(packet);
        }
        command.output().unwrap()
    };
    fails(prepare("verification-before-ci"), "native CI must pass");
    checks();
    let mut changed: Value = serde_json::from_slice(&fs::read(policy).unwrap()).unwrap();
    changed["minimum_merge_approvals"] = json!(0);
    let changed_policy = fixture.root.join("changed-review-policy.json");
    fs::write(&changed_policy, changed.to_string()).unwrap();
    fails(
        checks_with_policy(&changed_policy),
        "approval policy is pinned for this work order",
    );
    checks();
    fails(
        attest("fixture:unchecked", None),
        "requires an issued verification handoff",
    );

    let prepared = success(prepare("verify-one"));
    let packet = Path::new(prepared["handoff"].as_str().unwrap());
    let mut tampered: Value = serde_json::from_slice(&fs::read(packet).unwrap()).unwrap();
    assert_eq!(tampered["work_order_id"], id);
    assert_eq!(tampered["draft"]["head"], head);
    assert_eq!(tampered["base"], fixture.base);
    assert_eq!(tampered["changed_paths"], json!(["src/hello"]));
    assert_eq!(tampered["native_evidence"]["checks"][0]["app_id"], 42);
    assert!(tampered.get("repository").is_none());
    let checkout = Path::new(prepared["checkout"].as_str().unwrap());
    assert_eq!(git(checkout, &["rev-parse", "HEAD"]), head);
    let altered = fixture.root.join("altered-handoff.json");
    tampered["acceptance"] = json!(["Weakened requirement"]);
    fs::write(&altered, tampered.to_string()).unwrap();
    fails(
        attest("fixture:altered", Some(&altered)),
        "verification handoff was changed",
    );
    let first_evidence = format!("fixture:independent-one-{revision}");
    let verified = success(attest(&first_evidence, Some(packet)));
    assert_eq!(verified["review_ready"], true);
    assert_eq!(verified, success(attest(&first_evidence, Some(packet))));

    let refreshed = checks();
    assert_eq!(refreshed["review_ready"], false);
    fails(
        attest("fixture:obsolete", Some(packet)),
        "no longer matches the current draft and CI",
    );
    fails(
        fixture
            .command()
            .args(["close", id])
            .arg(fixture.review(head, pr, "fixture:outcome"))
            .output()
            .unwrap(),
        "current CI handoff before closing",
    );
    let prepared = success(prepare("verify-two"));
    let refreshed_packet = Path::new(prepared["handoff"].as_str().unwrap());
    let verified = success(attest(
        &format!("fixture:independent-two-{revision}"),
        Some(refreshed_packet),
    ));
    assert_eq!(verified["review_ready"], true);
}

#[test]
fn bitbucket_failure_pins_the_live_attempt_and_rejects_duplicate_admission() {
    let fixture = Fixture::new();
    let twg = fixture.fixture_executable();
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://bitbucket.org/team/service.git",
        ],
    );
    let policy = fixture.root.join("twg-policy.json");
    fs::write(
        &policy,
        json!({
            "schema_version": 1, "twg": twg, "profile": "saved-profile",
            "site": "team.atlassian.net", "workspace": "team", "repository": "service",
            "base_branch": "main", "required_steps": ["Verify candidate"], "check_definitions": []
        })
        .to_string(),
    )
    .unwrap();
    let order = fixture.order(
        "sample",
        "Fix the failing check while preserving its expectations",
    );
    let request = |order: &Path, step: &str| {
        let mut cmd = fixture.command();
        cmd.arg("delegate-bitbucket-failure")
            .arg(order)
            .arg(&policy)
            .args(["--build", "14", "--step", step])
            .env("FACTORY_FIXTURE_SHA", &fixture.base);
        cmd
    };
    let step = "babe0000-0000-0000-0000-000000000042";
    fails(
        request(&order, "12340000-0000-0000-0000-000000000042")
            .output()
            .unwrap(),
        "step is missing",
    );
    fails(
        request(&order, step)
            .env("FACTORY_FIXTURE_RECOVERED", "1")
            .output()
            .unwrap(),
        "not a completed failure",
    );
    fails(
        request(&order, step)
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
        "build 16 superseded the selected failure",
    );
    fails(
        request(&order, step)
            .env("FACTORY_FIXTURE_BRANCH", "0".repeat(40))
            .output()
            .unwrap(),
        "does not resolve to the expected commit",
    );
    let shadow = success(
        request(&order, step)
            .arg("--shadow")
            .env("FACTORY_FIXTURE_NEWER", "1")
            .env("FACTORY_FIXTURE_UNRELATED", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(shadow["decision"], "would_admit");
    fails(fixture.run(&["status", "sample"]), "unknown work order");
    let initial = success(request(&order, step).output().unwrap());
    assert_eq!(initial["work_order"]["require_native_checks"], true);
    let evidence = &initial["work_order"]["ci_failure"];
    assert_eq!(evidence["commit"], fixture.base);
    assert_eq!(evidence["step_uuid"], step);
    assert_eq!(evidence["completed_on"], "2026-11-21T03:30:00Z");
    assert_eq!(initial, success(request(&order, step).output().unwrap()));
    assert_eq!(
        success(request(&order, step).arg("--shadow").output().unwrap())["decision"],
        "already_admitted"
    );
    let rerun = fixture.order("rerun", "A later failed attempt");
    let occupied = success(
        request(&rerun, step)
            .arg("--shadow")
            .env("FACTORY_FIXTURE_RERUN", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(occupied["decision"], "blocked");
    assert_eq!(
        occupied["reason"],
        "the local review slot is occupied by sample"
    );
    let obsolete = success(
        request(&order, step)
            .arg("--shadow")
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(obsolete["decision"], "blocked");
    assert!(obsolete["reason"].as_str().unwrap().contains("superseded"));

    let candidate = fixture.commit("src/hello", "repair");
    success(fixture.run(&[
        "submit",
        "sample",
        "--commit",
        &candidate,
        "--note",
        "CI repair",
    ]));
    fails(
        fixture.run(&[
            "bind-draft",
            "sample",
            "--head",
            &candidate,
            "--reference",
            "https://bitbucket.org/team/other/pull-requests/42",
        ]),
        "repository that failed",
    );
    let pr = "https://bitbucket.org/team/service/pull-requests/42";
    success(fixture.run(&[
        "bind-draft",
        "sample",
        "--head",
        &candidate,
        "--reference",
        pr,
    ]));
    fails(
        fixture.attest("attest-checks", &candidate, pr, "operator-only"),
        "native CI must pass",
    );
    let check = |obsolete: bool, feedback_policy: Option<&Path>| {
        let mut cmd = fixture.command();
        cmd.args(["check-bitbucket-twg", "sample"])
            .arg(&policy)
            .args(["--build", "15"])
            .env("FACTORY_FIXTURE_SHA", &candidate)
            .env("FACTORY_FIXTURE_BRANCH", &fixture.base)
            .env("FACTORY_FIXTURE_PINNED_FAILURE", &fixture.base);
        if let Some(feedback_policy) = feedback_policy {
            cmd.arg("--feedback-policy").arg(feedback_policy);
        }
        if obsolete {
            cmd.env("FACTORY_FIXTURE_NEWER", "1");
        }
        cmd.output().unwrap()
    };
    let original = fs::read(&policy).unwrap();
    let mut changed: Value = serde_json::from_slice(&original).unwrap();
    changed["required_steps"] = json!(["Unrelated check"]);
    fs::write(&policy, changed.to_string()).unwrap();
    fails(check(false, None), "with the failed step required");
    fs::write(&policy, original).unwrap();
    fails(check(true, None), "superseded the selected failure");
    assert_eq!(
        success(fixture.run(&["status", "sample"]))["native_ci"],
        "unavailable"
    );
    let feedback = fixture.root.join("bitbucket-feedback.json");
    fs::write(
        &feedback,
        json!({
            "schema_version":1,"workspace":"team","repository":"service","base_branch":"main",
            "repository_uuid":"00000000-0000-0000-0000-000000000003",
            "reviewer_uuids":["00000000-0000-0000-0000-000000000002"],"minimum_merge_approvals":1
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        success(check(false, Some(&feedback)))["native_ci"],
        "passed"
    );
    fails(
        check(false, None),
        "approval policy is pinned for this work order",
    );
    assert_eq!(
        success(check(false, Some(&feedback)))["native_ci"],
        "passed"
    );
    assert_eq!(
        success(fixture.attest("attest-checks", &candidate, pr, "repair-verified"))["review_ready"],
        true
    );
    fails(
        fixture.attest("close", &candidate, pr, "fixture-outcome"),
        "requires native approval evidence",
    );
    success(fixture.run(&[
        "cancel",
        "sample",
        "--reason",
        "Finish the Bitbucket policy fixture",
    ]));
    let duplicate = fixture.order("duplicate", "Repeat the same failure");
    fails(
        request(&duplicate, step).output().unwrap(),
        "already delegated as sample",
    );
    assert_eq!(
        success(
            request(&rerun, step)
                .arg("--shadow")
                .env("FACTORY_FIXTURE_RERUN", "1")
                .output()
                .unwrap()
        )["decision"],
        "would_admit"
    );
    fails(fixture.run(&["status", "rerun"]), "unknown work order");

    let poll = |completed: &str| {
        let mut cmd = fixture.command();
        cmd.arg("poll-bitbucket-failure")
            .arg(&rerun)
            .arg(&policy)
            .args(["--step-name", "Verify candidate"])
            .env("FACTORY_FIXTURE_SHA", &fixture.base)
            .env("FACTORY_FIXTURE_COMPLETED", completed);
        cmd
    };
    let next_attempt = "2026-11-21T03:50:00Z";
    for (flag, reason) in [
        (
            "FACTORY_FIXTURE_MISSING_PIPELINE_REF",
            "missing pipeline ref type",
        ),
        (
            "FACTORY_FIXTURE_MISSING_PIPELINE_SELECTOR",
            "did not identify the pipeline selector",
        ),
    ] {
        let incomplete = success(
            poll(next_attempt)
                .arg("--admit")
                .env("FACTORY_FIXTURE_NEWER", "1")
                .env(flag, "1")
                .output()
                .unwrap(),
        );
        assert_eq!(incomplete["decision"], "blocked");
        assert!(incomplete["reason"].as_str().unwrap().contains(reason));
    }
    let shadow = success(poll(next_attempt).output().unwrap());
    assert_eq!(shadow["decision"], "would_admit");
    let generated_id = shadow["work_order_id"].as_str().unwrap();
    fails(fixture.run(&["status", generated_id]), "unknown work order");
    success(fixture.run(&["set-ci-intake", "pause"]));
    assert_eq!(
        success(poll(next_attempt).arg("--admit").output().unwrap())["decision"],
        "paused"
    );
    fails(fixture.run(&["status", generated_id]), "unknown work order");
    success(fixture.run(&["set-ci-intake", "resume"]));
    let home = fixture.root.join("bitbucket-codex-home");
    let work = fixture.root.join("bitbucket-ci-attempts");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&work).unwrap();
    assert_eq!(
        success(poll(next_attempt).arg("--admit").output().unwrap())["decision"],
        "admitted"
    );
    let dispatch = |flag: Option<&str>| {
        let mut command = fixture.command();
        command
            .arg("run-ci-repair")
            .arg("--bitbucket-policy")
            .arg(&policy)
            .arg("--codex")
            .arg(&twg)
            .arg("--codex-home")
            .arg(&home)
            .arg("--attempt-root")
            .arg(&work)
            .env("FACTORY_FIXTURE_SHA", &fixture.base)
            .env("FACTORY_FIXTURE_COMPLETED", next_attempt)
            .env("FACTORY_FIXTURE_NEWER", "1");
        if let Some(flag) = flag {
            command.env(flag, "1");
        }
        success(command.output().unwrap())
    };
    for (flag, reason) in [
        (
            Some("FACTORY_FIXTURE_MISSING_PIPELINE_REF"),
            "missing pipeline ref type",
        ),
        (
            Some("FACTORY_FIXTURE_MISSING_PIPELINE_SELECTOR"),
            "superseded the selected failure",
        ),
        (None, "superseded the selected failure"),
    ] {
        let blocked = dispatch(flag);
        assert_eq!(blocked["decision"], "blocked");
        assert!(blocked["reason"].as_str().unwrap().contains(reason));
    }
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
    let pending = success(fixture.run(&["status", generated_id]));
    assert_eq!(pending["state"], "delegated");
    let events = pending["events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "ci_dispatch_blocked")
            .count(),
        1
    );
    assert!(!events.iter().any(|event| event["kind"] == "runner_started"));
    let new_failure = success(
        poll("2026-11-21T04:00:00Z")
            .args(["--admit", "--daily-limit", "2"])
            .output()
            .unwrap(),
    );
    assert_eq!(new_failure["decision"], "blocked");
    assert_eq!(
        new_failure["reason"],
        format!("the local review slot is occupied by {generated_id}")
    );
    let cycle = success(
        fixture
            .command()
            .arg("run-ci-cycle")
            .arg(&rerun)
            .args([
                "--admit",
                "--follow-branch",
                "--order-digest",
                &craft_factory_evidence::sha256(&fs::read(&rerun).unwrap()),
                "--policy-digest",
                &craft_factory_evidence::sha256(&fs::read(&policy).unwrap()),
                "--step-name",
                "Verify candidate",
                "--bitbucket-policy",
            ])
            .arg(&policy)
            .arg("--codex")
            .arg(&twg)
            .arg("--codex-home")
            .arg(&home)
            .arg("--attempt-root")
            .arg(&work)
            .args(["--timeout-seconds", "15"])
            .env("FACTORY_FIXTURE_SHA", &fixture.base)
            .env("FACTORY_FIXTURE_COMPLETED", next_attempt)
            .output()
            .unwrap(),
    );
    assert_eq!(cycle["poll"]["decision"], "already_admitted");
    assert_eq!(cycle["poll"]["branch"]["observed_head"], fixture.base);
    assert_eq!(cycle["dispatch"]["decision"], "candidate_ready");
    assert_eq!(cycle["dispatch"]["work_order_id"], generated_id);
    success(fixture.run(&["cancel", generated_id, "--reason", "Finish the poll test"]));
    let capped = success(
        poll("2026-11-21T04:00:00Z")
            .arg("--admit")
            .output()
            .unwrap(),
    );
    assert_eq!(capped["decision"], "budget_exhausted");
    fails(
        fixture.run(&["status", capped["work_order_id"].as_str().unwrap()]),
        "unknown work order",
    );
    let latest = success(
        poll(next_attempt)
            .env("FACTORY_FIXTURE_NEWER", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(latest["decision"], "no_eligible_failure");
    let history = success(fixture.run(&["ci-poll-history", "--limit", "4"]));
    assert_eq!(history.as_array().unwrap().len(), 4);
    assert_eq!(history[0]["decision"], "no_eligible_failure");
    assert_eq!(history[1]["decision"], "budget_exhausted");
    assert_eq!(
        history[1]["ci_failure"]["completed_on"],
        "2026-11-21T04:00:00Z"
    );
    assert_eq!(history[1]["mode"], "admit");
    assert!(history[1]["observed_at"].as_u64().is_some());
}
