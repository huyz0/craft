"""Rehearse an in-place upgrade between extracted Factory shared-service kits."""

import argparse
from hashlib import file_digest, sha256
from http.client import HTTPSConnection
import json
from pathlib import Path
import ssl
import subprocess
import sys
from tempfile import TemporaryDirectory
from urllib.parse import urlsplit
from uuid import uuid4

from smoke_shared_queue import disposable_queue_database, queue_server, smoke_handoff


REPOSITORY = "fixture/upgrade"
VERIFIED = "installed-kit-smoke"
PENDING = "pending-kit-upgrade"


def qualify(previous, current, root, postgres_bin):
    suffix = ".exe" if sys.platform == "win32" else ""
    roles = {"operator": {}, "runner": {}, "verifier": {"verifier_repositories": [REPOSITORY]}}
    with disposable_queue_database(root, postgres_bin, roles) as database:
        migrate, _, command, service_env, ca, tokens, env = database
        prior_command = command(previous)
        next_command = command(current)

        def preflight(service, *, accepted):
            result = subprocess.run(service + ["--check-config"], cwd=root, env=service_env,
                                    capture_output=True, text=True, timeout=15)
            if accepted:
                assert result.returncode == 0, result.stderr
                assert json.loads(result.stdout)["database"]["paused"] is True
            else:
                assert result.returncode != 0 and result.stdout == "", result.stderr
                assert "the shared queue" in result.stderr or "the service login" in result.stderr, result.stderr
                startup = subprocess.run(service, cwd=root, env=service_env, capture_output=True,
                                         text=True, timeout=15)
                assert startup.returncode != 0 and startup.stdout == "", startup.stderr
                assert startup.stderr == result.stderr

        def cli(kit, origin, action, *arguments):
            return json.loads(subprocess.check_output(
                [kit / f"craft-factory{suffix}", action, "--queue", origin, "--ca", ca, *arguments],
                cwd=root, env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, timeout=15))

        def claim(origin, request_id):
            address = urlsplit(origin)
            connection = HTTPSConnection(address.hostname, address.port, timeout=10,
                                         context=ssl.create_default_context(cafile=str(ca)))
            try:
                body = {"action": "claim", "request_id": request_id, "available_repositories": [REPOSITORY]}
                connection.request("POST", "/v1/runner", json.dumps(body),
                                   {"Authorization": "Bearer " + tokens["runner"], "Content-Type": "application/json"})
                response = connection.getresponse()
                assert response.status == 200, response.status
                return json.loads(response.read())["claim"]
            finally:
                connection.close()

        previous_install = migrate(previous)
        preflight(prior_command, accepted=True)
        preflight(next_command, accepted=False)
        with queue_server(prior_command, root, service_env) as origin:
            smoke_handoff(previous / f"craft-factory{suffix}", root, origin, ca, tokens, env, REPOSITORY)
            before = cli(previous, origin, "shared-ops", "audit", VERIFIED)["audit"]
            [verdict] = before["verifications"]
            assert verdict["passed"]

            pending_order = root / "pending work order.json"
            template = json.loads((root / "approved order.json").read_bytes())
            order_bytes = json.dumps(template | {"id": PENDING}).encode("utf-8")
            pending_order.write_bytes(order_bytes)
            admission = ("admit", pending_order, "--repository", REPOSITORY,
                         "--order-digest", "sha256:" + sha256(order_bytes).hexdigest(),
                         "--event-key", "fixture:pending-upgrade", "--token-limit", "25")
            assert cli(previous, origin, "shared-work", "resume")["paused"] is False
            assert cli(previous, origin, "shared-work", *admission)["work_order_id"] == PENDING
            assert cli(previous, origin, "shared-work", "pause")["paused"] is True
            queued = cli(previous, origin, "shared-work", "status", PENDING)["work"]
            assert queued["state"] == "queued" and claim(origin, str(uuid4())) is None

        applied = migrate(current)
        migrations = [line.removeprefix("Applying ") for line in applied.splitlines() if line.startswith("Applying ")]
        assert migrations, applied
        assert "Applying " not in migrate(current)
        preflight(prior_command, accepted=False)
        preflight(next_command, accepted=True)
        with queue_server(next_command, root, service_env) as origin:
            assert cli(current, origin, "shared-ops", "audit", VERIFIED)["audit"] == before
            assert cli(current, origin, "shared-work", "status", PENDING)["work"] == queued
            assert claim(origin, str(uuid4())) is None
            assert cli(current, origin, "shared-work", "resume")["paused"] is False
            assert cli(current, origin, "shared-work", *admission)["work_order_id"] == PENDING
            request_id = str(uuid4())
            lease = claim(origin, request_id)
            assert lease["work_id"] == PENDING and lease["generation"] == 1
            assert claim(origin, request_id) == lease
            assert cli(current, origin, "shared-work", "pause")["paused"] is True

        binaries = {}
        for version, service in (("previous", prior_command), ("current", next_command)):
            with service[0].open("rb") as executable:
                binaries[version] = file_digest(executable, "sha256").hexdigest()
        return {"schema_version": 1, "platform": sys.platform, "queue_sha256": binaries,
                "previous_install": previous_install.splitlines()[-1], "applied_migrations": migrations,
                "migration_replay": "unchanged", "incompatible_startup": "rejected in both directions",
                "pause_preserved": True, "verified_work": VERIFIED, "verdict_sequence": verdict["sequence"],
                "resumed_work": PENDING, "resumed_generation": lease["generation"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("previous", type=Path, help="Extracted previous shared-service kit")
    parser.add_argument("current", type=Path, help="Extracted shared-service kit to qualify")
    parser.add_argument("--postgres-bin", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    if sys.platform not in {"linux", "win32"}:
        parser.error("run on Windows or Linux")
    with TemporaryDirectory(prefix="craft-queue-upgrade-", dir="/tmp" if sys.platform == "linux" else None) as scratch:
        report = qualify(args.previous.resolve(strict=True), args.current.resolve(strict=True), Path(scratch),
                         args.postgres_bin.resolve(strict=True) if args.postgres_bin else None)
    if args.report:
        with args.report.open("x", encoding="utf-8") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
    else:
        print(json.dumps(report))


if __name__ == "__main__":
    main()
