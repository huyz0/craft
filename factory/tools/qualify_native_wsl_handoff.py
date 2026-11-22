"""Run a native Codex takeover between Ubuntu WSL and Windows with a disposable queue."""

import argparse
from base64 import b64encode
from contextlib import closing, suppress
from hashlib import file_digest, sha256
from http.client import HTTPSConnection
import json
import os
from pathlib import Path
import re
import signal
import ssl
import subprocess
from tempfile import TemporaryDirectory
from threading import Event
from time import monotonic, sleep
from urllib.parse import urlsplit
from uuid import uuid4

from qualify_native_coordinator import native_codex_fixture
from qualify_native_worker import git, prepare_order, run_native_worker
from qualify_wsl_handoff import REPOSITORY, windows_path
from smoke_shared_queue import disposable_queue


TARGET = "src/greeting.txt"
OUTSIDE_SCOPE = "docs/greeting.txt"
CRASH_LEASE_SECONDS = 15


def clean_environment():
    return {name: value for name, value in os.environ.items() if not name.upper().startswith("CRAFT_FACTORY_")}


def protected_file(path, value):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as stream:
        stream.write(value + "\n")


def runner_command(launcher, origin, ca, repository_map, destination, token):
    return [launcher, "run-shared", "--queue", origin, "--ca", ca, "--repository-map", repository_map,
            "--request-id", str(uuid4()), "--destination", destination, "--runner-token-file", token]


def native_attempt(launcher, codex, origin, ca, fixture, repository_map, order_path, token, target):
    order = json.loads(order_path.read_bytes())
    session = fixture / "model"
    session.mkdir()
    command = runner_command(launcher, origin, ca, repository_map, fixture / "attempt", token)

    def launch(options, temp):
        return subprocess.run([*command, *options, "--renew-seconds", "1"], cwd=fixture, env=clean_environment() | temp,
                              capture_output=True, text=True, timeout=105)

    return run_native_worker(codex, session, order, target, launch)


def checked(result):
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "Native handoff command failed")
    return json.loads(result.stdout)


def wait_until(condition, seconds, message):
    deadline = monotonic() + seconds
    while not condition():
        if monotonic() >= deadline:
            raise RuntimeError(message)
        sleep(0.15)


def group_has_live_processes(group):
    processes = subprocess.check_output(["ps", "-e", "-o", "pgid=,stat="], text=True, timeout=5)
    return any(pgid == str(group) and not state.startswith("Z")
               for pgid, state in (line.split() for line in processes.splitlines()))


def crash_ubuntu_runner(codex, root, command):
    model = root / "model"
    model.mkdir()
    started, disconnected = Event(), Event()

    def hold_request(connection):
        started.set()
        connection.settimeout(12)
        try:
            if not connection.recv(1):
                disconnected.set()
        except ConnectionResetError:
            disconnected.set()
        except TimeoutError:
            pass

    with native_codex_fixture(codex, model, None, hold_connection=hold_request) as (shim, home, requests):
        command.extend(["--codex", shim, "--codex-home", home, "--renew-seconds", "1", "--timeout-seconds", "60"])
        group = None
        with subprocess.Popen(command, cwd=root, env=clean_environment(),
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) as worker:
            try:
                assert started.wait(25) and worker.poll() is None, "Ubuntu worker did not reach the model"
                children = subprocess.check_output(
                    ["ps", "-o", "pid=,pgid=", "--ppid", str(worker.pid)], text=True, timeout=5)
                child, pgid = map(int, children.split())
                assert child == pgid and child != os.getpgrp(), "fixture child does not have its own group"
                group = pgid
                worker.kill()
                assert worker.wait(timeout=5) == -signal.SIGKILL
                wait_until(lambda: not group_has_live_processes(group), 5,
                           "Ubuntu runner left a live worker process after the launcher died")
                assert disconnected.wait(2), "the native model connection stayed open after the crash"
            finally:
                if worker.poll() is None:
                    worker.kill()
                if group is not None and group_has_live_processes(group):
                    with suppress(ProcessLookupError):
                        os.killpg(group, signal.SIGKILL)
        assert len(requests) == 1 and not (home / "auth.json").exists()


def qualify(args, root, windows_root):
    linux_launcher = args.ubuntu_kit / "craft-factory"
    order = prepare_order(root)
    order["execution_profile"]["max_attempts"] = 2
    source = Path(order["repository"])
    work_id = order["id"]
    order_file = root / "order.json"
    order_bytes = json.dumps(order).encode("utf-8")
    order_file.write_bytes(order_bytes)
    windows_order_file = windows_root / "order.json"
    windows_order_file.write_bytes(order_bytes)
    windows_source, verifier_source = (windows_root / name for name in ("runner source", "verifier source"))
    for checkout in (windows_source, verifier_source):
        git(windows_root, "clone", "--quiet", "--no-local", str(source), str(checkout))
    linux_map, windows_map = root / "repo-map.json", windows_root / "repo-map.json"
    verifier_map = windows_root / "verifier-map.json"
    for path, checkout in ((linux_map, str(source)), (windows_map, windows_path(windows_source)),
                           (verifier_map, windows_path(verifier_source))):
        path.write_text(json.dumps({REPOSITORY: {"path": checkout}}), encoding="utf-8")
    database = root / "database"
    database.mkdir()
    roles = {"operator": {}, "runner": {}, "replacement": {"role": "runner"},
             "verifier": {"verifier_repositories": [REPOSITORY]}}

    crash = args.failure == "crash"
    lease_seconds = CRASH_LEASE_SECONDS if crash else 120
    with disposable_queue(args.ubuntu_kit, database, args.postgres_bin, roles,
                          lease_seconds=lease_seconds) as (origin, ca, tokens, env):
        windows_ca = windows_root / "ca.pem"
        windows_ca.write_bytes(ca.read_bytes())
        initial_token, replacement_token = root / "runner-token", windows_root / "runner-token"
        verifier_token = windows_root / "verifier-token"
        for path, role in ((initial_token, "runner"), (replacement_token, "replacement"), (verifier_token, "verifier")):
            protected_file(path, tokens[role])

        def operator(*command):
            return json.loads(subprocess.check_output(
                [linux_launcher, command[0], "--queue", origin, "--ca", ca, *command[1:]],
                cwd=root, env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, timeout=20))

        for runner in ("smoke-runner", "smoke-replacement"):
            assert operator("shared-ops", "set-runner-enrollment", runner, REPOSITORY,
                            "--expected-revision", "0", "--decision", "grant",
                            "--evidence", "fixture:native handoff enrollment")["decision"]["enrolled"]
        operator("shared-work", "resume")
        assert operator("shared-work", "admit", order_file, "--repository", REPOSITORY,
                        "--order-digest", "sha256:" + sha256(order_bytes).hexdigest(),
                        "--event-key", "fixture:native-platform-takeover", "--token-limit", "400")["work_order_id"] == work_id

        if crash:
            initial = runner_command(linux_launcher, origin, ca, linux_map, root / "attempt", initial_token)
            crash_ubuntu_runner(args.ubuntu_codex, root, initial)
            dispatch = operator("shared-ops", "dispatch-status", work_id)["dispatch"]
            assert dispatch["live_lease"], dispatch
            empty_home = windows_root / "unclaimed home"
            empty_home.mkdir()
            probe = runner_command(args.windows_launcher, origin, windows_path(windows_ca), windows_path(windows_map),
                                   windows_path(windows_root / "premature attempt"), windows_path(replacement_token))
            idle = checked(subprocess.run(
                [*probe, "--codex", windows_path(args.windows_codex), "--codex-home", windows_path(empty_home)],
                cwd=windows_root, env=clean_environment(), capture_output=True, text=True, timeout=15))
            assert idle == {"schema_version": 1, "claim": None}
            assert not (windows_root / "premature attempt").exists()
            wait_until(lambda: not operator("shared-ops", "dispatch-status", work_id)["dispatch"]["live_lease"],
                       lease_seconds + 5, "the crashed worker's lease did not expire")
        else:
            first = native_attempt(linux_launcher, args.ubuntu_codex, origin, ca, root, linux_map,
                                   order_file, initial_token, OUTSIDE_SCOPE)
            assert first.returncode and "scope" in first.stderr.lower(), first.stderr
            assert (root / "attempt" / "work" / "checkout" / OUTSIDE_SCOPE).read_text().strip() == "changed greeting"
        assert not (root / "attempt" / "submission.json").exists()
        audit = operator("shared-ops", "audit", work_id)["audit"]
        assert audit["work"]["candidate"] is None
        [stopped] = audit["attempts"]
        assert (stopped["runner_id"], stopped["generation"]) == ("smoke-runner", 1)
        if crash:
            assert stopped["worker_stop_reported_at"] is None and stopped["reported_tokens"] is None
        else:
            assert stopped["worker_stop_reported_at"] and stopped["worker_stop_reported_at"] == stopped["expires_at"]

        # Invoke Python and Codex on Windows. Each model fixture listens only on its own platform.
        replacement = checked(subprocess.run(
            [args.windows_python, "-B", windows_path(Path(__file__).resolve()), "windows-runner",
             windows_path(args.windows_launcher), windows_path(args.windows_codex), origin, windows_path(windows_ca),
             windows_path(windows_root), windows_path(windows_map), windows_path(windows_order_file),
             windows_path(replacement_token)], cwd=windows_root, env=clean_environment(),
            capture_output=True, text=True, timeout=135))
        assert replacement["work_id"] == work_id and replacement["generation"] == 2
        head = replacement["candidate"]
        pack = (windows_root / "attempt" / "work" / "candidate.pack").read_bytes()
        assert replacement["candidate_evidence"] == "sha256:" + sha256(pack).hexdigest()
        assert subprocess.run(["git", "-C", verifier_source, "cat-file", "-e", head],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15).returncode
        address = urlsplit(origin)
        trust = ssl.create_default_context(cafile=str(ca))
        with closing(HTTPSConnection(address.hostname, address.port, context=trust, timeout=10)) as client:
            late = {"action": "submit_pack", "work_id": work_id, "generation": 1, "commit": head,
                    "pack_base64": b64encode(pack).decode("ascii"), "tokens": 300}
            client.request("POST", "/v1/runner", json.dumps(late),
                           {"Authorization": "Bearer " + tokens["runner"], "Content-Type": "application/json"})
            response = client.getresponse()
            assert response.status == 409, (response.status, response.read())

        def verify(command, *arguments):
            return subprocess.run(
                [args.windows_launcher, command, "--queue", origin, "--ca", windows_path(windows_ca), *arguments,
                 "--verifier-token-file", windows_path(verifier_token)],
                cwd=windows_root, env=clean_environment(), capture_output=True, text=True, timeout=30)

        verification = windows_root / "independent verification"
        handoff = checked(verify("prepare-next-verification", "--repository", REPOSITORY, "--repository-map",
                                 windows_path(verifier_map), "--destination", windows_path(verification)))
        assert handoff["generation"] == 2 and handoff["head"] == head
        assert handoff["pack_digest"] == replacement["candidate_evidence"]
        checkout = verification / "checkout"
        assert (checkout / TARGET).read_text().strip() == "changed greeting"
        assert git(checkout, "diff", "--name-only", order["base"], head) == TARGET
        for trusted in (source, windows_source, verifier_source):
            assert git(trusted, "rev-parse", "HEAD") == order["base"]
            assert (trusted / TARGET).read_text().strip() == "original greeting"
        attest = ("attest-shared-verification", "--handoff", windows_path(verification / "handoff.json"),
                  "--result", "passed", "--evidence", "fixture:detached readback",
                  "--note", "Verified the exact replacement candidate and changed file.")
        target = checkout / TARGET
        candidate_bytes = target.read_bytes()
        target.write_bytes(b"fixture tampering\n")
        rejected = verify(*attest)
        assert rejected.returncode and "verification checkout has tracked changes" in rejected.stderr
        target.write_bytes(candidate_bytes)
        verdict = checked(verify(*attest))
        audit = operator("shared-ops", "audit", work_id)["audit"]
        assert audit["work"]["verification"]["sequence"] == verdict["verification_sequence"]
        assert audit["work"]["verification"]["passed"] and audit["work"]["verification"]["verifier"] == "smoke-verifier"
        attempts = sorted(audit["attempts"], key=lambda item: item["generation"])
        assert [(item["generation"], item["runner_id"], item["outcome"]) for item in attempts] == [
            (1, "smoke-runner", "expired"), (2, "smoke-replacement", "candidate")]
        assert attempts[1]["reported_tokens"] == 300
        if crash:
            assert attempts[0]["worker_stop_reported_at"] is None and attempts[0]["reported_tokens"] is None

    digests = {}
    for label, path in (("ubuntu_launcher", linux_launcher), ("windows_launcher", args.windows_launcher),
                        ("queue", args.ubuntu_kit / "shared" / "craft-factory-queue"),
                        ("ubuntu_codex", args.ubuntu_codex), ("windows_codex", args.windows_codex)):
        with path.open("rb") as stream:
            digests[label] = file_digest(stream, "sha256").hexdigest()
    report = {"schema_version": 1, "mode": "single_machine_wsl_and_windows", "model": "scripted_loopback",
            "binary_sha256": digests, "ubuntu_candidate": "launcher_killed" if crash else "outside_scope_rejected",
            "windows_candidate": head, "candidate_pack_sha256": handoff["pack_digest"],
            "stale_generation_submission": "rejected", "tampered_verifier_checkout": "rejected",
            "verification_sequence": verdict["verification_sequence"],
            "attempts": [{key: item[key] for key in ("generation", "runner_id", "outcome", "reported_tokens")}
                         for item in attempts]}
    if crash:
        report["crash_recovery"] = {"lease_seconds": lease_seconds, "live_lease_takeover": "denied",
                                    "lease_expires_at": stopped["expires_at"], "worker_processes": "stopped",
                                    "model_connection": "closed", "worker_stop_receipt": None}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    handoff = commands.add_parser("qualify")
    handoff.add_argument("--failure", choices=("scope", "crash"), default="scope")
    for name in ("ubuntu-kit", "ubuntu-codex", "windows-launcher", "windows-codex", "windows-python",
                 "windows-workspace", "postgres-bin"):
        handoff.add_argument("--" + name, type=Path, required=True)
    remote = commands.add_parser("windows-runner")
    for name in ("launcher", "codex", "origin", "ca", "fixture", "repository_map", "order", "token"):
        remote.add_argument(name, type=str if name == "origin" else Path)
    args = parser.parse_args()
    if args.command == "windows-runner":
        if os.name != "nt":
            parser.error("the replacement runner must execute on Windows")
        result = native_attempt(args.launcher, args.codex, args.origin, args.ca, args.fixture,
                                args.repository_map, args.order, args.token, TARGET)
        print(json.dumps(checked(result)))
        return
    for name, path in vars(args).items():
        if isinstance(path, Path):
            setattr(args, name, path.resolve(strict=True))
    if not re.match(r"^[A-Za-z]:\\", windows_path(args.windows_workspace)):
        parser.error("the Windows workspace must be on a mounted Windows drive")
    with TemporaryDirectory(prefix="craft-native-wsl-") as working, \
         TemporaryDirectory(prefix="craft-native-win-", dir=args.windows_workspace) as windows:
        report = qualify(args, Path(working), Path(windows))
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
