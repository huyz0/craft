"""Exercise a disposable runner takeover and verifier handoff across Ubuntu WSL and Windows."""

import argparse
from base64 import b64encode
from contextlib import closing
from hashlib import sha256
from http.client import HTTPSConnection
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import subprocess
from tempfile import TemporaryDirectory
from urllib.parse import urlsplit
from uuid import uuid4

from qualify_native_worker import git
from smoke_shared_queue import disposable_queue


REPOSITORY = "fixture/cross-platform"
WORK_ID = "mixed-platform-takeover"


def windows_path(path):
    return subprocess.check_output(["wslpath", "-w", path], text=True, timeout=5).strip()


def fixture_candidate(root, windows_root):
    source = root / "source"
    source.mkdir()
    git(source, "init", "-q", "--template=", "-b", "main")
    git(source, "config", "user.name", "Craft fixture")
    git(source, "config", "user.email", "fixture@example.invalid")
    git(source, "config", "core.autocrlf", "false")
    (source / "greeting.txt").write_bytes(b"original\n")
    (source / "SKILL.md").write_bytes(b"# Fixture\nCheck the greeting.\n")
    git(source, "add", ".")
    git(source, "commit", "-qm", "Approved fixture")
    base = git(source, "rev-parse", "HEAD")
    verifier_source = windows_root / "source"
    git(windows_root, "clone", "--no-local", "-q", source, verifier_source)
    (source / "greeting.txt").write_bytes(b"verified\n")
    git(source, "commit", "-qam", "Candidate fixture")
    head = git(source, "rev-parse", "HEAD")
    pack = subprocess.check_output(["git", "-C", source, "pack-objects", "--stdout", "--revs"],
                                   input=f"{head}\n^{base}\n".encode(), timeout=15)
    order = {"schema_version": 1, "id": WORK_ID, "issue": "FIXTURE-1", "owner": "Fixture owner",
             "repository": str(source), "base": base, "intent": "Update the fixture greeting.",
             "acceptance": ["Only greeting.txt changes to the verified greeting."], "scope": ["greeting.txt"],
             "skill": {"location": "repo:SKILL.md", "revision": base},
             "context": [{"location": "fixture:cross-platform", "revision": base}]}
    return order, head, pack, verifier_source


def qualify(linux_kit, windows_launcher, postgres_bin, curl, root, windows_root):
    order, head, pack, verifier_source = fixture_candidate(root, windows_root)
    mapping = windows_root / "map.json"
    mapping.write_text(json.dumps({REPOSITORY: {"path": windows_path(verifier_source)}}), encoding="utf-8")
    roles = {"operator": {}, "runner": {}, "replacement": {"role": "runner"},
             "verifier": {"verifier_repositories": [REPOSITORY]}}
    database = root / "db"
    database.mkdir()
    with disposable_queue(linux_kit, database, postgres_bin, roles) as (origin, ca, tokens, env):
        address = urlsplit(origin)
        trust = ssl.create_default_context(cafile=str(ca))

        def request(role, action, expected=200, **fields):
            with closing(HTTPSConnection(address.hostname, address.port, context=trust, timeout=10)) as client:
                client.request("POST", f"/v1/{role}", json.dumps({"action": action, **fields}),
                               {"Authorization": "Bearer " + tokens[role], "Content-Type": "application/json"})
                response = client.getresponse()
                body = json.loads(response.read())
                assert response.status == expected, (action, response.status, body)
                return body

        ca_file = windows_root / "ca.pem"
        shutil.copyfile(ca, ca_file)
        for name, data in (("verifier-token", tokens["verifier"]),
                           ("runner-header", "Authorization: Bearer " + tokens["replacement"])):
            with os.fdopen(os.open(windows_root / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600),
                           "w", encoding="ascii") as credential:
                credential.write(data + "\n")

        def replacement(action, **fields):
            raw = subprocess.check_output([curl, "--disable", "--silent", "--show-error", "--noproxy", "*",
                                           "--ssl-no-revoke",
                                           "--max-time", "15", "--cacert", windows_path(ca_file),
                                           "--header", "@" + windows_path(windows_root / "runner-header"),
                                           "--header", "Content-Type: application/json", "--data-binary", "@-",
                                           "--write-out", "\n%{http_code}", origin + "/v1/runner"],
                                          input=json.dumps({"action": action, **fields}), text=True, timeout=20)
            payload, status = raw.rsplit("\n", 1)
            assert status == "200", (action, status, payload)
            return json.loads(payload)

        def verifier(command, *args):
            return json.loads(subprocess.check_output(
                [windows_launcher, command, "--queue", origin, "--ca", windows_path(ca_file), *args,
                 "--verifier-token-file", windows_path(windows_root / "verifier-token")],
                cwd=windows_root, timeout=30))

        def operator(*args):
            return json.loads(subprocess.check_output(
                [linux_kit / "craft-factory", "shared-ops", "--queue", origin, "--ca", ca, *args],
                env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, timeout=15))

        for runner in ("smoke-runner", "smoke-replacement"):
            enrolled = operator("set-runner-enrollment", runner, REPOSITORY, "--expected-revision", "0",
                                "--decision", "grant", "--evidence", "fixture:platform enrollment")
            assert enrolled["decision"]["enrolled"]
        request("operator", "set_paused", paused=False)
        request("operator", "admit", repository=REPOSITORY, event_key="fixture:mixed-platform",
                work_order=order, token_limit=25)
        lease = request("runner", "claim", request_id=str(uuid4()), available_repositories=[REPOSITORY])["claim"]
        assert lease["work_id"] == WORK_ID and lease["generation"] == 1
        first = {"work_id": WORK_ID, "generation": lease["generation"]}
        request("runner", "start_attempt", **first)
        assert replacement("claim", request_id=str(uuid4()), available_repositories=[REPOSITORY])["claim"] is None
        assert request("runner", "finish_failed_attempt", **first, tokens=None)["recorded"]

        lease = replacement("claim", request_id=str(uuid4()), available_repositories=[REPOSITORY])["claim"]
        assert lease["work_id"] == WORK_ID and lease["generation"] == 2
        current = {"work_id": WORK_ID, "generation": lease["generation"]}
        replacement("start_attempt", **current)
        candidate = {"commit": head, "pack_base64": b64encode(pack).decode(), "tokens": 5}
        request("runner", "submit_pack", 409, **first, **candidate)
        uploaded = replacement("submit_pack", **current, **candidate)

        destination = windows_root / "verification"
        handoff = verifier("prepare-next-verification", "--repository", REPOSITORY, "--repository-map",
                           windows_path(mapping), "--destination", windows_path(destination))
        assert all(handoff[key] == value for key, value in current.items())
        assert handoff["head"] == head and handoff["pack_digest"] == uploaded["candidate_evidence"]
        assert handoff["pack_digest"] == "sha256:" + sha256(pack).hexdigest()
        assert (destination / "checkout/greeting.txt").read_bytes() == b"verified\n"
        assert (verifier_source / "greeting.txt").read_bytes() == b"original\n"
        verdict = verifier("attest-shared-verification", "--handoff", windows_path(destination / "handoff.json"),
                           "--result", "passed", "--evidence", "fixture:windows readback",
                           "--note", "Checked the transferred greeting.")
        stored = request("operator", "status", work_id=WORK_ID)["work"]["verification"]
        assert stored["passed"] and stored["verifier"] == "smoke-verifier"
        assert stored["sequence"] == verdict["verification_sequence"]
        attempts = sorted(operator("audit", WORK_ID)["audit"]["attempts"], key=lambda row: row["generation"])
        assert [(row["runner_id"], row["outcome"]) for row in attempts] == [
            ("smoke-runner", "expired"), ("smoke-replacement", "candidate")]

    return {"schema_version": 1, "mode": "single_host_loopback_fixture", "queue": "ubuntu",
            "initial_runner": "ubuntu protocol fixture", "replacement_runner": "windows protocol fixture",
            "verifier": "windows launcher", "attempts": [first, current], "stale_submission": "rejected",
            "candidate_head": head, "candidate_pack_sha256": handoff["pack_digest"],
            "verdict": "passed", "verification_sequence": verdict["verification_sequence"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ubuntu_kit", type=Path)
    parser.add_argument("windows_launcher", type=Path)
    parser.add_argument("windows_workspace", type=Path, help="Existing private scratch directory on a Windows drive")
    parser.add_argument("--postgres-bin", type=Path, required=True)
    parser.add_argument("--windows-curl", default=shutil.which("curl.exe"))
    args = parser.parse_args()
    if not shutil.which("wslpath") or not args.windows_curl:
        parser.error("Run on WSL with Windows interop and curl.exe available")
    workspace = args.windows_workspace.resolve(strict=True)
    if not workspace.is_dir() or not re.match(r"^[A-Za-z]:\\", windows_path(workspace)):
        parser.error("Provide an existing scratch directory on a mounted Windows drive")
    with TemporaryDirectory(prefix="craft-wsl-") as root, \
         TemporaryDirectory(prefix="craft-wsl-", dir=workspace) as windows_root:
        report = qualify(args.ubuntu_kit.resolve(strict=True), args.windows_launcher.resolve(strict=True),
                         args.postgres_bin.resolve(strict=True), args.windows_curl, Path(root), Path(windows_root))
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
