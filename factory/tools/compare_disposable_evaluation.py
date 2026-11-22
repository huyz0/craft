"""Replay the paired simulated M3 scorecard and M6 queue comparison."""

import argparse
from base64 import b64encode
import csv
from hashlib import sha256
from http.client import HTTPSConnection
import json
import os
from pathlib import Path
import ssl
import subprocess
import sys
from tempfile import TemporaryDirectory
from urllib.parse import urlsplit
from uuid import uuid4

from smoke_shared_queue import disposable_queue
from summarize_scorecard import COUNTS, HUMAN, IDENTITY, MEASURES, summarize


REPOSITORY = "fixture/evaluation"
RESERVATION = 400
ADAPTERS = {"baseline": "codex", "challenger": "claude"}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")


def scorecard(cases, output):
    path = output / "m3-scorecard.csv"
    columns = (*IDENTITY, *COUNTS, *MEASURES, "currency", "acceptance_evidence")
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, columns, lineterminator="\n")
        writer.writeheader()
        for case in cases:
            for lane in ("ordinary", "factory"):
                (outcome, started, finished, abandoned, repairs, queue, elapsed,
                 minutes, inputs, cached, tokens, cost, misses) = case[lane]
                writer.writerow({
                    "comparison_path": lane.title(), "work_item": case["id"], "task_class": case["class"],
                    "outcome": outcome, "review_kind": "simulated", "started_attempts": started,
                    "finished_attempts": finished, "abandoned_attempts": abandoned, "repair_cycles": repairs,
                    "queue_minutes": queue, "elapsed_minutes": elapsed, **dict(zip(HUMAN, minutes, strict=True)),
                    "input_tokens": inputs, "cached_input_tokens": cached, "output_tokens": tokens,
                    "metered_cost": cost, "currency": "USD", "material_misses": misses,
                    "acceptance_evidence": f"fixture:simulated/{case['id']}/{lane}",
                })
    report = summarize(path)
    if any(item["cases"] != len(cases) or item["review_kinds"] != {"simulated": len(cases)}
           or item["accepted_changes"] for item in report["paths"].values()):
        raise RuntimeError("The mock scorecard must contain both paths without human acceptance.")
    fields = ("started_attempts", "finished_attempts", "abandoned_attempts", "repair_cycles",
              "elapsed_minutes", "queue_minutes", "input_tokens", "cached_input_tokens",
              "output_tokens", "material_misses")
    return {
        lane: {key: data[key] for key in ("cases", "outcomes", "review_kinds", "accepted_changes",
                                         "human_active_minutes", "metered_cost")}
        | {"measurements": {key: data["measurements"][key] for key in fields},
           "by_task_class": {key: values["outcomes"] for key, values in data["task_classes"].items()}}
        for lane, data in report["paths"].items()
    }


def compare_profiles(cases, kit, postgres_bin, scratch):
    launcher = kit / ("craft-factory.exe" if os.name == "nt" else "craft-factory")
    database = scratch / "db"
    database.mkdir()
    checkout = scratch / "checkout"
    checkout.mkdir()
    profiles = {
        side: {"id": f"mock-{adapter}", "model": f"mock-{adapter}",
               "reasoning_effort": "medium" if side == "baseline" else "high",
               "timeout_seconds": 30, "max_attempts": 1, "approval_reference": f"fixture:simulated/{adapter}"}
        for side, adapter in ADAPTERS.items()
    }
    roles = {"operator": {}, "runner": {}, "verifier": {"verifier_repositories": [REPOSITORY]}}
    with disposable_queue(kit, database, postgres_bin, roles, review_limit=2 * len(cases)) as (origin, ca, tokens, env):
        address = urlsplit(origin)
        trust = ssl.create_default_context(cafile=str(ca))

        def request(endpoint, role, action, **fields):
            connection = HTTPSConnection(address.hostname, address.port, context=trust, timeout=15)
            try:
                connection.request("POST", f"/v1/{endpoint}", json.dumps({"action": action, **fields}),
                                   {"Authorization": f"Bearer {tokens[role]}", "Content-Type": "application/json"})
                response = connection.getresponse()
                body = json.loads(response.read())
                if response.status != 200:
                    raise RuntimeError(f"Fixture {action} failed with HTTP {response.status}: {body}")
                return body
            finally:
                connection.close()

        def cli(command, *args):
            result = subprocess.run([launcher, command, "--queue", origin, "--ca", ca, *args],
                                    cwd=scratch, env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
                                    capture_output=True, timeout=20)
            if result.returncode:
                raise RuntimeError(result.stderr.decode(errors="replace"))
            return json.loads(result.stdout)

        git_env = env | {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                         "GIT_AUTHOR_DATE": "2026-11-23T00:00:00+00:00",
                         "GIT_COMMITTER_DATE": "2026-11-23T00:00:00+00:00"}

        def git(*args, input=None):
            return subprocess.check_output(["git", "-C", checkout, *args], env=git_env, input=input, timeout=15)

        def commit(message):
            git("add", ".")
            git("-c", "user.name=Fixture worker", "-c", "user.email=fixture@example.invalid",
                "-c", "commit.gpgSign=false", "commit", "--quiet", "-m", message)
            return git("rev-parse", "HEAD").decode().strip()

        git("init", "--quiet", "--template=", "--object-format=sha1")
        git("config", "core.autocrlf", "false")
        (checkout / "SKILL.md").write_bytes(b"# Fixture verification\nCheck the pinned expected bytes.\n")
        for case in cases:
            (checkout / case["file"]).write_bytes(b"unrepaired fixture\n")
        base = commit("Synthetic baseline")
        cli("shared-work", "resume")
        cli("shared-ops", "set-runner-enrollment", "smoke-runner", REPOSITORY, "--expected-revision", "0",
            "--decision", "grant", "--evidence", "fixture:simulated/enrollment")
        cli("shared-ops", "approve-claude-repository", REPOSITORY, "--approval-id", "mock-approval",
            "--evidence", "fixture:simulated/adapter-approval", "--valid-seconds", "120")
        pairs = []
        expected = {}
        for case in cases:
            pair = {"case_id": case["id"]}
            for side, adapter in ADAPTERS.items():
                work_id = f"{case['id'].lower()}-{side}"
                pair[side] = work_id
                order = {
                    "schema_version": 1, "id": work_id, "issue": case["id"], "owner": "Fixture owner",
                    "repository": str(checkout), "base": base, "intent": case["intent"],
                    "acceptance": [f"{case['file']} is the only change and matches the pinned expected bytes."],
                    "scope": [case["file"]], "context": [{"location": "fixture:simulated/evaluation", "revision": base}],
                    "skill": {"location": "repo:SKILL.md", "revision": base},
                    "agent_adapter": adapter, "execution_profile": profiles[side],
                }
                order_path = scratch / f"{work_id}.json"
                write_json(order_path, order)
                cli("shared-work", "admit", order_path, "--repository", REPOSITORY, "--event-key", work_id,
                    "--order-digest", "sha256:" + sha256(order_path.read_bytes()).hexdigest(),
                    "--token-limit", str(RESERVATION))
                lease = request("runner", "runner", "claim", request_id=str(uuid4()), profile_capable=True,
                                available_repositories=[REPOSITORY], agent_adapter=adapter)["claim"]
                if not lease or lease["work_id"] != work_id:
                    raise RuntimeError(f"The fixture did not claim {work_id}.")
                binding = {"work_id": work_id, "generation": lease["generation"]}
                request("runner", "runner", "start_attempt", **binding)
                mock = case[adapter]
                candidate = mock["candidate"]
                if candidate is None:
                    expected[work_id] = ("unscored", 0, RESERVATION)
                    continue
                git("checkout", "--quiet", "--detach", base)
                (checkout / case["file"]).write_bytes(candidate.encode())
                head = commit(work_id)
                pack = git("pack-objects", "--stdout", "--revs", input=f"{head}\n^{base}\n".encode())
                upload = request("runner", "runner", "submit_pack", **binding, commit=head,
                                 pack_base64=b64encode(pack).decode(), tokens=mock["tokens"])
                passed = (git("diff", "--name-only", base, head).decode().splitlines() == [case["file"]]
                          and git("show", f"{head}:{case['file']}") == case["expected"].encode())
                request("verifier", "verifier", "record_verification", **binding, head=head,
                        digest=upload["candidate_evidence"], previous_sequence=0, passed=passed,
                        evidence=f"fixture:simulated/{case['id']}/{adapter}", note="Checked the scoped Git candidate.")
                expected[work_id] = ("passed" if passed else "failed", mock["tokens"], 0)
            pairs.append(pair)
        pairs_path = scratch / "pairs.json"
        write_json(pairs_path, pairs)
        report = cli("shared-ops", "compare-profiles", pairs_path)["comparison"]
    actual = {row["work_id"]: (row["independent_result"], row["reported_tokens"], row["reserved_tokens_without_usage"])
              for row in report["results"]}
    profiles_match = all(report[f"{side}_adapter"] == adapter and report[f"{side}_profile"] == profiles[side]
                         for side, adapter in ADAPTERS.items())
    if actual != expected or len(report["results"]) != len(expected) or not profiles_match:
        raise RuntimeError("The comparison did not match the fixture verdict and usage inputs.")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kit", required=True, type=Path)
    parser.add_argument("--postgres-bin", type=Path)
    parser.add_argument("--output", required=True, type=Path, help="New directory for the comparison results")
    args = parser.parse_args()
    fixture = Path(__file__).resolve().parents[1] / "examples/simulated-evaluation/cases.json"
    fixture_bytes = fixture.read_bytes()
    data = json.loads(fixture_bytes)
    if data["schema_version"] != 1 or data["provenance"] != "simulated":
        raise ValueError("Expected simulated evaluation input version 1.")
    args.output.mkdir()
    provenance = {"schema_version": 1, "provenance": "simulated",
                  "fixture_sha256": sha256(fixture_bytes).hexdigest()}
    write_json(args.output / "m3-results.json", provenance | {"paths": scorecard(data["cases"], args.output)})
    postgres_bin = args.postgres_bin.resolve(strict=True) if args.postgres_bin else None
    with TemporaryDirectory(prefix="craft-eval-", dir="/tmp" if sys.platform == "linux" else None) as scratch:
        report = compare_profiles(data["cases"], args.kit.resolve(strict=True), postgres_bin, Path(scratch))
    write_json(args.output / "m6-results.json", provenance | {"comparison": report})


if __name__ == "__main__":
    main()
