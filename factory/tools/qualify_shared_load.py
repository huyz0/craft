"""Exercise a packaged queue under bounded contention with disposable PostgreSQL."""

import argparse
from base64 import b64encode
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from hashlib import file_digest, sha256
from http.client import HTTPSConnection
import json
import math
import os
from pathlib import Path
import ssl
import subprocess
from tempfile import TemporaryDirectory
from threading import Lock
from time import perf_counter
from urllib.parse import urlsplit
from uuid import uuid4

from qualify_native_worker import git, prepare_order
from smoke_shared_queue import disposable_queue


REPOSITORY = "fixture/queue-load"
ORDER_COUNT = 24
CONCURRENCY = 8
REVIEW_LIMIT = 4


def candidate_fixture(root):
    order = prepare_order(root)
    del order["execution_profile"]
    repo = Path(order["repository"])
    (repo / "src/greeting.txt").write_bytes(b"changed greeting\n")
    git(repo, "commit", "-qam", "Synthetic candidate")
    head = git(repo, "rev-parse", "HEAD")
    git_env = {key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")}
    pack = subprocess.check_output(["git", "-C", repo, "pack-objects", "--revs", "--stdout"],
                                   input=f"{head}\n^{order['base']}\n".encode(), env=git_env, timeout=15)
    return order, head, pack


def qualify(kit, root, postgres_bin):
    queue_binary = "craft-factory-queue.exe" if os.name == "nt" else "craft-factory-queue"
    with (kit / "shared" / queue_binary).open("rb") as executable:
        queue_sha256 = file_digest(executable, "sha256").hexdigest()
    order, head, pack = candidate_fixture(root)
    digest = "sha256:" + sha256(pack).hexdigest()
    candidate = {"commit": head, "pack_base64": b64encode(pack).decode("ascii"), "tokens": 5}
    roles = {"operator": {}, "metrics": {"role": "metrics_reader"}}
    runners = [f"runner-{number}" for number in range(CONCURRENCY)]
    verifiers = [f"verifier-{number}" for number in range(CONCURRENCY)]
    roles.update({name: {"role": "runner"} for name in runners})
    roles.update({name: {"role": "verifier", "verifier_repositories": [REPOSITORY]} for name in verifiers})
    samples = defaultdict(list)
    timing_lock = Lock()
    orders = {f"load-{number:02d}" for number in range(ORDER_COUNT)}
    processed = set()
    batches = []
    database = root / "db"
    database.mkdir()
    with disposable_queue(kit, database, postgres_bin, roles, review_limit=REVIEW_LIMIT) as (origin, ca, tokens, _):
        address = urlsplit(origin)
        trust = ssl.create_default_context(cafile=str(ca))

        def request(route, identity, action=None, **fields):
            path = f"/v1/{route}" if action else "/metrics"
            headers = {"Authorization": "Bearer " + tokens[identity]}
            started = perf_counter()
            with closing(HTTPSConnection(address.hostname, address.port, context=trust, timeout=15)) as connection:
                if action:
                    headers["Content-Type"] = "application/json"
                    connection.request("POST", path, json.dumps({"action": action, **fields}), headers)
                else:
                    connection.request("GET", path, headers=headers)
                response = connection.getresponse()
                raw = response.read()
                assert response.status == 200, (route, action, response.status, raw)
            with timing_lock:
                samples[action or "metrics"].append((perf_counter() - started) * 1000)
            return json.loads(raw) if action else raw.decode("ascii")

        def summary():
            return request("operations", "operator", "summary")["summary"]

        def claim(name):
            result = request("runner", name, "claim", request_id=str(uuid4()),
                             available_repositories=[REPOSITORY])["claim"]
            return name, result

        def submit(pair):
            name, lease = pair
            binding = {"work_id": lease["work_id"], "generation": lease["generation"]}
            request("runner", name, "start_attempt", **binding)
            receipt = request("runner", name, "submit_pack", **binding, **candidate)
            assert receipt["candidate_evidence"] == digest

        def pickup(name):
            result = request("verifier", name, "claim_pending_verification",
                             repository=REPOSITORY)["verification_pickup"]
            return name, result

        def attest(pair):
            name, record = pair
            assert record["head"] == head and record["pack_digest"] == digest
            report = request("verifier", name, "record_verification", work_id=record["work_id"],
                             generation=record["generation"], head=head, digest=digest, previous_sequence=0,
                             passed=True, evidence="fixture:known checkout", note="Synthetic load fixture.",
                             pickup_id=record["pickup_id"])
            assert report["verification_sequence"] > 0
            return record["work_id"]

        def cancel(work_id):
            assert request("operator", "operator", "cancel", work_id=work_id,
                           reason="Load fixture complete")["cancelled"]

        for name in runners:
            request("operations", "operator", "set_runner_enrollment", runner_id="smoke-" + name,
                    repository=REPOSITORY, expected_revision=0, enrolled=True, evidence="fixture:load enrollment")
        request("operator", "operator", "set_paused", paused=False)
        for work_id in sorted(orders):
            admitted = request("operator", "operator", "admit", repository=REPOSITORY, event_key=work_id,
                               work_order=order | {"id": work_id}, token_limit=25)
            assert admitted["work_order_id"] == work_id

        request("operator", "operator", "set_paused", paused=True)
        assert claim(runners[0])[1] is None and summary()["queued"] == ORDER_COUNT
        request("operator", "operator", "set_paused", paused=False)
        with ThreadPoolExecutor(max_workers=CONCURRENCY) as pool:
            while processed != orders:
                expected = min(REVIEW_LIMIT, len(orders - processed))
                leases = [(name, lease) for name, lease in pool.map(claim, runners) if lease]
                current = {lease["work_id"] for _, lease in leases}
                assert len(leases) == len(current) == expected and current <= orders - processed
                assert all(lease["generation"] == 1 for _, lease in leases)
                assert summary()["review_reserved"] == expected
                list(pool.map(submit, leases))
                waiting = summary()
                assert waiting["review_reserved"] == waiting["waiting_verification_candidates"] == expected
                assert waiting["active_verification_pickups"] == 0
                pickups = [(name, record) for name, record in pool.map(pickup, verifiers) if record]
                assert len(pickups) == expected and {record["work_id"] for _, record in pickups} == current
                active = summary()
                assert active["active_verification_pickups"] == expected
                assert active["waiting_verification_candidates"] == 0
                assert f"craft_queue_active_verification_pickups {expected}\n" in request("metrics", "metrics")
                assert set(pool.map(attest, pickups)) == current
                list(pool.map(cancel, sorted(current)))
                assert summary()["review_reserved"] == 0
                processed.update(current)
                batches.append(len(current))

        final = summary()
        assert final["queued"] == final["live_leases"] == final["waiting_verification_candidates"] == 0
        assert final["active_verification_pickups"] == 0
        assert final["tokens_charged_or_reserved"] == ORDER_COUNT * candidate["tokens"]

    def measurement(durations):
        durations.sort()
        index = math.ceil(len(durations) * 0.95) - 1
        return {"samples": len(durations), "p95_ms": round(durations[index], 2)}

    return {"schema_version": 1, "mode": "disposable_loopback_fixture",
            "platform": "windows" if os.name == "nt" else "linux",
            "queue_sha256": queue_sha256,
            "orders": len(processed), "concurrent_clients": CONCURRENCY,
            "review_limit": REVIEW_LIMIT, "batch_sizes": batches,
            "verified_orders": len(processed), "fixture_tokens_recorded": final["tokens_charged_or_reserved"],
            "request_latency": {action: measurement(values) for action, values in sorted(samples.items())}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kit", type=Path)
    parser.add_argument("--postgres-bin", type=Path, required=True)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="craft-queue-load-") as scratch:
        result = qualify(args.kit.resolve(strict=True), Path(scratch), args.postgres_bin.resolve(strict=True))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
