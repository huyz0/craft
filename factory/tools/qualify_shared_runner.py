"""Check an extracted shared runner against a disposable loopback queue."""

import argparse
from base64 import b64encode
from hashlib import sha256
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import ssl
import subprocess
from tempfile import TemporaryDirectory
from threading import Thread
from uuid import uuid4

from disposable_tls import create_tls_files


REPOSITORY = "fixture/service"


def tls_context(root):
    create_tls_files(root)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(root / "server.pem", root / "server.key")
    return context


def qualify(launcher, root):
    checkout = root / "local checkout"
    attempts = root / "attempt root"
    home = root / "codex home"
    for directory in (checkout, attempts, home):
        directory.mkdir()
    subprocess.run(["git", "init", "--quiet", checkout], check=True, timeout=10)
    mapping = root / "checkout map.json"
    mapping.write_text(json.dumps({REPOSITORY: {"path": str(checkout)}}), encoding="utf-8")
    env = {name: value for name, value in os.environ.items() if not name.upper().startswith("CRAFT_FACTORY_")}
    env["CRAFT_FACTORY_RUNNER_TOKEN"] = "wrong-ambient-token-for-runner-fixture"
    env["CRAFT_FACTORY_OPERATOR_TOKEN"] = "wrong-ambient-token-for-operator-fixture"

    def invoke(command, *options):
        return subprocess.run([launcher, command, *options], check=True, capture_output=True, text=True,
                              env=env, timeout=15).stdout

    token_file = root / "runner token"
    invoke("issue-queue-identity", "runner-fixture", "--role", "runner", "--token-file", token_file)
    token = token_file.read_text(encoding="ascii").strip()
    operator_file = root / "operator token"
    invoke("issue-queue-identity", "operator-fixture", "--role", "operator", "--token-file", operator_file)
    operator = operator_file.read_text(encoding="ascii").strip()
    verifier_file = root / "scoped verifier token"
    verifier = json.loads(invoke("issue-queue-identity", "verifier-fixture", "--role", "verifier",
                                 "--verifier-repository", REPOSITORY, "--token-file", verifier_file))
    assert verifier["role"] == "verifier" and verifier["verifier_repositories"] == [REPOSITORY]
    verifier_token = verifier_file.read_text(encoding="ascii").strip()
    order = {"schema_version": 1, "id": "fixture-order", "issue": "FIXTURE-1", "owner": "Operator",
             "repository": str(checkout), "base": "1" * 40, "intent": "Check packaged admission.",
             "acceptance": ["The declared check passes."], "scope": ["src"],
             "context": [{"location": "issue:FIXTURE-1", "revision": "reviewed"}],
             "skill": {"location": "repo:skills/fixture", "revision": "1" * 40}}
    order_bytes = json.dumps(order).encode("utf-8")
    order_file = root / "approved work order.json"
    order_file.write_bytes(order_bytes)
    stop = attempts / ".stop"
    polls = []
    admissions = []
    releases = []
    uploads = []
    pack = b"PACK\0fixture"
    pack_digest = "sha256:" + sha256(pack).hexdigest()
    pickup_id = "11111111-2222-3333-4444-555555555555"

    class QueueFixture(BaseHTTPRequestHandler):
        def do_POST(self):
            identity = {"/v1/runner": token, "/v1/operator": operator, "/v1/verifier": verifier_token}.get(self.path)
            if identity is None or self.headers.get("Authorization") != "Bearer " + identity:
                self.send_error(401)
                return
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path == "/v1/operator" and body.get("action") == "admit":
                admissions.append(body)
                response = {"work_order_id": "fixture-order"}
            elif self.path == "/v1/verifier" and body.get("action") == "claim_pending_verification":
                response = {"verification_pickup": {"repository": REPOSITORY, "work_id": "fixture-order",
                             "generation": 1, "head": "2" * 40, "pack_digest": "sha256:" + "3" * 64,
                             "pickup_id": pickup_id, "pickup_expires_at": "2026-01-01T12:00:00Z"}}
            elif self.path == "/v1/verifier" and body.get("action") == "release_verification_pickup":
                releases.append(body["pickup_id"])
                response = {"released": True}
            elif self.path == "/v1/verifier" and body == {"action": "download_pack", "work_id": "fixture-order"}:
                response = {"candidate": {"work_id": "fixture-order", "repository": REPOSITORY, "generation": 2,
                                          "commit": "2" * 40, "digest": "sha256:" + "3" * 64,
                                          "work_order": {}, "pack_base64": ""}}
            elif self.path != "/v1/runner":
                self.send_error(400)
                return
            elif body == {"action": "check_enrollment", "mapped_repositories": [REPOSITORY]}:
                response = {"setup": {"runner_id": "runner-fixture", "mode": "active", "queue_paused": False,
                                      "enrolled_repositories": [REPOSITORY]}}
            elif body.get("action") == "submit_pack":
                uploads.append(body)
                if len(uploads) == 1:
                    self.send_error(503)
                    return
                response = {"candidate_evidence": pack_digest}
            elif body.get("action") == "claim" and body.get("available_repositories") == [REPOSITORY]:
                polls.append(body["request_id"])
                if len(polls) == 1:
                    self.send_error(503)
                    return
                stop.touch()
                response = {"claim": None}
            else:
                self.send_error(400)
                return
            payload = json.dumps({"schema_version": 1, **response}).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *_args):
            pass

    with HTTPServer(("127.0.0.1", 0), QueueFixture) as server:
        server.socket = tls_context(root).wrap_socket(server.socket, server_side=True)
        worker = Thread(target=server.serve_forever)
        worker.start()
        try:
            address = f"https://127.0.0.1:{server.server_port}"
            admitted = invoke("shared-work", "--queue", address, "--ca", root / "ca.pem",
                              "--operator-token-file", operator_file, "admit", order_file, "--repository", REPOSITORY,
                              "--order-digest", "sha256:" + sha256(order_bytes).hexdigest(),
                              "--event-key", "fixture:approved", "--token-limit", "25")
            if (json.loads(admitted)["work_order_id"] != "fixture-order" or operator in admitted
                    or len(admissions) != 1 or admissions[0]["work_order"]["repository"] != REPOSITORY
                    or admissions[0]["event_key"] != "fixture:approved" or admissions[0]["token_limit"] != 25):
                raise RuntimeError("Packaged admission failed to send the expected approved work order and repository key")
            options = ("--queue", address, "--ca", root / "ca.pem", "--runner-token-file", token_file,
                       "--repository-map", mapping, "--attempt-root", attempts, "--codex", launcher,
                       "--codex-home", home)
            preflight = json.loads(invoke("check-shared-runner", *options))
            if not preflight["ready"] or preflight["usable_repositories"] != 1 or polls:
                raise RuntimeError("Runner preflight failed or took a lease")

            stale_destination = root / "stale candidate"
            stale = subprocess.run([launcher, "prepare-next-verification", "--queue", address,
                                    "--ca", root / "ca.pem", "--repository", REPOSITORY,
                                    "--repository-map", mapping, "--destination", stale_destination,
                                    "--verifier-token-file", verifier_file],
                                   capture_output=True, text=True, timeout=15, env=env)
            if stale.returncode == 0 or "candidate is no longer current" not in stale.stderr or stale_destination.exists():
                raise RuntimeError("The next-candidate handoff accepted a changed generation with the same commit.")
            if releases != [pickup_id]:
                raise RuntimeError("The rejected handoff did not return its pickup to the queue.")

            status_file = attempts / ".runner-status.json"
            loop = (*options, "--write-status", "--poll-seconds", "1")
            first = json.loads(invoke("run-shared-loop", *loop, "--max-polls", "1"))
            status = json.loads(status_file.read_text(encoding="utf-8"))
            pending = attempts / ".pending" / first["request_id"]
            if (first["state"] != "retrying" or not pending.is_dir() or status["state"] != "stopped"
                    or first["reason"] != "queue_unavailable" or status["last_poll"]["reason"] != first["reason"]
                    or status["last_poll"]["state"] != "retrying" or status["last_poll"]["http_status"] != 503
                    or status["last_poll"]["request_id"] != first["request_id"]):
                raise RuntimeError("Runner did not retain and report the interrupted poll")

            second = json.loads(invoke("run-shared-loop", *loop))
            report = status_file.read_text(encoding="utf-8")
            status = json.loads(report)
            if (polls != [first["request_id"]] * 2 or second["state"] != "idle" or not stop.is_file()
                    or any(pending.parent.iterdir()) or status["state"] != "stopped" or token in report
                    or status["last_poll"]["state"] != "idle" or status["last_poll"]["request_id"] != polls[-1]):
                raise RuntimeError("Runner did not replay the poll and report a clean stop")

            stop.unlink()

            def interrupted_submission(digest):
                request = str(uuid4())
                marker = pending.parent / request
                marker.mkdir()
                destination = attempts / request
                (destination / "work").mkdir(parents=True)
                (destination / "work/candidate.pack").write_bytes(pack)
                temporary = destination / "submission.pending"
                temporary.write_text(json.dumps({"endpoint": address + "/v1/runner", "request_id": request,
                                                 "work_id": "fixture-order", "generation": 1, "commit": "2" * 40,
                                                 "tokens": 5, "digest": digest}), encoding="utf-8")
                return marker, temporary, destination / "submission.json"

            corrupt, temporary, committed = interrupted_submission("sha256:" + "0" * 64)
            rejected = json.loads(invoke("run-shared-loop", *loop, "--max-polls", "1"))
            if (rejected["state"] != "attempt_failed" or rejected["reason"] != "candidate_pack_mismatch"
                    or uploads or corrupt.exists() or committed.exists()):
                raise RuntimeError("Runner accepted a pending submission whose candidate digest did not match")
            assert temporary.is_file()
            marker, temporary, committed = interrupted_submission(pack_digest)
            retry = json.loads(invoke("run-shared-loop", *loop, "--max-polls", "1"))
            if retry["state"] != "retrying" or not marker.is_dir() or temporary.exists() or not committed.is_file():
                raise RuntimeError("Runner did not recover and retain the interrupted prepared submission")
            recovered = json.loads(invoke("run-shared-loop", *loop, "--max-polls", "1"))
            expected_upload = {"action": "submit_pack", "work_id": "fixture-order", "generation": 1,
                               "commit": "2" * 40, "tokens": 5, "pack_base64": b64encode(pack).decode("ascii")}
            if (recovered["state"] != "submitted" or recovered["request_id"] != marker.name or marker.exists()
                    or uploads != [expected_upload] * 2 or polls != [first["request_id"]] * 2):
                raise RuntimeError("Runner did not replay the exact candidate without making another claim")
            report = json.loads(status_file.read_text(encoding="utf-8"))
            failure = report["last_failed_poll"]
            if (not failure or failure["request_id"] != rejected["request_id"]
                    or failure["reason"] != rejected["reason"]):
                raise RuntimeError("Runner restart discarded the previous failed poll")
            private_marker = "fixture-private-report-value"
            report["last_failed_poll"]["request_id"] = private_marker
            report["last_failed_run"] = {"http_status": 401, "reason": private_marker, "failed_at": 2}
            status_file.write_text(json.dumps(report), encoding="utf-8")
            stop.touch()
            assert not invoke("run-shared-loop", *loop, "--max-polls", "1")
            report_text = status_file.read_text(encoding="utf-8")
            sanitized = json.loads(report_text)
            if private_marker in report_text or sanitized["last_failed_poll"] or sanitized["last_failed_run"]:
                raise RuntimeError("Runner copied invalid failure metadata from the prior report")
        finally:
            server.shutdown()
            worker.join()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launcher", type=Path)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="factory runner fixture ") as scratch:
        qualify(args.launcher.resolve(strict=True), Path(scratch).resolve())
    print("Extracted launcher passed authenticated admission, restart replay, and stale verification rejection.")


if __name__ == "__main__":
    main()
