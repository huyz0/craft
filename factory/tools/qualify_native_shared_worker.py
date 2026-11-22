"""Qualify installed Codex from shared admission through a review-requested repair."""

import argparse
from hashlib import file_digest, sha256
from http.client import HTTPSConnection
import json
import os
from pathlib import Path
import ssl
import subprocess
import sys
from tempfile import TemporaryDirectory
from time import time
from urllib.parse import urlsplit
from uuid import uuid4

from qualify_native_worker import git, prepare_order, run_native_worker
from smoke_shared_queue import disposable_queue


REPOSITORY = "fixture/service"
GREETING = "src/greeting.txt"
DRAFT = "https://github.com/fixture/service/pull/1"
ISSUE_FIXTURE_SOURCE = Path(__file__).resolve().parents[1] / "tests/fixtures/agent.rs"
ISSUE = "team/service#42"
ISSUE_URL = "https://github.com/team/service/issues/42"
JIRA_ISSUE = "FIXTURE-99"
JIRA_CLOUD_ID = "11111111-2222-3333-4444-555555555555"


def fixture_publication(origin, ca, token, binding, observation=None, expected=200):
    """Inject a simulated forge observation through the queue's publication API."""
    address = urlsplit(origin)
    connection = HTTPSConnection(address.hostname, address.port, timeout=5,
                                 context=ssl.create_default_context(cafile=str(ca)))
    body = {"action": "bind_github_review", **binding}
    if observation is not None:
        body["observation"] = observation
    try:
        connection.request("POST", "/v1/publication", json.dumps(body),
                           {"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        result = connection.getresponse()
        reply = result.read()
        if result.status != expected:
            raise RuntimeError(f"Unexpected HTTP {result.status} binding the synthetic draft")
        if expected == 200:
            return json.loads(reply)["publication"]
    finally:
        connection.close()


def assert_runner_idle(root, codex, call, success, label):
    empty_home = root / f"{label} home"
    empty_home.mkdir()
    blocked = root / f"{label} attempt"
    idle = success(call("run-shared", "--repository-map", root / "runner mapping.json", "--request-id", str(uuid4()),
                        "--destination", blocked, "--codex", codex, "--codex-home", empty_home, role="runner"))
    assert idle == {"schema_version": 1, "claim": None} and not blocked.exists()


def approve_standing_issue(root, order_file, profile, codex, call, success, tracker):
    fixture = root / ("issue-fixture.exe" if os.name == "nt" else "issue-fixture")
    subprocess.run(["rustc", ISSUE_FIXTURE_SOURCE, "-o", fixture], check=True, timeout=30)
    watch = root / "watch.json"
    rule = {
        "id": "native-standing", "intake_id": "smoke-intake", "repository": REPOSITORY,
        "human_owner": "Fixture issue owner", "required_label": "automate",
        "allowed_paths": ["src"], "preferred_runner": "smoke-runner", "token_limit": 400,
        "approval_reference": "fixture:standing rule", "agent_adapter": "codex", "execution_profile": profile,
    }
    if tracker == "jira":
        return approve_jira_issue(root, fixture, watch, order_file, rule, codex, call, success)

    native_issue = root / "native issue.json"
    native_issue.write_text(json.dumps({
        "id": 707, "number": 42, "repository_url": "https://api.github.com/repos/team/service",
        "html_url": ISSUE_URL, "state": "open", "title": "Update greeting",
        "body": "The operator reviews this fixture body before approval.",
        "updated_at": "2026-11-01T00:00:00Z", "assignees": [{"id": 36}], "labels": [{"name": "automate"}],
    }), encoding="utf-8")
    rule.update(source="github:team/service", project_key="GITHUB", human_account="36", eligible_status="open")
    registered = register_standing_rule(root, rule, "approve-standing-rule", call, success)
    config = {"rule": registered["id"], "issue": {
        "schema_version": 1, "gh": str(fixture), "repository": "team/service", "number": 42, "issue_id": 707,
    }}

    def poll():
        watch.write_text(json.dumps(config), encoding="utf-8")
        return success(call("poll-standing-github-issue", watch, order_file, role="intake",
                            extra_env={"FACTORY_FIXTURE_GITHUB_ISSUE_JSON": str(native_issue)}))

    pending = poll()
    assert pending["decision"]["state"] == "review_required" and "work_id" not in pending["decision"]
    config["approved_intent"] = pending["snapshot"]["intent_digest"]
    confirmed = poll()
    assert confirmed["decision"] == pending["decision"]
    inbox = success(call("shared-ops", "operator-brief", "--repository", REPOSITORY, "--json"))
    [item] = inbox["operator_brief"]["standing_intent_reviews"]["items"]
    assert item["issue_url"] == ISSUE_URL and item["review_token"] == confirmed["decision"]["review_token"]
    assert_runner_idle(root, codex, call, success, "unapproved")
    evidence = "fixture:operator reviewed the issue and work order"
    approval = success(call("shared-ops", "approve-standing-intent", item["rule_id"], "--issue", item["issue"],
                            "--source-revision", str(item["source_revision"]), "--review-token", item["review_token"],
                            "--evidence", evidence))["intent_approval"]
    assert approval["actor"] == "smoke-operator"
    admitted = poll()["decision"]
    assert admitted["state"] == "admitted"
    assert poll()["decision"] == {"state": "already_admitted", "work_id": admitted["work_id"]}
    status = success(call("shared-work", "status", admitted["work_id"]))["work"]
    assert status["standing_delegation"]["intent_approval_reference"] == evidence
    return admitted["work_id"], {"url": ISSUE_URL, "initial_claim_before_approval": "denied",
                                 "operator": approval["actor"], "decision_reference": evidence}


def register_standing_rule(root, rule, command, call, success):
    path = root / "standing-rule.json"
    content = json.dumps(rule).encode("utf-8")
    path.write_bytes(content)
    registered = success(call("shared-ops", command, path,
                              "--digest", "sha256:" + sha256(content).hexdigest()))["standing_rule"]
    assert registered["id"] == rule["id"]
    return registered


def approve_jira_issue(root, fixture, watch, order_file, rule, codex, call, success):
    rule.update(source=f"jira:{JIRA_CLOUD_ID}", project_key="FIXTURE",
                human_account="relay-human", eligible_status="10001")
    registered = register_standing_rule(root, rule, "approve-jira-standing-rule", call, success)
    watched = {"issue": JIRA_ISSUE, "issue_id": "1042", "rule": registered["id"],
               "work_order": json.loads(order_file.read_bytes())}
    config = {"schema_version": 1, "craft": str(fixture), "craft_state": str(root),
              "craft_profile": "fixture-jira", "cloud_id": JIRA_CLOUD_ID, "watches": [watched]}

    def poll(*, revised=False):
        watch.write_text(json.dumps(config), encoding="utf-8")
        environment = {
            "FACTORY_FIXTURE_JIRA_OBSERVED": str(int(time() * 1000)),
            "FACTORY_FIXTURE_JIRA_UPDATED": "2026-11-01T00:01:00Z" if revised else "2026-11-01T00:00:00Z",
        }
        if revised:
            environment["FACTORY_FIXTURE_JIRA_CHANGED"] = "1"
        result = success(call("poll-standing-jira", watch, role="intake", extra_env=environment))
        assert result["submitted"] and result["errors"] == []
        [observation] = result["observations"]
        return observation

    def approval_inbox():
        brief = success(call("shared-ops", "operator-brief", "--repository", REPOSITORY, "--json"))
        return brief["operator_brief"]["standing_intent_reviews"]

    first = poll()
    assert first["decision"]["state"] == "review_required" and "work_id" not in first["decision"]
    [pending] = approval_inbox()["items"]
    assert pending["tracker"] == "jira" and pending["issue"] == JIRA_ISSUE and pending["cloud_id"] == JIRA_CLOUD_ID
    assert pending["state"] == "confirm_relay_input" and "review_token" not in pending
    watched["approved_intent"] = first["snapshot"]["intent_digest"]
    confirmed = poll()
    assert confirmed["decision"] == first["decision"]
    assert_runner_idle(root, codex, call, success, "unapproved")

    def approve(observation, evidence):
        return call("shared-ops", "approve-jira-standing-intent", registered["id"], "--issue", JIRA_ISSUE,
                    "--source-revision", str(observation["snapshot"]["source_revision"]),
                    "--review-token", observation["decision"]["review_token"], "--evidence", evidence)

    def approve_from_inbox(observation, evidence):
        inbox = approval_inbox()
        assert inbox["ready"] == 1
        [item] = inbox["items"]
        assert item["state"] == "ready_for_approval" and item["review_token"] == observation["decision"]["review_token"]
        return success(call("shared-ops", "approve-jira-standing-intent", item["rule_id"], "--issue", item["issue"],
                            "--source-revision", str(item["source_revision"]), "--review-token", item["review_token"],
                            "--evidence", evidence))["intent_approval"]

    first_evidence = "fixture:operator reviewed the first Jira revision"
    first_approval = approve_from_inbox(confirmed, first_evidence)
    assert first_approval["actor"] == "smoke-operator"
    assert approval_inbox() == {"total": 0, "ready": 0, "items": []}
    updated = poll(revised=True)
    assert updated["decision"]["state"] == "review_required"
    assert updated["snapshot"]["intent_digest"] != first["snapshot"]["intent_digest"]
    assert success(approve(confirmed, first_evidence))["intent_approval"] == first_approval
    stale = approve(confirmed, "fixture:attempt to approve the old Jira revision again")
    assert stale.returncode and "HTTP 409" in stale.stderr
    assert poll(revised=True)["decision"] == updated["decision"]
    assert_runner_idle(root, codex, call, success, "stale approval")
    watched["approved_intent"] = updated["snapshot"]["intent_digest"]
    reviewed = poll(revised=True)
    assert reviewed["decision"] == updated["decision"]
    evidence = "fixture:operator reviewed the revised Jira issue and work order"
    approval = approve_from_inbox(reviewed, evidence)
    assert approval["actor"] == "smoke-operator"
    admitted = poll(revised=True)["decision"]
    assert admitted["state"] == "admitted"
    assert poll(revised=True)["decision"] == {"state": "already_admitted", "work_id": admitted["work_id"]}
    status = success(call("shared-work", "status", admitted["work_id"]))["work"]
    assert status["standing_delegation"]["intent_approval_reference"] == evidence
    return admitted["work_id"], {"issue": JIRA_ISSUE, "initial_claim_before_approval": "denied",
                                 "stale_approval_replay": "receipt_only", "stale_new_approval": "rejected",
                                 "intent_digest": watched["approved_intent"],
                                 "operator": approval["actor"], "decision_reference": evidence}


def qualify(kit, codex, root, postgres_bin, *, tracker=None):
    launcher = kit / ("craft-factory.exe" if os.name == "nt" else "craft-factory")
    env = {name: value for name, value in os.environ.items() if not name.upper().startswith("CRAFT_FACTORY_")}
    order = prepare_order(root)
    order["execution_profile"]["max_attempts"] = 2
    if tracker:
        order["issue"] = JIRA_ISSUE if tracker == "jira" else ISSUE
    source = Path(order["repository"])
    verifier_source = root / "verifier source"
    git(root, "clone", "--no-local", "--quiet", str(source), str(verifier_source))

    def mapping(name, checkout):
        path = root / name
        path.write_text(json.dumps({REPOSITORY: {"path": str(checkout)}}), encoding="utf-8")
        return path

    runner_map = mapping("runner mapping.json", source)
    verifier_map = mapping("verifier mapping.json", verifier_source)
    order_file = root / "approved order.json"
    template = order.copy()
    if tracker:
        del template["execution_profile"]
    order_bytes = json.dumps(template).encode("utf-8")
    order_file.write_bytes(order_bytes)
    roles = {"operator": {}, "runner": {}, "verifier": {"verifier_repositories": [REPOSITORY]},
             "reviewer": {}, "publisher": {}, "intake": {}}
    with disposable_queue(kit, root, postgres_bin, roles) as (origin, ca, tokens, _):
        def call(command, *args, role="operator", token_role=None, extra_env=None):
            credential = f"CRAFT_FACTORY_{role.upper()}_TOKEN"
            return subprocess.run(
                [launcher, command, "--queue", origin, "--ca", ca, *args],
                cwd=root, env=env | {credential: tokens[token_role or role]} | (extra_env or {}),
                text=True, capture_output=True, timeout=105,
            )

        def success(run):
            if run.returncode:
                raise RuntimeError(run.stderr.strip() or "Shared worker fixture command failed")
            return json.loads(run.stdout)

        grant = success(call("shared-ops", "set-runner-enrollment", "smoke-runner", REPOSITORY,
                             "--expected-revision", "0", "--decision", "grant", "--evidence", "fixture:enrollment"))
        assert grant["decision"]["enrolled"]
        assert success(call("shared-work", "resume"))["paused"] is False
        issue = None
        if tracker:
            work_id, issue = approve_standing_issue(root, order_file, order["execution_profile"], codex, call, success,
                                                   tracker)
        else:
            admitted = success(call("shared-work", "admit", order_file, "--repository", REPOSITORY,
                                   "--order-digest", "sha256:" + sha256(order_bytes).hexdigest(),
                                   "--event-key", "fixture:native-shared-worker", "--token-limit", "400"))
            work_id = order["id"]
            assert admitted["work_order_id"] == work_id
        success(call("shared-ops", "configure-draft-policy", REPOSITORY, "--base-branch", "main",
                     "--publisher", "smoke-publisher", "--forge", "github", "--expected-revision", "0",
                     "--enabled", "true", "--evidence", "fixture:approved draft lane"))
        success(call("shared-ops", "grant-review-inbox", "fixture-review-grant", "--reviewer-id", "smoke-reviewer",
                     "--repository", REPOSITORY, "--evidence", "fixture:reviewer grant", "--valid-seconds", "600"))

        def prepare_and_verify(generation, greeting, repair_context=()):
            fixture = root / f"native session {generation}"
            fixture.mkdir()
            runner_args = ["--repository-map", runner_map, "--request-id", str(uuid4()),
                           "--destination", root / f"worker attempt {generation}"]
            launched = []

            def launch(options, temp):
                launched.extend(options)
                return call("run-shared", *runner_args, *options, role="runner", extra_env=temp)

            receipt = success(run_native_worker(codex, fixture, order, GREETING, launch,
                                                greeting=greeting, repair_context=repair_context))
            candidate = receipt["candidate"]
            assert receipt["generation"] == generation and receipt["work_id"] == work_id
            assert success(call("run-shared", *runner_args, *launched, role="runner")) == receipt
            verification = root / f"independent verification {generation}"
            prepare = ["--work-id", work_id, "--head", candidate, "--repository-map", verifier_map,
                       "--destination", verification, "--verifier"]
            if generation == 1:
                denied = call("prepare-shared-verification", *prepare, role="verifier", token_role="runner")
                assert denied.returncode and "HTTP 403" in denied.stderr and not verification.exists()
            handoff = success(call("prepare-shared-verification", *prepare, role="verifier"))
            assert handoff["head"] == candidate and handoff["pack_digest"] == receipt["candidate_evidence"]
            brief = Path(handoff["review_brief"]).read_bytes()
            assert handoff["review_brief_digest"] == "sha256:" + sha256(brief).hexdigest()
            assert order["intent"].encode() in brief and GREETING.encode() in brief
            checked = Path(handoff["checkout"])
            assert git(checked, "rev-parse", "HEAD") == candidate and not git(checked, "status", "--porcelain")
            assert git(checked, "diff", "--name-only", order["base"], candidate) == GREETING
            assert (checked / GREETING).read_text().strip() == greeting
            evidence = f"fixture:exact-candidate-greeting-{generation}"
            attestation = ["--handoff", verification / "handoff.json", "--result", "passed", "--evidence", evidence,
                           "--note", "Checked the exact detached candidate and its single changed path."]
            verified = success(call("attest-shared-verification", *attestation, role="verifier"))
            current = success(call("shared-work", "status", work_id))["work"]
            assert current["candidate"] == candidate and current["verification"]["passed"]
            assert current["verification"]["verifier"] == "smoke-verifier"
            assert current["verification"]["evidence"] == evidence
            assert current["verification"]["sequence"] == verified["verification_sequence"]
            return {"work_id": work_id, "head": candidate, "generation": generation,
                    "verification_sequence": verified["verification_sequence"]}, verification / "handoff.json"

        def bind_draft(binding, previous=None):
            target = fixture_publication(origin, ca, tokens["operator"], binding)
            assert target["expected_draft_url"] == (DRAFT if previous else None)
            observed = {key: target[key] for key in ("head", "base", "source_branch", "base_branch")}
            if previous:
                fixture_publication(origin, ca, tokens["operator"], binding,
                                    observed | {"native_url": DRAFT + "2"}, expected=409)
            publication = fixture_publication(origin, ca, tokens["operator"], binding, observed | {"native_url": DRAFT})
            assert publication["result_url"] == DRAFT and publication["head"] == binding["head"]
            if previous:
                assert publication["prior_action_key"] == previous["action_key"]
            return publication

        initial, initial_handoff = prepare_and_verify(1, "changed greeting")
        first_draft = bind_draft(initial)
        review_id = "fixture-review"
        assignment = success(call("shared-ops", "assign-review", review_id, "--work-id", work_id,
                                  "--generation", "1", "--head", initial["head"], "--verification-sequence",
                                  str(initial["verification_sequence"]), "--reviewer-id", "smoke-reviewer",
                                  "--evidence", "fixture:assigned review"))["handoff"]
        assert assignment["reviewer_id"] == "smoke-reviewer"
        feedback = "Use reviewed greeting as the exact final value."
        decision_args = [review_id, "--head", initial["head"], "--decision", "changes-requested",
                         "--evidence", "fixture:request changes", "--note", feedback]
        decision = success(call("shared-ops", "record-review-decision", *decision_args, role="reviewer"))["handoff"]
        assert decision["decision"] == "changes_requested" and decision["decision_note"] == feedback
        repair = success(call("shared-ops", "apply-review-repair", review_id))["repair"]
        assert repair["head"] == initial["head"] and repair["native_url"] == DRAFT
        assert success(call("shared-ops", "apply-review-repair", review_id))["repair"] == repair
        updated, _ = prepare_and_verify(2, "reviewed greeting", repair_context=(feedback, initial["head"]))
        assert updated["head"] != initial["head"]
        stale = call("attest-shared-verification", "--handoff", initial_handoff, "--result", "passed",
                     "--evidence", "fixture:stale evidence", "--note", "Old candidate must remain stale.", role="verifier")
        assert stale.returncode and "HTTP 409" in stale.stderr
        repaired_draft = bind_draft(updated, first_draft)

        for checkout in (source, verifier_source):
            assert git(checkout, "rev-parse", "HEAD") == order["base"]
            assert (checkout / GREETING).read_text().strip() == "original greeting"
        audit = success(call("shared-ops", "audit", work_id))["audit"]
        assert sorted(attempt["generation"] for attempt in audit["attempts"]) == [1, 2]
        assert all(attempt["runner_id"] == "smoke-runner" and attempt["reported_tokens"] == 300
                   for attempt in audit["attempts"])
        digests = {}
        queue = kit / "shared" / ("craft-factory-queue.exe" if os.name == "nt" else "craft-factory-queue")
        for label, path in (("launcher", launcher), ("queue", queue), ("codex", codex)):
            with path.open("rb") as executable:
                digests[label] = file_digest(executable, "sha256").hexdigest()
        return {"schema_version": 1, "platform": sys.platform, "model": "scripted_loopback",
                "binary_sha256": digests, "standing_issue": issue, "work_id": work_id,
                "draft": repaired_draft["result_url"], "publication_observations": "synthetic",
                "reviewer": decision["reviewer_id"], "first_candidate": initial, "repaired_candidate": updated,
                "stale_verification": "rejected",
                "attempts": [{key: row[key] for key in ("generation", "runner_id", "reported_tokens")}
                             for row in sorted(audit["attempts"], key=lambda row: row["generation"])]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kit", type=Path, help="Extracted shared-service kit")
    parser.add_argument("codex", type=Path)
    parser.add_argument("--postgres-bin", type=Path)
    tracker = parser.add_mutually_exclusive_group()
    tracker.add_argument("--github-issue", action="store_const", const="github", dest="tracker",
                         help="Admit via a synthetic GitHub issue and operator decision")
    tracker.add_argument("--jira-issue", action="store_const", const="jira", dest="tracker",
                         help="Admit via a synthetic Jira issue and a renewed operator decision")
    parser.add_argument("--report", type=Path, help="Write the completed evidence as JSON to a new file")
    args = parser.parse_args()
    if sys.platform not in {"linux", "win32"} or (sys.platform == "linux" and os.geteuid() == 0):
        parser.error("run on Windows or as a non-root Linux user")
    postgres_bin = args.postgres_bin.resolve(strict=True) if args.postgres_bin else None
    with TemporaryDirectory(prefix="craft-native-shared-", dir="/tmp" if sys.platform == "linux" else None) as scratch:
        report = qualify(args.kit.resolve(strict=True), args.codex.resolve(strict=True), Path(scratch), postgres_bin,
                         tracker=args.tracker)
    if args.report:
        with args.report.open("x", encoding="utf-8") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
    print("Installed Codex passed shared admission, candidate replay, review-requested repair, and independent verification.")


if __name__ == "__main__":
    main()
