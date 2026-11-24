use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use craft_factory_evidence::sha256;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};

#[derive(PartialEq, Eq)]
enum Scenario {
    Routine,
    ProtectedEdit,
    LostAcknowledgement,
    Sensitive,
}

fn exercise(scenario: Scenario) {
    let sensitive = scenario == Scenario::Sensitive;
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
    let forge = fixture.root.join("github-policy.json");
    fs::write(
        &forge,
        json!({
            "schema_version": 1, "repository": "team/service", "base_branch": "main",
            "check_definitions": ["src/guard"],
            "required_checks": [{"name":"Checks","app_id":42}],
            "ci_repair": {
                "workflow_id":9, "job_name":"Checks",
                "sensitive_paths": if sensitive { vec!["SRC/HELLO"] } else { vec![] }
            }
        })
        .to_string(),
    )
    .unwrap();

    let secret = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(secret.as_ref()).unwrap();
    let signing_key = fixture.root.join("fixture-key.der");
    fs::write(&signing_key, secret.as_ref()).unwrap();
    let policy_file = fixture.root.join("verification-policy.json");
    let harness_digest = sha256(b"trusted fixture assertions");
    fs::write(
        &policy_file,
        json!({
            "schema_version":1,"verifier_issuer":"fixture-independent-verifier",
            "verifier_public_key_base64":STANDARD.encode(key.public_key().as_ref()),
            "harness_digest":harness_digest,"protected_paths":["src/guard"],
            "checks":{"repair":"regression","keep-existing":"preserve"}
        })
        .to_string(),
    )
    .unwrap();
    let policy_pin = sha256(&fs::read(&policy_file).unwrap());
    let order_file = fixture.order("repair", "Repair without changing the verifier");
    let mut order: Value = serde_json::from_slice(&fs::read(&order_file).unwrap()).unwrap();
    order["require_native_checks"] = true.into();
    order["ci_verification_policy_digest"] = policy_pin.clone().into();
    fs::write(&order_file, order.to_string()).unwrap();
    let marker = fixture.root.join("draft-posted");
    let call = |candidate: &str| {
        let mut command = fixture.command();
        command
            .env("FACTORY_FIXTURE_SHA", candidate)
            .env("FACTORY_FIXTURE_BASE", &fixture.base)
            .env("FACTORY_FIXTURE_DRAFT_SENT", &marker);
        command
    };
    let delegate = || {
        call(&fixture.base)
            .arg("delegate-github-failure")
            .arg(&order_file)
            .arg(&forge)
            .args(["--run", "73", "--attempt", "2", "--gh"])
            .arg(&gh)
            .output()
            .unwrap()
    };
    if sensitive {
        fails(delegate(), "separate operator plan approval");
        let blocked_poll = success(
            call(&fixture.base)
                .arg("poll-github-failure")
                .arg(&order_file)
                .arg(&forge)
                .arg("--gh")
                .arg(&gh)
                .arg("--admit")
                .output()
                .unwrap(),
        );
        assert_eq!(blocked_poll["decision"], "blocked");
        assert!(
            blocked_poll["reason"]
                .as_str()
                .unwrap()
                .contains("separate operator plan approval")
        );
        let approval = success(
            fixture
                .command()
                .arg("approve-ci-repair-plan")
                .arg(&order_file)
                .arg("--github-policy")
                .arg(&forge)
                .args(["--reference", "Operator plan decision 1"])
                .output()
                .unwrap(),
        );
        assert_eq!(approval["source"], "operator_observed");
        order["intent"] = "Different intent".into();
        fs::write(&order_file, order.to_string()).unwrap();
        fails(delegate(), "separate operator plan approval");
        order["intent"] = "Repair without changing the verifier".into();
        fs::write(&order_file, order.to_string()).unwrap();
    }
    let delegated = success(delegate());
    assert_eq!(
        delegated["work_order"]["ci_verification_policy_digest"],
        policy_pin
    );
    if scenario == Scenario::ProtectedEdit {
        fixture.commit("src/guard", "temporarily weaken the check");
        git(&fixture.repo, &["revert", "--no-edit", "HEAD"]);
    }
    let candidate = fixture.commit("src/hello", "repaired");
    success(fixture.run(&[
        "submit",
        "repair",
        "--commit",
        &candidate,
        "--note",
        "Scoped repair",
    ]));
    fails(
        fixture.run(&[
            "bind-draft",
            "repair",
            "--reference",
            "https://github.com/team/service/pull/42",
            "--head",
            &candidate,
        ]),
        "pinned independent CI repair verification",
    );

    let seal = |reference: &str, commit: &str, outcome: &str| {
        let run = fixture.root.join(format!("{reference}.json"));
        let output = fixture.root.join(format!("{reference}.signed"));
        let preserved = git(&fixture.repo, &["show", &format!("{commit}:outside")]) == "original";
        fs::write(&run, json!({
            "schema_version":1,"repository":"team/service","work_order_id":"repair",
            "commit":commit,"issuer":"fixture-independent-verifier","reference":reference,
            "policy_digest":policy_pin,"harness_digest":harness_digest,"completed":true,
            "checks":[{"id":"repair","outcome":outcome},
                      {"id":"keep-existing","outcome":if preserved {"passed"} else {"assertion_failed"}}]
        }).to_string()).unwrap();
        success(
            fixture
                .command()
                .arg("seal-ci-repair-run")
                .arg(run)
                .arg("--policy")
                .arg(&policy_file)
                .arg("--policy-digest")
                .arg(&policy_pin)
                .arg("--signing-key")
                .arg(&signing_key)
                .arg("--destination")
                .arg(&output)
                .output()
                .unwrap(),
        );
        output
    };
    let observed_outcome = |commit: &str| {
        if git(&fixture.repo, &["show", &format!("{commit}:src/hello")]) == "repaired" {
            "passed"
        } else {
            "assertion_failed"
        }
    };
    let baseline = seal("baseline", &fixture.base, observed_outcome(&fixture.base));
    let green = seal("candidate", &candidate, observed_outcome(&candidate));
    let publish = |before: &Path, after: &Path| {
        let mut command = call(&candidate);
        command
            .args(["publish-ci-draft", "repair"])
            .arg("--policy")
            .arg(&policy_file)
            .arg("--github-policy")
            .arg(&forge)
            .arg("--baseline-run")
            .arg(before)
            .arg("--candidate-run")
            .arg(after)
            .arg("--gh")
            .arg(&gh);
        command
    };
    if scenario == Scenario::ProtectedEdit {
        fails(
            publish(&baseline, &green).output().unwrap(),
            "protected check definition",
        );
        assert!(!marker.exists());
        return;
    }
    for bad in ["skipped", "error", "passed"] {
        let bad_baseline = seal(bad, &fixture.base, bad);
        fails(
            publish(&bad_baseline, &green).output().unwrap(),
            "paired verification failed",
        );
    }
    fails(
        publish(&baseline, &baseline).output().unwrap(),
        "different work order, repository or commit",
    );
    let mut forged: Value = serde_json::from_slice(&fs::read(&green).unwrap()).unwrap();
    forged["signatures"][0]["sig"] = "AAAA".into();
    let tampered = fixture.root.join("tampered");
    fs::write(&tampered, forged.to_string()).unwrap();
    let refused = publish(&baseline, &tampered).output().unwrap();
    assert!(!refused.status.success());
    assert!(!marker.exists());

    let mut command = publish(&baseline, &green);
    if scenario == Scenario::LostAcknowledgement {
        command.env("FACTORY_FIXTURE_DRAFT_LOSE_ACK", "1");
        fails(
            command.output().unwrap(),
            "publication could not be confirmed",
        );
    } else {
        let result = success(command.output().unwrap());
        assert_eq!(result["receipt"]["draft"]["head"], candidate);
        assert_eq!(result["verification"]["head"], candidate);
        let body = fs::read_to_string(&marker).unwrap();
        assert!(body.contains(&format!(
            "https://github.com/team/service/compare/{}...{candidate}",
            fixture.base
        )));
        assert!(
            body.contains(
                result["verification"]["candidate_receipt"]
                    .as_str()
                    .unwrap()
            )
        );
        assert!(body.contains(if sensitive {
            "Operator plan decision 1"
        } else {
            "Outside the configured sensitive paths"
        }));
        assert!(body.contains("after required checks pass"));
        let current = fixture.run(&["brief", "repair"]);
        assert!(current.status.success());
        assert!(
            String::from_utf8_lossy(&current.stdout)
                .contains("Independent paired verification passed")
        );
    }
    assert!(marker.is_file());
    fails(
        publish(&baseline, &green).output().unwrap(),
        "unpublished current candidate",
    );
    if scenario == Scenario::Routine {
        git(
            &fixture.repo,
            &["commit", "--allow-empty", "-m", "retry CI"],
        );
        let replacement = git(&fixture.repo, &["rev-parse", "HEAD"]);
        success(fixture.run(&[
            "retry-ci",
            "repair",
            "--previous",
            &candidate,
            "--commit",
            &replacement,
        ]));
        let brief = fixture.run(&["brief", "repair"]);
        assert!(brief.status.success());
        let brief = String::from_utf8(brief.stdout).unwrap();
        assert!(brief.contains(&replacement));
        assert!(!brief.contains(&candidate));
        assert!(brief.contains("Independent verification is pending"));
        fails(
            fixture
                .command()
                .args(["attest-checks", "repair"])
                .arg(fixture.review(
                    &candidate,
                    "https://github.com/team/service/pull/42",
                    "Previous revision",
                ))
                .output()
                .unwrap(),
            "current pull request",
        );
    }
}

#[test]
fn draft_requires_exact_independent_paired_checks() {
    exercise(Scenario::Routine);
}

#[test]
fn draft_refuses_a_protected_edit_even_after_a_revert() {
    exercise(Scenario::ProtectedEdit);
}

#[test]
fn interrupted_draft_creation_blocks_a_second_send() {
    exercise(Scenario::LostAcknowledgement);
}

#[test]
fn sensitive_draft_requires_a_separate_exact_plan_approval() {
    exercise(Scenario::Sensitive);
}
