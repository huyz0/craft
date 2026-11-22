"""Run native Claude Code against a scripted model in a loopback-only Linux namespace."""

import argparse
from contextlib import contextmanager
from hashlib import file_digest, sha256
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Event, Thread
from uuid import uuid4

from qualify_native_coordinator import MODEL as CODEX_MODEL
from qualify_native_worker import git, prepare_order, run_native_worker
from smoke_shared_queue import disposable_queue


FIXTURE_KEY = "fixture-claude-key-no-account"
FIXTURE_ENV = {"ANTHROPIC_API_KEY": FIXTURE_KEY, "ANTHROPIC_MODEL": "claude-haiku-4-5",
               "CLAUDE_CODE_EFFORT_LEVEL": "low"}
MODEL = "claude-sonnet-4-6"
TARGET = "src/greeting.txt"
GREETING = "Verified fixture greeting"
NEW_FILE = "src/native-note.txt"
EXPECTED_USAGE = (130, 15, 45)
REPOSITORY = "fixture/service"


def tool_reply(request, index, outside, create_note):
    details = (("Read", {"file_path": str(outside)}),
               ("Read", {"file_path": f"/work/{TARGET}"}),
               ("Edit", {"file_path": f"/work/{TARGET}", "old_string": "original greeting",
                         "new_string": GREETING}),
               ("Edit", {"file_path": f"/work/{NEW_FILE}", "old_string": "", "new_string": "Fixture note.\n"}),
               ("StructuredOutput", {"outcome": "candidate", "summary": "Edited the fixture greeting."}))
    if not create_note:
        details = details[:3] + details[4:]
    if not 0 <= index < len(details):
        raise ValueError("Unexpected number of fixture model requests")
    name, arguments = details[index]
    available = {tool.get("name") for tool in request["tools"]}
    if not available.issubset({"Read", "Edit", "Write", "Glob", "Grep", "StructuredOutput"}):
        raise ValueError("The native client published tools outside the approved file and result tools")
    if name not in available:
        raise ValueError(f"The native client did not publish its {name} tool. Available: {sorted(available)}")
    usage = {"input_tokens": 21, "cache_creation_input_tokens": 2, "cache_read_input_tokens": 3, "output_tokens": 9}
    item = {"type": "tool_use", "id": f"toolu_fixture{index}", "name": name, "input": arguments}
    message = {"id": f"msg_fixture{index}", "type": "message", "role": "assistant", "model": MODEL,
               "content": [item], "stop_reason": "tool_use", "stop_sequence": None, "usage": usage}
    if not request.get("stream"):
        return json.dumps(message).encode(), "application/json"
    events = (
        {"type": "message_start", "message": message | {"content": [], "stop_reason": None,
                                                        "usage": usage | {"output_tokens": 0}}},
        {"type": "content_block_start", "index": 0, "content_block": item | {"input": {}}},
        {"type": "content_block_delta", "index": 0,
         "delta": {"type": "input_json_delta", "partial_json": json.dumps(arguments)}},
        {"type": "content_block_stop", "index": 0},
        {"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": None},
         "usage": {"output_tokens": usage["output_tokens"]}},
        {"type": "message_stop"},
    )
    return "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events).encode(), "text/event-stream"


def fixture_order(root):
    order = prepare_order(root)
    order["execution_profile"].update(id="fixture-claude-profile", model=MODEL, reasoning_effort="high",
                                      timeout_seconds=25, approval_reference="fixture:approved Claude profile")
    order.update(agent_adapter="claude", intent=f"Edit the fixture greeting in {TARGET}.")
    return order


@contextmanager
def native_model(bubblewrap, root, respond):
    requests = []
    errors = []

    class MessagesFixture(BaseHTTPRequestHandler):
        def do_POST(self):
            if self.path.split("?", 1)[0] != "/v1/messages" or self.headers.get("x-api-key") != FIXTURE_KEY:
                self.send_error(403)
                return
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(request)
            try:
                response = respond(request, len(requests) - 1, self.connection)
            except (KeyError, ValueError) as error:
                errors.append(str(error))
                self.send_error(400, str(error))
                return
            if response is None:
                return
            payload, content_type = response
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *_args):
            pass

    wrapper = root / "fixture-bwrap"
    native_stderr = root / "native-stderr.txt"
    native_stdout = root / "native-output.json"
    with ThreadingHTTPServer(("127.0.0.1", 0), MessagesFixture) as server:
        base = f"http://127.0.0.1:{server.server_port}"
        command = shlex.join([str(bubblewrap), "--setenv", "ANTHROPIC_BASE_URL", base,
                              "--setenv", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"])
        wrapper.write_text(
            f'#!/bin/bash\nset -o pipefail\n{command} "$@" 2>{shlex.quote(str(native_stderr))} '
            f'| tee {shlex.quote(str(native_stdout))}\n', encoding="utf-8")
        wrapper.chmod(0o700)
        thread = Thread(target=server.serve_forever)
        thread.start()
        try:
            yield wrapper, requests, errors, (native_stderr, native_stdout)
        finally:
            server.shutdown()
            thread.join()


def run_native_model(bubblewrap, root, order, launch, *, excluded_tokens=(), create_note=True):
    outside = root / "fixture-host.txt"
    sentinel = "fixture file outside the worker sandbox"
    outside.write_text(sentinel, encoding="utf-8")
    with native_model(bubblewrap, root,
                      lambda request, index, _: tool_reply(request, index, outside, create_note)) as fixture:
        wrapper, requests, errors, logs = fixture
        run = launch(wrapper)
    if run.returncode or errors:
        diagnostics = "\n".join(path.read_text(encoding="utf-8", errors="replace")[-2500:]
                                for path in logs if path.exists())
        raise RuntimeError(
            f"Native Claude failed after {len(requests)} fixture model requests: {run.stderr.strip()}\n"
            + "\n".join(errors + [diagnostics])
        )
    assert len(requests) == (5 if create_note else 4) and order["intent"] in json.dumps(requests[0]["messages"])
    assert all(request["model"] == MODEL for request in requests)
    assert all(request.get("output_config", {}).get("effort") == "high" for request in requests), [
        request.get("output_config") for request in requests
    ]
    [blocked] = [item for message in requests[1]["messages"] if isinstance(message["content"], list)
                 for item in message["content"] if item.get("type") == "tool_result"]
    assert blocked["tool_use_id"] == "toolu_fixture0" and blocked["is_error"]
    assert sentinel not in json.dumps(blocked)
    if excluded_tokens:
        payload = json.dumps(requests)
        if any(token in payload for token in excluded_tokens):
            raise RuntimeError("A queue credential appeared in the model fixture's request")
    return json.loads(run.stdout)


def check_edits(repository, base, candidate, *, create_note=True):
    changed = {TARGET, NEW_FILE} if create_note else {TARGET}
    assert set(git(repository, "diff", "--name-only", base, candidate).splitlines()) == changed
    assert git(repository, "show", f"{candidate}:{TARGET}") == GREETING
    if create_note:
        assert git(repository, "show", f"{candidate}:{NEW_FILE}") == "Fixture note."


def qualify(launcher, claude, bubblewrap, root):
    order = fixture_order(root)
    order_file = root / "order.json"
    cli = [launcher, "--state", root / "work.db"]
    unsupported = order | {"execution_profile": order["execution_profile"] | {"reasoning_effort": "ultra"}}
    order_file.write_text(json.dumps(unsupported), encoding="utf-8")
    denied = subprocess.run(cli + ["delegate", order_file], text=True, capture_output=True, timeout=15)
    assert denied.returncode and "Claude execution profiles do not support the ultra effort" in denied.stderr
    order_file.write_text(json.dumps(order), encoding="utf-8")
    subprocess.run(cli + ["delegate", order_file], check=True, stdout=subprocess.DEVNULL, timeout=15)

    def launch(wrapper):
        return subprocess.run(
            cli + ["run", order["id"], "--destination", root / "attempt",
                   "--claude", claude, "--bubblewrap", wrapper, "--timeout-seconds", "60"],
            env=os.environ | FIXTURE_ENV, text=True, capture_output=True, timeout=70,
        )

    receipt = run_native_model(bubblewrap, root, order, launch)
    repository = Path(order["repository"])
    assert git(repository, "rev-parse", "HEAD") == order["base"]
    check_edits(repository, order["base"], receipt["candidate"])
    observation = next(event for event in receipt["events"] if event["kind"] == "runner_observed")
    observed = json.loads(observation["note"])
    assert (observed["input_tokens"], observed["cached_input_tokens"], observed["output_tokens"]) == EXPECTED_USAGE, observed
    selected = next(event for event in receipt["events"] if event["kind"] == "runner_profile_selected")
    assert json.loads(selected["note"]) == {
        "requested_profile": order["execution_profile"], "effective_timeout_seconds": 25,
    }


def qualify_shared(kit, claude, bubblewrap, root, postgres_bin, *, codex=None):
    launcher = kit / "craft-factory"
    order = fixture_order(root)
    if codex:
        order["acceptance"] = [f"The approved greeting reads {GREETING} and no other file changes."]
    source = Path(order["repository"])
    trusted = root / "trusted verifier"
    git(root, "clone", "--no-local", "--quiet", str(source), str(trusted))
    runner_map = root / "runner.json"
    runner_map.write_text(json.dumps({REPOSITORY: {"path": str(source)}}), encoding="utf-8")
    verifier_map = root / "verifier.json"
    verifier_map.write_text(json.dumps({REPOSITORY: {"path": str(trusted)}}), encoding="utf-8")
    env = {name: value for name, value in os.environ.items() if not name.startswith("CRAFT_FACTORY_")}
    env.update(FIXTURE_ENV)
    database = root / "db"
    database.mkdir()
    roles = {"operator": {}, "runner": {}, "verifier": {"verifier_repositories": [REPOSITORY]}}
    if codex:
        roles["codex"] = {"role": "runner"}
    with disposable_queue(kit, database, postgres_bin, roles, review_limit=2 if codex else 1) as (origin, ca, tokens, _):
        def invocation(command, *args, role):
            return [launcher, command, "--queue", origin, "--ca", ca, *args], env | {
                f"CRAFT_FACTORY_{role.upper()}_TOKEN": tokens[role]
            }

        def call(command, *args, role="operator", extra_env=None):
            arguments, child_env = invocation(command, *args, role=role)
            return subprocess.run(arguments, env=child_env | (extra_env or {}), text=True, capture_output=True, timeout=100)

        def checked(run):
            if run.returncode:
                raise RuntimeError(run.stderr.strip() or "Shared Claude qualification command failed")
            return json.loads(run.stdout)

        def admit(item, filename):
            path = root / filename
            content = json.dumps(item).encode("utf-8")
            path.write_bytes(content)
            reply = checked(call("shared-work", "admit", path, "--order-digest", "sha256:" + sha256(content).hexdigest(),
                                 "--repository", REPOSITORY, "--event-key", f"fixture:{item['id']}", "--token-limit", "400"))
            assert reply["work_order_id"] == item["id"]

        checked(call("shared-work", "resume"))
        checked(call("shared-ops", "set-runner-enrollment", "smoke-runner", REPOSITORY,
                     "--expected-revision", "0", "--decision", "grant", "--evidence", "fixture:runner enrollment"))
        if codex:
            checked(call("shared-ops", "set-runner-enrollment", "smoke-codex", REPOSITORY,
                         "--expected-revision", "0", "--decision", "grant", "--evidence", "fixture:Codex enrollment"))
        admit(order, "order.json")
        unsupported_profile = root / "unsupported-fallback.json"
        unsupported_profile.write_text(json.dumps(order["execution_profile"] | {"reasoning_effort": "ultra"}),
                                       encoding="utf-8")
        fallback = call("shared-ops", "approve-profile-fallback", order["id"], "--replacement-id", "unsupported-claude",
                        "--profile", unsupported_profile, "--evidence", "fixture:unsupported effort", "--valid-seconds", "120")
        assert fallback.returncode and "HTTP 409" in fallback.stderr, fallback.stderr
        attempt_root = root / "preflight attempts"
        attempt_root.mkdir()

        def check_setup(approved):
            result = call("check-shared-runner", "--repository-map", runner_map, "--attempt-root", attempt_root,
                          "--claude", claude, "--bubblewrap", bubblewrap, role="runner")
            assert (result.returncode == 0) == approved, result.stdout or result.stderr
            report = json.loads(result.stdout)
            assert report["ready"] is approved and report["usable_repositories"] == int(approved)
            [repository] = report["repositories"]
            assert repository["enrolled"] and repository["checkout_available"]
            assert repository["adapter_approved"] is approved
            assert not list(attempt_root.iterdir())

        check_setup(False)
        destination = root / "worker attempt"
        runner_args = ("--repository-map", runner_map, "--request-id", str(uuid4()), "--destination", destination,
                       "--claude", claude, "--timeout-seconds", "60")
        denied = checked(call("run-shared", *runner_args, "--bubblewrap", bubblewrap, role="runner"))
        assert denied["claim"] is None and not destination.exists()
        approval_id = "native-claude-fixture"
        checked(call("shared-ops", "approve-claude-repository", REPOSITORY, "--approval-id", approval_id,
                     "--evidence", "fixture:repository approval", "--valid-seconds", "120"))
        check_setup(True)
        assert not checked(call("shared-ops", "audit", order["id"]))["audit"]["attempts"]

        def launch(wrapper):
            return call("run-shared", *runner_args, "--bubblewrap", wrapper, role="runner")

        receipt = run_native_model(bubblewrap, root, order, launch, excluded_tokens=tokens.values(), create_note=not codex)
        assert checked(launch(bubblewrap)) == receipt

        def verify(item, candidate_receipt):
            candidate = candidate_receipt["candidate"]
            assert candidate_receipt["generation"] == 1 and candidate_receipt["work_id"] == item["id"]
            verification = root / ("verification-" + item["id"])
            handoff = checked(call("prepare-shared-verification", "--work-id", item["id"], "--head", candidate,
                                   "--repository-map", verifier_map, "--destination", verification, "--verifier",
                                   role="verifier"))
            assert handoff["head"] == candidate and handoff["pack_digest"] == candidate_receipt["candidate_evidence"]
            detached = Path(handoff["checkout"])
            assert git(detached, "rev-parse", "HEAD") == candidate and not git(detached, "status", "--porcelain")
            check_edits(detached, item["base"], candidate, create_note=not codex)
            checked(call("attest-shared-verification", "--handoff", verification / "handoff.json", "--result", "passed",
                         "--evidence", f"fixture:{item['id']}-checked", "--note", "Checked the exact detached candidate.",
                         role="verifier"))
            return git(detached, "rev-parse", "HEAD^{tree}")

        claude_tree = verify(order, receipt)
        final = checked(call("shared-work", "status", order["id"]))["work"]
        assert final["verification"]["passed"] and final["verification"]["verifier"] == "smoke-verifier"
        assert final["execution_profile"] == order["execution_profile"] and final["profile_started_at"]
        assert final["attempt_limit_reached"] and not final["attempt_limit_blocks_pickup"]
        [attempt] = checked(call("shared-ops", "audit", order["id"]))["audit"]["attempts"]
        assert attempt["runner_id"] == "smoke-runner" and attempt["claude_approval_id"] == approval_id
        assert attempt["reported_tokens"] == (140 if codex else EXPECTED_USAGE[0] + EXPECTED_USAGE[2])

        baseline = order | {"id": "comparison-codex", "agent_adapter": "codex", "execution_profile": {
            "id": "fixture-codex-profile", "model": CODEX_MODEL, "reasoning_effort": "medium",
            "timeout_seconds": 90, "max_attempts": 1, "approval_reference": "fixture:comparison baseline",
        }}
        admit(baseline, "comparison-codex.json")
        if codex:
            fixture = root / "codex-model"
            fixture.mkdir()

            def launch_baseline(options, temp):
                return call("run-shared", "--repository-map", runner_map, "--request-id", str(uuid4()),
                            "--destination", root / "codex attempt", *options, role="runner",
                            extra_env=temp | {"CRAFT_FACTORY_RUNNER_TOKEN": tokens["codex"]})

            codex_receipt = checked(run_native_worker(codex, fixture, baseline, TARGET, launch_baseline, greeting=GREETING))
            assert verify(baseline, codex_receipt) == claude_tree
            [codex_attempt] = checked(call("shared-ops", "audit", baseline["id"]))["audit"]["attempts"]
            assert codex_attempt["runner_id"] == "smoke-codex" and codex_attempt["reported_tokens"] == 300
        assert all(git(checkout, "rev-parse", "HEAD") == order["base"] and not git(checkout, "status", "--porcelain")
                   for checkout in (source, trusted))
        pairs = root / "comparison.json"
        pairs.write_text(json.dumps([{"case_id": "greeting", "baseline": baseline["id"], "challenger": order["id"]}]),
                         encoding="utf-8")
        comparison = checked(call("shared-ops", "compare-profiles", pairs))["comparison"]
        assert (comparison["baseline_adapter"], comparison["challenger_adapter"]) == ("codex", "claude")
        if codex:
            assert comparison["totals"]["baseline"]["passed"] == 1
            assert comparison["totals"]["baseline"]["reported_tokens"] == 300
        else:
            assert comparison["totals"]["baseline"]["unscored"] == 1
        assert comparison["totals"]["challenger"]["passed"] == 1
        assert comparison["totals"]["challenger"]["reported_tokens"] == attempt["reported_tokens"]
        checked(call("shared-work", "cancel", baseline["id"], "--reason", "fixture comparison finished"))
        checked(call("shared-work", "cancel", order["id"], "--reason", "fixture verification finished"))
        next_order = order | {"id": "active-revocation"}
        admit(next_order, "active-revocation.json")
        active = root / "active revocation"
        active.mkdir()
        started, disconnected = Event(), Event()

        def hold_request(_request, _index, connection):
            started.set()
            connection.settimeout(15)
            try:
                if not connection.recv(1):
                    disconnected.set()
            except ConnectionResetError:
                disconnected.set()
            except TimeoutError:
                return

        expired = root / "expired attempt"
        with native_model(bubblewrap, active, hold_request) as (wrapper, requests, errors, _):
            command, child_env = invocation(
                "run-shared", "--repository-map", runner_map, "--request-id", str(uuid4()),
                "--destination", expired, "--claude", claude, "--bubblewrap", wrapper,
                "--renew-seconds", "1", "--timeout-seconds", "60", role="runner",
            )
            with subprocess.Popen(command, env=child_env, text=True, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE) as worker:
                try:
                    assert started.wait(12), "the native Claude process did not reach the model fixture"
                    checked(call("shared-ops", "revoke-claude-repository", approval_id,
                                 "--evidence", "fixture:approval withdrawn during a model request"))
                    _, stderr = worker.communicate(timeout=9)
                    assert worker.returncode != 0 and "HTTP 409" in stderr, stderr
                    assert disconnected.wait(2), "the in-flight model connection stayed open after revocation"
                finally:
                    if worker.poll() is None:
                        worker.kill()
            assert len(requests) == 1 and not errors
        assert not (expired / "submission.json").exists()
        audit = checked(call("shared-ops", "audit", next_order["id"]))["audit"]
        assert audit["work"]["state"] == "queued" and audit["attempts"][0]["outcome"] == "expired"
        blocked = checked(call("run-shared", "--repository-map", runner_map, "--request-id", str(uuid4()),
                               "--destination", root / "blocked attempt", "--claude", claude,
                               "--bubblewrap", bubblewrap, role="runner"))
        assert blocked["claim"] is None and not (root / "blocked attempt").exists()
        check_setup(False)

    if codex:
        def executable_digest(path):
            with path.open("rb") as executable:
                return file_digest(executable, "sha256").hexdigest()

        return {
            "schema_version": 1,
            "model_responses": "scripted_loopback",
            "launcher_sha256": executable_digest(kit / "craft-factory"),
            "queue_sha256": executable_digest(kit / "shared/craft-factory-queue"),
            "base_commit": order["base"],
            "verified_tree": claude_tree,
            "candidate_pack_digests": {"codex": codex_receipt["candidate_evidence"],
                                       "claude": receipt["candidate_evidence"]},
            "comparison": comparison,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launcher", type=Path)
    parser.add_argument("claude", type=Path)
    parser.add_argument("bubblewrap", type=Path)
    parser.add_argument("--shared-kit", type=Path, help="Also qualify the extracted shared queue")
    parser.add_argument("--postgres-bin", type=Path, help="PostgreSQL bin directory for shared qualification")
    parser.add_argument("--codex", type=Path, help="Run native Codex on the same task in the shared comparison")
    parser.add_argument("--comparison-report", type=Path, help="Save the native comparison after qualification passes")
    args = parser.parse_args()
    if sys.platform != "linux" or {name for _, name in socket.if_nameindex()} != {"lo"}:
        parser.error("Run this fixture inside a Linux network namespace with only loopback")
    launcher, claude, bubblewrap = (path.resolve(strict=True) for path in (args.launcher, args.claude, args.bubblewrap))
    kit = args.shared_kit.resolve(strict=True) if args.shared_kit else None
    if args.codex and not kit:
        parser.error("--codex requires --shared-kit")
    if args.comparison_report and not args.codex:
        parser.error("--comparison-report requires --codex")
    if kit and launcher != (kit / "craft-factory").resolve(strict=True):
        parser.error("Specify the shared kit's launcher for shared qualification")
    with TemporaryDirectory(prefix="craft-native-claude-") as scratch:
        root = Path(scratch).resolve()
        qualify(launcher, claude, bubblewrap, root)
        print("Installed Claude Code passed native file editing, host-file isolation, candidate import, and usage checks.")
        if kit:
            database = args.postgres_bin.resolve(strict=True) if args.postgres_bin else None
            shared = root / "shared"
            shared.mkdir()
            codex = args.codex.resolve(strict=True) if args.codex else None
            report = qualify_shared(kit, claude, bubblewrap, shared, database, codex=codex)
            if args.comparison_report:
                with args.comparison_report.open("x", encoding="utf-8") as output:
                    json.dump(report, output, indent=2, sort_keys=True)
                    output.write("\n")
            print("Native Claude passed shared approval, candidate replay, independent verification, and revocation checks.")
            if codex:
                print("Native Codex and Claude produced the same verified tree, with both verdicts and usage in one comparison.")


if __name__ == "__main__":
    main()
