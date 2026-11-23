"""Exercise authenticated HTTPS against the shared PostgreSQL queue."""

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, contextmanager
from datetime import UTC, datetime
from hashlib import sha256
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import sqlite3
import ssl
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Thread
from time import monotonic, sleep, time
from urllib.parse import urlencode, urlsplit
from uuid import uuid4

from test_queue import BASE, CANDIDATE, ROOT, sql


def run_queue_migrations(*options, database=None, source=ROOT):
    env = os.environ.copy()
    if database is not None:
        env["PGDATABASE"] = database
    return subprocess.run([sys.executable, str(source / "migrate_queue.py"),
                           "--service-role", "craft_test_service", *options],
                          env=env, capture_output=True, text=True, timeout=45)


def exercise_queue_installer(source):
    functions = (source / "service-functions.txt").read_text(encoding="utf-8").splitlines()
    declared = {(name, len(types.split(",")) if types else 0)
                for signature in functions
                for name, types in [signature.removeprefix("craft_queue.")[:-1].split("(", 1)]}
    called = {(name, len(args.split(",")) if args else 0)
              for path in (ROOT / "src").glob("*.rs")
              for name, args in re.findall(r"craft_queue\.([a-z_]+)\(([^()]*)\)", path.read_text(encoding="utf-8"))}
    assert declared == called, (declared - called, called - declared)
    with ThreadPoolExecutor(max_workers=2) as pool:
        installs = [pool.submit(run_queue_migrations, source=source) for _ in range(2)]
        results = [task.result() for task in installs]
    assert all(result.returncode == 0 for result in results), [result.stderr for result in results]
    assert sum("Applying " in result.stdout for result in results) == 1
    count = len(list((ROOT / "migrations").glob("*.sql")))
    assert sql("SELECT count(*) FROM craft_migration.queue_history WHERE NOT external_baseline;") == str(count)
    assert sql("SELECT has_schema_privilege('craft_test_service', 'craft_migration', 'USAGE');") == "f"
    assert sql("SELECT paused FROM craft_queue.control;") == "t"
    for internal in ("observe_standing_before_intent_guard(text,text,jsonb,jsonb)",
                     "observe_standing_before_github_approval(text,text,jsonb,jsonb)",
                     "observe_standing_before_jira_inbox(text,text,jsonb,jsonb)",
                     "operator_brief_before_standing_intents(text,integer,integer)",
                     "operator_brief_before_dispatch_blockers(text,integer,integer)",
                     "operator_brief_before_jira_intents(text,integer,integer)",
                     "dispatch_status_at(text,jsonb)"):
        function = "craft_queue." + internal
        assert sql(f"SELECT has_function_privilege('craft_test_service', '{function}', 'EXECUTE');") == "f"
        assert sql(f"SELECT prosecdef FROM pg_proc WHERE oid = '{function}'::regprocedure;") == "f"
    legacy = "craft_queue.claim(text,uuid)"
    sql(f"GRANT EXECUTE ON FUNCTION {legacy} TO PUBLIC, craft_test_service;")
    replay = run_queue_migrations(source=source)
    assert replay.returncode == 0 and "Applying " not in replay.stdout, replay.stderr
    assert sql(f"SELECT has_function_privilege('craft_test_service', '{legacy}', 'EXECUTE');") == "f"
    with TemporaryDirectory() as directory:
        copied = Path(directory)
        shutil.copy2(source / "migrate_queue.py", copied)
        shutil.copy2(source / "service-functions.txt", copied)
        shutil.copytree(source / "migrations", copied / "migrations")
        latest = sorted((copied / "migrations").glob("*.sql"))[-1]
        raw = latest.read_bytes().replace(b"\r\n", b"\n")
        latest.write_bytes(raw.replace(b"\n", b"\r\n"))
        portable = run_queue_migrations(source=copied)
        assert portable.returncode == 0 and "Applying " not in portable.stdout, portable.stderr
        latest.write_bytes(raw.replace(b"BEGIN;", b"BEGIN;\n-- fixture drift", 1))
        drift = run_queue_migrations(source=copied)
        assert drift.returncode != 0 and "differs from the installed migration files" in drift.stderr
        latest.write_bytes(raw)
        (copied / "migrations" / f"{count + 1:03d}_failure_fixture.sql").write_text(
            "BEGIN;\nCREATE TABLE craft_queue.failed_migration_fixture (id integer);\nSELECT 1 / 0;\nCOMMIT;\n",
            encoding="utf-8")
        failed = run_queue_migrations(source=copied)
        assert failed.returncode != 0 and "division by zero" in failed.stderr
        assert sql("SELECT to_regclass('craft_queue.failed_migration_fixture') IS NULL;") == "t"
        assert sql("SELECT count(*) FROM craft_migration.queue_history;") == str(count)


def exercise_standing_intent_upgrades(source):
    database = "craft_jira_intent_upgrade_" + uuid4().hex[:12]
    sql(f"CREATE DATABASE {database};")
    try:
        paths = sorted((source / "migrations").glob("*.sql"))
        upgrade = next(path for path in paths if path.name == "124_require_jira_standing_intent.sql")
        sql("\n".join(path.read_text(encoding="utf-8") for path in paths if path < upgrade), database)
        sql("""
            INSERT INTO craft_queue.standing_rules (
                id, intake_id, source, project_key, repository, human_account, human_owner, required_label,
                eligible_status, allowed_paths, preferred_runner, token_limit, approval_reference, enabled
            ) VALUES
                ('jira', 'intake', 'jira:11111111-2222-3333-4444-555555555555', 'FIXTURE', 'team/service',
                    'human', 'Owner', 'automate', 'Ready', ARRAY['src'], 'runner', 20, 'fixture:jira-rule', true),
                ('generic', 'intake', 'fixture:tracker', 'FIXTURE', 'team/service',
                    'human', 'Owner', 'automate', 'Ready', ARRAY['src'], 'runner', 20, 'fixture:tracker-rule', true),
                ('old-github', 'intake', 'github:team/service', 'GITHUB', 'team/service',
                    '36', 'Owner', 'automate', 'open', ARRAY['src'], 'runner', 20, 'fixture:tracker-rule', true);
            INSERT INTO craft_queue.standing_observations (
                rule_id, issue_key, source_revision, source_read_at, fingerprint, jira_source_digest, jira_source_approved
            ) VALUES
                ('jira', 'FIXTURE-1', 1, 1, sha256('legacy'::bytea), NULL, false),
                ('jira', 'FIXTURE-2', 1, 1, sha256('pinned'::bytea), sha256('intent'::bytea), true),
                ('generic', 'FIXTURE-3', 1, 1, sha256('generic'::bytea), NULL, false),
                ('old-github', 'GITHUB-4', 17, 1, sha256('old-github'::bytea), NULL, false);
            INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens)
                SELECT issue_key, 'team/service', issue_key, '{}', 20 FROM craft_queue.standing_observations;
            INSERT INTO craft_queue.standing_work (
                work_id, rule_id, issue_key, source_revision, human_account, approval_reference, preferred_runner
            ) SELECT observation.issue_key, observation.rule_id, observation.issue_key, observation.source_revision,
                'human', 'fixture:legacy', 'runner' FROM craft_queue.standing_observations AS observation;
        """, database)
        sql(upgrade.read_text(encoding="utf-8"), database)
        states = """
            SELECT work.id || ':' || work.state || ':' || (binding.blocked_at IS NOT NULL)::text
                || ':' || observation.reassessment_required::text
            FROM craft_queue.work AS work
            JOIN craft_queue.standing_work AS binding ON binding.work_id = work.id
            JOIN craft_queue.standing_observations AS observation USING (rule_id, issue_key)
            ORDER BY work.id;
        """
        baseline = ["FIXTURE-1:cancelled:true:true", "FIXTURE-2:queued:false:false", "FIXTURE-3:queued:false:false"]
        assert sql(states, database).splitlines() == [*baseline, "GITHUB-4:queued:false:false"]
        github = next(path for path in paths if path.name == "125_github_standing_rules.sql")
        sql(github.read_text(encoding="utf-8"), database)
        assert sql(states, database).splitlines() == [*baseline, "GITHUB-4:cancelled:true:true"]
        assert sql("SELECT github_conflict_revision FROM craft_queue.standing_observations WHERE rule_id = 'old-github';",
                   database) == "17"
        sql("""
            INSERT INTO craft_queue.standing_observations (
                rule_id, issue_key, source_revision, source_read_at, fingerprint, jira_source_digest, jira_source_approved
            ) VALUES ('old-github', 'GITHUB-5', 19, 1, sha256('v125'::bytea), sha256('intent'::bytea), true);
            INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens)
                VALUES ('GITHUB-5', 'team/service', 'GITHUB-5', '{}', 20);
            INSERT INTO craft_queue.standing_work (
                work_id, rule_id, issue_key, source_revision, human_account, approval_reference, preferred_runner
            ) VALUES ('GITHUB-5', 'old-github', 'GITHUB-5', 19, '36', 'fixture:v125', 'runner');
        """, database)
        approval = next(path for path in paths if path.name == "126_github_standing_intent_approval.sql")
        sql(approval.read_text(encoding="utf-8"), database)
        assert sql(states, database).splitlines() == [*baseline, "GITHUB-4:cancelled:true:true",
                                                      "GITHUB-5:cancelled:true:true"]
        assert sql("SELECT github_conflict_revision FROM craft_queue.standing_observations WHERE issue_key = 'GITHUB-5';",
                   database) == "19"
        inbox = next(path for path in paths if path.name == "127_operator_standing_intent_inbox.sql")
        sql(inbox.read_text(encoding="utf-8"), database)
        sql("""
            INSERT INTO craft_queue.standing_observations (
                rule_id, issue_key, source_revision, source_read_at, fingerprint,
                reassessment_required, jira_source_approved, github_pending_intent
            ) VALUES
                ('old-github', 'GITHUB-10', 41, floor(extract(epoch FROM now()))::bigint - 301,
                    sha256('old'::bytea), true, true, sha256('pending'::bytea)),
                ('old-github', 'GITHUB-11', 42, floor(extract(epoch FROM now()))::bigint,
                    sha256('ready'::bytea), true, true, sha256('pending'::bytea));
        """, database)
        report = "SELECT craft_queue.operator_brief('team/service', 24, {limit})->'standing_intent_reviews';"
        bounded = json.loads(sql(report.format(limit=1), database))
        assert bounded["total"] == 2 and bounded["ready"] == 1
        [ready] = bounded["items"]
        assert ready["issue"] == "GITHUB-11" and ready["state"] == "ready_for_approval"
        ready, stale = json.loads(sql(report.format(limit=2), database))["items"]
        assert stale["issue"] == "GITHUB-10" and stale["state"] == "refresh_observation"
        assert "review_token" not in stale
    finally:
        sql(f"DROP DATABASE {database} WITH (FORCE);")


def exercise_dependency_approval_upgrade(source):
    database = "craft_dependency_upgrade_" + uuid4().hex[:12]
    sql(f"CREATE DATABASE {database};")
    try:
        paths = sorted((source / "migrations").glob("*.sql"))
        upgrade = next(path for path in paths if path.name == "048_dependency_evidence_approval.sql")
        sql("\n".join(path.read_text(encoding="utf-8") for path in paths if path < upgrade), database)
        pack = "sha256:" + "a" * 64
        pin = "sha256:" + "b" * 64
        sql(f"""
            INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens, state, generation,
                candidate, candidate_evidence)
            VALUES
                ('pending-dependency', 'team/service', 'pending-dependency',
                    '{{"dependency_intake_digest":"{pin}","require_native_checks":true}}', 20, 'candidate', 1, '{CANDIDATE}', '{pack}'),
                ('released-dependency', 'team/service', 'released-dependency',
                    '{{"dependency_intake_digest":"{pin}","require_native_checks":true}}', 20, 'released', 1, '{CANDIDATE}', '{pack}'),
                ('ordinary', 'team/service', 'ordinary', '{{}}', 20, 'candidate', 1, '{CANDIDATE}', '{pack}');
            INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at,
                token_limit, outcome)
                SELECT w.id, 1, 'old-runner', gen_random_uuid(), now(), now(), 20, 'candidate' FROM craft_queue.work w;
            INSERT INTO craft_queue.candidate_packs SELECT w.id, 1, '{pack}', decode('5041434b', 'hex') FROM craft_queue.work w;
            INSERT INTO craft_queue.verifications (work_id, generation, head, digest, verifier, passed, evidence, note)
                SELECT w.id, 1, '{CANDIDATE}', '{pack}', 'old-verifier', true, 'fixture:prior-pass', 'Passed'
                FROM craft_queue.work w;
        """, database)
        sql(upgrade.read_text(encoding="utf-8"), database)
        assert sql("""SELECT string_agg(work_id || ':' || passed::text, ',' ORDER BY work_id)
            FROM (SELECT DISTINCT ON (work_id) work_id, passed FROM craft_queue.verifications
                  ORDER BY work_id, sequence DESC) current;""", database) == (
            "ordinary:true,pending-dependency:false,released-dependency:true")
        assert sql("SELECT count(*) FROM craft_queue.verifications WHERE evidence = 'fixture:prior-pass';", database) == "3"
        sql("""
            INSERT INTO craft_queue.release_series (id, owner_name, approval_reference, registered_by, plan_digest)
            VALUES ('prior-series', 'Fixture operator', 'fixture:approved-series', 'operator', 'fixture');
            INSERT INTO craft_queue.series_members (series_id, position, work_id, observer_id, release_target)
            VALUES ('prior-series', 1, 'pending-dependency', 'release-one', '{}'),
                   ('prior-series', 2, 'released-dependency', 'release-one', '{}'),
                   ('prior-series', 3, 'ordinary', 'release-one', '{}');
        """, database)
        rollout_upgrade = next(path for path in paths if path.name == "049_dependency_release_rollout.sql")
        sql(rollout_upgrade.read_text(encoding="utf-8"), database)
        assert sql("SELECT string_agg(work_id || ':' || requires_rollout_proof::text, ',' ORDER BY work_id) "
                   "FROM craft_queue.series_members;", database) == (
            "ordinary:false,pending-dependency:true,released-dependency:false")
    finally:
        sql(f"DROP DATABASE {database};")


def api_client(address, certificate, tokens):
    parsed = urlsplit(address)
    trust = ssl.create_default_context(cafile=str(certificate))

    def request(role, identity, action, expected=200, **body):
        connection = http.client.HTTPSConnection(parsed.hostname, parsed.port, context=trust, timeout=5)
        headers = {"Content-Type": "application/json"}
        if identity is not None:
            headers["Authorization"] = f"Bearer {tokens[identity]}"
        try:
            payload = {"action": action, **body} if action is not None else body
            connection.request("POST", f"/v1/{role}", json.dumps(payload), headers)
            result = connection.getresponse()
            data = result.read()
            expected_statuses = expected if isinstance(expected, tuple) else (expected,)
            assert result.status in expected_statuses, (action, result.status, data)
            return json.loads(data) if result.getheader("Content-Type") == "application/json" else None
        finally:
            connection.close()

    return request


def clone_reviewable_work(source_work, source_verdict, work_id, pull_request):
    return int(sql(f"""
        INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens, state, generation,
                candidate, candidate_evidence)
            SELECT '{work_id}', repository, '{work_id}', jsonb_set(payload, '{{id}}', '"{work_id}"'),
                reserved_tokens, state, generation, candidate, candidate_evidence
            FROM craft_queue.work WHERE id = '{source_work}';
        INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, started_at, expires_at,
                token_limit, reported_tokens, outcome)
            SELECT '{work_id}', generation, runner_id, gen_random_uuid(), started_at, expires_at, token_limit, 0, outcome
            FROM craft_queue.attempts WHERE work_id = '{source_work}' AND generation = 1;
        INSERT INTO craft_queue.candidate_packs SELECT '{work_id}', generation, digest, payload
            FROM craft_queue.candidate_packs WHERE work_id = '{source_work}' AND generation = 1;
        WITH verdict AS (
            INSERT INTO craft_queue.verifications (work_id, generation, head, digest, verifier, passed, evidence, note)
            SELECT '{work_id}', generation, head, digest, verifier, passed, evidence, note
            FROM craft_queue.verifications WHERE sequence = {source_verdict}
            RETURNING sequence
        ) INSERT INTO craft_queue.publications (action_key, work_id, generation, verification_sequence, head, base,
            pack_digest, repository, base_branch, source_branch, publisher_id, state, result_url)
            SELECT '{work_id}', '{work_id}', p.generation, verdict.sequence, p.head, p.base, p.pack_digest,
                p.repository, p.base_branch, 'craft-factory/{work_id}', p.publisher_id, p.state, '{pull_request}'
            FROM craft_queue.publications AS p CROSS JOIN verdict WHERE p.work_id = '{source_work}' AND p.generation = 1
            RETURNING verification_sequence;
    """))


@contextmanager
def block_release_context(work_id):
    # Simulate an unavailable prerequisite on an already-issued lease.
    sql(f"""
        INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens)
            VALUES ('context-predecessor', 'team/other', 'fixture:context-predecessor', '{{}}', 1);
        INSERT INTO craft_queue.release_series (id, owner_name, approval_reference, registered_by, plan_digest)
            VALUES ('context-series', 'Fixture owner', 'fixture:context', 'operator', 'fixture');
        INSERT INTO craft_queue.series_members (series_id, position, work_id, observer_id, release_target) VALUES
            ('context-series', 1, 'context-predecessor', 'release-one', '{{}}'),
            ('context-series', 2, '{work_id}', 'release-one', '{{}}');
    """)
    try:
        yield
    finally:
        sql("DELETE FROM craft_queue.series_members WHERE series_id = 'context-series';\n"
            "DELETE FROM craft_queue.release_series WHERE id = 'context-series';\n"
            "DELETE FROM craft_queue.work WHERE id = 'context-predecessor';")


def exercise_prometheus(address, certificate, token, expected):
    binary = os.environ.get("CRAFT_TEST_PROMETHEUS")
    if not binary:
        return
    with TemporaryDirectory(prefix="craft-prometheus-") as directory:
        root = Path(directory)
        credential = root / "token"
        with os.fdopen(os.open(credential, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
            file.write(token)
        config = root / "prometheus.yml"
        config.write_text(json.dumps({"scrape_configs": [{
            "job_name": "craft-factory-queue", "scheme": "https", "scrape_interval": "1s", "scrape_timeout": "750ms",
            "authorization": {"type": "Bearer", "credentials_file": str(credential)},
            "tls_config": {"ca_file": str(certificate)}, "static_configs": [{"targets": [urlsplit(address).netloc]}],
        }]}), encoding="utf-8")
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        with (root / "prometheus.log").open("w+") as log, subprocess.Popen([
            binary, f"--config.file={config}", f"--storage.tsdb.path={root / 'data'}",
            f"--web.listen-address=127.0.0.1:{port}",
        ], stdout=log, stderr=log) as process:
            try:
                deadline = monotonic() + 10
                while process.poll() is None and monotonic() < deadline:
                    with closing(http.client.HTTPConnection("127.0.0.1", port, timeout=1)) as connection:
                        try:
                            selector = '{__name__=~"craft_queue_(' + "|".join(expected) + ')"}'
                            connection.request("GET", "/api/v1/query?" + urlencode({"query": selector}))
                            response = connection.getresponse()
                            if response.status == 200:
                                series = json.loads(response.read())["data"]["result"]
                                if len(series) == len(expected):
                                    assert {sample["metric"]["__name__"].removeprefix("craft_queue_"): int(sample["value"][1])
                                            for sample in series} == expected
                                    assert all(sample["metric"]["job"] == "craft-factory-queue" for sample in series)
                                    print("Verified Prometheus ingested queue gauges over authenticated TLS.")
                                    return
                        except (OSError, http.client.HTTPException):
                            pass
                    sleep(0.1)
                log.seek(0)
                raise AssertionError(log.read().replace(token, "[redacted]"))
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()


def exercise(address, certificate, tokens, launcher):
    request = api_client(address, certificate, tokens)
    parsed = urlsplit(address)
    trust = ssl.create_default_context(cafile=str(certificate))

    def scrape(identity, expected=200, method="GET"):
        client = http.client.HTTPSConnection(parsed.hostname, parsed.port, context=trust, timeout=5)
        try:
            headers = {"Authorization": f"Bearer {tokens[identity]}"} if identity else {}
            client.request(method, "/metrics", headers=headers)
            result = client.getresponse()
            body = result.read().decode()
            assert result.status == expected, body
            if expected != 200:
                return None
            assert result.getheader("Content-Type") == "text/plain; version=0.0.4; charset=utf-8"
            assert result.getheader("Cache-Control") == "no-store"
            assert body.endswith("\n") and "team/service" not in body and "service-one" not in body
            samples = {name.removeprefix("craft_queue_"): int(value)
                       for name, value in (line.split() for line in body.splitlines() if not line.startswith("#"))}
            assert all(value not in body for value in ("draft-review-101", "overrun-candidate", "fixture-overrun"))
            assert body.count(" gauge\n") == len(samples) <= 19
            return samples
        finally:
            client.close()

    scrape(None, 401)
    scrape("first", 403)
    scrape("operator", 403)
    scrape("monitor", 405, "POST")
    request("operations", "monitor", "summary", 403)
    brief_filters = {"repository": "team/service", "usage_hours": 24, "limit": 1}
    request("operations", "monitor", "operator_brief", 403, **brief_filters)
    request("operations", "reviewer", "operator_brief", 403, **brief_filters)
    request("operations", "operator", "operator_brief", 400, **(brief_filters | {"limit": 21}))
    request("operations", "operator", "operator_brief", 400, **(brief_filters | {"usage_hours": 0}))
    request("operations", "operator", "operator_brief", 400, **(brief_filters | {"repository": "team/service/other"}))
    def brief(**filters):
        return request("operations", "operator", "operator_brief",
                       **(brief_filters | filters))["operator_brief"]
    assert brief()["queue"]["paused"]
    request("operator", "monitor", "set_paused", 403, paused=False)
    idle = scrape("monitor")
    assert idle["paused"] == 1 and idle["queued"] == idle["live_leases"] == 0
    assert idle["standing_delegation_blocked"] == 0
    assert idle["token_overrun_attempts_15m"] == idle["tokens_above_reservation_15m"] == 0
    assert idle["pending_review_assignments"] == 0 and "oldest_pending_review_seconds" not in idle
    assert idle["waiting_verification_candidates"] == idle["active_verification_pickups"] == 0
    assert "oldest_waiting_verification_seconds" not in idle
    assert "oldest_queued_seconds" not in idle and "oldest_live_lease_seconds" not in idle
    sql("REVOKE EXECUTE ON FUNCTION craft_queue.operation_summary() FROM craft_test_service;")
    try:
        scrape("monitor", 503)
    finally:
        sql("GRANT EXECUTE ON FUNCTION craft_queue.operation_summary() TO craft_test_service;")
    assert scrape("monitor") == idle
    exercise_prometheus(address, certificate, tokens["monitor"], {
        "standing_delegation_blocked": 0, "pending_review_assignments": 0,
        "waiting_verification_candidates": 0, "active_verification_pickups": 0,
        "token_overrun_attempts_15m": 0, "tokens_above_reservation_15m": 0,
    })
    if launcher is not None:
        with TemporaryDirectory(prefix="craft-queue-credentials-") as directory:
            root = Path(directory)
            variable = "CRAFT_FACTORY_OPERATOR_TOKEN"
            token_file = root / variable
            token = tokens["operator"]
            with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as file:
                file.write(token.encode() + b"\r\n")
            command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate), "summary"]
            env = {name: value for name, value in os.environ.items()
                   if name not in (variable, variable + "_FILE", "CREDENTIALS_DIRECTORY")}

            def read_summary(credentials):
                output = subprocess.run(command, env=env | credentials, text=True, capture_output=True, timeout=10)
                assert token not in output.stdout and token not in output.stderr
                return output

            for credentials in ({variable + "_FILE": str(token_file)}, {"CREDENTIALS_DIRECTORY": str(root)},
                                {variable: token, variable + "_FILE": str(root / "missing")}):
                output = read_summary(credentials)
                assert output.returncode == 0, output.stderr
                assert json.loads(output.stdout)["summary"]["paused"]
            output = read_summary({variable: "invalid", variable + "_FILE": str(token_file)})
            assert output.returncode != 0 and "invalid shared queue credential" in output.stderr
            for malformed in (token.encode() + b"\nextra", b"x" * 259):
                token_file.write_bytes(malformed)
                output = read_summary({"CREDENTIALS_DIRECTORY": str(root)})
                assert output.returncode != 0 and not output.stdout and str(token_file) not in output.stderr
    request("series", "operator", "reload_rollout_catalog", 409, digest="sha256:" + "0" * 64)
    order = {"schema_version": 1, "id": "service-one", "base": BASE, "repository": "C:/private/repo"}
    request("runner", None, "claim", 401, request_id=str(uuid4()))
    request("operator", "first", "set_paused", 403, paused=False)
    request("runner", "operator", "claim", 403, request_id=str(uuid4()))
    request("runner", "first", "claim", 422, request_id=str(uuid4()), runner="second")
    request("operator", "operator", "admit", 409, repository="team/service", event_key="service:1",
            work_order=order, token_limit=25)
    assert request("operator", "operator", "set_paused", paused=False)["paused"] is False
    assert request("operator", "operator", "admit", repository="team/service", event_key="service:1",
                   work_order=order, token_limit=25)["work_order_id"] == order["id"]
    queued = scrape("monitor")
    assert queued["queued"] == 1 and queued["paused"] == 0 and queued["oldest_queued_seconds"] >= 0
    waiting = brief()
    assert waiting["queue"]["queued"] == waiting["backlog"]["queued_total"] == 1
    [queued_item] = waiting["backlog"]["items"]
    assert queued_item["work_id"] == order["id"] and queued_item["kind"] == "queued"
    foreign = brief(repository="team/other")
    assert foreign["queue"]["queued"] == 1 and foreign["backlog"]["items"] == []
    assert foreign["review"]["items"] == foreign["usage"]["groups"] == []
    if launcher is not None:
        cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
               "operator-brief", "--repository", "team/service", "--limit", "1"]
        env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
        terminal = subprocess.check_output(cli, env=env, text=True, timeout=10)
        assert "Queued | service-one | team/service" in terminal and "Usage over 24h" in terminal
        structured = json.loads(subprocess.check_output(cli + ["--json"], env=env, text=True, timeout=10))
        assert structured["operator_brief"]["backlog"]["items"] == [queued_item]
        assert tokens["operator"] not in terminal and "C:/private/repo" not in json.dumps(structured)
    assert request("runner", "outside", "claim", request_id=str(uuid4()))["claim"] is None

    requests = {name: str(uuid4()) for name in ("first", "second")}
    with ThreadPoolExecutor(max_workers=2) as pool:
        claims = list(pool.map(lambda name: request("runner", name, "claim", request_id=requests[name])["claim"], requests))
    assert sum(claim is not None for claim in claims) == 1, claims
    winner, next_runner = ("first", "second") if claims[0] else ("second", "first")
    claim = request("runner", winner, "claim", request_id=requests[winner])["claim"]
    assert claim["generation"] == 1 and "repository" not in claim["work_order"]
    leased = scrape("monitor")
    assert leased["live_leases"] == 1 and leased["oldest_live_lease_seconds"] >= 0
    assert leased["queued"] == 0 and leased["review_reserved"] == 1 and leased["tokens_charged_or_reserved"] == 25
    sql("UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second';")
    expired = brief()
    assert expired["backlog"]["expired_lease_total"] == 1 and expired["backlog"]["queued_total"] == 0
    assert expired["backlog"]["items"][0]["runner_id"] == winner
    next_claim = request("runner", next_runner, "claim", request_id=str(uuid4()))["claim"]
    assert next_claim["generation"] == 2 and next_claim["work_id"] == order["id"]

    request("runner", winner, "renew", 409, work_id=order["id"], generation=1)
    submission = {"work_id": order["id"], "generation": 2, "commit": CANDIDATE,
                  "evidence_ref": "artifact:fixture", "tokens": 12}
    request("runner", winner, "submit_candidate", 409, **submission)
    assert request("runner", next_runner, "submit_candidate", **submission)["accepted"]
    assert request("runner", next_runner, "submit_candidate", **submission)["accepted"]
    status = request("operator", "operator", "status", work_id=order["id"])["work"]
    assert status["state"] == "candidate" and status["candidate"] == CANDIDATE
    visible = brief()
    [review_item] = visible["review"]["items"]
    assert review_item["work_id"] == order["id"] and review_item["next_action"] == "verify_candidate"
    assert "intent_excerpt" not in review_item and "acceptance_excerpts" not in review_item
    assert visible["usage"]["totals"]["reported_tokens"] == 12
    assert visible["usage"]["totals"]["reserved_tokens_without_usage"] == 25
    assert request("verifier", "service-verifier", "status", work_id=order["id"])["work"] == status
    missing = request("verifier", "other-verifier", "status", 404, work_id="unknown")
    assert request("verifier", "other-verifier", "status", 404, work_id=order["id"]) == missing
    request("operator", "operator", "status", 404, work_id="unknown")
    assert request("operator", "operator", "cancel", work_id=order["id"], reason="fixture complete")["cancelled"]
    exercise_runner_adapter_setup(request)
    exercise_runner_enrollment(address, certificate, tokens, request, launcher)
    exercise_dispatch_priority(address, certificate, tokens, request, launcher)
    if launcher is not None:
        exercise_launcher(address, certificate, tokens, request, launcher, scrape)
    assert request("operator", "operator", "set_paused", paused=True)["paused"]


def exercise_runner_adapter_setup(request):
    def setup(identity, *repositories):
        return request("runner", identity, "check_enrollment", mapped_repositories=repositories)["setup"]

    assert setup("first", "team/service")["claude_approved_repositories"] == []
    approval_id = "fixture-adapter-setup"
    request("operations", "operator", "approve_claude_repository", repository="team/service",
            approval_id=approval_id, evidence="fixture:adapter setup qualification", valid_seconds=60)
    assert setup("first", "team/service", "team/other")["claude_approved_repositories"] == ["team/service"]
    assert setup("first", "team/other")["claude_approved_repositories"] == []
    assert setup("outside", "team/other", "team/service")["claude_approved_repositories"] == []
    sql(f"UPDATE craft_queue.claude_repository_approvals "
        "SET approved_at = clock_timestamp() - interval '2 minutes', expires_at = clock_timestamp() - interval '1 minute' "
        f"WHERE grant_id = '{approval_id}';")
    expired = setup("first", "team/service")
    assert expired["enrolled_repositories"] == ["team/service"] and expired["claude_approved_repositories"] == []


def exercise_runner_enrollment(address, certificate, tokens, request, launcher):
    repository = "team/enrollment-fixture"

    def status(runner, repo=repository):
        return request("operations", "operator", "runner_enrollment", runner_id=runner, repository=repo)["enrollment"]

    def decide(runner, revision, enrolled, evidence, expected=200, identity="operator"):
        return request("operations", identity, "set_runner_enrollment", expected, runner_id=runner,
                       repository=repository, expected_revision=revision, enrolled=enrolled, evidence=evidence)

    first = status("first")
    assert first["runner_configured"] and not first["enrolled"] and first["revision"] == 0
    legacy = status("first", "team/service")
    assert legacy["enrolled"] and legacy["revision"] == 0 and legacy["decision"] is None
    decide("monitor", 0, True, "fixture:unconfigured-runner", 409)
    decide("first", 0, True, "fixture:wrong-actor", 403, identity="first")
    if launcher is not None:
        command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                   "set-runner-enrollment", "first", repository, "--expected-revision", "0",
                   "--decision", "grant", "--evidence", "fixture:approved-grant"]
        granted = json.loads(subprocess.check_output(command, env=os.environ | {
            "CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, text=True, timeout=15))["decision"]
    else:
        granted = decide("first", 0, True, "fixture:approved-grant")["decision"]
    assert granted["revision"] == 1 and granted["actor"] == "operator" and granted["enrolled"]
    assert granted == decide("first", 0, True, "fixture:approved-grant")["decision"]
    decide("first", 0, True, "fixture:stale-change", 409)
    assert status("first")["decision"] == granted
    decide("second", 0, True, "fixture:approved-backup")

    order = {"schema_version": 1, "id": "enrollment-fixture", "base": BASE, "repository": repository}
    request("operator", "operator", "admit", repository=repository, event_key=order["id"], work_order=order, token_limit=10)
    event = str(uuid4())
    first_lease = request("runner", "first", "claim", request_id=event, available_repositories=[repository])["claim"]
    assert first_lease["work_id"] == order["id"] and first_lease["generation"] == 1
    revoked = decide("first", 1, False, "fixture:retire-repo-access")["decision"]
    assert revoked["fenced_leases"] == 1 and revoked["revision"] == 2 and not revoked["enrolled"]
    assert revoked == decide("first", 1, False, "fixture:retire-repo-access")["decision"]
    assert not status("first")["enrolled"] and status("first", "team/service")["enrolled"]
    binding = {"work_id": order["id"], "generation": 1}
    request("runner", "first", "renew", 409, **binding)
    request("runner", "first", "submit_candidate", 409, **binding, commit=CANDIDATE, evidence_ref="fixture:revoked-result")
    decide("first", 2, True, "fixture:approved-regrant")
    assert request("runner", "first", "claim", request_id=event, available_repositories=[repository])["claim"] is None
    request("runner", "first", "renew", 409, **binding)
    second_lease = request("runner", "second", "claim", request_id=str(uuid4()), available_repositories=[repository])["claim"]
    assert second_lease["work_id"] == order["id"] and second_lease["generation"] == 2
    request("operator", "operator", "cancel", work_id=order["id"], reason="enrollment takeover fixture checked")
    assert decide("first", 3, False, "fixture:cleanup-first")["decision"]["revision"] == 4
    assert decide("second", 1, False, "fixture:cleanup-second")["decision"]["revision"] == 2
    assert sql("SELECT sum(token_limit) FROM craft_queue.attempts WHERE work_id = 'enrollment-fixture';") == "20"
    # Retire this disposable case before the unrelated launcher fixtures set a fixed token budget.
    sql("""
        BEGIN;
        DELETE FROM craft_queue.attempts WHERE work_id = 'enrollment-fixture';
        DELETE FROM craft_queue.work WHERE id = 'enrollment-fixture';
        COMMIT;
    """)


def exercise_dispatch_priority(address, certificate, tokens, request, launcher):
    sibling = "team/priority-fixture"
    oldest, peer, urgent = "dispatch-priority-oldest", "dispatch-priority-peer", "dispatch-priority-urgent"
    sql("INSERT INTO craft_queue.runner_repositories VALUES ('first', 'team/priority-fixture');")
    for work_id, repository in ((oldest, "team/service"), (peer, sibling), (urgent, sibling)):
        order = {"schema_version": 1, "id": work_id, "base": BASE, "repository": repository}
        request("operator", "operator", "admit", repository=repository, event_key=work_id,
                work_order=order, token_limit=5)

    def status():
        return request("operations", "operator", "dispatch_status", work_id=urgent)["dispatch"]

    def decide(revision, priority, evidence, expected=200, identity="operator"):
        return request("operations", identity, "set_dispatch_priority", expected, work_id=urgent,
                       expected_revision=revision, priority=priority, evidence=evidence)

    def claim(*available):
        options = {"available_repositories": list(available)} if available else {}
        return request("runner", "first", "claim", request_id=str(uuid4()), **options)["claim"]

    def cancel(work_id):
        request("operator", "operator", "cancel", work_id=work_id, reason="dispatch priority fixture")

    def backlog(repository, limit=1):
        return request("operations", "operator", "operator_brief", repository=repository,
                       usage_hours=24, limit=limit)["operator_brief"]["backlog"]

    assert status()["dispatch_priority"] == "normal" and status()["dispatch_priority_revision"] == 0
    decide(0, "expedited", "fixture:wrong-actor", 403, identity="first")
    decide(0, "urgent", "fixture:invalid-priority", 400)
    if launcher is not None:
        command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                   "set-dispatch-priority", urgent, "--expected-revision", "0",
                   "--priority", "expedited", "--evidence", "fixture:approved-order-priority"]
        first = json.loads(subprocess.check_output(command, env=os.environ | {
            "CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, text=True, timeout=15))["decision"]
    else:
        first = decide(0, "expedited", "fixture:approved-order-priority")["decision"]
    assert first["revision"] == 1 and first["actor"] == "operator" and first["priority"] == "expedited"
    decide(0, "normal", "fixture:stale-update", 409)
    second = decide(1, "normal", "fixture:returned-to-normal")["decision"]
    assert decide(0, "expedited", "fixture:approved-order-priority")["decision"] == first
    assert status()["dispatch_priority"] == "normal"
    third = decide(2, "expedited", "fixture:reprioritized")["decision"]
    assert status()["dispatch_priority_history"] == [third, second, first]

    request("operator", "operator", "set_paused", paused=True)
    assert claim() is None
    paused = backlog(sibling)
    assert paused["queued_total"] == 2
    [entry] = paused["items"]
    assert entry["repository"] == sibling and entry["work_id"] == peer
    assert entry["blockers"] == ["queue_paused"] == status()["blockers"]
    assert backlog("fixture/unknown")["items"] == []
    if launcher is not None:
        output = subprocess.check_output([str(launcher.resolve()), "shared-ops", "--queue", address, "--ca",
                                          str(certificate), "operator-brief", "--repository", sibling, "--limit", "1"],
                                         env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
                                         text=True, timeout=15)
        assert f"Queued | {peer} | {sibling}" in output and "Blocked by paused queue" in output
        assert oldest not in output and urgent not in output
    request("operator", "operator", "set_paused", paused=False)
    token_budget = int(sql("SELECT token_budget FROM craft_queue.control;"))
    try:
        sql("UPDATE craft_queue.control SET token_budget = 1;")
        assert claim() is None
        assert backlog(sibling)["items"][0]["blockers"] == ["token_budget"] == status()["blockers"]
    finally:
        sql(f"UPDATE craft_queue.control SET token_budget = {token_budget};")
    assert backlog(sibling)["items"][0]["blockers"] == []
    assert claim("team/service")["work_id"] == oldest
    assert claim() is None
    sql(f"UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second' WHERE work_id = '{oldest}';")
    [expired] = backlog("team/service")["items"]
    assert expired["work_id"] == oldest and expired["kind"] == "expired_lease" and expired["blockers"] == []
    takeover = claim()
    assert takeover["work_id"] == oldest and takeover["generation"] == 2
    cancel(oldest)
    assert claim()["work_id"] == urgent
    decide(3, "normal", "fixture:already-leased", 409)
    cancel(urgent)
    assert claim()["work_id"] == peer
    cancel(peer)
    sql("""
        BEGIN;
        DELETE FROM craft_queue.dispatch_priorities WHERE work_id LIKE 'dispatch-priority-%';
        DELETE FROM craft_queue.attempts WHERE work_id LIKE 'dispatch-priority-%';
        DELETE FROM craft_queue.work WHERE id LIKE 'dispatch-priority-%';
        DELETE FROM craft_queue.runner_repositories WHERE repository = 'team/priority-fixture';
        COMMIT;
    """)
    print("Verified scoped dispatch priority, ordered replay, pause, review capacity, and lease takeover.")


def exercise_shared_work(address, certificate, tokens, request, launcher, root, order):
    token_file = root / "operator-token"
    with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as credential:
        credential.write(tokens["operator"] + "\n")
    command = [str(launcher.resolve()), "shared-work", "--queue", address, "--ca", str(certificate)]
    env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": "unused-explicit-file-fixture"}

    def invoke(*arguments, success=True):
        output = subprocess.run(command + ["--operator-token-file", str(token_file), *arguments], env=env,
                                capture_output=True, text=True, timeout=15)
        assert tokens["operator"] not in output.stdout and tokens["operator"] not in output.stderr
        if success:
            assert output.returncode == 0, output.stderr
            return json.loads(output.stdout)
        assert output.returncode != 0
        return output

    work_path = root / "approved-order.json"
    order_bytes = json.dumps(order).encode("utf-8")
    work_path.write_bytes(order_bytes)
    digest = "sha256:" + sha256(order_bytes).hexdigest()
    admission = ["admit", str(work_path), "--order-digest", digest, "--repository", "team/service",
                 "--event-key", "launcher:1", "--token-limit", "40"]
    assert invoke("pause")["paused"] is True
    refusal = invoke(*admission, success=False)
    assert "HTTP 409" in refusal.stderr, refusal.stderr
    denied = subprocess.run(command + ["resume"], env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["first"]},
                            capture_output=True, text=True, timeout=15)
    assert denied.returncode != 0 and "HTTP 403" in denied.stderr
    assert invoke("resume")["paused"] is False
    work_path.write_bytes(order_bytes + b"\n")
    refusal = invoke(*admission, success=False)
    assert "work order does not match the approved digest" in refusal.stderr
    request("operator", "operator", "status", 404, work_id=order["id"])
    work_path.write_bytes(order_bytes)
    assert invoke(*admission)["work_order_id"] == order["id"]
    assert invoke(*admission)["work_order_id"] == order["id"]
    assert invoke("status", order["id"])["work"]["state"] == "queued"
    assert sql("SELECT payload->>'repository' FROM craft_queue.work WHERE id = 'launcher-one';") == "team/service"
    return invoke


def exercise_launcher(address, certificate, tokens, request, launcher, scrape):
    with TemporaryDirectory() as directory:
        root = Path(directory)
        repo = root / "repo"
        repo.mkdir()
        (repo / "src").mkdir()
        (repo / "skill").mkdir()
        (repo / "src" / "hello").write_text("base", encoding="utf-8")
        (repo / "skill" / "SKILL.md").write_text("Pinned skill for fixture", encoding="utf-8")

        def git(*args, **kwargs):
            return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, **kwargs).stdout

        git("init", "--quiet")
        git("add", ".")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "Base")
        base = git("rev-parse", "HEAD").decode().strip()
        agent = root / ("fixture.exe" if os.name == "nt" else "fixture")
        subprocess.run(["rustc", str(ROOT.parent / "tests" / "fixtures" / "agent.rs"), "-o", str(agent)],
                       check=True, cwd=ROOT.parent, timeout=30)
        home = root / "codex-home"
        home.mkdir()
        mapping = root / "mapping.json"
        mapping.write_text(json.dumps({"team/service": {"path": str(repo)}}), encoding="utf-8")
        order = {
            "schema_version": 1, "id": "launcher-one", "issue": "FIXTURE-1", "owner": "Operator",
            "repository": str(repo), "base": base, "intent": "Change the greeting",
            "acceptance": ["Change src/hello"], "scope": ["src"],
            "context": [{"location": "jira:FIXTURE-1", "revision": "fixture"}],
            "skill": {"location": "repo:skill/SKILL.md", "revision": base},
        }
        # The Jira-to-Bitbucket worker reports 30 tokens before the profile budget cases.
        sql("UPDATE craft_queue.control SET token_budget = 1130;")
        operator_work = exercise_shared_work(address, certificate, tokens, request, launcher, root, order)

        runner_credentials = root / "runner-credentials"
        runner_credentials.mkdir(mode=0o700)
        runner_variable = "CRAFT_FACTORY_RUNNER_TOKEN"
        runner_token_file = runner_credentials / runner_variable
        with os.fdopen(os.open(runner_token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
            file.write(tokens["first"])

        def launch(identity, destination, request_id, *, collect_knowledge=False, mounted_credentials=False,
                   timeout_seconds=18, fixture_env=None):
            optional_flags = ["--collect-knowledge"] if collect_knowledge else []
            env = os.environ.copy()
            env.update(fixture_env or {})
            if mounted_credentials:
                env.pop(runner_variable, None)
                env[runner_variable + "_FILE"] = str(runner_token_file)
                env["CREDENTIALS_DIRECTORY"] = str(runner_credentials)
            else:
                env[runner_variable] = tokens[identity]
            return subprocess.Popen(
                [str(launcher.resolve()), "run-shared", "--queue", address, "--ca", str(certificate),
                 "--request-id", request_id, "--repository-map", str(mapping),
                 "--destination", str(destination), "--codex", str(agent),
                 "--codex-home", str(home), "--renew-seconds", "1", "--timeout-seconds", str(timeout_seconds)] + optional_flags,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                env=env,
            )

        def finish(process):
            with process:
                stdout, stderr = process.communicate(timeout=25)
                assert process.returncode == 0, stderr
                return json.loads(stdout)

        def prepare(work_id, head, destination, signing_identity="operator", verifier=False):
            flag = ["--verifier"] if verifier else []
            variable = "CRAFT_FACTORY_VERIFIER_TOKEN" if verifier else "CRAFT_FACTORY_OPERATOR_TOKEN"
            return subprocess.run(
                [str(launcher.resolve()), "prepare-shared-verification", "--queue", address,
                 "--ca", str(certificate), "--work-id", work_id, "--head", head,
                 "--repository-map", str(mapping), "--destination", str(destination), *flag],
                capture_output=True, text=True, timeout=15,
                env=os.environ | {variable: tokens[signing_identity]},
            )

        def attest(handoff_path, result, evidence, identity="verifier", *, review_summary=None):
            flags = [] if review_summary is None else ["--review-summary", review_summary]
            return subprocess.run(
                [str(launcher.resolve()), "attest-shared-verification", "--queue", address,
                 "--ca", str(certificate), "--handoff", str(handoff_path), "--result", result,
                 "--evidence", evidence, "--note", "Independent fixture test", *flags],
                capture_output=True, text=True, timeout=15,
                env=os.environ | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens[identity]},
            )

        request_id = str(uuid4())
        destination = root / "attempt-one"
        result = finish(launch("first", destination, request_id, mounted_credentials=True))
        assert result["generation"] == 1
        assert finish(launch("first", destination, request_id)) == result
        request("operator", "first", "download_pack", 403, work_id=order["id"])
        pack = request("operator", "operator", "download_pack", work_id=order["id"])["candidate"]
        payload = base64.b64decode(pack["pack_base64"])
        assert payload == (destination / "work" / "candidate.pack").read_bytes()
        assert pack["digest"] == result["candidate_evidence"] == f"sha256:{sha256(payload).hexdigest()}"
        request("operations", "first", "review_inbox", 403)
        request("operations", "operator", "review_inbox", 400, limit=51)
        filters = {"repository": "team/service", "owner": order["owner"]}
        inbox = request("operations", "operator", "review_inbox", **filters)["inbox"]
        assert inbox["total"] == 1 and inbox["counts"] == {"verify_candidate": 1}
        [entry] = inbox["items"]
        assert entry["head"] == result["candidate"] and entry["pack_digest"] == pack["digest"]
        assert entry["intent_excerpt"] == order["intent"] and entry["acceptance_excerpts"] == order["acceptance"]
        assert entry["independent_verification"] is None
        assert request("operations", "operator", "review_inbox", owner="Different owner")["inbox"]["total"] == 0
        assert request("operations", "operator", "review_inbox", repository="team/other")["inbox"]["total"] == 0
        report = subprocess.run(
            [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
             "review-inbox", "--repository", filters["repository"], "--owner", filters["owner"]],
            capture_output=True, text=True, timeout=15,
            env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
        )
        assert report.returncode == 0, report.stderr
        assert json.loads(report.stdout)["inbox"] == inbox
        brief = subprocess.run(
            report.args + ["--brief"], capture_output=True, text=True, timeout=15,
            env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
        )
        assert brief.returncode == 0, brief.stderr
        assert f"Verify candidate | {order['id']} | team/service" in brief.stdout
        assert f"Intent  {order['intent']}" in brief.stdout and result["candidate"] in brief.stdout

        def pending(**filters):
            return request("verifier", "service-verifier", "pending_verifications",
                           **({"repository": "team/service", "limit": 1} | filters))["verification_inbox"]

        target = {"work_id": order["id"], "generation": result["generation"], "head": result["candidate"],
                  "pack_digest": pack["digest"], "requires_native_checks": False, "issue": order["issue"]}
        assert pending() == {"repository": "team/service", "items": [target], "next_after_work_id": None}
        request("verifier", "verifier", "pending_verifications", 403, repository="team/service", limit=1)
        request("verifier", "other-verifier", "pending_verifications", 403, repository="team/service", limit=1)
        request("verifier", "first", "pending_verifications", 403, repository="team/service", limit=1)
        request("verifier", "service-verifier", "pending_verifications", 400, repository="team/service", limit=51)
        request("verifier", "service-verifier", "pending_verifications", 400, repository="team/service", limit=1,
                after_work_id="../invalid")
        verifier_inbox = subprocess.check_output(
            [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
             "pending-verifications", "team/service", "--limit", "1"], text=True, timeout=15,
            env=os.environ | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["service-verifier"]},
        )
        assert json.loads(verifier_inbox)["verification_inbox"] == pending()

        def operator_review(repository="team/service"):
            return request("operations", "operator", "operator_brief", repository=repository,
                           usage_hours=24, limit=1)["operator_brief"]["review"]["items"]

        [waiting] = operator_review()
        assert waiting["work_id"] == order["id"] and waiting["next_action"] == "verify_candidate"
        assert "verification_pickup" not in waiting
        sql("UPDATE craft_queue.candidate_packs SET submitted_at = clock_timestamp() - interval '20 minutes' "
            f"WHERE work_id = '{order['id']}' AND generation = {target['generation']};")
        waiting_metrics = scrape("monitor")
        assert waiting_metrics["oldest_waiting_verification_seconds"] >= 1200

        def claim_verification(actor):
            return request("verifier", actor, "claim_pending_verification", repository="team/service")["verification_pickup"]

        verifiers = ("service-verifier", "peer-verifier")
        with ThreadPoolExecutor(max_workers=2) as pool:
            pickups = list(pool.map(claim_verification, verifiers))
        [winner] = [actor for actor, pickup in zip(verifiers, pickups) if pickup]
        loser = next(actor for actor in verifiers if actor != winner)
        claim = pickups[verifiers.index(winner)]
        assert all(claim[key] == value for key, value in target.items())
        [in_progress] = operator_review()
        assert in_progress["head"] == target["head"]
        assert in_progress["verification_pickup"]["verifier"] == winner
        assert in_progress["verification_pickup"]["expires_at"] == claim["pickup_expires_at"]
        assert all("verification_pickup" not in row for row in operator_review("team/other"))
        active_metrics = scrape("monitor")
        assert active_metrics["waiting_verification_candidates"] == waiting_metrics["waiting_verification_candidates"] - 1
        assert active_metrics["active_verification_pickups"] == waiting_metrics["active_verification_pickups"] + 1
        if active_metrics["waiting_verification_candidates"] == 0:
            assert "oldest_waiting_verification_seconds" not in active_metrics
        assert pending()["items"] == []
        pickup = {"repository": "team/service", "pickup_id": claim["pickup_id"]}
        request("verifier", "verifier", "renew_verification_pickup", 403, **pickup)
        request("verifier", "other-verifier", "renew_verification_pickup", 403, **pickup)
        request("verifier", loser, "renew_verification_pickup", 409, **pickup)
        extended = request("verifier", winner, "renew_verification_pickup", **pickup)["verification_pickup"]
        assert extended["pickup_id"] == claim["pickup_id"]
        assert datetime.fromisoformat(extended["pickup_expires_at"]) > datetime.fromisoformat(claim["pickup_expires_at"])
        assert operator_review()[0]["verification_pickup"]["expires_at"] == extended["pickup_expires_at"]
        assert pending()["items"] == []
        sql("UPDATE craft_queue.verification_pickups SET picked_at = clock_timestamp() - interval '55 minutes', "
            f"expires_at = clock_timestamp() + interval '2 minutes' WHERE id = '{claim['pickup_id']}'::uuid;")
        capped = request("verifier", winner, "renew_verification_pickup", **pickup)["verification_pickup"]
        assert capped["pickup_expires_at"] == capped["renewable_until"]
        assert request("verifier", winner, "renew_verification_pickup", **pickup)["verification_pickup"] == capped
        verdict = {"work_id": order["id"], "generation": target["generation"], "head": target["head"],
                   "digest": target["pack_digest"], "previous_sequence": 0, "passed": True,
                   "evidence": "fixture:pickup-verdict", "note": "First check."}
        request("verifier", loser, "release_verification_pickup", 409, **pickup)
        request("verifier", "verifier", "record_verification", 403, **verdict, pickup_id=claim["pickup_id"])
        request("verifier", loser, "record_verification", 409, **verdict, pickup_id=claim["pickup_id"])
        subprocess.check_call([str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                               "release-verification-pickup", pickup["repository"], pickup["pickup_id"]],
                              env=os.environ | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens[winner]}, timeout=15,
                              stdout=subprocess.DEVNULL)
        assert request("verifier", winner, "release_verification_pickup", **pickup)["released"]
        assert "verification_pickup" not in operator_review()[0]
        returned_metrics = scrape("monitor")
        for key in ("waiting_verification_candidates", "active_verification_pickups"):
            assert returned_metrics[key] == waiting_metrics[key]
        request("verifier", winner, "renew_verification_pickup", 409, **pickup)
        request("verifier", winner, "record_verification", 409, **verdict, pickup_id=claim["pickup_id"])
        abandoned = claim_verification(winner)
        sql(f"UPDATE craft_queue.verification_pickups SET expires_at = clock_timestamp() - interval '1 second' "
            f"WHERE id = '{abandoned['pickup_id']}'::uuid;")
        assert "verification_pickup" not in operator_review()[0]
        renewed = claim_verification(loser)
        assert renewed["work_id"] == order["id"] and renewed["pickup_id"] != abandoned["pickup_id"]
        assert operator_review()[0]["verification_pickup"]["verifier"] == loser
        request("verifier", winner, "renew_verification_pickup", 409, **(pickup | {"pickup_id": abandoned["pickup_id"]}))
        request("verifier", winner, "record_verification", 409, **verdict, pickup_id=abandoned["pickup_id"])
        request("verifier", winner, "release_verification_pickup", **(pickup | {"pickup_id": abandoned["pickup_id"]}))
        assert pending()["items"] == []
        request("verifier", loser, "release_verification_pickup", **(pickup | {"pickup_id": renewed["pickup_id"]}))
        assert pending()["items"] == [target]

        second_order = order | {"id": "zz-verifier-page"}
        sql("UPDATE craft_queue.control SET review_limit = 2;")
        request("operator", "operator", "admit", repository="team/service", event_key=second_order["id"],
                work_order=second_order, token_limit=10)
        second_lease = request("runner", "first", "claim", request_id=str(uuid4()), context_capable=True)["claim"]
        assert second_lease["work_id"] == second_order["id"]
        request("runner", "first", "submit_pack", work_id=second_order["id"], generation=second_lease["generation"],
                commit=result["candidate"], pack_base64=pack["pack_base64"], tokens=0)
        assert pending()["next_after_work_id"] == order["id"]
        after = pending(after_work_id=order["id"])
        assert after["items"] == [target | {"work_id": second_order["id"]}]
        assert after["next_after_work_id"] is None
        second_pickup = request("verifier", "service-verifier", "claim_pending_verification",
                                repository="team/service", after_work_id=order["id"])["verification_pickup"]
        second_verdict = verdict | {"work_id": second_order["id"], "pickup_id": second_pickup["pickup_id"]}
        recorded = request("verifier", "service-verifier", "record_verification", **second_verdict)
        request("verifier", "service-verifier", "renew_verification_pickup", 409,
                repository="team/service", pickup_id=second_pickup["pickup_id"])
        sql(f"UPDATE craft_queue.verification_pickups SET expires_at = clock_timestamp() - interval '1 second' "
            f"WHERE id = '{second_pickup['pickup_id']}'::uuid;")
        assert request("verifier", "service-verifier", "record_verification", **second_verdict) == recorded
        request("verifier", "service-verifier", "record_verification", 409,
                **(second_verdict | {"evidence": "fixture:new-evidence"}))
        request("operator", "operator", "cancel", work_id=second_order["id"], reason="fixture pagination verified")
        assert pending(after_work_id=order["id"])["items"] == []
        sql("UPDATE craft_queue.control SET review_limit = 1;")
        exercise_scoped_review(order, result, pack, request, report.args, tokens)
        # A separate verifier has only the trusted base, even if the runner retained the commit locally.
        verifier_repo = root / "verifier-repo"
        git("clone", "--no-local", "--quiet", str(repo), str(verifier_repo))
        mapping.write_text(json.dumps({"team/service": {"path": str(verifier_repo)}}), encoding="utf-8")
        verification = root / "verification"
        unauthorized = prepare(order["id"], result["candidate"], verification, "first")
        assert unauthorized.returncode != 0 and "HTTP 403" in unauthorized.stderr
        stale = prepare(order["id"], base, verification)
        assert stale.returncode != 0 and "no longer current" in stale.stderr
        assert not verification.exists()
        sql("UPDATE craft_queue.candidate_packs SET digest = 'sha256:' || repeat('0', 64);")
        assert pending()["items"] == []
        corrupt = prepare(order["id"], result["candidate"], verification)
        assert corrupt.returncode != 0 and "digest validation" in corrupt.stderr
        assert not verification.exists()
        sql(f"UPDATE craft_queue.candidate_packs SET digest = '{pack['digest']}';")
        prepared = prepare(order["id"], result["candidate"], verification)
        assert prepared.returncode == 0, prepared.stderr
        handoff = json.loads(prepared.stdout)
        assert handoff["status"] == "ready_for_independent_checks"
        assert handoff["head"] == result["candidate"] and handoff["pack_digest"] == pack["digest"]
        assert handoff["verification_sequence"] == 0
        assert json.loads((verification / "handoff.json").read_text()) == handoff
        brief_path = verification / "review-brief.md"
        assert Path(handoff["review_brief"]) == brief_path
        raw_brief = brief_path.read_bytes()
        assert handoff["review_brief_digest"] == f"sha256:{sha256(raw_brief).hexdigest()}"
        brief = raw_brief.decode("utf-8")
        assert f"Revision `{result['candidate']}` against `{base}`" in brief
        assert "### Test intent" in brief and "### Independent checks" in brief
        assert "- src/hello (+1 / -1)" in brief and pack["digest"] in brief
        assert "### Repair comparison" not in brief
        checkout = Path(handoff["checkout"])
        assert (checkout / "src" / "hello").read_text() == "changed by fixture agent"
        assert git("-C", str(checkout), "rev-parse", "HEAD").decode().strip() == result["candidate"]
        assert git("-C", str(checkout), "remote") == b""
        assert git("-C", str(verifier_repo), "rev-parse", "HEAD").decode().strip() == base
        isolated = subprocess.run(["git", "-C", str(verifier_repo), "cat-file", "-e", result["candidate"]],
                                  capture_output=True, timeout=5)
        assert isolated.returncode != 0, "the verifier imported the pack into the source checkout"
        handoff_path = verification / "handoff.json"
        denied = attest(handoff_path, "passed", "test:first", "first")
        assert denied.returncode != 0 and "HTTP 403" in denied.stderr
        rejected = attest(handoff_path, "failed", "test:first")
        assert rejected.returncode == 0, rejected.stderr
        assert pending()["items"] == []
        recorded = json.loads(rejected.stdout)
        assert json.loads(attest(handoff_path, "failed", "test:first").stdout) == recorded
        changed = attest(handoff_path, "passed", "test:first")
        assert changed.returncode != 0 and "HTTP 409" in changed.stderr
        stale = attest(handoff_path, "passed", "test:second")
        assert stale.returncode != 0 and "HTTP 409" in stale.stderr
        request("operator", "verifier", "cancel", 403, work_id=order["id"], reason="out of role")
        status = request("verifier", "verifier", "status", work_id=order["id"])["work"]
        assert not status["independent_checks_passed"] and status["verification"]["verifier"] == "verifier"
        fresh = root / "verification-rerun"
        prepared = prepare(order["id"], result["candidate"], fresh, "verifier", verifier=True)
        assert prepared.returncode == 0, prepared.stderr
        assert json.loads(prepared.stdout)["verification_sequence"] == recorded["verification_sequence"]
        review_summary = "Retried the missing-field case. The original value is preserved."
        fresh_handoff = fresh / "handoff.json"
        invalid_summary = attest(fresh_handoff, "passed", "test:second", review_summary="x" * 241)
        assert invalid_summary.returncode and "HTTP 400" in invalid_summary.stderr, invalid_summary.stderr
        accepted = attest(fresh_handoff, "passed", "test:second", review_summary=review_summary)
        assert accepted.returncode == 0, accepted.stderr
        replay = attest(fresh_handoff, "passed", "test:second", review_summary=review_summary)
        assert replay.returncode == 0 and json.loads(replay.stdout) == json.loads(accepted.stdout)
        conflicting = attest(fresh_handoff, "passed", "test:second", review_summary="A different result")
        assert conflicting.returncode and "HTTP 409" in conflicting.stderr
        status = request("operator", "operator", "status", work_id=order["id"])["work"]
        assert status["independent_checks_passed"] and status["verification"]["evidence"] == "test:second"
        assert status["state"] == "candidate"
        policy = root / "twg-policy.json"
        policy.write_text(json.dumps({
            "schema_version": 1, "twg": str(agent), "profile": "fixture",
            "site": "team.atlassian.net", "workspace": "team", "repository": "service",
            "base_branch": "main", "required_steps": ["Verify candidate"], "check_definitions": [],
        }), encoding="utf-8")

        def reconcile(key, wrong_head=False, head=None, pull_request="101"):
            env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"],
                                "FACTORY_FIXTURE_SHA": head or result["candidate"], "FACTORY_FIXTURE_BRANCH": base}
            if wrong_head:
                env["FACTORY_FIXTURE_DRAFT_HEAD"] = base
            return subprocess.run(
                [str(launcher.resolve()), "reconcile-shared-draft", "--queue", address,
                 "--ca", str(certificate), "--action-key", key, "--policy", str(policy),
                 "--pull-request", pull_request],
                capture_output=True, text=True, timeout=15, env=env,
            )

        exercise_draft_policy_setup(request, launcher, address, certificate, tokens)
        publication_key = exercise_publication(request, status, reconcile)
        shadow_handoff = root / "shadow-handoff"
        prepared = prepare(order["id"], result["candidate"], shadow_handoff)
        assert prepared.returncode == 0, prepared.stderr
        shadow_binding = exercise_shadow_receipts(
            root, repo, order, json.loads(prepared.stdout), shadow_handoff / "handoff.json",
            request, launcher, address, certificate, tokens,
        )
        assert sql("SELECT reported_tokens FROM craft_queue.attempts WHERE work_id = 'launcher-one';") == "30"
        feedback = {
            "work_id": order["id"], "generation": 1, "head": result["candidate"],
            "native_url": "https://bitbucket.org/team/service/pull-requests/101",
            "evidence": "fixture:simulated-review-101",
            "note": "Preserve the reviewed change and say repaired",
        }
        request("operator", "first", "request_repair", 403, **feedback)
        request("operator", "operator", "request_repair", 409, **(feedback | {"head": base}))
        assignment = {
            "handoff_id": "draft-review-101", "work_id": order["id"], "generation": 1, "head": result["candidate"],
            "verification_sequence": status["verification"]["sequence"], "reviewer_id": "reviewer",
            "evidence": "fixture:assigned-draft",
        }
        request("operations", "operator", "assign_review", 409, **assignment)
        for identity in ("reviewer", "reviewer-other"):
            request("operations", "operator", "grant_review_inbox", grant_id=f"handoff-{identity}",
                    reviewer_id=identity, repository="team/service", evidence="fixture:handoff-scope",
                    valid_seconds=86400 if identity == "reviewer" else 3600)
        request("operations", "operator", "assign_review", 409, **(assignment | {"head": base}))
        request("operations", "operator", "assign_review", 409,
                **(assignment | {"verification_sequence": assignment["verification_sequence"] - 1}))
        handoff = request("operations", "operator", "assign_review", **assignment)["handoff"]
        assert handoff["publication_key"] == publication_key
        assert request("operations", "operator", "assign_review", **assignment)["handoff"] == handoff
        other = assignment | {"handoff_id": "draft-review-other-101", "reviewer_id": "reviewer-other"}
        request("operations", "operator", "assign_review", **other)
        request("operations", "operator", "reassign_review", 409, handoff_id=assignment["handoff_id"],
                replacement_handoff_id="draft-review-duplicate", head=feedback["head"],
                reviewer_id="reviewer-other", evidence="fixture:duplicate-assignee")
        sql("""UPDATE craft_queue.review_inbox_grants
               SET granted_at = granted_at - interval '6 hours', expires_at = expires_at - interval '6 hours'
               WHERE grant_id = 'handoff-reviewer';
               UPDATE craft_queue.review_handoffs SET requested_at = clock_timestamp() - interval '5 hours'
               WHERE id = 'draft-review-101';""")
        review_metrics = scrape("monitor")
        assert review_metrics["pending_review_assignments"] == 2
        assert 18000 <= review_metrics["oldest_pending_review_seconds"] < 18100
        summary = request("operations", "operator", "summary")["summary"]
        assert summary["pending_review_assignments"] == 2 and summary["oldest_pending_review_seconds"] >= 18000
        overview = request("operations", "operator", "operator_brief", repository="team/service",
                           usage_hours=24, limit=1)["operator_brief"]
        assert overview["queue"]["pending_review_assignments"] == overview["assignments"]["total"] == 2
        [visible_assignment] = overview["assignments"]["assignments"]
        assert visible_assignment["work_id"] == order["id"] and visible_assignment["draft_url"] == feedback["native_url"]
        for identity in ("monitor", "reviewer"):
            request("operations", identity, "review_wait", 403, limit=1)
        request("operations", "operator", "review_wait", 400, limit=51)
        request("operations", "operator", "review_wait", 400, limit=1, repository="invalid")
        empty_wait = {"total": 0, "oldest_seconds": None, "assignments": []}
        assert request("operations", "operator", "review_wait", limit=1,
                       repository="team/other")["review_wait"] == empty_wait
        wait_command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                        "review-wait", "--repository", "team/service", "--limit", "1"]
        wait_env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
        backlog = json.loads(subprocess.check_output(wait_command, env=wait_env, text=True,
                                                     timeout=15))["review_wait"]
        [oldest] = backlog["assignments"]
        assert backlog["total"] == 2 and backlog["oldest_seconds"] == oldest["wait_seconds"] >= 18000
        assert oldest["handoff_id"] == assignment["handoff_id"] and oldest["reviewer_id"] == "reviewer"
        assert oldest["work_id"] == order["id"] and oldest["repository"] == "team/service"
        assert oldest["head"] == feedback["head"] and oldest["draft_url"] == feedback["native_url"]
        assert "note" not in oldest and "request_evidence" not in oldest
        sql("""UPDATE craft_queue.review_inbox_grants
               SET granted_at = granted_at - interval '2 hours', expires_at = expires_at - interval '2 hours'
               WHERE grant_id = 'handoff-reviewer-other';""")
        assert scrape("monitor")["pending_review_assignments"] == 1
        assert request("operations", "operator", "review_wait", limit=10)["review_wait"]["total"] == 1
        sql("""UPDATE craft_queue.review_inbox_grants
               SET granted_at = granted_at + interval '2 hours', expires_at = expires_at + interval '2 hours'
               WHERE grant_id = 'handoff-reviewer-other';""")
        native_target = {"handoff_id": assignment["handoff_id"]}
        request("reviewer", "reviewer-other", "review_feedback_target", 409, **native_target)
        assert request("reviewer", "reviewer", "review_feedback_target", **native_target)["target"] == {
            "draft": {"reference": feedback["native_url"], "head": feedback["head"]},
            "base": base, "issue": order["issue"],
        }
        [mine] = request("reviewer", "reviewer", "review_inbox")["inbox"]["assignments"]
        assert mine["head"] == feedback["head"] and mine["draft_url"] == feedback["native_url"]
        assert mine["id"] == assignment["handoff_id"]
        focused = request("reviewer", "reviewer", "review_inbox", assigned_only=True, limit=1)["inbox"]
        assert focused == {"assignments": [mine]}
        assert mine["issue"] == order["issue"] and mine["intent_excerpt"] == order["intent"][:240]
        assert mine["acceptance_excerpts"] == [criterion[:160] for criterion in order["acceptance"][:3]]
        assert mine["verification_sequence"] == assignment["verification_sequence"]
        assert mine["verification_evidence"] == status["verification"]["evidence"]
        assert mine["review_summary"] == review_summary and "note" not in mine
        assert request("reviewer", "reviewer", "review_inbox", assigned_only=True,
                       repository="team/other")["inbox"] == {"assignments": []}
        [other_view] = request("reviewer", "reviewer-other", "review_inbox", assigned_only=True)["inbox"]["assignments"]
        assert other_view["id"] == other["handoff_id"]
        request("operations", "operator", "review_inbox", 400, assigned_only=True)
        next_work = "launcher-page-next"
        next_sequence = clone_reviewable_work(order["id"], assignment["verification_sequence"], next_work,
                                             "https://bitbucket.org/team/service/pull-requests/1901")
        next_handoff = "draft-review-page-next"
        request("operations", "operator", "assign_review", **(assignment | {
            "handoff_id": next_handoff, "work_id": next_work, "verification_sequence": next_sequence, "queue_repair": True,
        }))
        routed_work = "launcher-rostered-review"
        routed_sequence = clone_reviewable_work(order["id"], assignment["verification_sequence"], routed_work,
                                               "https://bitbucket.org/team/service/pull-requests/1902")
        route = {
            "handoff_id": "draft-review-routed", "work_id": routed_work, "generation": 1,
            "head": feedback["head"], "verification_sequence": routed_sequence,
            "roster": ["reviewer", "reviewer-other"], "max_pending": 3, "evidence": "fixture:rostered-review",
        }
        request("operations", "reviewer", "route_review", 403, **route)
        request("operations", "operator", "route_review", 400, **(route | {"roster": ["reviewer", "reviewer"]}))
        request("operations", "operator", "route_review", 409, **(route | {"max_pending": 1}))
        request("operations", "operator", "route_review", 409, **(route | {"verification_sequence": next_sequence}))
        route_command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                         "route-review", route["handoff_id"], "--work-id", routed_work, "--generation", "1",
                         "--head", route["head"], "--verification-sequence", str(routed_sequence),
                         "--reviewer-id", "reviewer", "--reviewer-id", "reviewer-other",
                         "--max-pending", "3", "--evidence", route["evidence"]]
        routed = json.loads(subprocess.check_output(route_command, env=wait_env, text=True,
                                                    timeout=15))["assignment"]
        assert routed["reviewer_id"] == "reviewer-other" and routed["verification_sequence"] == routed_sequence
        assert routed["routing"] == {"roster": route["roster"], "max_pending": 3, "pending_before": 1}
        assert request("operations", "operator", "route_review", **route)["assignment"] == routed
        request("operations", "operator", "route_review", 409, **(route | {"roster": route["roster"][::-1]}))
        request("operations", "operator", "route_review", 409, **(route | {
            "handoff_id": "draft-review-routed-second", "roster": ["reviewer-other"],
        }))
        [routed_history] = request("operations", "operator", "audit", work_id=routed_work)["audit"]["review_handoffs"]
        assert routed_history["routing"] == routed["routing"]
        request("operator", "operator", "cancel", work_id=routed_work, reason="verified roster routing")
        assert request("operations", "operator", "route_review", **route)["assignment"] == routed
        print("Verified deterministic review routing, load limits, stale input rejection, and receipt replay.")
        first_page = request("reviewer", "reviewer", "review_inbox", assigned_only=True, limit=1)["inbox"]
        assert first_page == {"assignments": [mine], "next_after": mine["id"]}
        page = {"assigned_only": True, "after": mine["id"], "limit": 1}
        [later] = request("reviewer", "reviewer", "review_inbox", **page)["inbox"]["assignments"]
        assert later["id"] == next_handoff
        assert later["review_summary"] is None
        request("reviewer", "reviewer-other", "review_inbox", 409, **page)
        request("reviewer", "reviewer", "review_inbox", 409, **page, repository="team/other")
        request("reviewer", "reviewer", "review_inbox", 400, after=mine["id"])
        decision = {"handoff_id": assignment["handoff_id"], "head": feedback["head"],
                    "decision": "changes_requested", "evidence": feedback["evidence"], "note": feedback["note"]}
        request("operations", "operator", "apply_review_repair", 409, handoff_id=assignment["handoff_id"])
        request("reviewer", "reviewer-other", "record_review_decision", 409, **decision)
        request("reviewer", "reviewer", "record_review_decision", 409, **(decision | {"head": base}))
        shared_ops = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
        focus_command = shared_ops + ["review-inbox", "--reviewer", "--assigned-only", "--brief", "--limit", "1"]
        reviewer_env = os.environ | {"CRAFT_FACTORY_REVIEWER_TOKEN": tokens["reviewer"]}
        bitbucket_policy = root / "unused-bitbucket-import-policy.json"
        bitbucket_policy.write_text(json.dumps({"schema_version": 1, "workspace": "team", "repository": "service",
                                               "repository_uuid": "00000000-0000-0000-0000-000000000001",
                                               "base_branch": "main", "reviewer_uuids": []}), encoding="utf-8")
        missing_evidence_token = subprocess.run(
            shared_ops + ["import-bitbucket-review", assignment["handoff_id"], "--comment", "61",
                          "--policy", str(bitbucket_policy)],
            env=reviewer_env | {"CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN": ""},
            capture_output=True, text=True, timeout=15,
        )
        assert missing_evidence_token.returncode and "invalid read-only Bitbucket credential" in missing_evidence_token.stderr
        focus = subprocess.check_output(focus_command, env=reviewer_env, text=True, timeout=15)
        assert "Assigned reviews   1 shown" in focus and assignment["handoff_id"] in focus
        assert mine["verification_evidence"] in focus and feedback["native_url"] in focus
        assert f"Test summary  {review_summary}" in focus and "Independent fixture test" not in focus
        assert "Review inbox" not in focus and other["handoff_id"] not in focus and next_handoff not in focus
        assert f"--after {mine['id']}" in focus
        decision_cmd = shared_ops + ["record-review-decision", decision["handoff_id"], "--head", decision["head"],
                                     "--decision", "changes-requested", "--evidence", decision["evidence"],
                                     "--note", decision["note"]]
        voted = subprocess.run(decision_cmd, capture_output=True, text=True, timeout=15,
                               env=os.environ | {"CRAFT_FACTORY_REVIEWER_TOKEN": tokens["reviewer"]})
        assert voted.returncode == 0, voted.stderr
        receipt = json.loads(voted.stdout)["handoff"]
        assert receipt["decision"] == "changes_requested" and receipt["reviewer_id"] == "reviewer"
        assert request("reviewer", "reviewer", "record_review_decision", **decision)["handoff"] == receipt
        request("reviewer", "reviewer", "record_review_decision", 409,
                **(decision | {"decision": "acknowledged"}))
        focus = subprocess.check_output(focus_command + ["--after", mine["id"]],
                                        env=reviewer_env, text=True, timeout=15)
        assert next_handoff in focus and "Continue with --after" not in focus
        assert "Test summary" not in focus
        assert request("reviewer", "reviewer", "review_inbox", **page)["inbox"] == {"assignments": [later]}
        reassign = {
            "handoff_id": next_handoff, "replacement_handoff_id": "draft-review-page-replacement",
            "head": feedback["head"], "reviewer_id": "reviewer-other", "evidence": "fixture:reviewer-unavailable",
        }
        request("operations", "reviewer", "reassign_review", 403, **reassign)
        request("operations", "operator", "reassign_review", 409, **(reassign | {"head": base}))
        request("operations", "operator", "reassign_review", 404, **(reassign | {"reviewer_id": "unknown"}))
        assert request("reviewer", "reviewer", "review_inbox", **page)["inbox"] == {"assignments": [later]}
        replaced = json.loads(subprocess.check_output(
            shared_ops + ["reassign-review", next_handoff, reassign["replacement_handoff_id"],
                          "--head", feedback["head"], "--reviewer-id", reassign["reviewer_id"],
                          "--evidence", reassign["evidence"]],
            env=wait_env, text=True, timeout=15,
        ))["reassignment"]
        assert replaced["head"] == feedback["head"] and replaced["verification_sequence"] == next_sequence
        assert replaced["queue_repair"] is False and replaced["work_id"] == next_work
        assert request("operations", "operator", "reassign_review", **reassign)["reassignment"] == replaced
        request("operations", "operator", "reassign_review", 409, **(reassign | {"queue_repair": True}))
        request("reviewer", "reviewer", "record_review_decision", 409, **(decision | {"handoff_id": next_handoff}))
        assert request("reviewer", "reviewer", "review_inbox", **page)["inbox"] == {"assignments": []}
        others = request("reviewer", "reviewer-other", "review_inbox", assigned_only=True)["inbox"]["assignments"]
        assert {item["id"] for item in others} == {other["handoff_id"], reassign["replacement_handoff_id"]}
        returned = request("operations", "operator", "reassign_review", **(reassign | {
            "handoff_id": reassign["replacement_handoff_id"], "replacement_handoff_id": "draft-review-page-returned",
            "reviewer_id": "reviewer", "evidence": "fixture:reviewer-returned", "queue_repair": True,
        }))["reassignment"]
        assert returned["queue_repair"] is True
        assert request("operations", "operator", "reassign_review", **reassign)["reassignment"] == replaced
        history = request("operations", "operator", "audit", work_id=next_work)["audit"]["review_handoffs"]
        retired = next(item for item in history if item["id"] == next_handoff)
        assert retired["decision"] == "withdrawn" and retired["withdrawn_by"] == "operator"
        assert retired["replacement_handoff_id"] == reassign["replacement_handoff_id"]
        assert retired["decision_evidence"] == reassign["evidence"]
        [current] = request("reviewer", "reviewer", "review_inbox", **page)["inbox"]["assignments"]
        assert current["id"] == returned["replacement_handoff_id"] and current["queue_repair"]
        print("Verified atomic review reassignment, preserved grants, and replay after a subsequent reassignment.")
        request("operator", "operator", "cancel", work_id=next_work, reason="fixture pagination complete")
        assert scrape("monitor")["pending_review_assignments"] == 1
        assert not request("reviewer", "reviewer", "review_inbox")["inbox"]["assignments"]
        assert request("reviewer", "reviewer", "review_inbox", assigned_only=True)["inbox"] == {"assignments": []}
        assert request("operator", "operator", "status", work_id=order["id"])["work"]["state"] == "candidate"
        [operator_entry] = request("operations", "operator", "review_inbox", **filters)["inbox"]["items"]
        assert operator_entry["next_action"] == "resolve_review_changes"
        pending_repair = operator_entry["pending_repair"]
        assert pending_repair["handoff_id"] == assignment["handoff_id"] and pending_repair["decision_count"] == 1
        assert pending_repair["feedback_excerpt"] == feedback["note"] and pending_repair["automatic_repair"] is None
        [reviewer_entry] = request("reviewer", "reviewer", "review_inbox", **filters)["inbox"]["items"]
        assert "pending_repair" not in reviewer_entry and reviewer_entry["next_action"] == "inspect_draft"
        request("operations", "reviewer", "apply_review_repair", 403, handoff_id=assignment["handoff_id"])
        repair_cmd = shared_ops + ["apply-review-repair", assignment["handoff_id"]]
        repair_env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
        submitted = subprocess.run(repair_cmd, env=repair_env, capture_output=True, text=True, timeout=15)
        assert submitted.returncode == 0, submitted.stderr
        recorded_feedback = json.loads(submitted.stdout)["repair"]
        assert recorded_feedback["head"] == result["candidate"]
        assert recorded_feedback["native_url"] == feedback["native_url"]
        review_metrics = scrape("monitor")
        assert review_metrics["pending_review_assignments"] == 0 and "oldest_pending_review_seconds" not in review_metrics
        assert request("operations", "operator", "review_wait", limit=10)["review_wait"] == empty_wait
        print("Verified review-wait gauges exclude expired assignments, decisions, and superseded revisions.")
        replayed = subprocess.run(repair_cmd, env=repair_env, capture_output=True, text=True, timeout=15)
        assert replayed.returncode == 0 and json.loads(replayed.stdout)["repair"] == recorded_feedback
        assert not request("reviewer", "reviewer-other", "review_inbox")["inbox"]["assignments"]
        request("reviewer", "reviewer-other", "record_review_decision", 409,
                **(decision | {"handoff_id": other["handoff_id"]}))
        audit = request("operations", "operator", "audit", work_id=order["id"])["audit"]
        assert receipt in audit["review_handoffs"]
        for identity in ("reviewer", "reviewer-other"):
            request("operations", "operator", "revoke_review_inbox", grant_id=f"handoff-{identity}",
                    evidence="fixture:handoff-complete")
        request("reviewer", "reviewer", "review_inbox", 409, **page)
        request("reviewer", "reviewer", "review_feedback_target", 409, **native_target)
        request("operator", "operator", "request_repair", 409, **(feedback | {"note": "Conflicting feedback"}))
        summary = request("operations", "operator", "summary")["summary"]
        assert summary["queued_repairs"] == summary["review_reserved"] == 1
        request("runner", "first", "repair_context", 409, work_id=order["id"], generation=1)
        prior_publication = request("publication", "operator", "reserve", work_id=order["id"],
                                    generation=1, head=result["candidate"],
                                    verification_sequence=status["verification"]["sequence"])["publication"]
        assert prior_publication["action_key"] == publication_key

        repair_request = str(uuid4())
        repair_claim = request("runner", "second", "claim", request_id=repair_request)["claim"]
        assert repair_claim["work_id"] == order["id"] and repair_claim["generation"] == 2
        coordinates = {"work_id": order["id"], "generation": repair_claim["generation"]}
        bound_context = request("runner", "second", "repair_context", **coordinates)["repair"]
        assert bound_context["head"] == result["candidate"] and bound_context["digest"] == result["candidate_evidence"]
        assert bound_context["note"] == feedback["note"] and bound_context["evidence"] == feedback["evidence"]
        request("runner", "first", "repair_context", 409, **coordinates)

        with block_release_context(order["id"]):
            request("runner", "second", "renew", 409, **coordinates)
            denied = request("runner", "second", "repair_context", 409, **coordinates)
            assert "repair" not in denied and feedback["note"] not in json.dumps(denied)
        assert request("runner", "second", "repair_context", **coordinates)["repair"] == bound_context
        repaired = finish(launch("second", root / "shared-repair", repair_request))
        assert repaired["work_id"] == order["id"] and repaired["generation"] == 2
        assert repaired["candidate"] != result["candidate"]
        request("operations", "operator", "record_shadow_merge", 409, **(shadow_binding | {
            "receipt_id": "stale-shadow", "report_digest": f"sha256:{'9' * 64}",
        }))
        assert request("operations", "operator", "record_shadow_merge", **shadow_binding)["receipt"]["id"] == "shadow-positive"
        current = request("operator", "operator", "status", work_id=order["id"])["work"]
        assert not current["independent_checks_passed"]
        [entry] = request("operations", "operator", "review_inbox", **filters)["inbox"]["items"]
        assert entry["head"] == repaired["candidate"] and entry["generation"] == 2
        assert entry["next_action"] == "verify_candidate" and entry["publication"] is None
        assert entry["prior_draft_url"] == feedback["native_url"]
        reservation = {
            "work_id": order["id"], "generation": repaired["generation"], "head": repaired["candidate"],
            "verification_sequence": status["verification"]["sequence"], "update_draft": True,
        }
        request("publication", "operator", "reserve", 409, **reservation)
        stale = attest(fresh / "handoff.json", "passed", "fixture:stale")
        assert stale.returncode != 0 and "HTTP 409" in stale.stderr
        repair_verification = root / "repair-verification"
        prepared = prepare(order["id"], repaired["candidate"], repair_verification, "verifier", verifier=True)
        assert prepared.returncode == 0, prepared.stderr
        repair_packet = json.loads(prepared.stdout)
        repair_checkout = Path(repair_packet["checkout"])
        repair_brief = Path(repair_packet["review_brief"]).read_text(encoding="utf-8")
        assert f"Revision `{repaired['candidate']}` against `{base}`" in repair_brief
        assert f"Changes since reviewed revision `{result['candidate']}`" in repair_brief
        assert "### Repair comparison" in repair_brief and "Queue generation 2" in repair_brief
        assert (repair_checkout / "src" / "hello").read_text() == "repaired from reviewed revision"
        parent = git("-C", str(repair_checkout), "rev-parse", "HEAD^").decode().strip()
        assert parent == result["candidate"]
        accepted_repair = attest(repair_verification / "handoff.json", "passed", "fixture:repaired")
        assert accepted_repair.returncode == 0, accepted_repair.stderr
        reservation["verification_sequence"] = json.loads(accepted_repair.stdout)["verification_sequence"]
        request("publication", "operator", "reserve", 409, **(reservation | {"update_draft": False}))
        update_cmd = [str(launcher.resolve()), "reserve-shared-draft", "--queue", address,
                      "--ca", str(certificate), "--work-id", reservation["work_id"],
                      "--generation", str(reservation["generation"]), "--head", reservation["head"],
                      "--verification-sequence", str(reservation["verification_sequence"]), "--update-draft"]
        reserved = subprocess.run(update_cmd, env=repair_env, capture_output=True, text=True, timeout=15)
        assert reserved.returncode == 0, reserved.stderr
        update = json.loads(reserved.stdout)["publication"]
        assert update["prior_action_key"] == publication_key and update["repair_head"] == result["candidate"]
        assert update["expected_draft_url"] == feedback["native_url"] and update["result_url"] is None
        assert request("publication", "operator", "reserve", **reservation)["publication"] == update
        request("publication", "operator", "reserve", 409, **(reservation | {"update_draft": False}))
        key = update["action_key"]
        request("publisher", "publisher", "prepare", 409, action_key=key)
        request("publisher", "publisher-other", "prepare", 409, action_key=key, update_draft=True)
        prepared_update = request("publisher", "publisher", "prepare", action_key=key, update_draft=True)
        assert prepared_update["publication"]["repair_head"] == parent
        assert prepared_update["publication"]["pack_digest"] == repaired["candidate_evidence"]
        request("publisher", "publisher", "begin", 409, action_key=key)
        request("publisher", "publisher", "begin", action_key=key, update_draft=True)
        request("publisher", "publisher", "begin", 409, action_key=key, update_draft=True)
        request("operator", "operator", "cancel", 409, work_id=order["id"], reason="Unconfirmed update")

        wrong_draft = reconcile(key, head=repaired["candidate"], pull_request="42")
        assert wrong_draft.returncode != 0 and "existing draft's pull request" in wrong_draft.stderr
        wrong_head = reconcile(key)
        assert wrong_head.returncode != 0 and "exact open draft" in wrong_head.stderr
        request("publication", "operator", "reconcile", 409, action_key=key, observation={
            "outcome": "confirmed", "head": repaired["candidate"],
            "native_url": "https://bitbucket.org/team/service/pull-requests/42",
            "source_branch": update["source_branch"], "base_branch": update["base_branch"],
            "evidence": "fixture:mismatched existing draft",
        })
        reconciled_update = reconcile(key, head=repaired["candidate"])
        assert reconciled_update.returncode == 0, reconciled_update.stderr
        confirmed = json.loads(reconciled_update.stdout)["publication"]
        assert confirmed["result_url"] == feedback["native_url"] and confirmed["head"] == repaired["candidate"]
        assert [event["state"] for event in confirmed["events"]] == ["reserved", "sending", "confirmed"]
        published = request("publication", "operator", "status", action_key=publication_key)["publication"]
        assert published["result_url"] == feedback["native_url"] and published["head"] == result["candidate"]
        assignment_arguments = {"work_id": order["id"], "generation": repaired["generation"],
                                "head": repaired["candidate"], "verification_sequence": reservation["verification_sequence"],
                                "evidence": "fixture:allow-one-review-repair", "queue_repair": True}
        for identity in ("reviewer", "reviewer-other"):
            request("operations", "operator", "grant_review_inbox", grant_id=f"automatic-{identity}",
                    reviewer_id=identity, repository="team/service", evidence="fixture:automatic-review", valid_seconds=600)
        paused_assignment = assignment_arguments | {"handoff_id": "paused-auto-review", "reviewer_id": "reviewer-other"}
        request("operations", "operator", "assign_review", **paused_assignment)
        assignment = assignment_arguments | {"handoff_id": "auto-review", "reviewer_id": "reviewer"}
        assign = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                  "assign-review", "auto-review", "--work-id", order["id"], "--generation", str(repaired["generation"]),
                  "--head", repaired["candidate"], "--verification-sequence", str(reservation["verification_sequence"]),
                  "--reviewer-id", "reviewer", "--evidence", assignment["evidence"], "--queue-repair"]
        assigned = json.loads(subprocess.check_output(assign, env=repair_env, text=True, timeout=15))["handoff"]
        assert assigned["queue_repair"] and assigned["automatic_repair"] is None
        paused_decision = {"handoff_id": "paused-auto-review", "head": repaired["candidate"],
                           "decision": "changes_requested", "evidence": "fixture:first-review", "note": "Handle the empty value"}
        held = request("reviewer", "reviewer-other", "record_review_decision", **paused_decision)["handoff"]
        assert held["automatic_repair"] == "needs_operator"
        assert request("operator", "operator", "status", work_id=order["id"])["work"]["state"] == "candidate"
        request("operator", "operator", "set_paused", paused=True)
        request("operations", "operator", "apply_review_repair", 409, handoff_id=paused_decision["handoff_id"])
        [waiting] = request("operations", "operator", "review_inbox", **filters)["inbox"]["items"]
        assert waiting["next_action"] == "resolve_review_changes"
        assert waiting["pending_repair"]["blocker"] == "queue_paused"
        assert waiting["pending_repair"]["outstanding_assignments"] == 1
        assert waiting["pending_repair"]["handoff_id"] == "paused-auto-review"
        summary = subprocess.check_output(shared_ops + ["review-inbox", "--brief"],
                                          env=repair_env, text=True, timeout=15)
        assert "Resolve requested changes" in summary and "Admission is paused." in summary
        request("operator", "operator", "set_paused", paused=False)
        assert request("reviewer", "reviewer-other", "record_review_decision", **paused_decision)["handoff"] == held
        assert request("operator", "operator", "status", work_id=order["id"])["work"]["state"] == "candidate"
        [awaiting] = request("operations", "operator", "review_inbox", **filters)["inbox"]["items"]
        assert awaiting["pending_repair"]["blocker"] == "awaiting_reviews"
        assert awaiting["pending_repair"]["outstanding_assignments"] == 1
        brief = subprocess.check_output(shared_ops + ["review-inbox", "--brief"],
                                        env=repair_env, text=True, timeout=15)
        assert "1 review assignment is awaiting a decision." in brief
        [scoped] = request("reviewer", "reviewer", "review_inbox", **filters)["inbox"]["items"]
        assert "pending_repair" not in scoped
        sql("UPDATE craft_queue.review_inbox_grants SET granted_at = clock_timestamp() - interval '10 minutes', "
            "expires_at = clock_timestamp() - interval '1 second' "
            "WHERE grant_id = 'automatic-reviewer';")
        [unblocked] = request("operations", "operator", "review_inbox", **filters)["inbox"]["items"]
        assert unblocked["pending_repair"]["blocker"] is None
        assert "outstanding_assignments" not in unblocked["pending_repair"]
        sql("UPDATE craft_queue.review_inbox_grants SET expires_at = clock_timestamp() + interval '10 minutes' "
            "WHERE grant_id = 'automatic-reviewer';")

        [pending] = request("reviewer", "reviewer", "review_inbox")["inbox"]["assignments"]
        assert pending["id"] == "auto-review" and pending["queue_repair"] is True
        request("operations", "operator", "assign_review", 409, **(assignment | {"queue_repair": False}))
        decision = paused_decision | {"handoff_id": "auto-review", "evidence": "fixture:second-review-101",
                                      "note": "Second round on the same draft"}
        request("reviewer", "reviewer", "record_review_decision", 409, **(decision | {"head": result["candidate"]}))
        queued = request("reviewer", "reviewer", "record_review_decision", **decision)["handoff"]
        assert queued["automatic_repair"] == "queued" and queued["reviewer_id"] == "reviewer"
        assert request("reviewer", "reviewer", "record_review_decision", **decision)["handoff"] == queued
        follow_up = request("operations", "operator", "apply_review_repair", handoff_id="auto-review")["repair"]
        assert follow_up["from_generation"] == repaired["generation"]
        assert follow_up["review_handoff_ids"] == ["paused-auto-review", "auto-review"]
        assert follow_up["note"] == (
            "Review paused-auto-review (fixture:first-review)\nHandle the empty value\n\n"
            "Review auto-review (fixture:second-review-101)\nSecond round on the same draft"
        )
        assert request("operations", "operator", "apply_review_repair",
                       handoff_id=paused_decision["handoff_id"])["repair"] == follow_up
        assert request("operations", "operator", "review_inbox", **filters)["inbox"]["items"] == []
        assert finish(launch("first", root / "shared-auto-repair", str(uuid4())))["generation"] == 3
        for identity in ("reviewer", "reviewer-other"):
            request("operations", "operator", "revoke_review_inbox", grant_id=f"automatic-{identity}",
                    evidence="fixture:automatic-review-complete")
        mapping.write_text(json.dumps({"team/service": {"path": str(repo)}}), encoding="utf-8")
        for _ in range(2):
            assert operator_work("cancel", order["id"], "--reason", "fixture complete")["cancelled"] is True
        assert "did not confirm cancellation" in operator_work(
            "cancel", order["id"], "--reason", "different fixture reason", success=False).stderr
        assert operator_work("status", order["id"])["work"]["state"] == "cancelled"
        exercise_dependency_attestation(root, order, mapping, request, prepare, launcher, address, certificate, tokens)
        merged_github_work = exercise_github_review(root, order, request, launch, finish, prepare, attest,
                                                     launcher, agent, address, certificate, tokens, mapping)
        merged_bitbucket_work = exercise_bitbucket_closure(root, order, request, launch, finish, prepare, attest,
                                                            launcher, agent, address, certificate, tokens)
        exercise_team_skill(root, repo, mapping, home, order, request, launch, finish,
                            launcher, address, certificate, tokens)
        exercise_project_context(root, order, request, launch, finish, launcher, agent,
                                 address, certificate, tokens)
        exercise_standing_pickup(root, order, request, launch, finish, launcher, address, certificate, tokens)
        exercise_jira_standing(root, order, request, launcher, agent, address, certificate, tokens, scrape)
        exercise_github_standing(root, order, request, launcher, agent, address, certificate, tokens)
        exercise_release_series(root, repo, mapping, order, request, launch, finish, launcher, agent, address, certificate, tokens)

        order.update(id="launcher-takeover", intent="Change the greeting")
        request("operator", "operator", "admit", repository="team/service", event_key="launcher:2",
                work_order=order, token_limit=40)
        other = order | {"id": "launcher-drain-other"}
        request("operator", "operator", "admit", repository="team/service", event_key=other["id"],
                work_order=other, token_limit=40)
        sql("UPDATE craft_queue.control SET review_limit = 2;")
        request("operations", "first", "set_runner_mode", 403, runner_id="first",
                expected_revision=0, mode="draining", evidence="fixture:maintenance")
        request("operations", "operator", "runner_status", 404, runner_id="unknown-runner")
        initial = request("operations", "operator", "runner_status", runner_id="first")["runner"]
        assert initial["mode"] == "active" and initial["revision"] == 0
        def dispatch(work_id):
            return request("operations", "operator", "dispatch_status", work_id=work_id)["dispatch"]

        request("operations", "first", "dispatch_status", 403, work_id=order["id"])
        request("operations", "operator", "dispatch_status", 404, work_id="unknown-work")
        unassigned = order | {"id": "unassigned-dispatch-fixture"}
        request("operator", "operator", "admit", repository="team/dispatch-fixture", event_key=unassigned["id"],
                work_order=unassigned, token_limit=40)
        assert dispatch(unassigned["id"])["blockers"] == ["no_enrolled_runner"]
        brief = request("operations", "operator", "operator_brief", repository="team/dispatch-fixture",
                        usage_hours=24, limit=1)["operator_brief"]
        [blocked] = brief["backlog"]["items"]
        assert blocked["work_id"] == unassigned["id"] and blocked["blockers"] == ["no_enrolled_runner"]
        sql("INSERT INTO craft_queue.runner_repositories VALUES ('dispatch-fixture', 'team/dispatch-fixture');")
        enrolled = dispatch(unassigned["id"])
        assert enrolled["database_enrolled_runners"] == enrolled["active_enrolled_runners"] == 1
        assert enrolled["blockers"] == []
        request("operations", "operator", "set_runner_mode", runner_id="dispatch-fixture",
                expected_revision=0, mode="stopped", evidence="fixture:dispatch-diagnostics")
        assert dispatch(unassigned["id"])["blockers"] == ["no_active_enrolled_runner"]
        request("operator", "operator", "cancel", work_id=unassigned["id"], reason="dispatch fixture complete")
        request("operator", "operator", "set_paused", paused=True)
        command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                   "dispatch-status", order["id"]]
        snapshot = json.loads(subprocess.check_output(
            command, env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
            text=True, timeout=15))["dispatch"]
        assert snapshot["waiting_for_claim"] and snapshot["blockers"] == ["queue_paused"]
        assert "intent" not in snapshot and snapshot["required_adapter"] == "codex"
        request("operator", "operator", "set_paused", paused=False)
        token_budget = int(sql("SELECT token_budget FROM craft_queue.control;"))
        sql("UPDATE craft_queue.control SET token_budget = 1;")
        assert "token_budget" in dispatch(order["id"])["blockers"]
        sql(f"UPDATE craft_queue.control SET token_budget = {token_budget};")
        # Hold the first fixture process after launch to make takeover deterministic.
        (home / "fixture-wait").write_text("", encoding="utf-8")
        old_attempt = root / "old-attempt"
        original_request = str(uuid4())
        with launch("first", old_attempt, original_request) as old:
            try:
                observed = False
                until = monotonic() + 10
                while monotonic() < until and not observed:
                    state = old_attempt / "state.db"
                    if state.exists():
                        try:
                            with closing(sqlite3.connect(state)) as connection:
                                observed = connection.execute(
                                    "SELECT COUNT(*) FROM events WHERE kind = 'runner_started'"
                                ).fetchone()[0] == 1
                        except sqlite3.OperationalError:
                            pass
                    if not observed:
                        sleep(0.05)
                assert observed, "first launcher did not start the worker"
                duplicate_attempt = root / "duplicate-attempt"
                with launch("first", duplicate_attempt, original_request) as duplicate:
                    try:
                        _, duplicate_error = duplicate.communicate(timeout=6)
                        assert duplicate.returncode != 0 and "HTTP 409" in duplicate_error, duplicate_error
                        assert not (duplicate_attempt / "work").exists(), "duplicate reached the worker checkout"
                    finally:
                        if duplicate.poll() is None:
                            duplicate.terminate()
                live = dispatch(order["id"])
                assert live["live_lease"] and not live["waiting_for_claim"] and live["blockers"] == []
                sql("UPDATE craft_queue.control SET review_limit = 1;")
                assert "review_capacity_at_snapshot" in dispatch(other["id"])["blockers"]
                sql("UPDATE craft_queue.control SET review_limit = 2;")
                drain = request("operations", "operator", "set_runner_mode", runner_id="first",
                                expected_revision=0, mode="draining", evidence="fixture:maintenance")["decision"]
                assert drain["revision"] == 1 and drain["fenced_leases"] == 0
                assert request("runner", "first", "claim", request_id=original_request)["claim"]["generation"] == 1
                assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
                request("runner", "first", "renew", work_id=order["id"], generation=1)
                old_mode = request("operations", "operator", "runner_status", runner_id="first")["runner"]
                assert old_mode["mode"] == "draining" and old_mode["live_leases"] == 1
                cli_result = subprocess.run(
                    [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                     "set-runner-mode", "first", "--expected-revision", "1", "--mode", "stopped",
                     "--evidence", "fixture:immediate-maintenance"],
                    capture_output=True, text=True, timeout=15,
                    env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
                )
                assert cli_result.returncode == 0, cli_result.stderr
                stopped = json.loads(cli_result.stdout)["decision"]
                assert stopped["revision"] == 2 and stopped["fenced_leases"] == 1
                takeover = dispatch(order["id"])
                assert takeover["waiting_for_claim"] and not takeover["live_lease"]
                assert takeover["active_enrolled_runners"] == snapshot["active_enrolled_runners"] - 1
                _, stderr = old.communicate(timeout=8)
                assert old.returncode != 0 and "HTTP 409" in stderr, stderr
                assert not (old_attempt / "submission.json").exists()
                assert request("runner", "first", "claim", request_id=original_request)["claim"] is None
                assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
                request("runner", "first", "submit_candidate", 409, work_id=order["id"], generation=1,
                        commit=CANDIDATE, evidence_ref="fixture:late", tokens=5)
            finally:
                if old.poll() is None:
                    old.terminate()
        (home / "fixture-wait").unlink()
        replacement = finish(launch("second", root / "replacement", str(uuid4())))
        assert replacement["work_id"] == "launcher-takeover" and replacement["generation"] == 2
        assert sql("""SELECT count(*) FROM craft_queue.attempts
                    WHERE work_id = 'launcher-takeover' AND profile_started_at IS NOT NULL;""") == "2"
        print("Verified one supervised start per unprofiled generation, with a fresh start after takeover.")
        exercise_stale_publication(request, replacement)
        request("operator", "operator", "cancel", work_id=order["id"], reason="takeover verified")
        request("operations", "operator", "set_runner_mode", 409, runner_id="first",
                expected_revision=1, mode="active", evidence="fixture:stale")
        request("operations", "operator", "set_runner_mode", runner_id="first",
                expected_revision=2, mode="active", evidence="fixture:return-to-service")
        replay = request("operations", "operator", "set_runner_mode", runner_id="first",
                         expected_revision=1, mode="stopped", evidence="fixture:immediate-maintenance")["decision"]
        assert replay == stopped
        current = request("operations", "operator", "runner_status", runner_id="first")["runner"]
        assert current["mode"] == "active" and current["revision"] == 3
        available = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
        assert available["work_id"] == other["id"]
        request("operator", "operator", "cancel", work_id=other["id"], reason="runner lifecycle verified")
        sql("UPDATE craft_queue.control SET review_limit = 1;")

        order.update(id="launcher-outside")
        request("operator", "operator", "admit", repository="team/service", event_key="launcher:3",
                work_order=order, token_limit=40)
        outside = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
        (repo / "outside.txt").write_text("out of scope", encoding="utf-8")
        git("add", "outside.txt")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "Out of scope")
        disallowed = git("rev-parse", "HEAD").decode().strip()
        pack_bytes = git("pack-objects", "--revs", "--stdout", input=f"{disallowed}\n^{base}\n".encode())
        request("runner", "first", "submit_pack", work_id=outside["work_id"],
                generation=outside["generation"], commit=disallowed,
                pack_base64=base64.b64encode(pack_bytes).decode(), tokens=5)
        blocked = prepare(order["id"], disallowed, root / "verification-outside")
        assert blocked.returncode != 0 and "out-of-scope path" in blocked.stderr
        assert not (root / "verification-outside" / "handoff.json").exists()
        assert not (root / "verification-outside" / "review-brief.md").exists()
        request("operator", "operator", "cancel", work_id=order["id"], reason="scope verified")
        exercise_execution_profiles(root, order, request, launch, finish)
        exercise_queue_usage(address, certificate, tokens, request, launcher)
        exercise_worker_stop(root, order, request, launch)
        exercise_worker_token_overrun(root, order, request, launch, launcher, address, certificate, tokens, scrape)
        exercise_profile_comparison(root, order, request, launch, finish, launcher, address, certificate, tokens)
        exercise_profile_fallbacks(root, order, request, launch, finish, launcher, address, certificate, tokens)
        exercise_runner_loop(root, order, mapping, home, agent, request, launcher, address, certificate, tokens)
        exercise_claude_adapter(root, order, mapping, agent, request, launcher, address, certificate, tokens)
        exercise_knowledge_proposals(root, order, request, launcher, address, certificate, tokens)
        exercise_worker_knowledge(root, order, request, launch, finish)
        exercise_shared_github_failure(root, repo, order, mapping, request, launch, finish, prepare, attest,
                                       launcher, agent, address, certificate, tokens)
        exercise_shared_bitbucket_failure(root, repo, order, mapping, request, launch, finish, prepare, attest,
                                          launcher, agent, address, certificate, tokens)
        exercise_shared_ci_lane(root, repo, order, mapping, request, launch, finish, prepare, attest,
                                launcher, agent, address, certificate, tokens)
        exercise_cutover(address, certificate, tokens, request, launcher, root, order)
        recovery_epoch = str(uuid4())
        request("operations", "operator", "begin_recovery", epoch=recovery_epoch)
        merged = request("operator", "operator", "status", work_id=merged_github_work)["work"]
        assert merged["state"] == "merged" and merged["github_work_closure"]["result_commit"] == "c" * 40
        bitbucket = request("operator", "operator", "status", work_id=merged_bitbucket_work)["work"]
        assert bitbucket["state"] == "merged" and bitbucket["bitbucket_merge"]["closed_by"] == "operator"
        recovered_series = request("series", "operator", "status", id="series-fixture")["series"]
        assert recovered_series["released_members"] == 0
        assert all(member["latest_release"]["withdrawn_at"] for member in recovered_series["members"])
        assert all(member["latest_release"]["withdrawn_by"] == "operator" for member in recovered_series["members"])
        request("operations", "operator", "finish_recovery", epoch=recovery_epoch,
                evidence="fixture:series releases require fresh observations")


def exercise_dependency_attestation(root, template, mapping, request, prepare, launcher, address, certificate, tokens):
    repo = root / "dependency-repo"
    repo.mkdir()
    work_id = "dependency-attestation-fixture"
    repository = "team/service"

    def git(*args, **kwargs):
        return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, **kwargs).stdout

    def digest(raw):
        return "sha256:" + sha256(raw).hexdigest()

    def save(path, value):
        path.write_bytes(json.dumps(value).encode())
        return digest(path.read_bytes())

    git("init", "--quiet")
    (repo / "skill").mkdir()
    (repo / "skill" / "SKILL.md").write_text("Pinned skill for fixture", encoding="utf-8")
    baseline_osv, candidate_osv = (root / name for name in ("dependency-before-osv.json", "dependency-after-osv.json"))

    def revision(version, scan):
        manifest = {"name": "fixture-npm", "version": "1.0.0", "dependencies": {"sample-lib": version}}
        save(repo / "package.json", manifest)
        lock_digest = save(repo / "package-lock.json", {
            "name": "fixture-npm", "version": "1.0.0", "lockfileVersion": 3,
            "packages": {"": manifest, "node_modules/sample-lib": {"version": version,
                "resolved": "https://registry.npmjs.org/sample-lib/example.tgz",
                "integrity": "sha512-" + base64.b64encode(bytes(64)).decode()}},
        })
        git("add", ".")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", version)
        head = git("rev-parse", "HEAD").decode().strip()
        report_digest = save(scan, {"results": [{"source": {"path": "package-lock.json", "type": "lockfile"},
            "packages": [{"package": {"name": "sample-lib", "version": version, "ecosystem": "npm"}}]}]})
        return head, {"issuer": "fixture-scanner", "reference": f"fixture:osv/{head}", "commit": head,
                       "lockfile_digest": lock_digest, "report_digest": report_digest, "completed": True}

    base, baseline_scan = revision("1.0.1", baseline_osv)
    head, candidate_scan = revision("1.0.2", candidate_osv)
    intake = root / "dependency-intake.json"
    intake_record = {"schema_version": 1, "repository": repository, "updater_actor": "fixture-renovate",
        "scanner_issuer": "fixture-scanner", "proposal": {"repository": repository, "pull_request": "fixture:dependency-pr",
            "actor": "fixture-renovate", "base": base, "head": head, "package": "sample-lib",
            "from_version": "1.0.1", "to_version": "1.0.2"},
        "baseline_scan": baseline_scan, "candidate_scan": candidate_scan,
        "work_order": template | {"id": work_id, "base": base, "repository": str(repo),
            "scope": ["package.json", "package-lock.json"], "skill": template["skill"] | {"revision": base}}}
    update_args = [str(intake), "--baseline-osv", str(baseline_osv), "--candidate-osv", str(candidate_osv)]
    binary = str(launcher.resolve())
    scanner_key = root / "dependency-scanner-key.der"
    subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-outform", "DER", "-out", str(scanner_key)],
                   check=True, timeout=15, capture_output=True)
    scanner_public_key = subprocess.check_output(["openssl", "pkey", "-in", str(scanner_key), "-inform", "DER",
                                                 "-pubout", "-outform", "DER"], timeout=15)[-32:]
    scanner_key_base64 = base64.b64encode(scanner_public_key).decode()

    scan_time = int(time()) - 5
    def seal_scan(scan, receipt, repo_id=repository, completed_at=scan_time):
        lockfile = root / "scanner-lock.json"
        lockfile.write_bytes(git("show", f"{receipt['commit']}:package-lock.json"))
        command = [binary, "seal-dependency-scan", str(scan), "--lockfile", str(lockfile), "--repository", repo_id,
                "--commit", receipt["commit"], "--issuer", receipt["issuer"], "--reference", receipt["reference"],
                "--public-key-base64", scanner_key_base64, "--signing-key", str(scanner_key)]
        if completed_at is not None:
            command.extend(["--completed-at", str(completed_at)])
        return command

    base_scan_envelope = json.loads(subprocess.check_output(seal_scan(baseline_osv, baseline_scan), timeout=15))
    head_scan_envelope = json.loads(subprocess.check_output(seal_scan(candidate_osv, candidate_scan), timeout=15))
    other_repo_envelope = json.loads(subprocess.check_output(
        seal_scan(candidate_osv, candidate_scan, "fixture/other"), timeout=15))
    incomplete_report = root / "incomplete-osv.json"
    save(incomplete_report, {"results": [{"source": {"path": "package-lock.json", "type": "lockfile"}, "packages": []}]})
    rejected = subprocess.run(seal_scan(incomplete_report, candidate_scan), text=True, capture_output=True, timeout=15)
    assert rejected.returncode != 0 and "coverage differs" in rejected.stderr and not rejected.stdout

    signed_intake = intake_record | {"scanner_public_key_base64": scanner_key_base64,
                                     "scanner_max_age_seconds": 3600,
                                     "baseline_scan": base_scan_envelope, "candidate_scan": head_scan_envelope}
    prepare_command = [binary, "prepare-dependency-update", *update_args]
    payload = json.loads(base64.b64decode(head_scan_envelope["payload"]))
    payload["receipt"]["reference"] = "fixture:altered"
    altered = head_scan_envelope | {"payload": base64.b64encode(json.dumps(payload).encode()).decode()}
    for replacement_scan, error in ((candidate_scan, "signing policy"), (altered, "no DSSE signature matches"),
                                    (other_repo_envelope, "different repository"), (base_scan_envelope, "scan receipt")):
        save(intake, signed_intake | {"candidate_scan": replacement_scan})
        rejected = subprocess.run(prepare_command, text=True, capture_output=True, timeout=15)
        assert rejected.returncode != 0 and error in rejected.stderr and not rejected.stdout, rejected.stderr
    stale_baseline = json.loads(subprocess.check_output(
        seal_scan(baseline_osv, baseline_scan, completed_at=scan_time - 3601), timeout=15))
    untimed = json.loads(subprocess.check_output(seal_scan(baseline_osv, baseline_scan, completed_at=None), timeout=15))
    for invalid_intake, error in ((signed_intake | {"baseline_scan": stale_baseline}, "stale"),
                                  (signed_intake | {"baseline_scan": untimed}, "signed completion times"),
                                  (intake_record | {"scanner_max_age_seconds": 3600}, "requires a signing key")):
        save(intake, invalid_intake)
        rejected = subprocess.run(prepare_command, text=True, capture_output=True, timeout=15)
        assert rejected.returncode != 0 and error in rejected.stderr and not rejected.stdout, rejected.stderr
    rejected = subprocess.run(seal_scan(candidate_osv, candidate_scan, completed_at=scan_time + 86400),
                              text=True, capture_output=True, timeout=15)
    assert rejected.returncode != 0 and "invalid scanner completion time" in rejected.stderr
    intake_digest = save(intake, signed_intake)
    original_report = candidate_osv.read_bytes()
    candidate_osv.write_bytes(original_report + b" ")
    rejected = subprocess.run(prepare_command, text=True, capture_output=True, timeout=15)
    assert rejected.returncode != 0 and "scan receipt" in rejected.stderr and not rejected.stdout
    candidate_osv.write_bytes(original_report)
    prepared = json.loads(subprocess.check_output(prepare_command, text=True, timeout=15))
    assert prepared["scanner_signing_key_digest"] == digest(scanner_public_key)
    assert prepared["scanner_fresh_through"] == scan_time + 3600
    order = prepared["work_order"]
    assert order["dependency_intake_digest"] == intake_digest
    target = {"service": "fixture-npm", "environment": "staging", "region": "fixture", "resource": "fixture-npm"}
    member = {"repository": repository, "observer": "release-one", "target": target,
              "token_limit": 40, "work_order": order, "requires_rollout_proof": True}
    plan = {"id": work_id, "owner": order["owner"], "approval": "fixture:dependency-release"}
    request("series", "operator", "register", 400, **plan, members=[member | {"requires_rollout_proof": False}])
    ordinary = {key: value for key, value in order.items() if key != "dependency_intake_digest"}
    request("series", "operator", "register", 400, **plan, members=[member | {"work_order": ordinary}])
    series = [binary, "shared-series", "--queue", address, "--ca", str(certificate)]
    plan_file = root / "dependency-release-plan.json"
    save(plan_file, plan | {"members": [member]})
    operator = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    registered = json.loads(subprocess.check_output(series + ["register", str(plan_file)],
                                                    env=operator, text=True, timeout=15))["series"]
    assert registered == request("series", "operator", "register", **plan, members=[member])["series"]
    assert registered["total_members"] == 1 and registered["members"][0]["requires_rollout_proof"]
    claim = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
    assert claim["work_id"] == work_id
    pack = git("pack-objects", "--revs", "--stdout", input=f"{head}\n^{base}\n".encode())
    request("runner", "first", "submit_pack", work_id=work_id, generation=claim["generation"], commit=head,
            pack_base64=base64.b64encode(pack).decode(), tokens=0)
    request("verifier", "verifier", "record_verification", 409, work_id=work_id, generation=claim["generation"],
            head=head, digest=digest(pack), previous_sequence=0, passed=True, evidence="fixture:manual", note="Generic pass")
    previous_mapping = mapping.read_bytes()
    save(mapping, {repository: {"path": str(repo)}})
    try:
        verification = root / "dependency-verification"
        prepared = prepare(work_id, head, verification, "verifier", verifier=True)
        assert prepared.returncode == 0, prepared.stderr
    finally:
        mapping.write_bytes(previous_mapping)

    policy, before, after, report = (root / f"dependency-{name}.json" for name in ("policy", "before", "after", "report"))
    harness = "sha256:" + "3" * 64
    key = root / "dependency-signing-key.der"
    subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-outform", "DER", "-out", str(key)],
                   check=True, timeout=15, capture_output=True)
    public_key = subprocess.check_output(["openssl", "pkey", "-in", str(key), "-inform", "DER", "-pubout",
                                         "-outform", "DER"], timeout=15)[-32:]
    policy_pin = save(policy, {"schema_version": 1, "intake_digest": intake_digest, "candidate_head": head,
        "verifier_issuer": "fixture-ci", "harness_digest": harness, "approval_reference": "fixture:approved-policy",
        "run_public_key_base64": base64.b64encode(public_key).decode(),
        "checks": [{"id": "smoke", "expectation": "preserve"}]})
    receipt = {"schema_version": 1, "repository": repository, "issuer": "fixture-ci", "harness_digest": harness,
               "completed": True, "cases": [{"id": "smoke", "outcome": "passed"}]}
    def seal(path, raw):
        source = path.with_suffix(".raw.json")
        save(source, raw)
        command = [binary, "seal-dependency-compatibility-run", str(source), "--policy", str(policy),
                   "--policy-digest", policy_pin, "--repository", repository, "--commit", raw["commit"],
                   "--signing-key", str(key), "--destination", str(path)]
        result = json.loads(subprocess.check_output(command, text=True, timeout=15))
        assert result["receipt_digest"] == digest(path.read_bytes())
        return result["receipt_digest"]

    before_pin = seal(before, receipt | {"commit": base, "reference": "fixture:baseline",
                                        "lockfile_digest": baseline_scan["lockfile_digest"]})
    candidate_receipt = receipt | {"commit": head, "reference": "fixture:candidate",
                                   "lockfile_digest": candidate_scan["lockfile_digest"]}
    after_pin = seal(after, candidate_receipt)
    approval_file = root / "dependency-evidence-approval.json"
    approval = {"id": "dependency-first-approval", "work_id": work_id, "generation": claim["generation"],
                "head": head, "evidence": "fixture:operator-reviewed-ci",
                "pins": {"intake_digest": intake_digest, "policy_digest": policy_pin,
                         "baseline_run_digest": before_pin, "candidate_run_digest": after_pin,
                         "scanner_fresh_through": scan_time + 3600}}
    approve = [binary, "shared-ops", "--queue", address, "--ca", str(certificate),
               "approve-dependency-evidence", str(approval_file)]
    command = [binary, "attest-shared-dependency-compatibility", *update_args, "--queue", address,
        "--ca", str(certificate), "--handoff", str(verification / "handoff.json"), "--report", str(report),
        "--approval-id", approval["id"],
        "--policy", str(policy), "--baseline-run", str(before), "--candidate-run", str(after),
        "--policy-digest", policy_pin, "--baseline-run-digest", before_pin, "--candidate-run-digest", after_pin]
    env = os.environ | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["verifier"]}
    raw = after.read_bytes()
    after.write_bytes(raw + b" ")
    changed = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
    assert changed.returncode != 0 and "approved receipt digest" in changed.stderr and not report.exists()
    envelope = json.loads(raw)
    wrong_payload = candidate_receipt | {"cases": [{"id": "smoke", "outcome": "error"}]}
    envelope["payload"] = base64.b64encode(json.dumps(wrong_payload).encode()).decode()
    changed_pin = save(after, envelope)
    changed_command = command.copy()
    changed_command[changed_command.index("--candidate-run-digest") + 1] = changed_pin
    untrusted = subprocess.run(changed_command, env=env, capture_output=True, text=True, timeout=15)
    assert untrusted.returncode != 0 and "no DSSE signature matches" in untrusted.stderr and not report.exists()
    unsigned_pin = save(after, candidate_receipt)
    changed_command[changed_command.index("--candidate-run-digest") + 1] = unsigned_pin
    unsigned = subprocess.run(changed_command, env=env, capture_output=True, text=True, timeout=15)
    assert unsigned.returncode != 0 and not report.exists()
    after.write_bytes(raw)
    missing = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
    assert missing.returncode != 0 and "409" in missing.stderr
    request("operations", "verifier", "approve_dependency_evidence", 403, approval=approval)
    save(approval_file, approval)
    approved = json.loads(subprocess.check_output(approve, env=operator, text=True, timeout=15))["approval"]
    assert approved["pins"] == approval["pins"]
    assert request("operations", "operator", "approve_dependency_evidence", approval=approval)["approval"] == approved
    request("operations", "operator", "approve_dependency_evidence", 409,
            approval=approval | {"id": "dependency-stale-replacement"})
    request("verifier", "verifier", "record_dependency_verification", 409,
            work_id=work_id, generation=claim["generation"], head=head, digest=digest(pack),
            previous_sequence=0, passed=True, approval_id=approval["id"],
            proof=approval["pins"] | {"policy_digest": "sha256:" + "9" * 64, "report_digest": digest(report.read_bytes())})
    untimed_pins = {name: value for name, value in approval["pins"].items() if name != "scanner_fresh_through"}
    request("verifier", "verifier", "record_dependency_verification", 409,
            work_id=work_id, generation=claim["generation"], head=head, digest=digest(pack),
            previous_sequence=0, passed=True, approval_id=approval["id"],
            proof=untimed_pins | {"report_digest": digest(report.read_bytes())})
    handoff_file = verification / "handoff.json"
    handoff = json.loads(handoff_file.read_bytes())
    pickup = request("verifier", "service-verifier", "claim_pending_verification",
                     repository=repository)["verification_pickup"]
    assert pickup["work_id"] == work_id and pickup["head"] == handoff["head"]
    sql(f"UPDATE craft_queue.verification_pickups SET expires_at = clock_timestamp() - interval '1 second' "
        f"WHERE id = '{pickup['pickup_id']}'::uuid;")
    save(handoff_file, handoff | {"pickup_id": pickup["pickup_id"]})
    expired = subprocess.run(command, env=env | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["service-verifier"]},
                             capture_output=True, text=True, timeout=15)
    assert expired.returncode != 0 and "HTTP 409" in expired.stderr, expired.stderr
    replacement_pickup = request("verifier", "peer-verifier", "claim_pending_verification",
                                 repository=repository)["verification_pickup"]
    assert replacement_pickup["work_id"] == work_id
    save(handoff_file, handoff | {"pickup_id": replacement_pickup["pickup_id"]})
    verifier_token = root / "dependency-verifier-token"
    with os.fdopen(os.open(verifier_token, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as file:
        file.write(tokens["peer-verifier"] + "\n")
    command.extend(("--verifier-token-file", str(verifier_token)))
    env.pop("CRAFT_FACTORY_VERIFIER_TOKEN")
    missing_file = command[:-1] + [str(root / "missing-dependency-verifier-token")]
    rejected = subprocess.run(missing_file, env=env | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["peer-verifier"]},
                              capture_output=True, text=True, timeout=15)
    assert rejected.returncode != 0 and "could not read a bounded credential file" in rejected.stderr
    assert tokens["peer-verifier"] not in rejected.stdout + rejected.stderr
    manifest = Path(handoff["checkout"]) / "package.json"
    manifest_bytes = manifest.read_bytes()
    before_report = report.read_bytes()
    try:
        manifest.write_bytes(manifest_bytes + b" ")
        changed_checkout = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
        assert changed_checkout.returncode != 0 and "verification checkout has tracked changes" in changed_checkout.stderr
        assert report.read_bytes() == before_report
        assert request("verifier", "peer-verifier", "status", work_id=work_id)["work"]["verification"] is None
    finally:
        manifest.write_bytes(manifest_bytes)
    result = json.loads(subprocess.check_output(command, env=env, text=True, timeout=15))
    assert result["compatibility_verified"] and result["report_digest"] == digest(report.read_bytes())
    assert json.loads(report.read_bytes())["run_signing_key_digest"] == digest(public_key)
    assert json.loads(report.read_bytes())["scanner_signing_key_digest"] == digest(scanner_public_key)
    assert json.loads(report.read_bytes())["scanner_fresh_through"] == scan_time + 3600
    assert json.loads(subprocess.check_output(command, env=env, text=True, timeout=15)) == result
    binding = {"work_id": work_id, "generation": claim["generation"], "head": head,
               "verification_sequence": result["verification_sequence"]}
    request("series", "operator", "approve_rollout", 409, **binding,
            policy_digest=policy_pin, approval="fixture:missing-archive")
    request("release-observer", "release-one", "observe", 409, **binding, target=target,
            artifact_digest=digest(b"fixture artifact"), evidence="fixture:self-reported")
    awaiting = request("series", "operator", "status", id=work_id)["series"]
    assert awaiting["state"] == "open" and awaiting["members"][0]["latest_release"] is None
    audit = request("operations", "operator", "audit", work_id=work_id)["audit"]
    [proof] = audit["dependency_verifications"]
    assert proof["approval_id"] == approval["id"] and audit["dependency_approvals"] == [approved]
    assert proof["head"] == head and proof["passed"] and proof["verification_sequence"] == result["verification_sequence"]
    assert proof["proof"] == {"intake_digest": intake_digest, "policy_digest": policy_pin,
                              "baseline_run_digest": before_pin, "candidate_run_digest": after_pin,
                              "scanner_fresh_through": scan_time + 3600,
                              "report_digest": result["report_digest"]}
    failed_envelope = root / "dependency-failed-envelope.json"
    failed_pin = seal(failed_envelope, wrong_payload)
    replacement = approval | {"id": "dependency-replacement-approval", "previous_approval_id": approval["id"],
                              "pins": approval["pins"] | {"candidate_run_digest": failed_pin}}
    save(approval_file, replacement)
    rotated = json.loads(subprocess.check_output(approve, env=operator, text=True, timeout=15))["approval"]
    assert rotated["previous_approval_id"] == approval["id"]
    current = request("verifier", "verifier", "status", work_id=work_id)["work"]
    assert not current["independent_checks_passed"]
    stale = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
    assert stale.returncode != 0 and "409" in stale.stderr
    fresh = root / "dependency-recheck"
    save(mapping, {repository: {"path": str(repo)}})
    try:
        prepared = prepare(work_id, head, fresh, "verifier", verifier=True)
        assert prepared.returncode == 0, prepared.stderr
    finally:
        mapping.write_bytes(previous_mapping)
    failed_command = command.copy()
    for flag, value in (("--handoff", fresh / "handoff.json"), ("--report", root / "dependency-failure.json"),
                        ("--candidate-run", failed_envelope),
                        ("--candidate-run-digest", failed_pin), ("--approval-id", replacement["id"])):
        failed_command[failed_command.index(flag) + 1] = str(value)
    failed = subprocess.run(failed_command, env=env, capture_output=True, text=True, timeout=15)
    assert failed.returncode != 0, failed.stderr
    failed_result = json.loads(failed.stdout)
    assert not failed_result["compatibility_verified"]
    current = request("verifier", "verifier", "status", work_id=work_id)["work"]
    assert current["verification"]["sequence"] == failed_result["verification_sequence"]
    assert not current["independent_checks_passed"]

    refresh_file = root / "dependency-scanner-refresh.json"
    refresh = {"schema_version": 1, "baseline_scan": base_scan_envelope, "candidate_scan": head_scan_envelope}
    save(refresh_file, refresh)
    refreshed_prepare = [*prepare_command, "--refresh-scans", str(refresh_file)]
    repeated = subprocess.run(refreshed_prepare, text=True, capture_output=True, timeout=15)
    assert repeated.returncode != 0 and "newer than the original" in repeated.stderr
    for flag, scan_path, original_scan in (("baseline_scan", baseline_osv, baseline_scan),
                                           ("candidate_scan", candidate_osv, candidate_scan)):
        refresh[flag] = json.loads(subprocess.check_output(seal_scan(scan_path,
            original_scan | {"reference": original_scan["reference"] + "/refresh"}, completed_at=scan_time + 1), timeout=15))
    refresh_digest = save(refresh_file, refresh)
    expired_intake = root / "expired-scanner-intake.json"
    expired_digest = save(expired_intake, signed_intake | {"baseline_scan": stale_baseline})
    expired_command = refreshed_prepare.copy()
    expired_command[2] = str(expired_intake)
    recovered = json.loads(subprocess.check_output(expired_command, text=True, timeout=15))
    assert recovered["scanner_refresh_digest"] == refresh_digest and recovered["input_digest"] == expired_digest

    new_pins = approval["pins"] | {"scanner_refresh_digest": refresh_digest, "scanner_fresh_through": scan_time + 3601}
    refreshed_approval = approval | {"id": "dependency-refreshed-scans", "previous_approval_id": replacement["id"],
                                     "pins": new_pins}
    request("operations", "operator", "approve_dependency_evidence", 409,
            approval=refreshed_approval | {"pins": new_pins | {"scanner_fresh_through": scan_time + 3600}})
    save(approval_file, refreshed_approval)
    approved_refresh = json.loads(subprocess.check_output(approve, env=operator, text=True, timeout=15))["approval"]
    assert approved_refresh["pins"] == new_pins
    request("operations", "operator", "approve_dependency_evidence", 409,
            approval=approval | {"id": "dependency-discard-refresh", "previous_approval_id": refreshed_approval["id"]})
    refreshed_handoff = root / "dependency-fresh-scans-handoff"
    save(mapping, {repository: {"path": str(repo)}})
    try:
        checked = prepare(work_id, head, refreshed_handoff, "verifier", verifier=True)
        assert checked.returncode == 0, checked.stderr
    finally:
        mapping.write_bytes(previous_mapping)
    refreshed_command = command.copy()
    refresh_report = root / "dependency-fresh-scan-report.json"
    for flag, value in (("--handoff", refreshed_handoff / "handoff.json"), ("--report", refresh_report),
                        ("--approval-id", refreshed_approval["id"])):
        refreshed_command[refreshed_command.index(flag) + 1] = str(value)
    original_only = subprocess.run(refreshed_command, env=env, capture_output=True, text=True, timeout=15)
    assert original_only.returncode != 0 and "409" in original_only.stderr
    refresh_report.unlink()
    refreshed = json.loads(subprocess.check_output([*refreshed_command, "--refresh-scans", str(refresh_file)],
                                                     env=env, text=True, timeout=15))
    current = request("verifier", "verifier", "status", work_id=work_id)["work"]
    assert refreshed["compatibility_verified"] and current["independent_checks_passed"]
    [scanner_refresh, *_] = request("operations", "operator", "audit", work_id=work_id)["audit"]["dependency_verifications"]
    assert scanner_refresh["verification_sequence"] == refreshed["verification_sequence"]
    assert scanner_refresh["proof"] == new_pins | {"report_digest": refreshed["report_digest"]}
    assert scanner_refresh["head"] == head and scanner_refresh["generation"] == claim["generation"]
    request("operator", "operator", "cancel", work_id=work_id, reason="dependency verification fixture complete")
    print("Verified signed OSV evidence and CI receipts, candidate-bound scan refresh, and atomic supersession.")


def exercise_shared_github_failure(root, repo, template, mapping, request, launch, finish, prepare, attest,
                                   launcher, agent, address, certificate, tokens):
    order = template | {"id": "fixture-shared-github-ci", "repository": str(repo)}
    order_file = root / "shared-ci-order.json"
    policy = root / "shared-ci-policy.json"
    order_file.write_text(json.dumps(order), encoding="utf-8")
    policy.write_text(json.dumps({
        "schema_version": 1, "repository": "team/service", "base_branch": "main",
        "check_definitions": [], "required_checks": [{"name": "Checks", "app_id": 42}],
        "ci_repair": {"workflow_id": 9, "job_name": "Checks"},
    }), encoding="utf-8")
    policy_bytes = policy.read_bytes()
    digest = lambda path: "sha256:" + sha256(path.read_bytes()).hexdigest()
    command = [str(launcher.resolve()), "shared-work", "--queue", address, "--ca", str(certificate),
               "admit-github-failure", str(order_file), "--order-digest", digest(order_file),
               "--policy", str(policy), "--policy-digest", digest(policy), "--gh", str(agent),
               "--run", "73", "--attempt", "2", "--token-limit", "40"]
    forge = {"FACTORY_FIXTURE_SHA": order["base"], "FACTORY_FIXTURE_BASE": order["base"],
             "FACTORY_FIXTURE_REQUIRE_CI_ISOLATION": "1"}
    env = os.environ | forge | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    original_mapping = mapping.read_bytes()
    remote = ["git", "-C", str(repo), "remote"]
    origin = subprocess.check_output(remote + ["get-url", "origin"], text=True, timeout=10).strip()
    subprocess.run(remote + ["set-url", "origin", "https://github.com/team/service.git"], check=True, timeout=10)
    try:
        stale = subprocess.run(command, env=env | {"FACTORY_FIXTURE_NEWER": "1"}, text=True,
                               capture_output=True, timeout=15)
        assert stale.returncode != 0 and "superseded" in stale.stderr
        admitted = json.loads(subprocess.check_output(command, env=env, text=True, timeout=15))
        work_id = admitted["work_order_id"]
        assert work_id.startswith(order["id"] + "-ci-")
        assert json.loads(subprocess.check_output(command, env=env, text=True, timeout=15)) == admitted
        queued = request("operator", "operator", "status", work_id=work_id)["work"]
        assert queued["state"] == "queued"
        mapping.write_text(json.dumps({"team/service": {"path": str(repo), "github_ci": {
            "policy": str(policy), "gh": str(agent),
        }}}), encoding="utf-8")
        request_id = str(uuid4())
        blocked = root / "shared-ci-before-worker"
        reserved = request("operations", "operator", "summary")["summary"]["review_reserved"]
        for flags, error in (({}, "approved digest"), ({"FACTORY_FIXTURE_NEWER": "1"}, "superseded")):
            policy.write_bytes(policy_bytes if flags else policy_bytes + b"\n")
            with launch("first", blocked, request_id, fixture_env=forge | flags) as process:
                _, stderr = process.communicate(timeout=20)
                assert process.returncode != 0 and error in stderr, stderr
            assert not blocked.exists()
            if not flags:
                assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "leased"
        policy.write_bytes(policy_bytes)
        held = request("operator", "operator", "status", work_id=work_id)["work"]
        hold = held["ci_preflight_hold"]
        assert held["state"] == "blocked" and hold["reason"] == "workflow_superseded" and hold["generation"] == 1
        assert request("operations", "operator", "summary")["summary"]["review_reserved"] == reserved
        dispatch = request("operations", "operator", "dispatch_status", work_id=work_id)["dispatch"]
        assert dispatch["ci_preflight_hold"] == hold and dispatch["blockers"] == ["ci_preflight_hold"]
        assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
        request("runner", "first", "start_attempt", 409, work_id=work_id, generation=1)
        request("runner", "second", "hold_obsolete_ci", 409, work_id=work_id, generation=1, reason=hold["reason"])
        assert request("runner", "first", "hold_obsolete_ci", work_id=work_id,
                       generation=1, reason=hold["reason"])["ci_preflight_hold"] == hold
        retry = {"work_id": work_id, "generation": 1, "evidence": "fixture:forge evidence reconciled"}
        request("operations", "first", "retry_ci_preflight", 403, **retry)
        request("operations", "operator", "retry_ci_preflight", 409, **(retry | {"generation": 2}))
        released = json.loads(subprocess.check_output([
            str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
            "retry-ci-preflight", work_id, "--generation", "1", "--evidence", retry["evidence"],
        ], env=env, text=True, timeout=15))["ci_preflight_hold"]
        assert released["released_by"] == "operator" and released["released_at"]
        assert request("operations", "operator", "retry_ci_preflight", **retry)["ci_preflight_hold"] == released
        result = finish(launch("first", root / "shared-ci-verified-source", str(uuid4()), fixture_env=forge))
        generation = result["generation"]
        assert result["work_id"] == work_id and generation == 2
        assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "candidate"
        head = result["candidate"]

        def handoff(name):
            destination = root / name
            response = prepare(work_id, head, destination, "verifier", verifier=True)
            assert response.returncode == 0, response.stderr
            return destination / "handoff.json"

        def pass_verification(packet, evidence):
            passed = attest(packet, "passed", evidence)
            assert passed.returncode == 0, passed.stderr
            return json.loads(passed.stdout)["verification_sequence"]

        sequence = pass_verification(handoff("shared-ci-local-verification"), "fixture:local-ci-repair-checks")
        ready = handoff("shared-ci-forge-verification")
        native = env | {"FACTORY_FIXTURE_SHA": head, "FACTORY_FIXTURE_PULL_NUMBER": "155",
                        "FACTORY_FIXTURE_SOURCE_BRANCH": f"craft-factory/{work_id}",
                        "CRAFT_FACTORY_VERIFIER_TOKEN": tokens["service-verifier"]}
        cli = [str(launcher.resolve())]
        queue_args = ["--queue", address, "--ca", str(certificate)]
        bind = cli + ["bind-shared-github-review"] + queue_args + [
            "--handoff", str(ready), "--policy", str(policy), "--gh", str(agent), "--pull-request", "155",
        ]
        check = cli + ["shared-ops"] + queue_args + [
            "attest-github-ci-checks", "--handoff", str(ready), "--policy", str(policy), "--gh", str(agent),
        ]
        assignment = {"handoff_id": "shared-ci-review", "reviewer_id": "reviewer", "work_id": work_id,
                      "generation": generation, "head": head, "verification_sequence": sequence,
                      "evidence": "fixture:review-ci-repair"}
        sql("UPDATE craft_queue.draft_policies SET forge = 'github' WHERE repository = 'team/service';")
        try:
            rejected = subprocess.run(check, env=native, capture_output=True, text=True, timeout=15)
            assert rejected.returncode != 0 and "HTTP 409" in rejected.stderr
            published = json.loads(subprocess.check_output(bind, env=native, text=True, timeout=15))["publication"]
            grant = "shared-ci-reviewer"
            request("operations", "operator", "grant_review_inbox", grant_id=grant, reviewer_id="reviewer",
                    repository="team/service", valid_seconds=120, evidence="fixture:review-ci-repair")
            request("operations", "operator", "assign_review", 409, **assignment)
            bind_check = {"work_id": work_id, "generation": generation, "head": head, "pack_digest": published["pack_digest"],
                          "verification_sequence": sequence}
            request("verifier", "other-verifier", "record_github_ci_checks", 404, **bind_check)
            for change in ("FACTORY_FIXTURE_FAILED_CHECK", "FACTORY_FIXTURE_NEWER"):
                rejected = subprocess.run(check, env=native | {change: "1"}, capture_output=True, text=True, timeout=15)
                assert rejected.returncode != 0, change
                request("operations", "operator", "assign_review", 409, **assignment)
            receipt = json.loads(subprocess.check_output(check, env=native, text=True, timeout=15))["github_ci_checks"]
            assert receipt["verifier"] == "service-verifier" and receipt["verification_sequence"] == sequence
            assert receipt["proof"]["checks"] == [{"id": 7, "name": "Checks", "app_id": 42}]
            assert receipt["publication_key"] == published["action_key"]
            assert json.loads(subprocess.check_output(check, env=native, text=True, timeout=15))["github_ci_checks"] == receipt
            request("operations", "operator", "assign_review", **assignment)
            fresh = pass_verification(ready, "fixture:refreshed-local-ci-repair-checks")
            request("operations", "operator", "assign_review", 409,
                    **(assignment | {"handoff_id": "shared-ci-second-review", "verification_sequence": fresh}))
            audit = request("operations", "operator", "audit", work_id=work_id)["audit"]
            assert audit["github_ci_checks"] == [receipt]
            assert audit["ci_preflight_holds"] == [released] and audit["attempts"][-1]["reported_tokens"] == 0
            request("operations", "operator", "revoke_review_inbox", grant_id=grant, evidence="fixture:ci gate checked")
        finally:
            sql("UPDATE craft_queue.draft_policies SET forge = 'bitbucket' WHERE repository = 'team/service';")
        request("operator", "operator", "cancel", work_id=work_id, reason="fixture checked shared intake and dispatch")
    finally:
        mapping.write_bytes(original_mapping)
        subprocess.run(remote + ["set-url", "origin", origin], check=True, timeout=10)
    print("Verified shared GitHub CI intake, supersession hold, operator retry, and native review proof.")


def exercise_shared_bitbucket_failure(root, repo, template, mapping, request, launch, finish, prepare, attest,
                                     launcher, agent, address, certificate, tokens):
    order = template | {"id": "shared-bitbucket-ci", "repository": str(repo)}
    policy = root / "shared-bitbucket-ci-policy.json"
    policy.write_text(json.dumps({
        "schema_version": 1, "twg": str(agent), "profile": "fixture", "site": "fixture.atlassian.net",
        "workspace": "team", "repository": "service", "base_branch": "main",
        "required_steps": ["Verify candidate"], "check_definitions": ["ci"],
    }), encoding="utf-8")
    order_path = root / "shared-bitbucket-ci-order.json"
    order_path.write_text(json.dumps(order), encoding="utf-8")
    policy_digest = "sha256:" + sha256(policy.read_bytes()).hexdigest()
    cli = [str(launcher.resolve())]
    queue_args = ["--queue", address, "--ca", str(certificate)]
    admit = cli + ["shared-work"] + queue_args + [
        "admit-bitbucket-failure", str(order_path), "--order-digest", "sha256:" + sha256(order_path.read_bytes()).hexdigest(),
        "--policy", str(policy), "--policy-digest", policy_digest, "--build", "14",
        "--step", "babe0000-0000-0000-0000-000000000042", "--token-limit", "40",
    ]
    env = os.environ | {"FACTORY_FIXTURE_SHA": order["base"], "CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    stale = subprocess.run(admit, env=env | {"FACTORY_FIXTURE_NEWER": "1"}, capture_output=True, text=True, timeout=15)
    assert stale.returncode != 0 and "superseded" in stale.stderr, stale.stderr
    admitted = json.loads(subprocess.check_output(admit, env=env, text=True, timeout=15))
    work_id = admitted["work_order_id"]
    assert work_id.startswith(order["id"] + "-ci-")
    assert json.loads(subprocess.check_output(admit, env=env, text=True, timeout=15)) == admitted
    original_mapping = mapping.read_bytes()
    try:
        mapping.write_text(json.dumps({"team/service": {"path": str(repo), "bitbucket_ci": str(policy)}}), encoding="utf-8")
        request_id = str(uuid4())
        blocked = root / "shared-bitbucket-before-worker"
        with launch("first", blocked, request_id, fixture_env={
            "FACTORY_FIXTURE_SHA": order["base"], "FACTORY_FIXTURE_NEWER": "1",
            "FACTORY_FIXTURE_MISSING_PIPELINE_SELECTOR": "1",
        }) as process:
            _, stderr = process.communicate(timeout=20)
            assert process.returncode != 0 and "might have superseded" in stderr, stderr
        assert not blocked.exists()
        assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "leased"
        with launch("first", blocked, request_id, fixture_env={
            "FACTORY_FIXTURE_SHA": order["base"], "FACTORY_FIXTURE_NEWER": "1",
        }) as process:
            _, stderr = process.communicate(timeout=20)
            assert process.returncode != 0 and "is held" in stderr, stderr
        assert not blocked.exists()
        held = request("operator", "operator", "status", work_id=work_id)["work"]
        assert held["state"] == "blocked" and held["ci_preflight_hold"]["reason"] == "workflow_superseded"
        request("operations", "operator", "retry_ci_preflight", work_id=work_id, generation=1,
                evidence="fixture:Bitbucket evidence reconciled")
        result = finish(launch("first", root / "shared-bitbucket-ci-candidate", str(uuid4()),
                               fixture_env={"FACTORY_FIXTURE_SHA": order["base"]}))
        generation, head = result["generation"], result["candidate"]
        assert result["work_id"] == work_id and generation == 2

        def handoff(name):
            destination = root / name
            prepared = prepare(work_id, head, destination, "verifier", verifier=True)
            assert prepared.returncode == 0, prepared.stderr
            return destination / "handoff.json"

        checked = attest(handoff("shared-bitbucket-local-checks"), "passed", "fixture:local Bitbucket CI repair checks")
        assert checked.returncode == 0, checked.stderr
        sequence = json.loads(checked.stdout)["verification_sequence"]
        ready = handoff("shared-bitbucket-native-checks")
        identity = {"work_id": work_id, "head": head}
        publication = request("publication", "operator", "reserve", **identity,
                              generation=generation, verification_sequence=sequence)["publication"]
        key = publication["action_key"]
        url = "https://bitbucket.org/team/service/pull-requests/156"
        request("publisher", "publisher", "begin", action_key=key)
        request("publication", "operator", "reconcile", action_key=key, observation={
            "outcome": "confirmed", "head": head, "native_url": url, "source_branch": publication["source_branch"],
            "base_branch": publication["base_branch"], "evidence": "fixture:Bitbucket draft observed",
        })
        grant = "shared-bitbucket-ci-reviewer"
        request("operations", "operator", "grant_review_inbox", grant_id=grant, reviewer_id="reviewer",
                repository="team/service", valid_seconds=120, evidence="fixture:review Bitbucket CI repair")
        assignment = identity | {"handoff_id": "shared-bitbucket-ci-review", "reviewer_id": "reviewer",
                                 "generation": generation, "verification_sequence": sequence, "evidence": "fixture:review"}
        binding = identity | {"generation": generation, "pack_digest": publication["pack_digest"],
                              "verification_sequence": sequence}
        request("operator", "operator", "admit", 400, repository="team/service", event_key="ci:unpinned",
                token_limit=40, work_order=order | {
                    "ci_failure": {"repository": "team/service", "step_uuid": "babe0000-0000-0000-0000-000000000042"},
                })
        request("operations", "operator", "assign_review", 409, **assignment)
        request("verifier", "verifier", "confirm_bitbucket_work_merge", 409, **identity)
        request("verifier", "other-verifier", "record_bitbucket_ci_checks", 404, **binding)
        request("verifier", "service-verifier", "record_github_ci_checks", 409, **binding)
        request("verifier", "service-verifier", "record_bitbucket_ci_checks", 409,
                **(binding | {"verification_sequence": sequence + 1}))
        request("verifier", "service-verifier", "record_bitbucket_ci_checks", 409, **binding, proof={"checks": []})
        native = env | {"FACTORY_FIXTURE_SHA": head, "FACTORY_FIXTURE_BRANCH": order["base"],
                        "FACTORY_FIXTURE_PINNED_FAILURE": order["base"], "FACTORY_FIXTURE_PULL_NUMBER": "156",
                        "FACTORY_FIXTURE_SOURCE_BRANCH": publication["source_branch"],
                        "CRAFT_FACTORY_VERIFIER_TOKEN": tokens["service-verifier"]}
        check = cli + ["shared-ops"] + queue_args + [
            "attest-bitbucket-ci-checks", "--handoff", str(ready), "--policy", str(policy), "--build", "15",
        ]
        for flags in ({"FACTORY_FIXTURE_FAILED_CHECK": "1"}, {"FACTORY_FIXTURE_PULL_NUMBER": "157"},
                      {"FACTORY_FIXTURE_SOURCE_BRANCH": "another-branch-at-same-head"},
                      {"FACTORY_FIXTURE_NEWER": "1"}):
            rejected = subprocess.run(check, env=native | flags, capture_output=True, text=True, timeout=15)
            assert rejected.returncode != 0, flags
            request("operations", "operator", "assign_review", 409, **assignment)
        receipt = json.loads(subprocess.check_output(check, env=native, text=True, timeout=15))["bitbucket_ci_checks"]
        assert receipt["verifier"] == "service-verifier" and receipt["verification_sequence"] == sequence
        assert receipt["publication_key"] == key and receipt["proof"]["head"] == head
        assert receipt["proof"]["build_number"] == 15
        assert receipt["proof"]["policy_digest"] == policy_digest
        assert receipt["proof"]["checks"] == [{"id": "babe0000-0000-0000-0000-000000000043", "name": "Verify candidate"}]
        assert json.loads(subprocess.check_output(check, env=native, text=True, timeout=15))["bitbucket_ci_checks"] == receipt
        request("operations", "operator", "assign_review", **assignment)
        assert request("verifier", "verifier", "confirm_bitbucket_work_merge", **identity)["bitbucket_merge"]["pull_request"] == url
        refreshed = attest(ready, "passed", "fixture:refreshed independent verdict")
        assert refreshed.returncode == 0, refreshed.stderr
        fresh = json.loads(refreshed.stdout)["verification_sequence"]
        request("operations", "operator", "assign_review", 409, **(assignment | {
            "handoff_id": "shared-bitbucket-ci-fresh-review", "verification_sequence": fresh,
        }))
        request("verifier", "verifier", "confirm_bitbucket_work_merge", 409, **identity)
        audit = request("operations", "operator", "audit", work_id=work_id)["audit"]
        assert audit["bitbucket_ci_checks"] == [receipt]
        assert audit["attempts"][-1]["reported_tokens"] == 0
        request("operations", "operator", "revoke_review_inbox", grant_id=grant, evidence="fixture:Bitbucket CI gate checked")
        request("operator", "operator", "cancel", work_id=work_id, reason="fixture checked shared Bitbucket repair")
    finally:
        mapping.write_bytes(original_mapping)
    print("Verified shared Bitbucket CI intake, supersession hold, operator retry, and native review proof.")


def exercise_shared_ci_lane(root, repo, template, mapping, request, launch, finish, prepare, attest,
                            launcher, agent, address, certificate, tokens):
    order = template | {"id": "recurring-ci", "repository": str(repo)}
    def digest(path):
        return "sha256:" + sha256(path.read_bytes()).hexdigest()
    order_file = root / "recurring-ci-order.json"
    order_file.write_text(json.dumps(order), encoding="utf-8")
    policy = root / "recurring-ci-policy.json"
    policy.write_text(json.dumps({
        "schema_version": 1, "twg": str(agent), "profile": "fixture", "site": "fixture.atlassian.net",
        "workspace": "team", "repository": "service", "base_branch": "main",
        "required_steps": ["Verify candidate"], "check_definitions": [],
    }), encoding="utf-8")
    manifest = root / "recurring-ci.json"
    config = {
        "schema_version": 1, "observer": "intake", "order": str(order_file), "order_digest": digest(order_file),
        "policy": str(policy), "policy_digest": digest(policy), "source": {"forge": "bitbucket", "step_name": "Verify candidate"},
        "follow_branch": True, "allow_admission": False, "daily_limit": 1, "token_limit": 40,
    }
    cli = [str(launcher.resolve()), "shared-work", "--queue", address, "--ca", str(certificate)]
    git = ["git", "-C", str(repo)]
    tree = subprocess.check_output(git + ["rev-parse", f"{order['base']}^{{tree}}"], text=True, timeout=10).strip()
    commit_tree = git + ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit-tree", tree]
    descendant = subprocess.check_output(commit_tree + ["-p", order["base"], "-m", "CI lane descendant"],
                                         text=True, timeout=10).strip()
    divergent = subprocess.check_output(commit_tree + ["-m", "Unrelated branch"], text=True, timeout=10).strip()
    operator = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    intake = os.environ | {"CRAFT_FACTORY_INTAKE_TOKEN": tokens["intake"], "FACTORY_FIXTURE_SHA": descendant,
                           "FACTORY_FIXTURE_COMPLETED": "2026-11-21T04:30:00Z"}

    def configure(lane, **updates):
        manifest.write_text(json.dumps(config | {"lane": lane} | updates), encoding="utf-8")
        return [str(manifest), "--digest", digest(manifest)]

    def approve(args):
        return json.loads(subprocess.check_output(cli + ["approve-ci-lane", *args, "--evidence", "fixture:CI lane approval"],
                                                  env=operator, text=True, timeout=15))["ci_lane"]

    def poll(args, *, admit=False, env=intake):
        flags = ["--admit"] if admit else []
        return json.loads(subprocess.check_output(cli + ["poll-ci-lane", *args, *flags], env=env, text=True, timeout=15))

    shadow = configure("recurring-ci-shadow")
    original = approve(shadow)
    assert approve(shadow) == original and original["allow_admission"] is False
    assert poll(shadow, admit=True)["decision"]["state"] == "shadow"
    competing = configure("recurring-ci-active", allow_admission=True)
    conflict = subprocess.run(cli + ["approve-ci-lane", *competing, "--evidence", "fixture:overlap"],
                              env=operator, text=True, capture_output=True, timeout=15)
    assert conflict.returncode != 0 and "HTTP 409" in conflict.stderr, conflict.stderr
    disable = {"lane": "recurring-ci-shadow", "evidence": "fixture:enable reviewed revision"}
    request("operator", "intake", "disable_ci_lane", 403, **disable)
    stopped = request("operator", "operator", "disable_ci_lane", **disable)["ci_lane"]
    assert stopped["disabled_by"] == "operator"
    assert request("operator", "operator", "disable_ci_lane", **disable)["ci_lane"] == stopped
    request("intake", "intake", "observe_ci", 403, lane=disable["lane"], manifest_digest=shadow[-1],
            failure=None, admit=True)
    assert approve(competing)["allow_admission"] is True
    request("intake", "intake", "observe_ci", 403, lane="recurring-ci-active",
            manifest_digest="sha256:" + "0" * 64, failure=None, admit=False)
    request("intake", "operator", "observe_ci", 403, lane="recurring-ci-active",
            manifest_digest=competing[-1], failure=None, admit=True)
    assert poll(competing)["decision"]["state"] == "shadow"
    assert poll(competing, env=intake | {"FACTORY_FIXTURE_RECOVERED": "1"})["decision"]["state"] == "no_failure"
    failure = {
        "repository": "team/service", "branch": "main", "commit": descendant, "build": 14,
        "pipeline_uuid": "cafe0000-0000-0000-0000-000000000042", "step_uuid": "babe0000-0000-0000-0000-000000000042",
        "step_name": "Verify candidate", "completed_on": intake["FACTORY_FIXTURE_COMPLETED"],
        "evidence_url": "https://bitbucket.org/team/service/pipelines/results/14/steps/%7Bbabe0000-0000-0000-0000-000000000042%7D",
    }
    for off_policy in ({"branch": "another-branch"}, {"step_name": "Unapproved step"}):
        request("intake", "intake", "observe_ci", 403, lane="recurring-ci-active", manifest_digest=competing[-1],
                failure=failure | off_policy, admit=True)
    bad_head = subprocess.run(cli + ["poll-ci-lane", *competing, "--admit"],
                              env=intake | {"FACTORY_FIXTURE_SHA": divergent}, capture_output=True, text=True, timeout=15)
    assert bad_head.returncode != 0 and "descend from the approved" in bad_head.stderr, bad_head.stderr
    request("operator", "operator", "set_paused", paused=True)
    try:
        assert poll(competing, admit=True)["decision"]["state"] == "paused"
    finally:
        request("operator", "operator", "set_paused", paused=False)
    review_limit = request("operations", "operator", "summary")["summary"]["review_limit"]
    occupied = "recurring-ci-review-capacity"
    request("operator", "operator", "admit", repository="team/service", event_key=occupied,
            work_order=template | {"id": occupied}, token_limit=20)
    sql("UPDATE craft_queue.control SET review_limit = 1;")
    try:
        claimed = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
        assert claimed["work_id"] == occupied, claimed
        assert poll(competing, admit=True)["decision"]["state"] == "review_capacity"
    finally:
        request("operator", "operator", "cancel", work_id=occupied, reason="fixture:review capacity tested")
        sql(f"UPDATE craft_queue.control SET review_limit = {review_limit};")
    token_budget = request("operations", "operator", "summary")["summary"]["token_budget"]
    sql("UPDATE craft_queue.control SET token_budget = 1;")
    try:
        assert poll(competing, admit=True)["decision"]["state"] == "token_capacity"
    finally:
        sql(f"UPDATE craft_queue.control SET token_budget = {token_budget};")
    with ThreadPoolExecutor(max_workers=2) as pool:
        polls = list(pool.map(lambda _: poll(competing, admit=True), range(2)))
    decisions = [report["decision"] for report in polls]
    assert sorted(decision["state"] for decision in decisions) == ["admitted", "already_admitted"], decisions
    work_id = decisions[0]["work_order_id"]
    assert decisions[1]["work_order_id"] == work_id
    assert polls[0]["source"] == {"branch": "main", "anchor": order["base"], "observed_head": descendant}
    audit = request("operations", "operator", "audit", work_id=work_id)["audit"]["ci_lane_admission"]
    assert audit["lane"] == "recurring-ci-active" and audit["observer"] == "intake"
    assert audit["approved_by"] == "operator" and audit["manifest_digest"] == competing[-1]
    newer = intake | {"FACTORY_FIXTURE_COMPLETED": "2026-11-21T05:30:00Z"}
    assert poll(competing, admit=True, env=newer)["decision"]["state"] == "lane_occupied"
    initial_mapping = mapping.read_bytes()
    try:
        mapping.write_text(json.dumps({"team/service": {"path": str(repo), "bitbucket_ci": str(policy)}}),
                           encoding="utf-8")
        result = finish(launch("first", root / "recurring-ci-attempt", str(uuid4()), fixture_env=intake))
        assert result["work_id"] == work_id and result["generation"] == 1
        verification = root / "recurring-ci-verification"
        prepared = prepare(work_id, result["candidate"], verification, "verifier", verifier=True)
        assert prepared.returncode == 0, prepared.stderr
        pinned = json.loads((verification / "work-order.json").read_text(encoding="utf-8"))
        assert pinned["base"] == descendant and pinned["ci_anchor"] == order["base"]
        verdict = attest(verification / "handoff.json", "passed", "fixture:verify recurring repair")
        assert verdict.returncode == 0, verdict.stderr
    finally:
        mapping.write_bytes(initial_mapping)
    request("operator", "operator", "cancel", work_id=work_id, reason="fixture:recurring CI completed")
    assert poll(competing, admit=True, env=newer)["decision"]["state"] == "daily_limit"
    request("operator", "operator", "disable_ci_lane", lane="recurring-ci-active", evidence="fixture:finish")
    replacement = configure("recurring-ci-replacement", allow_admission=True)
    approve(replacement)
    assert poll(replacement, admit=True, env=newer)["decision"]["state"] == "daily_limit"
    request("operator", "operator", "disable_ci_lane", lane="recurring-ci-replacement", evidence="fixture:finish")
    actions_policy = root / "recurring-actions-policy.json"
    actions_policy.write_text(json.dumps({
        "schema_version": 1, "repository": "team/service", "base_branch": "main", "check_definitions": [],
        "required_checks": [{"name": "Checks", "app_id": 42}],
        "ci_repair": {"workflow_id": 9, "job_name": "Checks"},
    }), encoding="utf-8")
    origin = subprocess.check_output(git + ["remote", "get-url", "origin"], text=True, timeout=10).strip()
    subprocess.run(git + ["remote", "set-url", "origin", "https://github.com/team/service.git"], check=True, timeout=10)
    try:
        actions = configure("recurring-actions", allow_admission=True, follow_branch=False, daily_limit=2,
                            policy=str(actions_policy), policy_digest=digest(actions_policy),
                            source={"forge": "github", "gh": str(agent)})
        approve(actions)
        native = intake | {"FACTORY_FIXTURE_SHA": order["base"], "FACTORY_FIXTURE_BASE": order["base"]}
        assert poll(actions, admit=True, env=native)["decision"]["state"] == "already_admitted"
        rerun = poll(actions, admit=True, env=native | {"FACTORY_FIXTURE_RERUN": "1"})["decision"]
        assert rerun["state"] == "admitted" and rerun["work_order_id"] != work_id, rerun
        request("operator", "operator", "cancel", work_id=rerun["work_order_id"], reason="fixture:GitHub rerun")
        request("operator", "operator", "disable_ci_lane", lane="recurring-actions", evidence="fixture:finish")
    finally:
        subprocess.run(git + ["remote", "set-url", "origin", origin], check=True, timeout=10)
    # The archive fixtures later in this suite use a tightly bounded 24-hour token window.
    sql(f"""UPDATE craft_queue.attempts SET started_at = started_at - interval '25 hours'
        WHERE work_id IN ('{occupied}', '{work_id}');""")
    print("Verified recurring shared CI lane approvals, shadow mode, branch ancestry, race-safe admission, caps, and revocation.")


def exercise_github_enqueue(root, request, launcher, address, certificate, agent, policy, native,
                            receipt, destination, tokens, mapping, refresh_jira, *, lost_ack):
    intent_id = "github-enqueue-repaired" if lost_ack else "github-enqueue-initial"
    approval = {
        "intent_id": intent_id, "receipt_id": receipt["id"], "head": receipt["head"],
        "report_digest": receipt["report_digest"], "publisher_id": "publisher",
        "evidence": "fixture:approve exact reviewed head", "valid_seconds": 300,
    }
    request("publication", "publisher", "approve_github_enqueue", 403, approval=approval)
    request("publication", "operator", "approve_github_enqueue", 409,
            approval=approval | {"report_digest": "sha256:" + "0" * 64})
    request("publication", "operator", "approve_github_enqueue", 409, approval=approval | {"publisher_id": "first"})
    cli = [str(launcher.resolve()), "shared-github-enqueue", "--queue", address, "--ca", str(certificate)]
    risk = receipt["assessment"]["enqueue_risk"]
    assert risk["paths"] == ["src/hello"] and 1 <= risk["commits"] <= 4 and 1 <= risk["changed_lines"] <= 12
    standing = {
        "id": "routine-enqueue", "repository": "team/service", "base_branch": "main", "publisher": "publisher",
        "shadow_policy_digest": receipt["assessment"]["policy_digest"], "allowed_paths": ["src/hello"],
        "max_files": 1, "max_commits": 4, "max_changed_lines": 12, "daily_limit": 1,
        "valid_seconds": 600, "evidence": "fixture:approve standing source edits",
    }
    target = {key: approval[key] for key in ("intent_id", "receipt_id", "head", "report_digest")} | {
        "policy_id": standing["id"],
    }
    claim_action = "claim_scheduled_github_enqueue"
    dispatch = {"policy_id": standing["id"], "policy_digest": "sha256:" + sha256(policy.read_bytes()).hexdigest()}
    if lost_ack:
        # The policy's first send used this repository's rolling slot.
        assert request("publisher", "publisher", claim_action, **dispatch)["github_enqueue_dispatch"] == {}
        request("publisher", "publisher", "authorize_github_enqueue_policy", 409, target=target)
        request("publication", "operator", "revoke_github_enqueue_policy", policy_id=standing["id"],
                evidence="fixture:replace with closure opt-in")
        standing |= {"id": "routine-enqueue-closeout", "daily_limit": 2, "close_standalone_work": True}
        request("publication", "operator", "approve_github_enqueue_policy", policy=standing)
        scheduled = request("publisher", "publisher", claim_action,
                            **(dispatch | {"policy_id": standing["id"]}))["github_enqueue_dispatch"]
        approved = scheduled["intent"]
        intent_id = approved["id"]
    else:
        request("publication", "publisher", "approve_github_enqueue_policy", 403, policy=standing)
        request("publication", "operator", "approve_github_enqueue_policy", 400,
                policy=standing | {"allowed_paths": ["src/*"]})
        restricted = standing | {"id": "other-allowed-paths", "allowed_paths": ["docs/guide.md"]}
        request("publication", "operator", "approve_github_enqueue_policy", policy=restricted)
        assert request("publisher", "publisher", claim_action, **(dispatch | {
            "policy_id": restricted["id"],
        }))["github_enqueue_dispatch"] == {}
        request("publisher", "publisher", "authorize_github_enqueue_policy", 409,
                target=target | {"policy_id": restricted["id"]})
        request("publication", "operator", "revoke_github_enqueue_policy", policy_id=restricted["id"],
                evidence="fixture:replace the scope")
        limited = standing | {"id": "smaller-line-limit", "max_changed_lines": risk["changed_lines"] - 1}
        request("publication", "operator", "approve_github_enqueue_policy", policy=limited)
        request("publisher", "publisher", "authorize_github_enqueue_policy", 409,
                target=target | {"policy_id": limited["id"]})
        request("publication", "operator", "revoke_github_enqueue_policy", policy_id=limited["id"],
                evidence="fixture:replace the line limit")
        short = standing | {"id": "revoked-before-send"}
        request("publication", "operator", "approve_github_enqueue_policy", policy=short)
        revoked = target | {"intent_id": "revoked-policy-intent", "policy_id": short["id"]}
        request("publisher", "publisher", "authorize_github_enqueue_policy", target=revoked)
        request("publication", "operator", "revoke_github_enqueue_policy", policy_id=short["id"],
                evidence="fixture:revoke the unsent approval")
        for action in ("prepare_github_enqueue", "begin_github_enqueue"):
            request("publisher", "publisher", action, 409, intent_id=revoked["intent_id"])
        request("publication", "operator", "resolve_github_enqueue", intent_id=revoked["intent_id"], resolution={
            "outcome": "abandoned", "queue_entry_id": None, "evidence": "fixture:released unused intent",
        })
        expiring = standing | {"id": "expired-scheduled-enqueue"}
        request("publication", "operator", "approve_github_enqueue_policy", policy=expiring)
        expiring_dispatch = dispatch | {"policy_id": expiring["id"]}
        reserved = request("publisher", "publisher", claim_action, **expiring_dispatch)["github_enqueue_dispatch"]
        expired_id = reserved["intent"]["id"]
        sql(f"""UPDATE craft_queue.github_enqueue_intents SET expires_at = clock_timestamp()
            WHERE id = '{expired_id}';""")
        assert request("publisher", "publisher", claim_action, **expiring_dispatch)["github_enqueue_dispatch"] == {}
        expired = request("publication", "operator", "github_enqueue_status", intent_id=expired_id)["github_enqueue"]
        assert expired["state"] == "abandoned" and not expired["send_possible"]
        request("publication", "operator", "revoke_github_enqueue_policy", policy_id=expiring["id"],
                evidence="fixture:replace expired dispatch")
        path = root / "github-enqueue-policy.json"
        path.write_text(json.dumps(standing), encoding="utf-8")
        policy_pin = "sha256:" + sha256(path.read_bytes()).hexdigest()
        saved = json.loads(subprocess.check_output(cli + ["approve-policy", str(path), "--digest", policy_pin],
                                                  env=native, text=True, timeout=15))["github_enqueue_policy"]
        assert saved["policy"] == standing
        request("publisher", "publisher-other", "authorize_github_enqueue_policy", 409, target=target)
        request("publisher", "publisher-other", claim_action, 409, **dispatch)
        request("publisher", "operator", claim_action, 403, **dispatch)
        assert request("publisher", "publisher", claim_action, **(dispatch | {
            "policy_digest": "sha256:" + "0" * 64,
        }))["github_enqueue_dispatch"] == {}
        request("operator", "operator", "set_paused", paused=True)
        try:
            assert request("publisher", "publisher", claim_action, **dispatch)["github_enqueue_dispatch"] == {}
        finally:
            request("operator", "operator", "set_paused", paused=False)
        def claim(_):
            return request("publisher", "publisher", claim_action, **dispatch)["github_enqueue_dispatch"]
        with ThreadPoolExecutor(max_workers=2) as executors:
            first, second = executors.map(claim, range(2))
        assert first == second and first["candidate"]["commit"] == receipt["head"]
        approved = first["intent"]
        intent_id = approved["id"]
        assert approved["scheduled"]
        assert approved["standing_policy_id"] == standing["id"]
        prepared = request("publisher", "publisher", "prepare_github_enqueue", intent_id=intent_id)["github_enqueue"]
        assert prepared["target"]["risk"] == risk
    assert approved["state"] == "approved"
    assert not approved["send_possible"]
    sql("""UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301
        WHERE rule_id = 'jira-github-e2e' AND issue_key = 'FIXTURE-110';""")
    for action in ("prepare_github_enqueue", "begin_github_enqueue"):
        request("publisher", "publisher", action, 409, intent_id=intent_id)
    assert request("publication", "operator", "github_enqueue_status", intent_id=intent_id)["github_enqueue"] == approved
    assert refresh_jira()["decision"]["state"] == "already_admitted"
    assert request("publisher", "publisher", "prepare_github_enqueue", intent_id=intent_id)["github_enqueue"][
        "intent"] == approved
    request("publication", "operator", "approve_github_enqueue", 409,
            approval=approval | {"intent_id": intent_id + "-duplicate"})
    request("publisher", "publisher-other", "begin_github_enqueue", 409, intent_id=intent_id)
    request("operator", "operator", "set_paused", paused=True)
    try:
        request("publisher", "publisher", "begin_github_enqueue", 409, intent_id=intent_id)
    finally:
        request("operator", "operator", "set_paused", paused=False)
    token_file = root / f"{intent_id}-github-token"
    token_file.write_text("fixture-github-writer-token-not-a-real-secret\n", encoding="ascii")
    token_file.chmod(0o600)
    marker = root / f"{intent_id}-sent"
    env = native | {"CRAFT_FACTORY_PUBLISHER_TOKEN": tokens["publisher"], "FACTORY_FIXTURE_ENQUEUE_SENT": str(marker),
                    "FACTORY_FIXTURE_REQUIRE_CI_ISOLATION": "1"}
    command = cli + ["send", intent_id, "--checkout", str(destination / "checkout"),
                     "--policy", str(policy), "--gh", str(agent), "--github-token-file", str(token_file)]
    if not lost_ack:
        checkout_root = root / "scheduled-enqueue-checkouts"
        checkout_root.mkdir()
        poll_report = root / "scheduled-enqueue-report.json"
        poll_report.write_text('{"state":"previous"}', encoding="ascii")
        command = cli + ["poll", standing["id"], "--repository-map", str(mapping),
                         "--checkout-root", str(checkout_root), "--policy", str(policy), "--gh", str(agent),
                         "--github-token-file", str(token_file), "--report-file", str(poll_report)]
        alias = subprocess.run(command[:-1] + [str(token_file)], env=env,
                               capture_output=True, text=True, timeout=20)
        assert alias.returncode != 0 and "report path must be distinct" in alias.stderr
        assert token_file.read_text() == "fixture-github-writer-token-not-a-real-secret\n"
    denied = subprocess.run(command, env=env | {"FACTORY_FIXTURE_QUEUE_PR": "stacked"}, capture_output=True,
                            text=True, timeout=20)
    assert denied.returncode != 0 and "github_queue_pr_unavailable" in denied.stderr and not marker.exists(), (
        denied.returncode, denied.stdout, denied.stderr, marker.exists(),
    )
    assert request("publication", "operator", "github_enqueue_status", intent_id=intent_id)["github_enqueue"] == approved
    if lost_ack:
        uncertain = subprocess.run(command, env=env | {"FACTORY_FIXTURE_ENQUEUE_LOSE_ACK": "1"},
                                   capture_output=True, text=True, timeout=20)
        assert uncertain.returncode != 0 and "outcome is uncertain" in uncertain.stderr and marker.is_file()
        [hold] = request("publication", "operator", "github_enqueue_holds")["github_enqueue_holds"]
        assert hold["id"] == intent_id and hold["state"] == "uncertain" and hold["send_possible"]
        request("operator", "operator", "cancel", 409, work_id=receipt["work_id"], reason="fixture:enqueue unresolved")
        reconcile = cli + ["reconcile", intent_id, "--gh", str(agent), "--evidence", "fixture:observed native queue"]
        absent = subprocess.run(reconcile, env=native, capture_output=True, text=True, timeout=15)
        assert absent.returncode != 0 and "exact reviewed PR head" in absent.stderr
        queued = json.loads(subprocess.check_output(reconcile, env=native | {"FACTORY_FIXTURE_QUEUE_PR": "queued"},
                                                    text=True, timeout=15))["github_enqueue"]
    else:
        assert json.loads(poll_report.read_text()) == {"state": "previous"}
        def send(_):
            return subprocess.run(command, env=env, capture_output=True, text=True, timeout=20)
        with ThreadPoolExecutor(max_workers=2) as executors:
            sent = list(executors.map(send, range(2)))
        successful = [json.loads(result.stdout) for result in sent if result.returncode == 0]
        [queued] = [item["github_enqueue"] for item in successful if "github_enqueue" in item]
        assert all("HTTP 409" in result.stderr for result in sent if result.returncode != 0)
        assert marker.is_file() and not list(checkout_root.iterdir())
        idle = json.loads(subprocess.check_output(command, env=env, text=True, timeout=20))
        assert idle == {"state": "idle", "policy_id": standing["id"]}
        assert json.loads(poll_report.read_text()) == idle
    assert queued["state"] == "queued" and queued["queue_entry_id"] == "MQE_fixture" and queued["send_possible"]
    assert request("publication", "operator", "github_enqueue_holds")["github_enqueue_holds"] == []
    assert request("publication", "operator", "github_enqueue_holds", include_queued=True)["github_enqueue_holds"] == [queued]
    overview = request("operations", "operator", "operator_brief",
                       repository="team/service", usage_hours=24, limit=1)["operator_brief"]["github_queue"]
    assert overview["unresolved_total"] == 0 and overview["queued_total"] == 1
    [visible] = overview["items"]
    assert visible["intent_id"] == intent_id and visible["pull_request"] == queued["pull_request"]
    foreign = request("operations", "operator", "operator_brief",
                      repository="team/other", usage_hours=24, limit=1)["operator_brief"]["github_queue"]
    assert foreign == {"unresolved_total": 0, "queued_total": 0, "items": []}
    request("operator", "operator", "cancel", 409, work_id=receipt["work_id"], reason="fixture:still in GitHub queue")
    request("publisher", "publisher", "begin_github_enqueue", 409, intent_id=intent_id)
    request("publication", "operator", "approve_github_enqueue", 409,
            approval=approval | {"intent_id": intent_id + "-retry"})
    audit = request("operations", "operator", "audit", work_id=receipt["work_id"])["audit"]
    assert any(item == queued for item in audit["github_enqueue_intents"])
    return intent_id


def admit_jira_fixture(root, template, request, launcher, agent, address, certificate, tokens, *,
                       rule, issue, issue_id, execution_profile=None):
    site = "11111111-2222-3333-4444-555555555555"
    policy = {
        "id": rule, "intake_id": "intake", "source": f"jira:{site}", "project_key": "FIXTURE",
        "repository": "team/service", "human_account": "relay-human", "human_owner": "Jira delivery owner",
        "required_label": "automate", "eligible_status": "10001", "allowed_paths": ["src"],
        "preferred_runner": "first", "token_limit": 40, "approval_reference": f"fixture:approve-{rule}",
    }
    if execution_profile is not None:
        policy.update(agent_adapter="codex", execution_profile=execution_profile)
    policy_path = root / f"{rule}-approval.json"
    policy_path.write_text(json.dumps(policy), encoding="utf-8")
    checksum = "sha256:" + sha256(policy_path.read_bytes()).hexdigest()
    cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
           "approve-jira-standing-rule", str(policy_path), "--digest", checksum]
    env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    request("operations", "intake", "approve_jira_standing_rule", 403, policy=policy, digest=checksum)
    request("operations", "operator", "approve_jira_standing_rule", 409,
            policy=policy | {"preferred_runner": "outside"}, digest=checksum)
    if execution_profile is not None:
        request("operations", "operator", "approve_jira_standing_rule", 409, digest=checksum, policy=policy | {
            "agent_adapter": "claude", "execution_profile": execution_profile | {"reasoning_effort": "ultra"},
        })
    saved = json.loads(subprocess.check_output(cli, env=env, text=True, timeout=15))["standing_rule"]
    assert saved["approved_by"] == "operator" and saved["policy_digest"] == checksum
    assert request("operations", "operator", "approve_jira_standing_rule",
                   policy=policy, digest=checksum)["standing_rule"] == saved
    request("operations", "operator", "approve_jira_standing_rule", 409,
            policy=policy | {"token_limit": 41}, digest=checksum)
    if execution_profile is not None:
        assert saved["execution_profile"] == execution_profile and saved["agent_adapter"] == "codex"
        request("operations", "operator", "approve_jira_standing_rule", 409, digest=checksum, policy=policy | {
            "execution_profile": execution_profile | {"reasoning_effort": "medium"},
        })
    order = template | {"id": rule, "issue": issue,
                        "context": [{"location": "repo:skill/SKILL.md", "revision": template["base"]}] + [
                            entry for entry in template["context"] if entry["location"].startswith("project:")
                        ]}
    watch = {"issue": issue, "issue_id": issue_id, "rule": rule, "work_order": order}
    if execution_profile is not None:
        order["agent_adapter"] = "claude"
    config = root / f"{rule}.json"
    jira = [str(launcher.resolve()), "poll-standing-jira", str(config)]

    def relay(*, submit=True, reassigned=False):
        config.write_text(json.dumps({
            "schema_version": 1, "craft": str(agent), "craft_state": str(root),
            "craft_profile": "fixture-jira", "cloud_id": site, "watches": [watch],
        }), encoding="utf-8")
        env = os.environ | {"CRAFT_FACTORY_INTAKE_TOKEN": tokens["intake"],
                            "FACTORY_FIXTURE_JIRA_OBSERVED": str(int(time() * 1000)),
                            "FACTORY_FIXTURE_JIRA_UPDATED": "2024-09-05T00:00:00.001Z"}
        if reassigned:
            env |= {"FACTORY_FIXTURE_JIRA_REASSIGNED": "1", "FACTORY_FIXTURE_JIRA_UPDATED": "2024-09-05T00:02:00.001Z"}
        flags = ["--queue", address, "--ca", str(certificate)] if submit else []
        report = json.loads(subprocess.check_output(jira + flags, env=env, text=True, timeout=20))
        assert report["submitted"] == submit and report["errors"] == []
        [observation] = report["observations"]
        return observation

    observed = relay(submit=False)["snapshot"]
    assert observed["issue"] == issue and not observed["intent_approved"]
    if execution_profile is not None:
        legacy = {key: value for key, value in observed.items() if key not in ("intent_digest", "intent_approved")}
        skipped = request("intake", "intake", "observe", rule=rule, snapshot=legacy, work_order=order)["decision"]
        assert skipped["state"] == "invalid_observation"
        injected = request("intake", "intake", "observe", 403, rule=rule,
                           snapshot=observed | {"intent_approved": True},
                           work_order=order | {"execution_profile": execution_profile})
        assert injected["error"] == "execution_profile_requires_operator_admission"
    watch["approved_intent"] = observed["intent_digest"]
    admission = relay()["decision"]
    assert admission["state"] == "admitted"
    work_id = admission["work_id"]
    assert relay()["decision"]["state"] == "already_admitted"
    accepted_work = request("operator", "operator", "status", work_id=work_id)["work"]
    assert accepted_work["standing_delegation"]["issue_key"] == issue
    assert accepted_work["standing_delegation"]["rule_id"] == rule
    assert accepted_work.get("execution_profile") == execution_profile
    return work_id, relay


def exercise_github_review(root, order, request, launch, finish, prepare, attest,
                           launcher, agent, address, certificate, tokens, mapping):
    issue = "FIXTURE-110"
    context_release = {"repository": "team/service", "location": "project:github-enqueue",
                       "kind": "decision", "source_reference": "fixture:approved-design", "source_revision": "1",
                       "approval_reference": "fixture:design-approval", "excerpt": "Use the reviewed queue policy"}
    canonical = "\n".join(context_release[key] for key in (
        "kind", "source_reference", "source_revision", "approval_reference", "excerpt",
    ))
    context_release["revision"] = "sha256:" + sha256(canonical.encode()).hexdigest()
    context = request("operations", "operator", "register_project_context", release=context_release)["release"]
    closeout_context = request("operations", "operator", "register_project_context",
                               release=context_release | {"location": "project:github-closeout"})["release"]
    order = order | {"context": order["context"] + [
        {key: pin[key] for key in ("location", "revision")} for pin in (context, closeout_context)
    ]}
    profile = {"id": "fixture-jira-profile", "model": "fixture-jira-mini", "reasoning_effort": "low",
               "timeout_seconds": 15, "max_attempts": 2, "approval_reference": "fixture:approved-jira-profile"}
    work_id, relay = admit_jira_fixture(root, order, request, launcher, agent, address, certificate,
                                       tokens, rule="jira-github-e2e", issue=issue, issue_id="1045",
                                       execution_profile=profile)
    assert request("runner", "first", "claim", request_id=str(uuid4()), context_capable=True)["claim"] is None
    policy = root / "github-draft-policy.json"
    policy.write_text(json.dumps({"schema_version": 1, "repository": "team/service", "base_branch": "main",
                                  "check_definitions": [], "required_checks": [{"name": "Checks", "app_id": 42}],
                                  "minimum_merge_approvals": 1, "reviewer_ids": [88], "require_merge_queue": True}),
                      encoding="utf-8")
    url = "https://github.com/team/service/pull/42"
    sql("UPDATE craft_queue.draft_policies SET forge = 'github' WHERE repository = 'team/service';")
    try:
        previous = None
        for generation in (1, 2):
            assert relay()["decision"]["state"] == "already_admitted"
            destination = root / f"github-draft-{generation}"
            result = finish(launch("first", destination, str(uuid4())))
            with closing(sqlite3.connect(destination / "state.db")) as db:
                [selected] = db.execute("SELECT note FROM events WHERE kind = 'runner_profile_selected'").fetchone()
            assert json.loads(selected) == {"requested_profile": profile, "effective_timeout_seconds": 15}
            head = result["candidate"]
            assert result["generation"] == generation
            assert result["work_id"] == work_id
            check_dir = root / f"github-check-{generation}"
            check = prepare(work_id, head, check_dir, "verifier", verifier=True)
            assert check.returncode == 0, check.stderr
            accepted = attest(check_dir / "handoff.json", "passed", f"fixture:github-{generation}")
            assert accepted.returncode == 0, accepted.stderr
            sequence = json.loads(accepted.stdout)["verification_sequence"]
            binding = {"work_id": work_id, "generation": generation, "head": head,
                       "verification_sequence": sequence}
            request("publication", "operator", "reserve", 409, update_draft=generation == 2, **binding)
            request("publication", "reviewer", "bind_github_review", 403, **binding)
            request("publication", "operator", "bind_github_review", 409,
                    **(binding | {"verification_sequence": sequence - 1}))
            target = request("publication", "operator", "bind_github_review", **binding)["publication"]
            assert target["expected_draft_url"] == (url if generation == 2 else None)
            observation = {field: target[field] for field in ("head", "base", "source_branch", "base_branch")}
            request("publication", "operator", "bind_github_review", 409, **binding,
                    observation=observation | {"native_url": "https://bitbucket.org/team/service/pull-requests/42"})
            request("publication", "operator", "bind_github_review", 409, **binding,
                    observation=observation | {"native_url": url, "head": order["base"]})
            if generation == 2:
                request("publication", "operator", "bind_github_review", 409, **binding,
                        observation=observation | {"native_url": "https://github.com/team/service/pull/43"})
            fresh_dir = root / f"github-bind-{generation}"
            ready = prepare(work_id, head, fresh_dir)
            assert ready.returncode == 0, ready.stderr
            command = [str(launcher.resolve()), "bind-shared-github-review", "--queue", address,
                       "--ca", str(certificate), "--handoff", str(fresh_dir / "handoff.json"),
                       "--policy", str(policy), "--gh", str(agent), "--pull-request", "42"]
            env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"],
                                "FACTORY_FIXTURE_BASE": order["base"], "FACTORY_FIXTURE_SHA": head,
                                "FACTORY_FIXTURE_SOURCE_BRANCH": f"craft-factory/{work_id}"}
            wrong = subprocess.run(command, env=env | {"FACTORY_FIXTURE_SOURCE_BRANCH": "other"},
                                   capture_output=True, text=True, timeout=15)
            assert wrong.returncode != 0 and "pinned source branch" in wrong.stderr
            response = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
            assert response.returncode == 0, response.stderr
            receipt = json.loads(response.stdout)["publication"]
            assert receipt["state"] == "confirmed" and receipt["result_url"] == url
            assert receipt["head"] == head and receipt["prior_action_key"] == previous
            request("publisher", "publisher", "begin", 409, action_key=receipt["action_key"],
                    update_draft=generation == 2)
            replay = request("publication", "operator", "bind_github_review", **binding,
                             observation=observation | {"native_url": url})["publication"]
            assert replay == receipt
            previous = receipt["action_key"]
            shared_ops = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
            native = env | {"FACTORY_FIXTURE_SOURCE_BRANCH": "", "FACTORY_FIXTURE_GITHUB_SHADOW": "1",
                            "FACTORY_FIXTURE_ACTIVE_RULES": "queue"}

            def assess(name, fingerprint, environment):
                destination = root / name
                result = subprocess.run(shared_ops + ["assess-github-shadow", "--work-id", work_id, "--head", head,
                                                       "--repository-map", str(mapping), "--destination", str(destination),
                                                       "--policy", str(policy), "--gh", str(agent),
                                                       "--approved-protection-digest", fingerprint],
                                        env=environment, capture_output=True, text=True, timeout=20)
                assert result.returncode == 0, result.stderr
                return json.loads(result.stdout), destination

            if generation == 1:
                preview, _ = assess("github-enqueue-initial-pin", "sha256:" + "0" * 64, native)
                fingerprint = preview["observed_protection_digest"]
                subprocess.check_output(shared_ops + ["approve-github-shadow-policy", "--policy", str(policy),
                                        "--approved-protection-digest", fingerprint,
                                        "--evidence", "fixture:github-protection", "--valid-seconds", "600"],
                                        env=native, text=True, timeout=15)
                assessment, checkout = assess("github-enqueue-initial-ready", fingerprint, native)
                assert assessment["shadow_eligible"]
                readiness_policy = {
                    "id": "fixture-readiness", "repository": "team/service", "base_branch": "main",
                    "publisher": "publisher", "shadow_policy_digest": assessment["policy_digest"],
                    "allowed_paths": ["src/hello"], "max_files": 1, "max_commits": 4, "max_changed_lines": 12,
                    "daily_limit": 1, "valid_seconds": 600, "evidence": "fixture:readiness relay",
                }
                selection = {key: readiness_policy[key] for key in ("repository", "base_branch")} | {
                    "policy_digest": assessment["policy_digest"],
                }
                action = "next_github_readiness"
                request("verifier", "verifier", action, 403, **selection)
                request("verifier", "other-verifier", action, 403, **selection)
                assert request("verifier", "service-verifier", action, **selection)["github_readiness_target"] is None
                request("publication", "operator", "approve_github_enqueue_policy", policy=readiness_policy)

                def readiness_backlog(repository="team/service"):
                    return request("operations", "operator", "operator_brief", repository=repository,
                                   usage_hours=24, limit=1)["operator_brief"]["github_readiness"]

                waiting = readiness_backlog()
                assert waiting["total"] == 1
                [item] = waiting["items"]
                assert item["state"] == "waiting_for_check" and item["work_id"] == work_id
                assert item["pull_request"] == url and item["head"] == head and item["verification_sequence"] == sequence
                assert readiness_backlog("team/other") == {"total": 0, "items": []}
                relay_root = root / "github-readiness-checkouts"
                relay_root.mkdir()
                relay_token = root / "github-readiness-token"
                relay_token.write_text(tokens["service-verifier"] + "\n", encoding="ascii")
                relay_token.chmod(0o600)
                relay_report = root / "github-readiness-report.json"
                relay_report.write_text('{"state":"previous"}', encoding="ascii")
                readiness = shared_ops + ["poll-github-readiness", "--repository-map", str(mapping), "--checkout-root",
                                      str(relay_root), "--policy", str(policy), "--gh", str(agent),
                                      "--approved-protection-digest", fingerprint, "--verifier-token-file", str(relay_token),
                                      "--report-file", str(relay_report)]
                alias = subprocess.run(readiness[:-1] + [str(relay_token)], env=native,
                                       capture_output=True, text=True, timeout=20)
                assert alias.returncode != 0 and "report path must be distinct" in alias.stderr
                assert relay_token.read_text() == tokens["service-verifier"] + "\n"
                broken_map = root / "github-readiness-missing-mapping.json"
                broken_map.write_text("{}", encoding="utf-8")
                broken_poll = list(readiness)
                broken_poll[broken_poll.index("--repository-map") + 1] = str(broken_map)
                failure = subprocess.run(broken_poll, env=native, text=True, capture_output=True, timeout=20)
                assert failure.returncode != 0 and "no trusted checkout is mapped" in failure.stderr
                assert json.loads(relay_report.read_text()) == {"state": "previous"}
                assert not list(relay_root.iterdir())
                assert request("verifier", "service-verifier", action, **selection)["github_readiness_target"] is None
                assert sql(f"SELECT count(*) FROM craft_queue.shadow_merge_receipts WHERE work_id = '{work_id}';") == "0"
                [item] = readiness_backlog()["items"]
                assert item["state"] == "check_in_progress" and "attempted_at" in item and "receipt_id" not in item

                later_id = "github-readiness-later-draft"
                later_url = "https://github.com/team/service/pull/43"
                later_verdict = clone_reviewable_work(work_id, sequence, later_id, later_url)
                page = readiness_backlog()
                assert page["total"] == 2 and len(page["items"]) == 1 and page["items"][0]["work_id"] == later_id
                with ThreadPoolExecutor(max_workers=2) as voters:
                    polls = [voters.submit(request, "verifier", "service-verifier", action, **selection)
                             for _ in range(2)]
                    targets = [poll.result()["github_readiness_target"] for poll in polls]
                assert targets.count(None) == 1
                selected = next(target for target in targets if target)
                assert selected["work_id"] == later_id and selected["pull_request"] == later_url
                assert selected["verification_sequence"] == later_verdict
                assert request("operator", "operator", "cancel", work_id=later_id,
                               reason="fixture:readiness retry")["cancelled"]

                expire_attempt = f"""UPDATE craft_queue.github_readiness_attempts
                    SET attempted_at = attempted_at - interval '100 seconds'
                    WHERE publication_key = '{receipt['action_key']}';"""
                sql(expire_attempt)
                [item] = readiness_backlog()["items"]
                assert item["state"] == "check_overdue" and item["work_id"] == work_id
                brief = subprocess.check_output(shared_ops + ["operator-brief", "--repository", "team/service", "--limit", "1"],
                                                env=native, text=True, timeout=15)
                assert f"Check did not report | {work_id} | team/service" in brief and url in brief
                blocked_poll = json.loads(subprocess.check_output(readiness, env=native | {
                    "FACTORY_FIXTURE_QUEUE_PR": "stacked",
                }, text=True, timeout=20))["receipt"]
                assert blocked_poll["actor"] == "service-verifier" and not blocked_poll["assessment"]["eligible"]
                assert blocked_poll["head"] == head and blocked_poll["verification_sequence"] == sequence
                [item] = readiness_backlog()["items"]
                assert item["state"] == "blocked" and item["receipt_id"] == blocked_poll["id"]
                assert item["blockers"] == blocked_poll["assessment"]["blockers"][:3]
                assert json.loads(subprocess.check_output(readiness, env=native, text=True, timeout=20))["state"] == "idle"
                sql(f"""UPDATE craft_queue.shadow_merge_receipts
                    SET recorded_at = recorded_at - interval '100 seconds',
                        assessment = jsonb_set(assessment, '{{as_of}}', to_jsonb((assessment->>'as_of')::bigint - 100))
                    WHERE id = '{blocked_poll['id']}';""")
                sql(expire_attempt)
                initial_ready_receipt = json.loads(subprocess.check_output(readiness, env=native, text=True,
                                                                           timeout=20))["receipt"]
                assert initial_ready_receipt["actor"] == "service-verifier"
                assert initial_ready_receipt["assessment"]["eligible"] and not list(relay_root.iterdir())
                assert json.loads(relay_report.read_text())["receipt"] == initial_ready_receipt
                assert readiness_backlog() == {"total": 0, "items": []}
                print("Verified operator readiness triage, including repository scope, bounded results, and receipt links.")
                print("Verified readiness polling skips an interrupted draft, dispatches distinct work, and retries.")
                write = {name: initial_ready_receipt[name] for name in (
                    "work_id", "generation", "head", "pack_digest", "verification_sequence", "report_digest", "assessment",
                )} | {"receipt_id": initial_ready_receipt["id"]}
                request("verifier", "other-verifier", "record_github_readiness", 404, receipt=write)
                request("verifier", "verifier", "record_github_readiness", 403, receipt=write)
                replay = request("verifier", "service-verifier", "record_github_readiness", receipt=write)["receipt"]
                assert replay == initial_ready_receipt
                proof = {key: write[key] for key in ("receipt_id", "head", "report_digest")}
                request("verifier", "service-verifier", "confirm_github_shadow_merge", 409, **proof)
                assert request("verifier", "verifier", "confirm_github_shadow_merge", **proof)["github_merge"][
                    "pull_request"] == url
                request("publication", "operator", "revoke_github_enqueue_policy",
                        policy_id=readiness_policy["id"], evidence="fixture:replace standing policy")
                assert request("verifier", "service-verifier", action, **selection)["github_readiness_target"] is None
                initial_intent = exercise_github_enqueue(root, request, launcher, address, certificate, agent,
                                                         policy, native, initial_ready_receipt, checkout,
                                                         tokens, mapping, relay, lost_ack=False)
                grant_id = "github-import-reviewer"
                handoff_id = "github-native-change-request"
                request("operations", "operator", "grant_review_inbox", grant_id=grant_id,
                        reviewer_id="reviewer", repository="team/service", valid_seconds=120,
                        evidence="fixture:assigned-native-reviewer")
                request("operations", "operator", "assign_review", **binding, handoff_id=handoff_id,
                        reviewer_id="reviewer", evidence="fixture:native-review-assignment", queue_repair=True)
                final_grant = "github-final-reviewer"
                final_handoff = "github-final-review"
                request("operations", "operator", "grant_review_inbox", grant_id=final_grant,
                        reviewer_id="reviewer-other", repository="team/service", valid_seconds=120,
                        evidence="fixture:assigned-final-reviewer")
                request("operations", "operator", "assign_review", **binding, handoff_id=final_handoff,
                        reviewer_id="reviewer-other", evidence="fixture:final-review-assignment", queue_repair=False)
                request("reviewer", "reviewer-other", "review_feedback_target", 409, handoff_id=handoff_id)
                request("reviewer", "operator", "review_feedback_target", 403, handoff_id=handoff_id)
                target = request("reviewer", "reviewer", "review_feedback_target", handoff_id=handoff_id)["target"]
                assert target == {"draft": {"reference": url, "head": head}, "base": order["base"]}
                import_review = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                                 "import-github-review", handoff_id, "--review", "601", "--policy", str(policy),
                                 "--gh", str(agent)]
                review_env = env | {"CRAFT_FACTORY_REVIEWER_TOKEN": tokens["reviewer"],
                                    "FACTORY_FIXTURE_SHARED_REVIEW_BODY": "1"}
                for flag, value, reason in (
                    ("FACTORY_FIXTURE_WRONG_REVIEW_LOGIN", "1", "different GitHub account"),
                    ("FACTORY_FIXTURE_REVIEW_HEAD", order["base"], "exact pull request head"),
                    ("FACTORY_FIXTURE_REVIEW_SUPERSEDED", "1", "superseded"),
                ):
                    rejected = subprocess.run(import_review, env=review_env | {flag: value},
                                              capture_output=True, text=True, timeout=15)
                    assert rejected.returncode != 0 and reason in rejected.stderr, rejected.stderr
                assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "candidate"
                imported = json.loads(subprocess.check_output(import_review, env=review_env, text=True,
                                                               timeout=15))["handoff"]
                assert imported["automatic_repair"] == "needs_operator"
                assert imported["decision_evidence"] == url + "#pullrequestreview-601"
                assert imported["decision_note"] == "Preserve the reviewed change and say repaired"
                assert json.loads(subprocess.check_output(import_review, env=review_env, text=True,
                                                          timeout=15))["handoff"] == imported
                assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "candidate"
                acknowledgment = {"handoff_id": final_handoff, "head": head, "decision": "acknowledged",
                                  "evidence": "fixture:final-review", "note": "No additional changes requested"}
                request("reviewer", "reviewer", "record_review_decision", 409, **acknowledgment)
                request("reviewer", "reviewer-other", "record_review_decision", 409,
                        **(acknowledgment | {"head": order["base"]}))
                final = request("reviewer", "reviewer-other", "record_review_decision", **acknowledgment)["handoff"]
                assert final["decision"] == "acknowledged" and final["automatic_repair"] == "needs_operator"
                assert request("reviewer", "reviewer-other", "record_review_decision",
                               **acknowledgment)["handoff"] == final
                request("operations", "operator", "apply_review_repair", 409, handoff_id=handoff_id)
                assert sql(f"SELECT craft_queue.current_github_enqueue('{initial_ready_receipt['id']}', "
                           f"'{head}', '{initial_ready_receipt['report_digest']}') IS NULL;") == "t"
                clear = [str(launcher.resolve()), "shared-github-enqueue", "--queue", address, "--ca", str(certificate),
                         "clear", initial_intent, "--gh", str(agent), "--evidence", "fixture:native withdrawal"]
                for mode in ("queued", "merged", "retargeted", "auto_merge", "stacked"):
                    rejected = subprocess.run(clear, env=native | {"FACTORY_FIXTURE_QUEUE_PR": mode},
                                              capture_output=True, text=True, timeout=15)
                    assert rejected.returncode != 0 and "no active queue entry" in rejected.stderr, rejected.stderr
                marker = root / "github-clearance-read-marker"
                changed = subprocess.run(clear, env=native | {"FACTORY_FIXTURE_QUEUE_READ_MARKER": str(marker)},
                                         capture_output=True, text=True, timeout=15)
                assert changed.returncode != 0 and "changed during clearance" in changed.stderr, changed.stderr
                observation = {"pull_request_node_id": "PR_unrelated", "observed_head": head,
                               "evidence": "fixture:native withdrawal"}
                request("publication", "publisher", "clear_github_enqueue", 403,
                        intent_id=initial_intent, observation=observation)
                request("publication", "operator", "clear_github_enqueue", 409,
                        intent_id=initial_intent, observation=observation)
                cleared = json.loads(subprocess.check_output(clear, env=native, text=True, timeout=15))["github_enqueue"]
                assert cleared["state"] == "cleared" and cleared["queue_entry_id"] == "MQE_fixture"
                assert cleared["cleared_by"] == "operator" and cleared["clear_observed_head"] == head
                assert json.loads(subprocess.check_output(clear, env=native, text=True, timeout=15))["github_enqueue"] == cleared
                assert request("publication", "operator", "github_enqueue_holds", include_queued=True)["github_enqueue_holds"] == []
                bundled = request("operations", "operator", "apply_review_repair", handoff_id=handoff_id)["repair"]
                assert bundled["review_handoff_ids"] == [handoff_id] and bundled["actor"] == "operator"
                assert bundled["note"] == imported["decision_note"] and bundled["native_url"] == url
                request("operations", "operator", "revoke_review_inbox", grant_id=final_grant,
                        evidence="fixture:completed-final-review")
                request("operations", "operator", "revoke_review_inbox", grant_id=grant_id,
                        evidence="fixture:completed-native-review")
                request("reviewer", "reviewer", "review_feedback_target", 409, handoff_id=handoff_id)
            else:
                preview, _ = assess("github-shadow-pin", "sha256:" + "0" * 64, native)
                fingerprint = preview["observed_protection_digest"]
                assert preview["blocking_codes"] == ["github_protection_drift"] and fingerprint
                policy_file_digest = "sha256:" + sha256(policy.read_bytes()).hexdigest()
                pin = "sha256:" + sha256(f"github-shadow-v1\n{policy_file_digest}\n{fingerprint}\n".encode()).hexdigest()
                approval = subprocess.check_output(shared_ops + ["approve-github-shadow-policy", "--policy", str(policy),
                                                   "--approved-protection-digest", fingerprint,
                                                   "--evidence", "fixture:github-protection", "--valid-seconds", "600"],
                                                   env=env, text=True, timeout=15)
                assert json.loads(approval)["policy"]["digest"] == pin
                receipts = {}
                for name, flags, eligible in (
                    ("github-shadow-blocked", {"FACTORY_FIXTURE_MERGE_UNKNOWN": "1"}, False),
                    ("github-shadow-eligible", {}, True),
                ):
                    report, destination = assess(name, fingerprint, native | flags)
                    assert report["policy_digest"] == pin and report["shadow_eligible"] == eligible
                    arguments = ["record-shadow-merge", name, "--handoff", str(destination / "handoff.json"),
                                 "--report", str(destination / "github-shadow-report.json")]
                    recorded = subprocess.run(shared_ops + arguments, env=env, capture_output=True, text=True, timeout=15)
                    assert recorded.returncode == 0, recorded.stderr
                    stored = json.loads(recorded.stdout)["receipt"]
                    assert stored["publication_key"] == receipt["action_key"]
                    assert stored["assessment"]["native_verified"] == eligible
                    assert stored["assessment"]["blockers"] == ([] if eligible else ["github_pr_not_ready"])
                    if eligible:
                        assert stored["assessment"]["github_queue"] == {
                            "pull_request_node_id": "PR_fixture_42",
                            "merge_queue_ruleset_id": 56,
                            "active_ruleset_ids": [56, 78],
                        }
                        assert stored["assessment"]["enqueue_risk"]["paths"] == ["src/hello"]
                    else:
                        assert "github_queue" not in stored["assessment"]
                    assert json.loads(subprocess.check_output(shared_ops + arguments, env=env, text=True,
                                                              timeout=15))["receipt"] == stored
                    stale_handoff = destination / "stale-handoff.json"
                    original = json.loads((destination / "handoff.json").read_text(encoding="utf-8"))
                    stale_handoff.write_text(json.dumps(original | {"verification_sequence": sequence - 1}), encoding="utf-8")
                    stale = subprocess.run(shared_ops + ["record-shadow-merge", name + "-stale",
                                                         "--handoff", str(stale_handoff), "--report", arguments[-1]],
                                           env=env, capture_output=True, text=True, timeout=15)
                    assert stale.returncode != 0 and "exact queue verdict" in stale.stderr
                    receipts[name] = stored

                blocked = receipts["github-shadow-blocked"]
                check_page = {"repository": "team/service", "limit": 1}
                listed = request("verifier", "verifier", "pending_merge_checks", **check_page)["merge_checks"]
                [target] = listed["items"]
                first = min(receipts.values(), key=lambda receipt: (receipt["assessment"]["as_of"], receipt["id"]))
                assert target == {
                    "work_id": work_id, "forge": "github", "head": head, "repository": "team/service",
                    "pull_request": url, "base_branch": "main", "verification_sequence": sequence,
                    "receipt_id": first["id"], "report_digest": first["report_digest"],
                    "policy_file_digest": policy_file_digest,
                }
                assert listed["next_after_work_id"] is None
                direct = binding | {"receipt_id": "github-wrong-pr", "pack_digest": blocked["pack_digest"],
                                    "report_digest": "sha256:" + "7" * 64}
                assessment = blocked["assessment"]
                request("operations", "operator", "record_shadow_merge", 409, **direct,
                        assessment=assessment | {"pull_request": "https://github.com/team/service/pull/43"})
                request("operations", "operator", "record_shadow_merge", 400, **direct,
                        assessment=assessment | {"github": assessment["github"] | {"protection_digest": "sha256:" + "9" * 64}})
                request("operations", "operator", "record_shadow_merge", 409, **direct,
                        assessment={name: value for name, value in assessment.items() if name != "github"})
                ready_for_queue = receipts["github-shadow-eligible"]["assessment"]
                request("operations", "operator", "record_shadow_merge", 400, **direct,
                        assessment=ready_for_queue | {"github_queue": ready_for_queue["github_queue"] |
                                                    {"active_ruleset_ids": [78]}})
                audit = request("operations", "operator", "audit", work_id=work_id)["audit"]
                assert {item["id"] for item in audit["shadow_merge_receipts"]} == set(receipts) | {
                    initial_ready_receipt["id"], blocked_poll["id"],
                }
                request("publication", "operator", "approve_github_enqueue", 409, approval={
                    "intent_id": "github-enqueue-stale", "receipt_id": initial_ready_receipt["id"],
                    "head": initial_ready_receipt["head"], "report_digest": initial_ready_receipt["report_digest"],
                    "publisher_id": "publisher",
                    "evidence": "fixture:old generation", "valid_seconds": 300,
                })
                repaired_intent = exercise_github_enqueue(root, request, launcher, address, certificate, agent,
                                                          policy, native, receipts["github-shadow-eligible"],
                                                          root / "github-shadow-eligible", tokens, mapping, relay,
                                                          lost_ack=True)
                withdrawal = {key: context[key] for key in ("repository", "location", "revision")} | {
                    "evidence": "fixture:retire-context-after-enqueue",
                }
                retired = request("operations", "operator", "withdraw_project_context", **withdrawal)["release"]
                assert retired["retired_by"] == "operator" and retired["retired_at"]
                assert request("operations", "operator", "withdraw_project_context", **withdrawal)["release"] == retired
                assert request("publication", "operator", "github_enqueue_status",
                               intent_id=repaired_intent)["github_enqueue"]["state"] == "queued"
                assert relay(reassigned=True)["decision"]["state"] == "ineligible"
                fenced = request("operator", "operator", "status", work_id=work_id)["work"]
                assert fenced["state"] == "candidate" and fenced["standing_delegation"]["blocked_at"]
                scheduled_closure = {
                    "repository": "team/service", "base_branch": "main",
                    "policy_file_digest": policy_file_digest, "limit": 2,
                }
                close_action = "close_scheduled_github_work"
                assert request("verifier", "service-verifier", close_action,
                               **scheduled_closure)["closures"] == []
                merge_binding = {"receipt_id": blocked["id"], "head": head, "report_digest": blocked["report_digest"]}
                assert request("verifier", "service-verifier", "confirm_github_shadow_merge",
                               **merge_binding)["github_merge"]["pull_request"] == url
                request("verifier", "other-verifier", "confirm_github_shadow_merge", 404, **merge_binding)
                request("verifier", "other-verifier", "record_shadow_outcome", 404, **merge_binding,
                        basis="fixture", expected_eligible=False, evidence_digest=blocked["report_digest"],
                        evidence="fixture:denied", note="Cross-repository request")
                closure_request = {"work_id": work_id, "head": head, "merge_receipt_id": blocked["id"],
                                   "evidence": "fixture:standalone-merge-closure"}
                request("operations", "operator", "close_github_work", 409, **closure_request)
                request("verifier", "operator", "confirm_github_shadow_merge", 403, **merge_binding)
                request("verifier", "verifier", "confirm_github_shadow_merge", 409,
                        **(merge_binding | {"report_digest": "sha256:" + "0" * 64}))
                assert request("verifier", "verifier", "confirm_github_shadow_merge", **merge_binding)["github_merge"]["recorded"] is None
                command = shared_ops + ["confirm-github-shadow-merge", blocked["id"],
                                        "--head", head, "--report-digest", blocked["report_digest"],
                                        "--policy", str(policy), "--gh", str(agent)]
                verifier_env = env | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["verifier"]}
                batch_env = verifier_env | {"FACTORY_FIXTURE_REQUIRE_VERIFIER_ISOLATION": "1"}
                batch_command = shared_ops + ["check-github-merges", "--policy", str(policy), "--gh", str(agent),
                                              "--limit", "1", "--check-seconds", "1"]

                def batch(environment, budget="30"):
                    return json.loads(subprocess.check_output(
                        batch_command + ["--budget-seconds", budget], env=environment, text=True, timeout=15,
                    ))["merge_check_report"]

                assert batch(batch_env, budget="6") == {
                    "repository": "team/service", "items": [], "deferred_in_page": 1, "next_after_work_id": None,
                }
                assert batch(batch_env)["items"][0]["outcome"] == "not_merged"
                started = monotonic()
                timed = batch(batch_env | {"FACTORY_FIXTURE_HANG_MERGE_READ": "1"})
                assert timed["items"][0]["outcome"] == "check_timed_out" and monotonic() - started < 5
                cursor = root / "github-merge-cursor.db"
                last_report = root / "github-merge-last-report.json"
                verifier_file = root / "github-merge-verifier-token"
                verifier_file.write_text(tokens["verifier"] + "\n", encoding="ascii")
                recurring = batch_command + ["--cursor-db", str(cursor), "--report-file", str(last_report),
                                              "--verifier-token-file", str(verifier_file)]
                scheduled_env = batch_env.copy()
                scheduled_env.pop("CRAFT_FACTORY_VERIFIER_TOKEN")

                def recurring_pass():
                    report = json.loads(subprocess.check_output(recurring, env=scheduled_env, text=True, timeout=15))
                    assert json.loads(last_report.read_text(encoding="utf-8")) == report
                    return report["merge_check_report"]

                assert recurring_pass()["items"][0]["outcome"] == "not_merged"
                pinned = recurring + ["--policy-digest", policy_file_digest]
                assert json.loads(subprocess.check_output(pinned, env=scheduled_env, text=True,
                                                          timeout=15))["merge_check_report"]["items"][0]["outcome"] == "not_merged"
                before_drift = last_report.read_bytes()
                wrong_pin = subprocess.run(recurring + ["--policy-digest", "sha256:" + "0" * 64],
                                           env=scheduled_env, capture_output=True, text=True, timeout=15)
                assert wrong_pin.returncode != 0 and "differs from the supplied digest" in wrong_pin.stderr
                assert last_report.read_bytes() == before_drift
                with closing(sqlite3.connect(cursor)) as state, state:
                    state.execute("UPDATE merge_batch_cursor SET after_work_id = ?", (work_id,))
                assert recurring_pass()["items"] == []
                assert recurring_pass()["items"][0]["work_id"] == work_id
                previous_report = last_report.read_bytes()
                with closing(sqlite3.connect(cursor)) as state, state:
                    state.execute("BEGIN IMMEDIATE")
                    busy = subprocess.run(recurring, env=batch_env, capture_output=True, text=True, timeout=15)
                    assert busy.returncode != 0 and "database is locked" in busy.stderr
                assert last_report.read_bytes() == previous_report
                original_policy = policy.read_bytes()
                alias = subprocess.run(batch_command + ["--report-file", str(policy)], env=batch_env,
                                       capture_output=True, text=True, timeout=15)
                assert alias.returncode != 0 and "paths must be distinct" in alias.stderr
                assert policy.read_bytes() == original_policy
                with closing(sqlite3.connect(cursor)) as state, state:
                    state.execute("UPDATE merge_batch_cursor SET after_work_id = ?", (work_id,))
                try:
                    policy.write_bytes(original_policy + b"\n")
                    assert recurring_pass()["items"][0]["outcome"] == "outside_policy"
                finally:
                    policy.write_bytes(original_policy)
                sleep(1.05)
                merged = batch_env | {"FACTORY_FIXTURE_MERGED": "1", "FACTORY_FIXTURE_BRANCH": "d" * 40,
                                         "FACTORY_FIXTURE_MERGE_TIME": datetime.now(UTC).isoformat(timespec="seconds")}
                for flags, message in (
                    ({"FACTORY_FIXTURE_SHA": order["base"]}, "exact reviewed head"),
                    ({"FACTORY_FIXTURE_MERGE_TIME": "2025-11-21T03:50:00Z"}, "after the shadow evaluation"),
                    ({"FACTORY_FIXTURE_MERGE_DIVERGED": "1"}, "merge ancestry"),
                    ({"FACTORY_FIXTURE_BRANCH_REWRITTEN": "1"}, "merge ancestry"),
                ):
                    unavailable = subprocess.run(command, env=merged | flags,
                                                 capture_output=True, text=True, timeout=15)
                    assert unavailable.returncode != 0 and message in unavailable.stderr, unavailable.stderr
                report = batch(merged)
                assert report["deferred_in_page"] == 0 and report["items"][0]["outcome"] == "recorded"
                completed = json.loads(subprocess.check_output(command, env=verifier_env, text=True, timeout=15))["github_merge"]
                enqueued = request("publication", "operator", "github_enqueue_status",
                                   intent_id=repaired_intent)["github_enqueue"]
                assert enqueued["state"] == "merged" and enqueued["merge_observation_receipt_id"] == first["id"]
                assert enqueued["queue_entry_id"] == "MQE_fixture"
                retired = request("operations", "operator", "withdraw_project_context",
                                  repository=closeout_context["repository"], location=closeout_context["location"],
                                  revision=closeout_context["revision"],
                                  evidence="fixture:retire-context-after-github-merge")["release"]
                assert retired["retired_at"]
                assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "candidate"
                assert request("publication", "operator", "github_enqueue_holds", include_queued=True)["github_enqueue_holds"] == []
                assert request("verifier", "verifier", "pending_merge_checks",
                               **check_page)["merge_checks"]["items"] == []
                assert completed["observation"]["result_commit"] == "c" * 40
                assert completed["observation"]["destination_tip"] == "d" * 40
                eligible = receipts["github-shadow-eligible"]
                for altered in ({"pull_request": "https://github.com/team/service/pull/43"},
                                {"merged_at": eligible["assessment"]["as_of"]}):
                    request("verifier", "verifier", "confirm_github_shadow_merge", 409,
                            receipt_id=eligible["id"], head=head, report_digest=eligible["report_digest"],
                            observation=completed["observation"] | altered)
                assert json.loads(subprocess.check_output(command, env=verifier_env, text=True,
                                                          timeout=15))["github_merge"] == completed
                audited = request("operations", "operator", "audit", work_id=work_id)["audit"]
                confirmed = next(item for item in audited["shadow_merge_receipts"] if item["id"] == blocked["id"])
                assert not confirmed["assessment"]["eligible"] and confirmed["github_merge"] == completed
                assert audited["shadow_outcomes"] == []
                ready = request("operations", "operator", "review_inbox", limit=1)["inbox"]
                assert ready["counts"]["close_merged_work"] == 1
                assert ready["items"][0]["work_id"] == work_id
                assert ready["items"][0]["merge_closure"] == {
                    "forge": "github", "merge_receipt_id": blocked["id"], "result_commit": "c" * 40,
                }
                request("operations", "verifier", "close_github_work", 403, **closure_request)
                request("operations", "operator", "close_github_work", 409, **(closure_request | {"head": order["base"]}))
                request("operations", "operator", "close_github_work", 409,
                        **(closure_request | {"merge_receipt_id": eligible["id"]}))
                failed_dir = root / "github-closure-failed-check"
                prepared = prepare(work_id, head, failed_dir, "verifier", verifier=True)
                assert prepared.returncode == 0, prepared.stderr
                failed = attest(failed_dir / "handoff.json", "failed", "fixture:github-closure-failed")
                assert failed.returncode == 0, failed.stderr
                request("operations", "operator", "close_github_work", 409, **closure_request)
                assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "candidate"
                request("verifier", "verifier", close_action, 403, **scheduled_closure)
                request("verifier", "other-verifier", close_action, 403, **scheduled_closure)
                assert request("verifier", "service-verifier", close_action,
                               **scheduled_closure)["closures"] == []
                inbox = request("operations", "operator", "review_inbox", limit=1)["inbox"]
                assert "close_merged_work" not in inbox["counts"] and "merge_closure" not in inbox["items"][0]
                refreshed_dir = root / "github-closure-refreshed-check"
                prepared = prepare(work_id, head, refreshed_dir, "verifier", verifier=True)
                assert prepared.returncode == 0, prepared.stderr
                refreshed = attest(refreshed_dir / "handoff.json", "passed", "fixture:github-closure-refreshed")
                assert refreshed.returncode == 0, refreshed.stderr
                refreshed_sequence = json.loads(refreshed.stdout)["verification_sequence"]
                assert refreshed_sequence > sequence
                reserved = request("operations", "operator", "summary")["summary"]["review_reserved"]
                request("operator", "operator", "set_paused", paused=True)
                try:
                    assert request("verifier", "service-verifier", close_action,
                                   **scheduled_closure)["closures"] == []
                finally:
                    request("operator", "operator", "set_paused", paused=False)
                assert request("verifier", "service-verifier", close_action, **(scheduled_closure | {
                    "policy_file_digest": "sha256:" + "0" * 64,
                }))["closures"] == []
                sql("""UPDATE craft_queue.github_enqueue_policies SET expires_at = clock_timestamp()
                    WHERE id = 'routine-enqueue-closeout';""")
                try:
                    assert request("verifier", "service-verifier", close_action,
                                   **scheduled_closure)["closures"] == []
                finally:
                    sql("""UPDATE craft_queue.github_enqueue_policies SET expires_at = approved_at + interval '600 seconds'
                        WHERE id = 'routine-enqueue-closeout';""")
                release_command = shared_ops + ["check-github-merges", "--policy", str(policy), "--gh", str(agent),
                                                "--close-scheduled"]
                closure = json.loads(subprocess.check_output(
                    release_command, env=env | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["service-verifier"]},
                    text=True, timeout=15,
                ))["merge_check_report"]["scheduled_closure"]
                assert closure["outcome"] == "checked"
                [closure] = closure["items"]
                assert closure["result_commit"] == "c" * 40 and closure["head"] == head
                assert closure["verification_sequence"] == refreshed_sequence
                assert closure["enqueue_intent_id"] == repaired_intent
                assert closure["standing_policy_id"] == "routine-enqueue-closeout"
                assert closure["actor"] == "service-verifier"
                assert request("operations", "operator", "summary")["summary"]["review_reserved"] == reserved - 1
                assert request("verifier", "service-verifier", close_action,
                               **scheduled_closure)["closures"] == []
                request("operations", "operator", "close_github_work", 409, **closure_request)
                request("operator", "operator", "cancel", 409, work_id=work_id, reason="merge is already confirmed")
                merged_work = request("operations", "operator", "audit", work_id=work_id)["audit"]["work"]
                assert merged_work["state"] == "merged" and merged_work["github_work_closure"] == closure
                assert merged_work["independent_checks_passed"]
                assert merged_work["standing_delegation"]["issue_key"] == issue
                assert merged_work["standing_delegation"]["blocked_at"] == fenced["standing_delegation"]["blocked_at"]
                receipt = request("operations", "operator", "profile_usage", work_id=work_id)["usage"]
                usage = receipt["totals"]
                assert usage["supervised_starts"] == usage["attempts_with_usage"] == 2, usage
                [completed] = receipt["orders"]
                assert completed["independent_result"] == "passed"
                assert completed["verification_sequence"] == closure["verification_sequence"]
        print("Verified scheduled GitHub closeout after independent merge confirmation, including scope, pause, expiry, and reverification.")
    finally:
        sql("UPDATE craft_queue.draft_policies SET forge = 'bitbucket' WHERE repository = 'team/service';")
    print("Verified one Jira-delegated work item through GitHub review, repair, enqueue, reassignment, and audited closeout.")
    return work_id


def exercise_bitbucket_closure(root, order, request, launch, finish, prepare, attest,
                              launcher, agent, address, certificate, tokens):
    issue = "FIXTURE-111"
    skill_location = "team:bitbucket-merge-fixture"
    markdown = "# Reviewed Bitbucket merge skill"
    skill_revision = "sha256:" + sha256(markdown.encode()).hexdigest()
    sql(f"""
        INSERT INTO craft_queue.team_skill_releases (
            location, revision, markdown, owner_name, approval_reference, repositories
        ) VALUES ('{skill_location}', '{skill_revision}', '{markdown}', 'Fixture owners',
            'fixture:bitbucket-skill-approval', ARRAY['team/service']);
    """)
    order = order | {"skill": {"location": skill_location, "revision": skill_revision}}
    work_id, relay = admit_jira_fixture(root, order, request, launcher, agent, address, certificate,
                                       tokens, rule="jira-bitbucket-e2e", issue=issue, issue_id="1046")
    repository = "team/service"
    attempt = finish(launch("first", root / "jira-bitbucket-attempt", str(uuid4())))
    assert attempt["work_id"] == work_id
    head = attempt["candidate"]
    identity = {"work_id": work_id, "head": head}
    checks = {"repository": repository, "limit": 1}
    cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
    verifier_env = os.environ | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["verifier"]}
    verification = identity | {"generation": attempt["generation"],
                               "digest": attempt["candidate_evidence"], "previous_sequence": 0,
                               "passed": True, "evidence": "fixture:bitbucket-pass", "note": "Independent checks"}
    captured = request("verifier", "service-verifier", "download_pack", work_id=work_id)["candidate"]
    assert captured["digest"] == verification["digest"]
    request("verifier", "other-verifier", "download_pack", 404, work_id=work_id)
    request("verifier", "other-verifier", "record_verification", 404, **verification)
    request("verifier", "other-verifier", "record_dependency_verification", 404, **{
        key: value for key, value in verification.items() if key not in ("evidence", "note")
    }, approval_id="out-of-scope", proof={
        field: verification["digest"] for field in ("intake_digest", "policy_digest",
                                                   "baseline_run_digest", "candidate_run_digest", "report_digest")
    })
    candidate = root / "jira-bitbucket-verified"
    prepared = prepare(work_id, head, candidate, "service-verifier", verifier=True)
    assert prepared.returncode == 0, prepared.stderr
    verified = attest(candidate / "handoff.json", "passed", verification["evidence"], identity="service-verifier")
    assert verified.returncode == 0, verified.stderr
    sequence = json.loads(verified.stdout)["verification_sequence"]
    request("verifier", "verifier", "confirm_bitbucket_work_merge", 409, **identity)
    assert request("verifier", "verifier", "pending_merge_checks", **checks)["merge_checks"]["items"] == []
    publication = request("publication", "operator", "reserve", **identity,
                          generation=attempt["generation"], verification_sequence=sequence)["publication"]
    publication_key = publication["action_key"]
    closure = identity | {"publication_key": publication_key, "evidence": "fixture:close-merged-bitbucket"}
    request("operations", "operator", "close_bitbucket_work", 409, **closure)
    request("publisher", "publisher", "begin", action_key=publication_key)
    url = f"https://bitbucket.org/{repository}/pull-requests/153"
    request("publication", "operator", "reconcile", action_key=publication_key, observation={
        "outcome": "confirmed", "head": head, "native_url": url,
        "source_branch": publication["source_branch"], "base_branch": publication["base_branch"],
        "evidence": "fixture:bound-bitbucket-publication",
    })
    request("verifier", "operator", "confirm_bitbucket_work_merge", 403, **identity)
    request("verifier", "other-verifier", "confirm_bitbucket_work_merge", 404, **identity)
    request("verifier", "verifier", "confirm_bitbucket_work_merge", 409, **(identity | {"head": order["base"]}))
    target = request("verifier", "verifier", "confirm_bitbucket_work_merge", **identity)["bitbucket_merge"]
    assert target["publication_key"] == publication_key and target["pull_request"] == url
    assert target["recorded"] is None and target["repository"] == repository
    assert request("verifier", "service-verifier", "confirm_bitbucket_work_merge", **identity)["bitbucket_merge"] == target
    assert request("verifier", "service-verifier", "pending_merge_checks", **checks)["merge_checks"]["items"]
    request("verifier", "other-verifier", "pending_merge_checks", 403, **checks)
    request("verifier", "operator", "pending_merge_checks", 403, **checks)
    request("verifier", "verifier", "pending_merge_checks", 400, **(checks | {"limit": 51}))
    request("verifier", "verifier", "pending_merge_checks", 400, **(checks | {"repository": "team/service/extra"}))
    listing = subprocess.check_output(cli + ["pending-merge-checks", repository, "--limit", "1"],
                                      env=verifier_env, text=True, timeout=15)
    assert json.loads(listing)["merge_checks"] == {
        "repository": repository, "next_after_work_id": None,
        "items": [{"work_id": work_id, "repository": repository, "head": head, "pull_request": url,
                   "base_branch": publication["base_branch"], "verification_sequence": sequence,
                   "forge": "bitbucket", "publication_key": publication_key}],
    }
    assert request("verifier", "verifier", "pending_merge_checks", after_work_id=work_id,
                   **checks)["merge_checks"]["items"] == []
    policy = root / "bitbucket-merge-check-policy.json"
    scope = {"schema_version": 1, "workspace": "team", "repository": "service",
             "repository_uuid": "b52ecbdf-46d3-43cc-bad9-84322565927a", "reviewer_uuids": []}
    credentials = root / "bitbucket-merge-credentials"
    credentials.mkdir(mode=0o700)
    (credentials / "CRAFT_FACTORY_VERIFIER_TOKEN").write_text(tokens["verifier"] + "\n", encoding="ascii")
    bitbucket_token_file = credentials / "CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN"
    bitbucket_token_file.write_text("local-fixture-unused-in-batch\n", encoding="ascii")
    batch_env = verifier_env | {"CREDENTIALS_DIRECTORY": str(credentials)}
    batch_env.pop("CRAFT_FACTORY_VERIFIER_TOKEN", None)
    batch_env.pop("CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN", None)
    batch_env.pop("CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL", None)
    command = cli + ["check-bitbucket-merges", "--policy", str(policy), "--limit", "1"]
    policy.write_text(json.dumps(scope | {"base_branch": publication["base_branch"]}), encoding="utf-8")
    deferred = subprocess.check_output(command + ["--budget-seconds", "6"], env=batch_env, text=True, timeout=15)
    assert json.loads(deferred)["merge_check_report"] == {
        "repository": repository, "items": [], "deferred_in_page": 1, "next_after_work_id": None,
    }
    policy.write_text(json.dumps(scope | {"base_branch": "other-branch"}), encoding="utf-8")
    skipped = subprocess.check_output(command, env=batch_env, text=True, timeout=15)
    assert json.loads(skipped)["merge_check_report"]["items"] == [
        {"work_id": work_id, "head": head, "pull_request": url, "outcome": "outside_policy"},
    ]
    token_alias = subprocess.run(command + ["--bitbucket-token-file", str(bitbucket_token_file),
                                            "--report-file", str(bitbucket_token_file)],
                                 env=batch_env, capture_output=True, text=True, timeout=15)
    assert token_alias.returncode != 0 and "paths must be distinct" in token_alias.stderr
    assert bitbucket_token_file.read_text(encoding="ascii") == "local-fixture-unused-in-batch\n"
    commit = "c" * 40
    observation = {
        "pull_request": url, "head": head, "pinned_base": order["base"],
        "destination_branch": publication["base_branch"],
        "result_commit": commit, "reference": f"https://bitbucket.org/{repository}/commits/{commit}",
        "merged_at": int(datetime.now(UTC).timestamp()),
        "repository_uuid": "b52ecbdf-46d3-43cc-bad9-84322565927a",
        "actor": "bitbucket:user:5b3feb4d-9af7-40d5-a0a9-e7a9e5ff163e",
    }
    request("verifier", "verifier", "confirm_bitbucket_work_merge", 409, **identity,
            publication_key="draft:other", observation=observation)
    for changed in ({"pull_request": url + "0"}, {"head": order["base"]}, {"destination_branch": "release"},
                    {"merged_at": 1}, {"result_commit": commit[:12]}, {"repository_uuid": "invalid"}):
        request("verifier", "verifier", "confirm_bitbucket_work_merge", 409, **identity,
                publication_key=publication_key, observation=observation | changed)
    payload = identity | {"publication_key": publication_key, "observation": observation}
    recorded = request("verifier", "verifier", "confirm_bitbucket_work_merge", **payload)["bitbucket_merge"]
    assert recorded["observation"] == observation and recorded["closed_at"] is None
    request("operator", "operator", "cancel", 409, work_id=work_id, reason="fixture:observed merge awaits closure")
    request("operator", "operator", "request_repair", 409, **identity, generation=attempt["generation"],
            native_url=url, evidence="fixture:after-merge", note="A merged revision requires new work")
    assert request("verifier", "verifier", "pending_merge_checks", **checks)["merge_checks"]["items"] == []
    empty = subprocess.check_output(command, env=batch_env, text=True, timeout=15)
    assert json.loads(empty)["merge_check_report"]["items"] == []
    inbox = request("operations", "operator", "review_inbox", repository=repository, limit=1)["inbox"]
    [entry] = inbox["items"]
    assert entry["work_id"] == work_id and entry["next_action"] == "close_merged_work"
    assert inbox["counts"]["close_merged_work"] == 1
    assert entry["merge_closure"] == {"forge": "bitbucket", "publication_key": publication_key,
                                      "result_commit": commit}
    grant = "bitbucket-closure-inbox"
    request("operations", "operator", "grant_review_inbox", grant_id=grant, reviewer_id="reviewer",
            repository=repository, valid_seconds=120, evidence="fixture:bitbucket-review-visibility")
    reviewer = request("reviewer", "reviewer", "review_inbox", repository=repository, limit=1)["inbox"]
    [entry] = reviewer["items"]
    assert entry["work_id"] == work_id and entry["next_action"] == "inspect_draft"
    assert "close_merged_work" not in reviewer["counts"] and "merge_closure" not in entry
    request("operations", "operator", "revoke_review_inbox", grant_id=grant, evidence="fixture:visibility-checked")
    assert request("verifier", "verifier", "confirm_bitbucket_work_merge", **payload)["bitbucket_merge"] == recorded
    request("verifier", "verifier", "confirm_bitbucket_work_merge", 409,
            **(payload | {"observation": observation | {"actor": "bitbucket:user:changed"}}))
    revocation = {"location": skill_location, "revision": skill_revision,
                  "evidence": "fixture:revoke-after-bitbucket-merge"}
    revoked = request("operations", "operator", "withdraw_team_skill", **revocation)["release"]
    assert revoked["revoked_by"] == "operator" and revoked["revoked_at"]
    assert request("operations", "operator", "withdraw_team_skill", **revocation)["release"] == revoked
    assert request("operator", "operator", "status", work_id=work_id)["work"]["bitbucket_merge"] == recorded
    assert relay(reassigned=True)["decision"]["state"] == "ineligible"
    held = request("operator", "operator", "status", work_id=work_id)["work"]
    assert held["state"] == "candidate" and held["bitbucket_merge"] == recorded
    assert held["standing_delegation"]["blocked_at"]
    failed = request("verifier", "verifier", "record_verification", **(verification | {
        "previous_sequence": sequence, "passed": False, "evidence": "fixture:bitbucket-fail",
    }))["verification_sequence"]
    request("operations", "operator", "close_bitbucket_work", 409, **closure)
    refreshed = request("verifier", "verifier", "record_verification", **(verification | {
        "previous_sequence": failed, "evidence": "fixture:bitbucket-pass-again",
    }))["verification_sequence"]
    request("operations", "verifier", "close_bitbucket_work", 403, **closure)
    reserved = request("operations", "operator", "summary")["summary"]["review_reserved"]
    operator_env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    brief = subprocess.check_output(cli + ["review-inbox", "--brief", "--limit", "1"],
                                    env=operator_env, text=True, timeout=15)
    assert f"Close merged work | {work_id} | {repository}" in brief
    assert f"Merge result  {commit}" in brief and f"Publication key  {publication_key}" in brief
    output = subprocess.check_output(
        cli + ["close-bitbucket-work", work_id, "--publication-key", publication_key, "--head", head,
               "--evidence", closure["evidence"]],
        env=operator_env, text=True, timeout=15,
    )
    closed = json.loads(output)["closure"]
    assert closed["closure_verification_sequence"] == refreshed and closed["observation"] == observation
    assert request("operations", "operator", "summary")["summary"]["review_reserved"] == reserved - 1
    inbox = request("operations", "operator", "review_inbox", repository=repository)["inbox"]
    assert "close_merged_work" not in inbox["counts"] and all(item["work_id"] != work_id for item in inbox["items"])
    assert request("operations", "operator", "close_bitbucket_work", **closure)["closure"] == closed
    request("operations", "operator", "close_bitbucket_work", 409,
            **(closure | {"evidence": "fixture:conflicting-closure"}))
    request("operator", "operator", "cancel", 409, work_id=work_id, reason="already merged")
    audited = request("operations", "operator", "audit", work_id=work_id)["audit"]["work"]
    assert audited["state"] == "merged" and audited["bitbucket_merge"] == closed
    assert audited["standing_delegation"]["issue_key"] == issue
    assert audited["standing_delegation"]["blocked_at"] == held["standing_delegation"]["blocked_at"]
    output = subprocess.check_output(
        cli + ["confirm-bitbucket-work-merge", work_id, "--head", head, "--policy", "unneeded-on-replay.json"],
        env=verifier_env, text=True, timeout=15,
    )
    assert json.loads(output)["bitbucket_merge"] == closed
    print("Verified Jira intake, Bitbucket merge binding, reassignment after merge, operator closure, and replay.")
    return work_id


def exercise_knowledge_proposals(root, template, request, launcher, address, certificate, tokens):
    repo = "team/service"
    order = template | {"id": "knowledge-source"}
    request("operator", "operator", "admit", repository=repo, event_key=order["id"], work_order=order, token_limit=40)
    claim = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
    source = {"work_id": order["id"], "generation": claim["generation"], "head": CANDIDATE}
    assert claim["work_id"] == order["id"]
    data = {"kind": "runbook", "location": "project:fixture-troubleshooting",
            "body": "Check the clock on the fixture before comparing timestamps.", "evidence": "fixture:checked timestamps"}
    proposal = source | {"proposal_id": "fixture-lesson", "proposal": data}
    request("runner", "first", "propose_knowledge", 409, **proposal)
    pack = b"PACK\x00"
    request("runner", "first", "submit_pack", work_id=order["id"], generation=source["generation"], commit=CANDIDATE,
            pack_base64=base64.b64encode(pack).decode(), tokens=12)
    request("runner", "second", "propose_knowledge", 409, **proposal)
    request("runner", "first", "propose_knowledge", 409, **(proposal | {"head": BASE}))
    path = root / "knowledge-proposal.json"
    path.write_text(json.dumps(data), encoding="utf-8")
    cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
    result = subprocess.run(cli + ["propose-knowledge", proposal["proposal_id"],
                                  "--work-id", order["id"], "--generation", str(source["generation"]),
                                  "--head", CANDIDATE, "--proposal", str(path)],
                            capture_output=True, timeout=15, text=True,
                            env=os.environ | {"CRAFT_FACTORY_RUNNER_TOKEN": tokens["first"]})
    assert result.returncode == 0, result.stderr
    receipt = json.loads(result.stdout)["proposal"]
    assert request("runner", "first", "propose_knowledge", **proposal)["proposal"] == receipt
    request("runner", "first", "propose_knowledge", 409, **(proposal | {"proposal": data | {"body": "Changed"}}))
    skill = {"kind": "skill", "location": "team:fixture-derived", "body": "# Compare fixture timestamps",
             "evidence": "fixture:targeted test"}
    skill_receipt = request("runner", "first", "propose_knowledge", **source,
                            proposal_id="fixture-derived-skill", proposal=skill)["proposal"]
    discarded = request("runner", "first", "propose_knowledge", **source, proposal_id="fixture-discarded",
                        proposal=data | {"location": "project:fixture-discarded"})["proposal"]
    stale = request("runner", "first", "propose_knowledge", **source, proposal_id="fixture-stale",
                    proposal=data | {"location": "project:fixture-stale"})["proposal"]
    request("runner", "first", "propose_knowledge", 409, **source, proposal_id="fixture-over-cap",
            proposal=data | {"location": "project:fixture-over-cap"})
    pending = request("operations", "operator", "pending_knowledge", repository=repo)["pending"]
    assert [item["proposal_id"] for item in pending] == [
        "fixture-lesson", "fixture-derived-skill", "fixture-discarded", "fixture-stale",
    ]
    assert data["body"] not in json.dumps(pending)
    assert request("operations", "operator", "pending_knowledge", repository="team/other")["pending"] == []
    request("operations", "first", "inspect_knowledge", 403, proposal_id=receipt["proposal_id"])
    inspected = request("operations", "operator", "inspect_knowledge", proposal_id=receipt["proposal_id"])["proposal"]
    assert inspected["proposal"] == data and inspected["decision"] is None

    review = {"proposal_id": receipt["proposal_id"], "proposal_digest": receipt["proposal_digest"],
              "owner": "Fixture owners", "evidence": "fixture:owner approved", "promote": True}
    request("operations", "first", "review_knowledge", 403, verification_sequence=1, **review)
    request("operations", "operator", "review_knowledge", 409, verification_sequence=1, **review)
    verification = source | {"digest": "sha256:" + sha256(pack).hexdigest(), "note": "fixture checks"}
    passed = request("verifier", "verifier", "record_verification", **verification, previous_sequence=0,
                     passed=True, evidence="fixture:knowledge-check")["verification_sequence"]
    failed = request("verifier", "verifier", "record_verification", **verification, previous_sequence=passed,
                     passed=False, evidence="fixture:knowledge-failure")["verification_sequence"]
    for sequence in (passed, failed):
        request("operations", "operator", "review_knowledge", 409, verification_sequence=sequence, **review)
    fresh = request("verifier", "verifier", "record_verification", **verification, previous_sequence=failed,
                    passed=True, evidence="fixture:knowledge-recheck")["verification_sequence"]
    rejection = review | {"proposal_id": discarded["proposal_id"], "proposal_digest": discarded["proposal_digest"],
                          "promote": False, "evidence": "fixture:owner declined"}
    assert request("operations", "operator", "review_knowledge", **rejection)["decision"]["promoted"] is False
    request("operations", "operator", "review_knowledge", 409, verification_sequence=fresh,
            **(rejection | {"promote": True}))
    request("operations", "operator", "review_knowledge", 409, verification_sequence=fresh,
            **(review | {"proposal_digest": "sha256:" + "0" * 64}))
    request("operations", "operator", "review_knowledge", 400, **rejection, repositories=[repo])
    request("operations", "operator", "review_knowledge", 400, verification_sequence=fresh,
            **review, repositories=[repo, repo])
    request("operations", "operator", "review_knowledge", 409, verification_sequence=fresh,
            **review, repositories=["team/other"])
    sql("""
        INSERT INTO craft_queue.project_context_releases (
            repository, location, revision, excerpt, kind, source_reference, source_revision, approval_reference
        ) VALUES ('team/z-conflict', 'project:fixture-troubleshooting',
            craft_queue.project_context_revision('runbook', 'fixture:source', 'r1', 'fixture:approved', 'Existing'),
            'Existing', 'runbook', 'fixture:source', 'r1', 'fixture:approved');
    """)
    request("operations", "operator", "review_knowledge", 409, verification_sequence=fresh,
            **review, repositories=[repo, "team/other", "team/z-conflict"])
    assert request("operations", "operator", "inspect_knowledge",
                   proposal_id=receipt["proposal_id"])["proposal"]["decision"] is None
    for target in (repo, "team/other"):
        assert request("operations", "operator", "inspect_project_context", repository=target,
                       location=data["location"])["catalog"]["total"] == 0
    destinations = sorted(["team/other", repo, "team/Registry.v2"])
    promotion = request("operations", "operator", "review_knowledge", verification_sequence=fresh,
                        **review, repositories=destinations)["decision"]
    assert promotion["owner"] == review["owner"] and promotion["recorded_by"] == "operator"
    assert promotion["repositories"] == destinations
    assert request("operations", "operator", "review_knowledge", verification_sequence=fresh,
                   **review, repositories=list(reversed(destinations)))["decision"] == promotion
    request("operations", "operator", "review_knowledge", 409, verification_sequence=fresh,
            **review, repositories=[*destinations, "team/outside"])
    result = subprocess.run(cli + ["review-knowledge", skill_receipt["proposal_id"], "--proposal-digest",
                                  skill_receipt["proposal_digest"], "--owner", review["owner"], "--decision", "promote",
                                  "--evidence", review["evidence"], "--verification-sequence", str(fresh),
                                  "--repository", repo, "--repository", "team/other",
                                  "--repository", "team/Registry.v2"],
                            capture_output=True, timeout=15, text=True,
                            env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]})
    assert result.returncode == 0, result.stderr
    skill_decision = json.loads(result.stdout)["decision"]
    assert skill_decision["repositories"] == destinations
    skill_revision = skill_decision["release_revision"]
    request("operator", "operator", "cancel", work_id=order["id"], reason="knowledge proposal source checked")
    stale_review = review | {"proposal_id": stale["proposal_id"], "proposal_digest": stale["proposal_digest"]}
    request("operations", "operator", "review_knowledge", 409, verification_sequence=fresh, **stale_review)
    request("operations", "operator", "review_knowledge", **(stale_review | {"promote": False}))
    assert request("operations", "operator", "pending_knowledge", repository=repo)["pending"] == []
    consumer = template | {"id": "knowledge-consumer",
                           "context": [{"location": data["location"], "revision": promotion["release_revision"]}],
                           "skill": {"location": skill["location"], "revision": skill_revision}}
    request("operator", "operator", "admit", 409, repository="team/outside", event_key=consumer["id"],
            work_order=consumer, token_limit=40)
    request("operator", "operator", "admit", repository=repo, event_key=consumer["id"], work_order=consumer, token_limit=40)
    claim = request("runner", "first", "claim", request_id=str(uuid4()), context_capable=True)["claim"]
    assert claim["work_id"] == consumer["id"]
    lease = {"work_id": claim["work_id"], "generation": claim["generation"]}
    [context] = request("runner", "first", "project_context", **lease)["context"]
    assert context["excerpt"] == data["body"] and context["source_revision"] == CANDIDATE
    skill_response = request("runner", "first", "team_skill", **lease)["skill"]
    assert skill_response["markdown"] == skill["body"]
    with block_release_context(consumer["id"]):
        assert request("runner", "first", "project_context", 409, **lease)["error"] == "project_context_unavailable"
        assert request("runner", "first", "team_skill", 409, **lease)["error"] == "team_skill_unavailable"
    assert request("runner", "first", "project_context", **lease)["context"] == [context]
    assert request("runner", "first", "team_skill", **lease)["skill"] == skill_response
    request("operator", "operator", "cancel", work_id=consumer["id"], reason="promoted knowledge consumed")
    downstream = consumer | {"id": "knowledge-downstream"}
    request("operator", "operator", "admit", repository="team/other", event_key=downstream["id"],
            work_order=downstream, token_limit=40)
    claim = request("runner", "outside", "claim", request_id=str(uuid4()), context_capable=True)["claim"]
    assert claim["work_id"] == downstream["id"]
    lease = {"work_id": claim["work_id"], "generation": claim["generation"]}
    [received] = request("runner", "outside", "project_context", **lease)["context"]
    assert received["revision"] == context["revision"] and received["excerpt"] == context["excerpt"]
    assert received["source_reference"] == context["source_reference"] and received["approval_reference"] == review["evidence"]
    assert request("runner", "outside", "team_skill", **lease)["skill"]["markdown"] == skill["body"]
    request("runner", "outside", "submit_pack", **lease, commit=CANDIDATE,
            pack_base64=base64.b64encode(pack).decode(), tokens=0)
    verified = request("verifier", "verifier", "record_verification", **lease, head=CANDIDATE,
                       digest="sha256:" + sha256(pack).hexdigest(), previous_sequence=0, passed=True,
                       evidence="fixture:downstream checks", note="Fixture check")["verification_sequence"]
    reserved = request("publication", "operator", "reserve", **lease, head=CANDIDATE,
                       verification_sequence=verified)["publication"]
    scope = {"proposal_id": receipt["proposal_id"], "release_revision": promotion["release_revision"],
             "evidence": "fixture:withdraw promoted context"}
    request("operations", "first", "withdraw_knowledge", 403, **scope)
    request("operations", "operator", "withdraw_knowledge", 409,
            **(scope | {"release_revision": "sha256:" + "0" * 64}))
    request("operations", "operator", "withdraw_knowledge", 409,
            **(scope | {"proposal_id": rejection["proposal_id"]}))
    registry = "team/Registry.v2"
    previous = request("operations", "operator", "withdraw_project_context",
                       repository=registry, location=data["location"], revision=promotion["release_revision"],
                       evidence="fixture:older context replaced")["release"]
    sql("""
        INSERT INTO craft_queue.project_context_releases (
            repository, location, revision, excerpt, kind, source_reference, source_revision, approval_reference
        ) VALUES ('team/Registry.v2', 'project:fixture-troubleshooting',
            craft_queue.project_context_revision('runbook', 'fixture:replacement', 'r2', 'fixture:approved', 'Newer'),
            'Newer', 'runbook', 'fixture:replacement', 'r2', 'fixture:approved');
    """)
    waiting = consumer | {"id": "knowledge-withdrawal-waiting"}
    request("operator", "operator", "admit", repository=repo, event_key=waiting["id"],
            work_order=waiting, token_limit=40)
    result = subprocess.run(
        cli + ["withdraw-knowledge", scope["proposal_id"], "--release-revision", scope["release_revision"],
               "--evidence", scope["evidence"]], capture_output=True, text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
    )
    assert result.returncode == 0, result.stderr
    withdrawn = json.loads(result.stdout)["withdrawal"]
    assert withdrawn["retired_now"] == ["team/other", repo] and withdrawn["already_retired"] == [registry]
    assert request("operations", "operator", "withdraw_knowledge", **scope)["withdrawal"] == withdrawn
    request("operations", "operator", "withdraw_knowledge", 409, **(scope | {"evidence": "fixture:other decision"}))
    audit = request("operations", "operator", "inspect_knowledge", proposal_id=scope["proposal_id"])["proposal"]
    assert audit["withdrawal"] == withdrawn and audit["decision"] == promotion
    assert request("operations", "operator", "inspect_project_context", repository=registry,
                   location=data["location"], revision=scope["release_revision"])["catalog"]["items"] == [previous]
    replacement = request("operations", "operator", "inspect_project_context", repository=registry,
                          location=data["location"])["catalog"]["items"]
    assert replacement[0]["source_reference"] == "fixture:replacement"
    assert request("operator", "operator", "status", work_id=waiting["id"])["work"]["state"] == "cancelled"
    key = reserved["action_key"]
    assert request("publication", "operator", "status", action_key=key)["publication"]["state"] == "reserved"
    request("publisher", "publisher", "begin", 409, action_key=key)
    request("publication", "operator", "reconcile", action_key=key, observation={
        "outcome": "absent", "head": CANDIDATE, "evidence": "fixture:reserved send never began",
    })
    request("operator", "operator", "cancel", work_id=downstream["id"], reason="approved downstream knowledge consumed")

    skill_only = consumer | {"id": "knowledge-skill-only", "context": []}
    request("operator", "operator", "admit", repository=repo, event_key=skill_only["id"],
            work_order=skill_only, token_limit=40)
    skill_withdrawal = request("operations", "operator", "withdraw_knowledge",
        proposal_id=skill_receipt["proposal_id"], release_revision=skill_revision,
        evidence="fixture:withdraw promoted skill")["withdrawal"]
    assert skill_withdrawal["retired_now"] == destinations and not skill_withdrawal["already_retired"]
    assert request("operator", "operator", "status", work_id=skill_only["id"])["work"]["state"] == "cancelled"
    print("Verified atomic knowledge promotion and withdrawal, scoped use, and publication reconciliation.")


def exercise_worker_knowledge(root, template, request, launch, finish):
    pending = request("operations", "operator", "pending_knowledge", repository="team/service")["pending"]
    for suffix, option, intent in (
        ("disabled", False, "Return a knowledge proposal"),
        ("malformed", True, "Return an invalid knowledge proposal"),
        ("enabled", True, "Return a knowledge proposal"),
    ):
        order = template | {"id": f"worker-knowledge-{suffix}", "intent": intent}
        request("operator", "operator", "admit", repository="team/service", event_key=order["id"],
                work_order=order, token_limit=40)
        request_id = str(uuid4())
        destination = root / order["id"]
        result = finish(launch("first", destination, request_id, collect_knowledge=option))
        assert result["work_id"] == order["id"] and result["candidate"]
        with closing(sqlite3.connect(destination / "state.db")) as db:
            note, = db.execute("SELECT note FROM events WHERE kind = 'runner_observed'").fetchone()
            ignored, = db.execute("SELECT count(*) FROM events WHERE kind = 'knowledge_proposal_ignored'").fetchone()
        assert "knowledge_proposal" not in note
        assert ignored == int(suffix == "malformed")
        if suffix == "enabled":
            proposal = result["knowledge_proposal"]
            assert proposal["state"] == "recorded" and proposal["id"].startswith("auto-")
            # The journal holds the same proposal even when replay is invoked without the collection flag.
            assert finish(launch("first", destination, request_id)) == result
            journal = destination / "submission.json"
            original = journal.read_bytes()
            changed = json.loads(original)
            changed["knowledge_proposal"]["body"] = "Conflicting replay content"
            journal.write_text(json.dumps(changed), encoding="utf-8")
            retried = finish(launch("first", destination, request_id))
            assert retried["candidate"] == result["candidate"]
            assert retried["knowledge_proposal"] == {"id": proposal["id"], "state": "rejected", "http_status": 409}
            journal.write_bytes(original)
            inspected = request("operations", "operator", "inspect_knowledge", proposal_id=proposal["id"])["proposal"]
            assert inspected["proposal_digest"] == proposal["digest"] and inspected["decision"] is None
            assert inspected["head"] == result["candidate"] and inspected["pack_digest"] == result["candidate_evidence"]
            assert inspected["proposal"]["body"] == "# Compare fixture timestamps"
            assert sql("SELECT count(*) FROM craft_queue.team_skill_releases "
                       "WHERE location = 'team:worker-fixture-skill';") == "0"
            request("operations", "operator", "review_knowledge", proposal_id=proposal["id"],
                    proposal_digest=proposal["digest"], promote=False, owner="Fixture owners",
                    evidence="fixture:review completed")
        else:
            assert "knowledge_proposal" not in result
        assert request("operations", "operator", "pending_knowledge", repository="team/service")["pending"] == pending
        request("operator", "operator", "cancel", work_id=order["id"], reason="worker knowledge checked")
    print("Verified supervisor opt-in, invalid suggestion isolation, candidate-bound proposal, and submission replay.")


def exercise_shadow_receipts(root, repo, template, handoff, handoff_path, request, launcher, address, certificate, tokens):
    policy = {
        "schema_version": 1, "repository": "team/service", "base_branch": "main",
        "protection_revision": "fixture:restrictions", "observation_issuer": "fixture:forge",
        "approval_reference": "fixture:approval", "allowed_paths": ["src"], "protected_paths": [],
        "max_changed_paths": 2, "max_snapshot_age_seconds": 120, "max_review_age_seconds": 600,
        "minimum_approvals": 1, "eligible_reviewers": ["reviewer"],
        "required_checks": [{"name": "Verify", "issuer": "fixture:checks"}],
    }
    policy_path = root / "shadow-policy.json"
    policy_path.write_bytes(json.dumps(policy).encode())
    digest = f"sha256:{sha256(policy_path.read_bytes()).hexdigest()}"
    approve = {"repository": "team/service", "policy_digest": digest, "require_native": False,
               "evidence": "fixture:shadow-policy-approval", "valid_seconds": 300}
    request("operations", "first", "approve_shadow_policy", 403, **approve)
    pin = request("operations", "operator", "approve_shadow_policy", **approve)["policy"]
    assert request("operations", "operator", "approve_shadow_policy", **approve)["policy"] == pin
    request("operations", "operator", "approve_shadow_policy", 409, **(approve | {"require_native": True}))
    strict_digest = f"sha256:{'8' * 64}"
    request("operations", "operator", "approve_shadow_policy", **(approve | {
        "policy_digest": strict_digest, "require_native": True,
    }))

    subprocess.run(["git", "-C", str(repo), "remote", "add", "origin", "https://bitbucket.org/team/service"],
                   check=True, capture_output=True, timeout=10)
    work_path, observation_path = root / "shadow-order.json", root / "shadow-observation.json"
    work_path.write_text(json.dumps(template | {"repository": str(repo)}), encoding="utf-8")
    head, now = handoff["head"], int(time())
    observation = {
        "schema_version": 1, "issuer": "fixture:forge", "evidence": "fixture:shadow-101",
        "policy_digest": digest, "repository": "team/service", "base_branch": "main",
        "draft_url": "https://bitbucket.org/team/service/pull-requests/101",
        "base": handoff["base"], "head": head, "author": "author",
        "open": True, "draft": False, "mergeable": True, "protected": True,
        "protection_revision": "fixture:restrictions", "observed_at": now,
        "checks": [{"name": "Verify", "issuer": "fixture:checks", "head": head, "passed": True,
                    "evidence": "fixture:successful-ci"}],
        "reviews": [{"actor": "reviewer", "head": head, "decision": "approved", "decided_at": now,
                     "evidence": "fixture:head-approval"}],
    }
    shared_ops = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
    env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}

    def record(name, evidence):
        observation_path.write_text(json.dumps(evidence), encoding="utf-8")
        result = subprocess.run([str(launcher.resolve()), "shadow-merge", str(work_path),
                                 "--head", head, "--policy", str(policy_path), "--approved-policy-digest", digest,
                                 "--observation", str(observation_path), "--as-of", str(now)],
                                capture_output=True, timeout=15)
        assert result.returncode == 0, result.stderr
        report_path = root / f"{name}.json"
        report_path.write_bytes(result.stdout)
        received = subprocess.run(shared_ops + ["record-shadow-merge", name, "--handoff",
                                  str(handoff_path), "--report", str(report_path)],
                                  capture_output=True, timeout=15, env=env)
        assert received.returncode == 0, received.stderr
        receipt = json.loads(received.stdout)["receipt"]
        assert receipt["report_digest"] == f"sha256:{sha256(result.stdout).hexdigest()}"
        return receipt

    positive = record("shadow-positive", observation)
    blocked = record("shadow-blocked", observation | {"checks": []})
    assert positive["assessment"]["eligible"] and not positive["assessment"]["native_verified"]
    assert blocked["assessment"]["blockers"] == ["required_checks_missing_or_failed"]
    binding = {
        "receipt_id": positive["id"], "report_digest": positive["report_digest"], "work_id": handoff["work_id"],
        "generation": handoff["generation"], "head": head, "pack_digest": handoff["pack_digest"],
        "verification_sequence": handoff["verification_sequence"], "assessment": positive["assessment"],
    }
    request("operations", "reviewer", "record_shadow_merge", 403, **binding)
    assert request("operations", "operator", "record_shadow_merge", **binding)["receipt"] == positive
    request("operations", "operator", "record_shadow_merge", 409, **(binding | {"receipt_id": "duplicate-evaluation"}))
    request("operations", "operator", "record_shadow_merge", 409, **(binding | {
        "receipt_id": "strict-receipt", "assessment": positive["assessment"] | {"policy_digest": strict_digest},
    }))
    request("operations", "operator", "record_shadow_merge", 409, **(binding | {
        "receipt_id": "wrong-verification", "verification_sequence": binding["verification_sequence"] - 1,
    }))
    summary = request("operations", "operator", "shadow_merge_summary", repository="team/service", since=now-1)["summary"]
    assert summary["total"] == 2 and summary["eligible"] == 1 and summary["native_verified"] == 0
    assert summary["blockers"] == {"required_checks_missing_or_failed": 1}
    audit = request("operations", "operator", "audit", work_id=handoff["work_id"])["audit"]
    assert {receipt["id"] for receipt in audit["shadow_merge_receipts"]} == {"shadow-positive", "shadow-blocked"}
    assert summary["qualification"] == [{
        "policy_digest": digest, "require_native": False, "total": 2, "unassessed": 2, "by_basis": {},
    }]

    qualified = {
        "receipt_id": positive["id"], "head": head, "report_digest": positive["report_digest"],
        "basis": "fixture", "expected_eligible": False,
        "evidence": "fixture:independent-shadow-comparison", "note": "The fixture policy requires blocking.",
    }
    evidence_bytes = b'{"fixture_verdict":"blocked"}'
    evidence_path = root / "shadow-verdict.json"
    evidence_path.write_bytes(evidence_bytes)
    qualified["evidence_digest"] = f"sha256:{sha256(evidence_bytes).hexdigest()}"
    request("verifier", "operator", "record_shadow_outcome", 403, **qualified)
    request("verifier", "first", "record_shadow_outcome", 403, **qualified)
    request("verifier", "verifier", "record_shadow_outcome", 409, **(qualified | {"head": handoff["base"]}))
    request("verifier", "verifier", "record_shadow_outcome", 409,
            **(qualified | {"report_digest": f"sha256:{'2' * 64}"}))
    outcome = subprocess.run(
        shared_ops + ["record-shadow-outcome", positive["id"], "--head", head, "--report-digest",
                      positive["report_digest"], "--basis", "fixture", "--expected", "blocked",
                      "--evidence-file", str(evidence_path), "--evidence", qualified["evidence"], "--note", qualified["note"]],
        capture_output=True, timeout=15, env=os.environ | {"CRAFT_FACTORY_VERIFIER_TOKEN": tokens["verifier"]},
    )
    assert outcome.returncode == 0, outcome.stderr
    outcome = json.loads(outcome.stdout)["outcome"]
    assert outcome["predicted_eligible"] and not outcome["expected_eligible"] and outcome["verifier"] == "verifier"
    assert outcome["evidence_digest"] == qualified["evidence_digest"]
    assert request("verifier", "verifier", "record_shadow_outcome", **qualified)["outcome"] == outcome
    request("verifier", "verifier", "record_shadow_outcome", 409, **(qualified | {"expected_eligible": True}))
    request("verifier", "verifier", "record_shadow_outcome", **(qualified | {
        "receipt_id": blocked["id"], "report_digest": blocked["report_digest"], "expected_eligible": True,
        "evidence": "fixture:opposite-mismatch", "note": "The fixture should be eligible.",
    }))
    observed = record("shadow-observed-sample", observation | {"evidence": "fixture:separate-observation"})
    request("verifier", "verifier", "record_shadow_outcome", **(qualified | {
        "receipt_id": observed["id"], "report_digest": observed["report_digest"], "expected_eligible": True,
        "basis": "forge_observed", "evidence": "fixture:simulated-observed-basis", "note": "Simulated forge evidence.",
    }))
    record("shadow-unassessed", observation | {"reviews": []})
    summary = request("operations", "operator", "shadow_merge_summary", repository="team/service", since=now-1)["summary"]
    [qualification] = summary["qualification"]
    assert qualification["total"] == 4 and qualification["unassessed"] == 1
    assert qualification["by_basis"] == {
        "fixture": {"correct_allow": 0, "correct_block": 0, "false_allow": 1, "false_block": 1},
        "forge_observed": {"correct_allow": 1, "correct_block": 0, "false_allow": 0, "false_block": 0},
    }
    audit = request("operations", "operator", "audit", work_id=handoff["work_id"])["audit"]
    assert outcome in audit["shadow_outcomes"] and len(audit["shadow_outcomes"]) == 3
    return binding


def exercise_scoped_review(order, candidate, pack, request, inbox_command, tokens):
    def inbox(reviewer="reviewer", **filters):
        return request("reviewer", reviewer, "review_inbox", **filters)["inbox"]

    empty = {"total": 0, "counts": {}, "items": [], "assignments": []}
    assert inbox() == empty
    request("reviewer", "operator", "review_inbox", 403)
    request("operations", "reviewer", "review_inbox", 403)
    request("operator", "reviewer", "download_pack", 403, work_id=order["id"])
    request("verifier", "reviewer", "status", 403, work_id=order["id"])

    foreign = order | {"id": "review-foreign", "intent": "private-foreign-review-intent"}
    sql("UPDATE craft_queue.control SET review_limit = 2;")
    request("operator", "operator", "admit", repository="team/other", event_key=foreign["id"],
            work_order=foreign, token_limit=40)
    lease = request("runner", "outside", "claim", request_id=str(uuid4()))["claim"]
    assert lease["work_id"] == foreign["id"]
    binding = {"work_id": foreign["id"], "generation": lease["generation"]}
    request("runner", "outside", "submit_pack", **binding, commit=candidate["candidate"],
            pack_base64=pack["pack_base64"], tokens=5)
    request("verifier", "verifier", "record_verification", **binding, head=candidate["candidate"],
            digest=candidate["candidate_evidence"], previous_sequence=0, passed=False,
            evidence="fixture:foreign-failure", note="Foreign fixture decision")

    approval = {"grant_id": "review-team-service", "reviewer_id": "reviewer", "repository": "team/service",
                "evidence": "fixture:review-scope", "valid_seconds": 3600}
    request("operations", "reviewer", "grant_review_inbox", 403, **approval)
    request("operations", "operator", "grant_review_inbox", 404, **(approval | {"reviewer_id": "first"}))
    grant = request("operations", "operator", "grant_review_inbox", **approval)["grant"]
    assert grant["approved_by"] == "operator"
    assert request("operations", "operator", "grant_review_inbox", **approval)["grant"] == grant
    request("operations", "operator", "grant_review_inbox", 409, **(approval | {"grant_id": "duplicate-scope"}))
    view = inbox(limit=1)
    assert view["total"] == 1 and view["counts"] == {"verify_candidate": 1}
    assert view["items"][0]["work_id"] == order["id"]
    assert "private-foreign-review-intent" not in json.dumps(view)
    assert inbox(repository="team/other") == empty and inbox("reviewer-other") == empty

    cli = subprocess.run(inbox_command + ["--reviewer"], capture_output=True, text=True, timeout=15,
                         env=os.environ | {"CRAFT_FACTORY_REVIEWER_TOKEN": tokens["reviewer"]})
    assert cli.returncode == 0, cli.stderr
    assert json.loads(cli.stdout)["inbox"] == view
    other_approval = approval | {"grant_id": "review-expiry", "repository": "team/other"}
    request("operations", "operator", "grant_review_inbox", **other_approval)
    assert inbox(limit=1)["items"][0]["work_id"] == foreign["id"]
    sql("UPDATE craft_queue.review_inbox_grants SET granted_at = clock_timestamp() - interval '2 hours', "
        "expires_at = clock_timestamp() - interval '1 hour' WHERE grant_id = 'review-expiry';")
    assert inbox(limit=1) == view
    revocation = {"grant_id": approval["grant_id"], "evidence": "fixture:scope-withdrawn"}
    revoked = request("operations", "operator", "revoke_review_inbox", **revocation)["grant"]
    assert revoked["revoked_by"] == "operator"
    assert request("operations", "operator", "revoke_review_inbox", **revocation)["grant"] == revoked
    assert request("operations", "operator", "grant_review_inbox", **approval)["grant"] == revoked
    assert inbox() == empty
    request("operator", "operator", "cancel", work_id=foreign["id"], reason="review scope verified")
    sql("UPDATE craft_queue.control SET review_limit = 1;")


def exercise_execution_profiles(root, template, request, launch, finish):
    routine = {
        "id": "routine", "model": "fixture-mini", "reasoning_effort": "low",
        "timeout_seconds": 8, "max_attempts": 1, "approval_reference": "fixture:profile-review/7",
    }
    routine_order = template | {
        "id": "profile-routine", "intent": "Routine profile fixture", "execution_profile": routine,
    }
    denied = request("intake", "intake", "observe", 403, rule="unused",
                     snapshot={"source": "jira:fixture", "issue": "FIXTURE-1", "source_revision": 1,
                               "observed_at": int(time()), "status": "ready",
                               "labels": [], "assignee": "Operator"}, work_order=routine_order)
    assert denied["error"] == "execution_profile_requires_operator_admission"
    for invalid in (
        {"model": "--sandbox"}, {"reasoning_effort": "fast"}, {"max_attempts": "1"},
        {"timeout_seconds": 3601}, {"approval_reference": " "}, {"extra": True},
    ):
        request("operator", "operator", "admit", 409, repository="team/service",
                event_key="profile:invalid",
                work_order=routine_order | {"execution_profile": routine | invalid}, token_limit=40)

    deep = routine | {
        "id": "deep", "model": "fixture-reasoning", "reasoning_effort": "high", "timeout_seconds": 35,
    }
    for order in (
        routine_order,
        template | {"id": "profile-deep", "intent": "Deep profile fixture", "execution_profile": deep},
    ):
        work_id, profile = order["id"], order["execution_profile"]
        request("operator", "operator", "admit", repository="team/service", event_key=work_id,
                work_order=order, token_limit=40)
        request_id = str(uuid4())
        assert request("runner", "first", "claim", request_id=request_id, context_capable=True)["claim"] is None
        destination = root / work_id
        candidate = finish(launch("first", destination, request_id))
        assert candidate["work_id"] == work_id and candidate["generation"] == 1
        with closing(sqlite3.connect(destination / "state.db")) as db:
            note, = db.execute("SELECT note FROM events WHERE kind = 'runner_profile_selected'").fetchone()
        observed = json.loads(note)
        assert observed == {
            "requested_profile": profile, "effective_timeout_seconds": min(profile["timeout_seconds"], 18),
        }
        work = request("operator", "operator", "status", work_id=work_id)["work"]
        assert work["execution_profile"] == profile
        assert work["profile_started_at"]
        assert work["attempt_limit_reached"] and not work["attempt_limit_blocks_pickup"]
        request("operator", "operator", "cancel", work_id=work_id, reason="profile fixture verified")

    capped = routine_order | {"id": "profile-capped"}
    request("operator", "operator", "admit", repository="team/service", event_key=capped["id"],
            work_order=capped, token_limit=40)
    request_id = str(uuid4())
    lease = request("runner", "first", "claim", request_id=request_id, profile_capable=True)["claim"]
    assert lease["work_id"] == capped["id"] and lease["generation"] == 1
    assert request("runner", "first", "claim", request_id=request_id, profile_capable=True)["claim"] == lease
    assert request("runner", "first", "claim", request_id=request_id)["claim"] is None
    binding = {"work_id": capped["id"], "generation": lease["generation"]}
    request("runner", "first", "report_failed_attempt_usage", 409, tokens=30, **binding)
    request("runner", "second", "start_profiled_attempt", 409, **binding)
    request("runner", "first", "start_profiled_attempt", **binding)
    request("runner", "first", "report_failed_attempt_usage", 409, tokens=41, **binding)
    request("runner", "first", "start_profiled_attempt", 409, **binding)
    escalation = binding | {
        "replacement_id": "profile-escalated", "execution_profile": deep,
        "evidence": "fixture:approved-profile-after-cap",
    }
    request("operations", "first", "escalate_profile", 403, **escalation)
    request("operations", "operator", "escalate_profile", 409, **escalation)
    sql("UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second' "
        "WHERE work_id = 'profile-capped';")
    assert request("runner", "second", "claim", request_id=str(uuid4()), profile_capable=True)["claim"] is None
    work = request("operator", "operator", "status", work_id=capped["id"])["work"]
    assert work["attempt_limit_reached"] and work["attempt_limit_blocks_pickup"]
    dispatch = request("operations", "operator", "dispatch_status", work_id=capped["id"])["dispatch"]
    assert dispatch["requires_execution_profile"] and "attempt_limit_reached" in dispatch["blockers"]
    request("operations", "operator", "escalate_profile", 409, **(escalation | {"generation": 2}))
    request("operations", "operator", "escalate_profile", 409,
            **(escalation | {"execution_profile": routine | {"max_attempts": 2}}))
    def approve():
        return request("operations", "operator", "escalate_profile", **escalation)["escalation"]

    with ThreadPoolExecutor(max_workers=2) as pool:
        first, second = [pool.submit(approve) for _ in range(2)]
        receipt = first.result()
        assert second.result() == receipt
    assert receipt["actor"] == "operator" and receipt["evidence"] == escalation["evidence"]
    assert request("operations", "operator", "escalate_profile", **escalation)["escalation"] == receipt
    request("operations", "operator", "escalate_profile", 409,
            **(escalation | {"replacement_id": "profile-escalation-duplicate"}))
    audit = request("operations", "operator", "audit", work_id=capped["id"])["audit"]
    assert audit["work"]["state"] == "cancelled"
    assert audit["attempts"][0]["outcome"] == "expired" and audit["attempts"][0]["token_limit"] == 40
    assert audit["profile_escalation"] == receipt
    replacement_id = escalation["replacement_id"]
    assert request("operations", "operator", "audit", work_id=replacement_id)["audit"]["profile_escalation"] == receipt
    request("operations", "operator", "compare_profiles", 409,
            pairs=[{"case_id": "superseded", "baseline": capped["id"], "challenger": replacement_id}])
    next_lease = request("runner", "second", "claim", request_id=str(uuid4()), profile_capable=True)["claim"]
    assert next_lease["work_id"] == replacement_id and next_lease["generation"] == 1
    assert next_lease["work_order"] == lease["work_order"] | {"id": replacement_id, "execution_profile": deep}
    request("runner", "first", "renew", 409, **binding)
    sql("UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second' "
        "WHERE work_id = 'profile-escalated';")
    request("operations", "operator", "escalate_profile", 409, **(escalation | {
        "work_id": replacement_id, "replacement_id": "profile-escalation-chain", "execution_profile": routine,
    }))
    request("operator", "operator", "cancel", work_id=replacement_id, reason="fixture escalation verified")

    invalid_result = routine_order | {
        "id": "profile-invalid-result", "intent": "Routine profile fixture. Return an invalid final result",
    }
    request("operator", "operator", "admit", repository="team/service", event_key=invalid_result["id"],
            work_order=invalid_result, token_limit=40)
    destination = root / invalid_result["id"]
    with launch("first", destination, str(uuid4())) as process:
        _, stderr = process.communicate(timeout=12)
        assert process.returncode != 0 and "valid final candidate or blocker result" in stderr
    assert not (destination / "submission.json").exists()
    failed = {"work_id": invalid_result["id"], "generation": 1}
    request("runner", "first", "renew", 409, **failed)
    request("runner", "second", "report_failed_attempt_usage", 409, tokens=30, **failed)
    request("runner", "first", "report_failed_attempt_usage", 409, tokens=29, **failed)
    assert request("runner", "first", "report_failed_attempt_usage", tokens=30, **failed)["recorded"]
    usage = request("operations", "operator", "profile_usage", work_id=invalid_result["id"])["usage"]
    assert usage["totals"]["reported_tokens"] == 30 and usage["totals"]["reserved_tokens_without_usage"] == 0
    assert usage["orders"][0]["independent_result"] == "unscored"
    request("operator", "operator", "cancel", work_id=invalid_result["id"], reason="fixture usage verified")

    timeout = routine_order | {
        "id": "profile-timeout", "intent": "Routine profile fixture. Wait for timeout",
        "execution_profile": routine | {"timeout_seconds": 1},
    }
    request("operator", "operator", "admit", repository="team/service", event_key=timeout["id"],
            work_order=timeout, token_limit=40)
    destination = root / timeout["id"]
    with launch("first", destination, str(uuid4())) as process:
        _, stderr = process.communicate(timeout=12)
        assert process.returncode != 0 and "supervised worker timed out" in stderr
    assert not (destination / "submission.json").exists()
    usage = request("operations", "operator", "profile_usage", work_id=timeout["id"])["usage"]
    assert usage["totals"]["reported_tokens"] == 0 and usage["totals"]["reserved_tokens_without_usage"] == 40
    request("operator", "operator", "cancel", work_id=timeout["id"], reason="profile timeout verified")


def exercise_queue_usage(address, certificate, tokens, request, launcher):
    filters = {"repository": "team/service", "since_hours": 24, "limit": 100}
    request("operations", "monitor", "queue_usage", 403, **filters)
    request("operations", "first", "queue_usage", 403, **filters)
    request("operations", "operator", "queue_usage", 400, **(filters | {"since_hours": 721}))
    request("operations", "operator", "queue_usage", 400, **(filters | {"limit": 0}))
    request("operations", "operator", "queue_usage", 400, **(filters | {"repository": "invalid"}))

    def report(**overrides):
        return request("operations", "operator", "queue_usage", **(filters | overrides))["usage"]

    def model_group(usage, name):
        return next(row for row in usage["groups"] if row["model"] == name)

    scoped = report()
    routine = model_group(scoped, "fixture-mini")
    assert routine == {
        "repository": "team/service", "adapter": "codex", "model": "fixture-mini", "reasoning_effort": "low",
        "attempts": 4, "supervised_starts": 4, "candidate_attempts": 1,
        "attempts_with_usage": 2, "reported_tokens": 60, "reserved_tokens_without_usage": 80,
        "overrun_attempts": 0, "tokens_above_reservation": 0,
    }
    deep = model_group(scoped, "fixture-reasoning")
    assert deep["attempts"] == 2 and deep["reasoning_effort"] == "high"
    assert deep["reported_tokens"] == 30 and deep["reserved_tokens_without_usage"] == 40
    assert any(row["model"] is None and row["reasoning_effort"] is None for row in scoped["groups"])
    across = report(repository=None)
    assert across["totals"]["attempts"] > scoped["totals"]["attempts"]
    assert report(repository="team/other")["totals"]["reported_tokens"] >= 5
    assert any(row["repository"] == "team/other" for row in across["groups"])
    assert all(row["repository"] == "team/service" for row in scoped["groups"])

    command = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
               "queue-usage", "--repository", "team/service", "--since-hours", "24", "--limit", "1"]
    limited = json.loads(subprocess.check_output(command, env=os.environ | {
        "CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, text=True, timeout=15))["usage"]
    assert limited["groups"] == scoped["groups"][:1]
    assert limited["total_groups"] == scoped["total_groups"] and limited["totals"] == scoped["totals"]
    empty = report(repository="team/no-usage")
    assert empty["groups"] == [] and empty["total_groups"] == 0 and not any(empty["totals"].values())

    sql("UPDATE craft_queue.attempts SET started_at = started_at - interval '25 hours' "
        "WHERE work_id = 'profile-timeout';")
    narrower = model_group(report(), "fixture-mini")
    assert narrower["attempts"] == 3 and narrower["reserved_tokens_without_usage"] == 40
    assert model_group(report(since_hours=48), "fixture-mini") == routine
    sql("UPDATE craft_queue.attempts SET started_at = started_at + interval '25 hours' "
        "WHERE work_id = 'profile-timeout';")
    print("Verified queue usage windows, profile grouping, unknown reservations, and bounded group output.")


def exercise_worker_stop(root, template, request, launch):
    work_id = "launcher-stop-receipt"
    order = template | {"id": work_id, "intent": "Wait for timeout"}
    request("operator", "operator", "admit", repository="team/service", event_key=work_id,
            work_order=order, token_limit=5)
    request_id = str(uuid4())
    first = request("runner", "first", "claim", request_id=request_id)["claim"]
    assert first["work_id"] == work_id and first["generation"] == 1
    binding = {"work_id": work_id, "generation": 1, "tokens": None}
    request("runner", "first", "finish_failed_attempt", 409, **binding)
    with launch("first", root / work_id, request_id, timeout_seconds=1) as process:
        _, stderr = process.communicate(timeout=12)
        assert process.returncode != 0 and "supervised worker timed out" in stderr
    [stopped] = request("operations", "operator", "audit", work_id=work_id)["audit"]["attempts"]
    assert stopped["worker_stop_reported_at"] == stopped["expires_at"]
    assert stopped["worker_stop_reported_at"] and stopped["reported_tokens"] is None and stopped["token_limit"] == 5
    assert request("runner", "first", "finish_failed_attempt", **binding)["recorded"]
    request("runner", "second", "finish_failed_attempt", 409, **binding)
    request("runner", "first", "finish_failed_attempt", 409, **(binding | {"tokens": 1}))
    replacement = request("runner", "second", "claim", request_id=str(uuid4()))["claim"]
    assert replacement["work_id"] == work_id and replacement["generation"] == 2
    assert request("runner", "first", "finish_failed_attempt", **binding)["recorded"]
    assert request("operations", "operator", "dispatch_status", work_id=work_id)["dispatch"]["live_lease"]
    request("operator", "operator", "cancel", work_id=work_id, reason="verified stop receipt and takeover")
    sql("""BEGIN;
           DELETE FROM craft_queue.attempts WHERE work_id = 'launcher-stop-receipt';
           DELETE FROM craft_queue.work WHERE id = 'launcher-stop-receipt';
           COMMIT;""")
    print("Verified immediate takeover after a worker stop with unknown token usage.")


def exercise_worker_token_overrun(root, template, request, launch, launcher, address, certificate, tokens, scrape):
    reservation, reported = 5, 30
    initial = request("operations", "operator", "summary")["summary"]
    budget = initial["token_budget"]
    charged = initial["tokens_charged_or_reserved"]
    profile = {"id": "overrun", "model": "fixture-overrun", "reasoning_effort": "low",
               "timeout_seconds": 15, "max_attempts": 2, "approval_reference": "fixture:overrun"}
    sql(f"UPDATE craft_queue.control SET token_budget = {charged + reported - 1};")
    try:
        for work_id, intent, error in (
            ("overrun-candidate", "Change the greeting", "HTTP 409"),
            ("overrun-rejected-result", "Return an invalid final result", "valid final candidate or blocker result"),
        ):
            order = template | {"id": work_id, "intent": intent, "execution_profile": profile}
            request("operator", "operator", "admit", repository="team/service", event_key=work_id,
                    work_order=order, token_limit=reservation)
            directory = root / work_id
            with launch("first", directory, str(uuid4())) as process:
                _, stderr = process.communicate(timeout=25)
                assert process.returncode and error in stderr, stderr
            audit = request("operations", "operator", "audit", work_id=work_id)["audit"]
            [attempt] = audit["attempts"]
            assert attempt["reported_tokens"] == reported and attempt["token_limit"] == reservation, attempt
            assert attempt["worker_stop_reported_at"] == attempt["expires_at"]
            assert not audit["work"].get("candidate")
            assert sql(f"SELECT count(*) FROM craft_queue.candidate_packs WHERE work_id = '{work_id}'") == "0"
            request("runner", "first", "renew", 409, work_id=work_id, generation=1)
            binding = {"work_id": work_id, "generation": 1, "tokens": reported}
            assert request("runner", "first", "finish_failed_attempt", **binding)["recorded"]
            request("runner", "second", "finish_failed_attempt", 409, **binding)
            request("runner", "first", "finish_failed_attempt", 409, **(binding | {"tokens": reported + 1}))
            summary = request("operations", "operator", "summary")["summary"]
            assert summary["tokens_charged_or_reserved"] == charged + reported, summary
            assert request("runner", "first", "claim", request_id=str(uuid4()), profile_capable=True)["claim"] is None
            request("operator", "operator", "cancel", work_id=work_id, reason="fixture token accounting verified")
            sql(f"UPDATE craft_queue.attempts SET started_at = started_at - interval '25 hours' WHERE work_id = '{work_id}'")
    finally:
        sql(f"UPDATE craft_queue.control SET token_budget = {budget};")

    filters = {"repository": "team/service", "since_hours": 48, "limit": 1}
    usage = request("operations", "operator", "queue_usage", **filters)["usage"]
    assert usage["totals"]["overrun_attempts"] == 2
    assert usage["totals"]["tokens_above_reservation"] == 2 * (reported - reservation)
    [attempt] = usage["overruns"]
    assert attempt["work_id"] == "overrun-rejected-result" and attempt["generation"] == 1
    assert attempt["repository"] == "team/service" and attempt["model"] == "fixture-overrun"
    assert attempt["reported_tokens"] == reported and attempt["token_limit"] == reservation
    all_usage = request("operations", "operator", "queue_usage", **(filters | {"limit": 100}))["usage"]
    group = next(group for group in all_usage["groups"] if group["model"] == "fixture-overrun")
    assert group["overrun_attempts"] == 2 and group["tokens_above_reservation"] == 2 * (reported - reservation)
    for changes in ({"since_hours": 24}, {"repository": "team/other"}):
        elsewhere = request("operations", "operator", "queue_usage", **(filters | changes))["usage"]
        assert elsewhere["overruns"] == [] and elsewhere["totals"]["overrun_attempts"] == 0
    brief = subprocess.check_output(
        [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate), "operator-brief",
         "--repository", "team/service", "--usage-hours", "48", "--limit", "1"], text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
    )
    assert "Token overruns (1/2) | 50 tokens above reservation" in brief
    assert "overrun-rejected-result | generation 1 | 30 reported / 5 reserved" in brief
    expected = {"token_overrun_attempts_15m": 2, "tokens_above_reservation_15m": 2 * (reported - reservation)}
    metrics = scrape("monitor")
    assert {key: metrics[key] for key in expected} == expected
    exercise_prometheus(address, certificate, tokens["monitor"], expected)
    sql("""UPDATE craft_queue.attempts SET worker_stop_reported_at = clock_timestamp() - interval '16 minutes'
           WHERE work_id IN ('overrun-candidate', 'overrun-rejected-result')""")
    metrics = scrape("monitor")
    assert metrics["token_overrun_attempts_15m"] == metrics["tokens_above_reservation_15m"] == 0
    print("Verified token overruns block publication and dispatch, and appear in bounded, scoped operator reports.")


def exercise_profile_comparison(root, template, request, launch, finish, launcher, address, certificate, tokens):
    basic = {
        "id": "routine", "model": "fixture-mini", "reasoning_effort": "low",
        "timeout_seconds": 8, "max_attempts": 1, "approval_reference": "fixture:compare/7",
    }
    profiles = (basic, basic | {"id": "deep", "model": "fixture-reasoning", "reasoning_effort": "high"})
    pairs = [
        {"case_id": case, "baseline": f"trial-{case}-baseline", "challenger": f"trial-{case}-challenger"}
        for case in ("one", "two")
    ]

    orders = {}
    sql("UPDATE craft_queue.control SET review_limit = 2;")
    for pair in pairs:
        for side, profile in zip(("baseline", "challenger"), profiles):
            work_id = pair[side]
            order = template | {
                "id": work_id, "intent": f"Apply fixture change {pair['case_id']}" + "x" * 250 + "omit-intent-tail",
                "acceptance": ["y" * 170 + "omit-criterion-tail", "second", "third", "omit-fourth-criterion"],
                "execution_profile": profile,
            }
            orders[work_id] = order
            request("operator", "operator", "admit", repository="team/service", event_key=work_id,
                    work_order=order, token_limit=40)

    for side in ("baseline", "challenger"):
        work_id = pairs[0][side]
        candidate = finish(launch("first", root / work_id, str(uuid4())))
        assert candidate["work_id"] == work_id
        binding = {
            "work_id": work_id, "generation": candidate["generation"],
            "head": candidate["candidate"], "digest": candidate["candidate_evidence"],
        }
        seq = request("verifier", "verifier", "record_verification", **binding,
                      previous_sequence=0, passed=True, evidence=f"fixture:observed/{side}",
                      note="Initial check result")["verification_sequence"]
        if side == "baseline":
            request("verifier", "verifier", "record_verification", **binding,
                    previous_sequence=seq, passed=False, evidence="fixture:superseding-baseline",
                    note="Subsequent check result")
    # Give the second case one attempt with no reported usage. A full review
    # queue must leave the remaining case unclaimed until capacity is raised.
    request_id = str(uuid4())
    assert request("runner", "first", "claim", request_id=request_id, profile_capable=True)["claim"] is None
    sql("UPDATE craft_queue.control SET review_limit = 3;")
    lease = request("runner", "first", "claim", request_id=request_id, profile_capable=True)["claim"]
    assert lease["work_id"] == pairs[1]["baseline"]
    sql("UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second' "
        "WHERE work_id = 'trial-two-baseline';")
    inbox = request("operations", "operator", "review_inbox", repository="team/service", limit=1)["inbox"]
    assert inbox["total"] == 3 and inbox["counts"] == {
        "resolve_failed_verification": 1, "choose_publication": 1, "resolve_attempt_cap": 1,
    }
    [first] = inbox["items"]
    assert first["work_id"] == pairs[0]["baseline"]
    assert first["independent_verification"]["evidence"] == "fixture:superseding-baseline"
    assert len(first["intent_excerpt"]) == 240
    assert first["acceptance_excerpts"] == ["y" * 160, "second", "third"]
    assert "omit-" not in json.dumps(inbox)

    request("operations", "first", "compare_profiles", 403, pairs=pairs)
    request("operations", "operator", "compare_profiles", 400, pairs=pairs + [pairs[0]])
    request("operations", "operator", "compare_profiles", 409,
            pairs=[pairs[0] | {"challenger": pairs[1]["challenger"]}])
    request("operations", "operator", "compare_profiles", 409,
            pairs=[pairs[0] | {"challenger": "unknown-case"}])
    report = request("operations", "operator", "compare_profiles", pairs=pairs)["comparison"]
    assert (report["baseline_profile"], report["challenger_profile"]) == profiles
    assert report["baseline_adapter"] == report["challenger_adapter"] == "codex"
    assert {result["adapter"] for result in report["results"]} == {"codex"}
    assert report["totals"] == {
        "baseline": {
            "cases": 2, "passed": 0, "failed": 1, "unscored": 1, "attempts": 2, "supervised_starts": 1,
            "attempts_with_usage": 1, "reported_tokens": 30, "reserved_tokens_without_usage": 40,
        },
        "challenger": {
            "cases": 2, "passed": 1, "failed": 0, "unscored": 1, "attempts": 1, "supervised_starts": 1,
            "attempts_with_usage": 1, "reported_tokens": 30, "reserved_tokens_without_usage": 0,
        },
    }
    assert report["results"][0]["verification_evidence"] == "fixture:superseding-baseline"
    pairs_path = root / "comparison-pairs.json"
    pairs_path.write_text(json.dumps(pairs), encoding="utf-8")
    result = subprocess.run(
        [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
         "compare-profiles", str(pairs_path)], capture_output=True, text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["comparison"] == report

    task = pairs[0]
    original = orders[task["challenger"]]
    alternates = {
        "trial-one-claude": original | {"agent_adapter": "claude"},
        "trial-one-explicit-codex": original | {"agent_adapter": "codex"},
        "trial-one-unprofiled": original | {"agent_adapter": "claude", "execution_profile": None},
    }
    for work_id, order in alternates.items():
        request("operator", "operator", "admit", repository="team/service", event_key=work_id,
                work_order=order | {"id": work_id}, token_limit=40)
    cross_adapter = task | {"baseline": task["challenger"], "challenger": "trial-one-claude"}
    cross_report = request("operations", "operator", "compare_profiles", pairs=[cross_adapter])["comparison"]
    assert cross_report["baseline_adapter"] == "codex" and cross_report["challenger_adapter"] == "claude"
    assert cross_report["baseline_profile"] == cross_report["challenger_profile"] == profiles[1]
    assert [(row["adapter"], row["independent_result"]) for row in cross_report["results"]] == [
        ("codex", "passed"), ("claude", "unscored"),
    ]
    for invalid in ("trial-one-explicit-codex", "trial-one-unprofiled"):
        request("operations", "operator", "compare_profiles", 409, pairs=[cross_adapter | {"challenger": invalid}])
    request("operations", "operator", "compare_profiles", 409,
            pairs=[task | {"challenger": "trial-one-claude"}, pairs[1]])
    for work_id in alternates:
        request("operator", "operator", "cancel", work_id=work_id, reason="fixture comparison verified")
    for pair in pairs:
        for side in ("baseline", "challenger"):
            request("operator", "operator", "cancel", work_id=pair[side], reason="fixture comparison verified")
    sql("UPDATE craft_queue.control SET review_limit = 1;")


def exercise_profile_fallbacks(root, template, request, launch, finish, launcher, address, certificate, tokens):
    profile = {
        "id": "routine", "model": "fixture-mini", "reasoning_effort": "low",
        "timeout_seconds": 8, "max_attempts": 1, "approval_reference": "fixture:initial-profile",
    }
    fallback = profile | {"id": "deep", "reasoning_effort": "high", "approval_reference": "fixture:fallback-profile"}
    sql("UPDATE craft_queue.control SET token_budget = 2000;")

    def prepare(name):
        order = template | {"id": name, "execution_profile": profile}
        request("operator", "operator", "admit", repository="team/service", event_key=name,
                work_order=order, token_limit=40)
        approval = {"work_id": name, "replacement_id": f"{name}-next", "execution_profile": fallback,
                    "valid_seconds": 3600, "evidence": f"fixture:approval/{name}"}
        request("operations", "scheduler", "approve_profile_fallback", 403, **approval)
        first = request("operations", "operator", "approve_profile_fallback", **approval)["fallback"]
        assert first["source_generation"] == 1 and first["approved_by"] == "operator"
        assert first == request("operations", "operator", "approve_profile_fallback", **approval)["fallback"]
        request("operations", "operator", "approve_profile_fallback", 409,
                **(approval | {"replacement_id": "other-replacement"}))
        lease = request("runner", "first", "claim", request_id=str(uuid4()), profile_capable=True)["claim"]
        assert lease["work_id"] == name
        return approval

    def sweep():
        return request("profile-scheduler", "scheduler", "apply_profile_fallbacks")["sweep"]

    def expire(name):
        sql(f"UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second' "
            f"WHERE work_id = '{name}';")

    request("profile-scheduler", "operator", "apply_profile_fallbacks", 403)
    request("operations", "scheduler", "summary", 403)
    empty = {"paused": False, "applied": [], "blocked": []}
    approval = prepare("fallback-automatic")
    request("operations", "first", "profile_usage", 403, work_id=approval["work_id"])
    request("operations", "operator", "profile_usage", 404, work_id="missing-profile-usage")
    before = request("operations", "operator", "profile_usage", work_id=approval["work_id"])["usage"]
    assert before["replacement_id"] is None and len(before["orders"]) == 1
    assert before["totals"]["reserved_tokens_without_usage"] == 40
    assert sweep() == empty
    expire(approval["work_id"])
    request("operator", "operator", "set_paused", paused=True)
    assert sweep() == empty | {"paused": True}
    request("operator", "operator", "set_paused", paused=False)
    result = subprocess.run(
        [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
         "apply-profile-fallbacks"], capture_output=True, text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_PROFILE_SCHEDULER_TOKEN": tokens["scheduler"]},
    )
    assert result.returncode == 0, result.stderr
    [applied] = json.loads(result.stdout)["sweep"]["applied"]
    assert applied["replacement_work_id"] == approval["replacement_id"] and applied["actor"] == "operator"
    assert sweep() == empty
    audit = request("operations", "operator", "audit", work_id=approval["work_id"])["audit"]
    assert audit["profile_escalation"] == applied and audit["profile_fallback"]["applied_by"] == "scheduler"
    assert audit["attempts"][0]["outcome"] == "expired"
    request("operations", "operator", "revoke_profile_fallback", 409, work_id=approval["work_id"])
    replacement = finish(launch("first", root / "fallback-usage-attempt", str(uuid4())))
    assert replacement["work_id"] == approval["replacement_id"]
    observed = request("verifier", "verifier", "record_verification", work_id=replacement["work_id"],
                       generation=replacement["generation"], head=replacement["candidate"],
                       digest=replacement["candidate_evidence"], previous_sequence=0, passed=True,
                       evidence="fixture:fallback-check", note="Verified the fallback candidate")
    result = subprocess.check_output(
        [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
         "profile-usage", approval["replacement_id"]], text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]})
    usage = json.loads(result)["usage"]
    assert usage == request("operations", "operator", "profile_usage", work_id=approval["work_id"])["usage"]
    assert usage["escalation"] == applied
    initial, fallback_result = usage["orders"]
    assert initial["profile"] == profile and initial["state"] == "cancelled"
    assert initial["reserved_tokens_without_usage"] == 40 and initial["independent_result"] == "unscored"
    assert fallback_result["profile"] == fallback and fallback_result["independent_result"] == "passed"
    assert fallback_result["verification_sequence"] == observed["verification_sequence"]
    assert usage["totals"] == {"attempts": 2, "supervised_starts": 1, "attempts_with_usage": 1,
                               "reported_tokens": 30, "reserved_tokens_without_usage": 40}
    request("operator", "operator", "cancel", work_id=approval["replacement_id"], reason="fixture completed")

    for name in ("fallback-revoked", "fallback-expired", "fallback-collision"):
        approval = prepare(name)
        if name == "fallback-revoked":
            revoked = request("operations", "operator", "revoke_profile_fallback", work_id=name)["fallback"]
            assert revoked["revoked_by"] == "operator"
        if name == "fallback-expired":
            sql("UPDATE craft_queue.profile_fallbacks SET approved_at = clock_timestamp() - interval '2 hours', "
                "expires_at = clock_timestamp() - interval '1 hour' WHERE source_work_id = 'fallback-expired';")
        if name == "fallback-collision":
            request("operator", "operator", "admit", repository="team/service", event_key="fixture:collision",
                    work_order=template | {"id": approval["replacement_id"]}, token_limit=40)
        expire(name)
        expected = empty | {"blocked": [name]} if name == "fallback-collision" else empty
        assert sweep() == expected
        assert sweep() == empty
        assert request("operator", "operator", "status", work_id=name)["work"]["state"] == "leased"
        request("operator", "operator", "cancel", work_id=name, reason="fixture completed")
        if name == "fallback-collision":
            request("operator", "operator", "cancel", work_id=approval["replacement_id"], reason="fixture completed")

    approval = prepare("fallback-runner-stop")
    current = request("operations", "operator", "runner_status", runner_id="first")["runner"]
    stop = request("operations", "operator", "set_runner_mode", runner_id="first",
                   expected_revision=current["revision"], mode="stopped", evidence="fixture:stop")["decision"]
    assert stop["fenced_leases"] == 1
    assert sweep() == empty
    audit = request("operations", "operator", "audit", work_id=approval["work_id"])["audit"]
    assert audit["profile_fallback"]["revoked_by"] == "runner-stop:operator"
    assert audit["work"]["attempt_limit_blocks_pickup"]
    request("operator", "operator", "cancel", work_id=approval["work_id"], reason="fixture completed")
    request("operations", "operator", "set_runner_mode", runner_id="first",
            expected_revision=stop["revision"], mode="active", evidence="fixture:resume")


def exercise_runner_loop(root, template, mapping, home, agent, request, launcher, address, certificate, tokens):
    attempts = root / "runner-loop"
    attempts.mkdir()
    checkout_map = json.loads(mapping.read_text(encoding="utf-8"))
    plain = root / "plain-directory"
    plain.mkdir()
    nested = Path(checkout_map["team/service"]["path"]) / "src"
    assert nested.is_dir()
    loop_map = root / "runner-loop-map.json"
    loop_map.write_text(json.dumps(checkout_map | {
        "team/missing": {"path": str(root / "missing-checkout"), "jira_snapshot": str(root / "missing-snapshot.json")},
        "team/plain": {"path": str(plain)},
        "team/nested": {"path": str(nested)},
        "team/other": checkout_map["team/service"],
    }), encoding="utf-8")
    profile = {
        "id": "loop", "model": "fixture-mini", "reasoning_effort": "low",
        "timeout_seconds": 1, "max_attempts": 1, "approval_reference": "fixture:loop-profile",
    }
    sql("UPDATE craft_queue.control SET review_limit = 2;")
    sql("INSERT INTO craft_queue.runner_repositories VALUES "
        "('first', 'team/missing'), ('second', 'team/missing'), "
        "('first', 'team/plain'), ('first', 'team/nested');")
    unavailable = (("loop-missing", "team/missing"), ("loop-plain", "team/plain"),
                   ("loop-nested", "team/nested"), ("loop-foreign", "team/other"))
    for work_id, repository in unavailable:
        request("operator", "operator", "admit", repository=repository, event_key=work_id,
                work_order=template | {"id": work_id}, token_limit=40)
    for order in (
        template | {"id": "loop-bad", "intent": "Wait for timeout", "execution_profile": profile},
        template | {"id": "loop-good", "intent": "Follow-on fixture change"},
    ):
        request("operator", "operator", "admit", repository="team/service", event_key=order["id"],
                work_order=order, token_limit=40)

    token_file = root / "runner-loop-token"
    with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as credential:
        credential.write(tokens["first"] + "\n")
    env = os.environ | {"CRAFT_FACTORY_RUNNER_TOKEN": "incorrect-ambient-runner-token-fixture"}
    preflight = [str(launcher.resolve()), "check-shared-runner", "--queue", address, "--ca", str(certificate),
                 "--runner-token-file", str(token_file),
                 "--attempt-root", str(attempts), "--codex", str(agent), "--codex-home", str(home),
                 "--repository-map"]
    request("runner", "operator", "check_enrollment", 403, mapped_repositories=["team/service"])
    request("runner", "first", "check_enrollment", 400, mapped_repositories=[""])
    receipt_count = sql("SELECT count(*) FROM craft_queue.runner_claim_polls;")
    ready = json.loads(subprocess.check_output(preflight + [str(loop_map)], env=env, text=True, timeout=15))
    rows = {row["repository"]: row for row in ready["repositories"]}
    assert ready["ready"] and ready["runner_id"] == "first" and ready["usable_repositories"] == 1
    assert rows["team/service"]["checkout_available"] and rows["team/service"]["enrolled"]
    assert rows["team/plain"]["enrolled"] and not rows["team/plain"]["checkout_available"]
    assert rows["team/missing"]["jira_snapshot_file_available"] is False
    assert rows["team/other"]["checkout_available"] and not rows["team/other"]["enrolled"]
    wrong_map = root / "unusable-map.json"
    wrong_map.write_text(json.dumps({"team/plain": {"path": str(plain)}}), encoding="utf-8")
    blocked = subprocess.run(preflight + [str(wrong_map)], env=env, text=True, capture_output=True, timeout=15)
    assert blocked.returncode == 1 and json.loads(blocked.stdout)["ready"] is False
    unsafe_root = preflight.copy()
    unsafe_root[unsafe_root.index("--attempt-root") + 1] = str(nested)
    blocked = subprocess.run(unsafe_root + [str(loop_map)], env=env, text=True, capture_output=True, timeout=15)
    assert blocked.returncode != 0 and "outside all mapped checkouts" in blocked.stderr
    assert sql("SELECT count(*) FROM craft_queue.runner_claim_polls;") == receipt_count
    assert not list(attempts.iterdir())
    assert request("operator", "operator", "status", work_id="loop-bad")["work"]["generation"] == 0

    for repositories in ([], ["team/other"]):
        assert request("runner", "first", "claim", request_id=str(uuid4()),
                       available_repositories=repositories)["claim"] is None
    assert request("runner", "first", "claim", request_id=str(uuid4()),
                   available_repositories=[f"ungranted/repo-{i}" for i in range(120)])["claim"] is None
    def last_poll():
        status = request("operations", "operator", "dispatch_status", work_id="loop-good")["dispatch"]
        return next(item for item in status["claim_poll_reports"] if item["runner_id"] == "first")

    missed = last_poll()
    assert missed["observed_at"] and missed["checkout_reported"] is False
    assert sql("SELECT cardinality(available_repositories) FROM craft_queue.runner_claim_polls "
               "WHERE runner_id = 'first' AND adapter = 'codex';") == "0"
    request("runner", "first", "claim", 400, request_id=str(uuid4()), available_repositories=[""])
    request("runner", "first", "claim", 400, request_id=str(uuid4()),
            available_repositories=["r" * 201])
    assert last_poll()["observed_at"] == missed["observed_at"]

    def loop_command(endpoint, attempt_root):
        return [str(launcher.resolve()), "run-shared-loop", "--queue", endpoint, "--ca", str(certificate),
                "--runner-token-file", str(token_file),
                "--repository-map", str(loop_map), "--attempt-root", str(attempt_root), "--codex", str(agent),
                "--codex-home", str(home), "--renew-seconds", "1", "--timeout-seconds", "18",
                "--poll-seconds", "1", "--write-status"]

    environment = env

    def loop(count, endpoint=address, attempt_root=attempts):
        result = subprocess.run(
            loop_command(endpoint, attempt_root) + ["--max-polls", str(count)],
            capture_output=True, text=True, timeout=20, env=environment,
        )
        assert result.returncode == 0, result.stderr
        return [json.loads(line) for line in result.stdout.splitlines()]

    stop = attempts / ".stop"
    stop.touch()
    status_file = attempts / ".runner-status.json"
    status_file.write_text("unrelated file", encoding="utf-8")
    blocked = subprocess.run(loop_command(address, attempts), capture_output=True, text=True,
                             timeout=15, env=environment)
    assert blocked.returncode != 0 and "not a runner status report" in blocked.stderr
    assert status_file.read_text(encoding="utf-8") == "unrelated file"
    status_file.unlink()
    assert loop(2) == []
    report = json.loads(status_file.read_text(encoding="utf-8"))
    assert report["state"] == "stopped" and report["last_poll"] is None
    assert request("operator", "operator", "status", work_id="loop-bad")["work"]["generation"] == 0
    stop.unlink()

    failed, submitted, idle = loop(3)
    assert failed["state"] == "attempt_failed" and submitted["state"] == "submitted"
    assert failed["reason"] == "worker_failed"
    assert idle["state"] == "idle"
    report = json.loads(status_file.read_text(encoding="utf-8"))
    assert report["state"] == "stopped" and report["kind"] == "runner_status"
    poll = report["last_poll"]
    assert poll["state"] == "idle" and poll["request_id"] == idle["request_id"]
    assert poll["http_status"] is None and report["started_at"] <= poll["completed_at"] <= report["updated_at"]
    assert report["last_failed_poll"]["request_id"] == failed["request_id"]
    assert report["last_failed_poll"]["reason"] == failed["reason"]
    reported = last_poll()
    assert reported["checkout_reported"] and reported["context_capable"] and reported["profile_capable"]
    assert reported["observed_at"] > missed["observed_at"]
    assert len({failed["request_id"], submitted["request_id"], idle["request_id"]}) == 3
    assert submitted["receipt"]["work_id"] == "loop-good"
    assert not (attempts / failed["request_id"] / "submission.json").exists()
    assert not list((attempts / ".pending").iterdir())
    good_dir = attempts / submitted["request_id"]
    assert (good_dir / "submission.json").is_file()
    assert not (good_dir / "submission.pending").exists()

    # Simulate a restart before retiring an acknowledged request marker.
    marker = attempts / ".pending" / submitted["request_id"]
    marker.mkdir()
    with closing(sqlite3.connect(good_dir / "state.db")) as db:
        before, = db.execute("SELECT count(*) FROM events WHERE kind = 'runner_started'").fetchone()
    assert loop(1) == [submitted]
    # Also exercise the window after writing the submission and before its atomic rename.
    marker.mkdir()
    (good_dir / "submission.json").rename(good_dir / "submission.pending")
    assert loop(1) == [submitted]
    assert (good_dir / "submission.json").is_file() and not (good_dir / "submission.pending").exists()
    with closing(sqlite3.connect(good_dir / "state.db")) as db:
        after, = db.execute("SELECT count(*) FROM events WHERE kind = 'runner_started'").fetchone()
    assert before == after == 1 and not marker.exists()
    audit = request("operations", "operator", "audit", work_id="loop-good")["audit"]
    assert len(audit["attempts"]) == 1

    disconnected = root / "disconnected-loop"
    disconnected.mkdir()
    with socket.socket() as unserved:
        unserved.bind(("127.0.0.1", 0))
        endpoint = f"https://127.0.0.1:{unserved.getsockname()[1]}"
        first, again = loop(2, endpoint, disconnected)
    assert first == again and first["state"] == "retrying"
    assert (disconnected / ".pending" / first["request_id"]).is_dir()
    report = json.loads((disconnected / ".runner-status.json").read_text(encoding="utf-8"))
    assert report["last_poll"]["state"] == "retrying" and report["last_poll"]["request_id"] == first["request_id"]
    assert report["last_poll"]["reason"] == "queue_unreachable"
    unauthorized = loop_command(address, attempts)
    unauthorized[unauthorized.index("--runner-token-file") + 1] = str(root / "denied-runner-token")
    with os.fdopen(os.open(root / "denied-runner-token", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as credential:
        credential.write("wrong-explicit-runner-token-fixture\n")
    blocked = subprocess.run(unauthorized, env=environment, text=True, capture_output=True, timeout=15)
    assert blocked.returncode != 0
    report = json.loads(status_file.read_text(encoding="utf-8"))
    assert report["state"] == "failed" and report["http_status"] == 401 and report["last_poll"] is None
    assert report["failure_reason"] == "queue_authentication_failed"
    assert "wrong-explicit" not in status_file.read_text(encoding="utf-8")
    assert len(list((attempts / ".pending").iterdir())) == 1
    assert report["last_failed_poll"]["request_id"] == failed["request_id"]
    # Recover a prior process failure written before last_failed_run was introduced.
    previous_failure = report.pop("last_failed_run")
    status_file.write_text(json.dumps(report), encoding="utf-8")
    [restarted] = loop(1)
    assert restarted["state"] == "idle"
    report = json.loads(status_file.read_text(encoding="utf-8"))
    assert report["state"] == "stopped" and report["failure_reason"] is None
    assert report["last_failed_run"] == previous_failure
    assert report["last_failed_poll"]["request_id"] == failed["request_id"]
    assert loop(1)[0]["state"] == "idle"
    assert json.loads(status_file.read_text(encoding="utf-8"))["last_failed_run"] == previous_failure

    for work_id in ("loop-bad", "loop-good"):
        request("operator", "operator", "cancel", work_id=work_id, reason="runner loop fixture verified")
    if os.name == "posix":
        draining = template | {"id": "loop-drain-first", "intent": "Wait for planned runner stop"}
        next_order = template | {"id": "loop-drain-next"}
        request("operator", "operator", "admit", repository="team/service", event_key=draining["id"],
                work_order=draining, token_limit=40)
        started = home / "fixture-drain-started"
        release = home / "fixture-drain-release"
        with subprocess.Popen(loop_command(address, attempts), env=environment, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            try:
                deadline = monotonic() + 6
                while not started.exists():
                    assert process.poll() is None and monotonic() < deadline, "runner did not start worker"
                    sleep(0.02)
                request("operator", "operator", "admit", repository="team/service", event_key=next_order["id"],
                        work_order=next_order, token_limit=40)
                process.terminate()
                release.touch()
                stdout, stderr = process.communicate(timeout=12)
                assert process.returncode == 0, stderr
            finally:
                if process.poll() is None:
                    release.touch()
                    process.kill()
            [completed] = [json.loads(line) for line in stdout.splitlines()]
            assert completed["state"] == "submitted" and completed["receipt"]["work_id"] == draining["id"]
        assert not list((attempts / ".pending").iterdir())
        assert request("operator", "operator", "status", work_id=draining["id"])["work"]["state"] == "candidate"
        assert request("operator", "operator", "status", work_id=next_order["id"])["work"]["generation"] == 0
        for work_id in (draining["id"], next_order["id"]):
            request("operator", "operator", "cancel", work_id=work_id, reason="planned runner stop verified")
    for work_id, _ in unavailable:
        assert request("operator", "operator", "status", work_id=work_id)["work"]["generation"] == 0
    for work_id in ("loop-plain", "loop-nested"):
        status = request("operations", "operator", "dispatch_status", work_id=work_id)["dispatch"]
        assert status["claim_poll_reports"][0]["checkout_reported"] is False
        request("operator", "operator", "cancel", work_id=work_id, reason="invalid checkout fixture verified")
    request_id = str(uuid4())
    lease = request("runner", "second", "claim", request_id=request_id,
                    available_repositories=["team/missing"])["claim"]
    assert lease["work_id"] == "loop-missing"
    assert request("runner", "second", "claim", request_id=request_id,
                   available_repositories=["team/service"])["claim"] is None
    assert request("runner", "second", "claim", request_id=request_id,
                   available_repositories=["team/missing"])["claim"] == lease
    for work_id in ("loop-missing", "loop-foreign"):
        request("operator", "operator", "cancel", work_id=work_id, reason="checkout scope fixture verified")
    sql("DELETE FROM craft_queue.runner_repositories WHERE repository IN ('team/missing', 'team/plain', 'team/nested');")
    sql("UPDATE craft_queue.control SET review_limit = 1;")


def exercise_claude_adapter(root, template, mapping, agent, request, launcher, address, certificate, tokens):
    bubblewrap = shutil.which("bwrap") if sys.platform.startswith("linux") else None
    if bubblewrap is None:
        assert os.environ.get("FACTORY_REQUIRE_CLAUDE_FIXTURE") != "1", "Bubblewrap is required by this fixture"
        print("Skipped Claude namespace fixture because Bubblewrap is unavailable.")
        return
    sentinel = root / "host-only-sentinel"
    sentinel.write_text("host file must remain outside the namespace", encoding="utf-8")
    order = template | {"id": "claude-one", "agent_adapter": "claude", "intent": f"Namespace sentinel {sentinel}"}
    request("operator", "operator", "admit", 409, repository="team/service", event_key="bad-adapter",
            work_order=order | {"agent_adapter": None}, token_limit=40)
    profile = {"id": "unsupported-claude", "model": "claude-sonnet-4-6", "reasoning_effort": "ultra",
               "timeout_seconds": 20, "max_attempts": 1, "approval_reference": "fixture:unsupported effort"}
    request("operator", "operator", "admit", 409, repository="team/service", event_key="bad-claude-effort",
            work_order=order | {"execution_profile": profile}, token_limit=40)
    request("operator", "operator", "admit", repository="team/service", event_key=order["id"],
            work_order=order, token_limit=40)
    request_id = str(uuid4())
    assert request("runner", "first", "claim", request_id=request_id)["claim"] is None
    assert request("runner", "first", "claim", request_id=request_id, agent_adapter="claude")["claim"] is None
    dispatch = request("operations", "operator", "dispatch_status", work_id=order["id"])["dispatch"]
    assert dispatch["required_adapter"] == "claude" and "claude_repository_approval_required" in dispatch["blockers"]
    backlog = request("operations", "operator", "operator_brief", repository="team/service",
                      usage_hours=24, limit=20)["operator_brief"]["backlog"]["items"]
    assert next(item for item in backlog if item["work_id"] == order["id"])["blockers"] == dispatch["blockers"]
    approval_fields = {"repository": "team/service", "approval_id": "claude-fixture",
                       "evidence": "fixture:approved source egress", "valid_seconds": 60}
    approve = "approve_claude_repository"
    revoke = "revoke_claude_repository"
    request("operations", "first", approve, 403, **approval_fields)
    shared_ops = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
    result = subprocess.run(
        shared_ops + ["approve-claude-repository", approval_fields["repository"],
                      "--approval-id", approval_fields["approval_id"], "--evidence", approval_fields["evidence"],
                      "--valid-seconds", str(approval_fields["valid_seconds"])],
        capture_output=True, check=True, timeout=12, text=True,
        env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
    )
    approval = request("operations", "operator", approve, **approval_fields)["approval"]
    assert json.loads(result.stdout)["approval"] == approval
    request("operations", "operator", approve, 409, **(approval_fields | {"approval_id": "overlap"}))
    lease = request("runner", "first", "claim", request_id=request_id, agent_adapter="claude")["claim"]
    assert lease["work_id"] == order["id"] and lease["generation"] == 1
    assert lease["expires_at"] == approval["expires_at"]
    renewed = request("runner", "first", "renew", work_id=order["id"], generation=1)
    assert renewed["expires_at"] == approval["expires_at"]
    assert request("runner", "first", "claim", request_id=request_id, agent_adapter="codex")["claim"] is None

    def command(destination, key, collect=False):
        optional_flags = ["--collect-knowledge"] if collect else []
        return [str(launcher.resolve()), "run-shared", "--queue", address, "--ca", str(certificate),
             "--request-id", key, "--repository-map", str(mapping), "--destination", str(destination),
             "--claude", str(agent), "--bubblewrap", bubblewrap, "--renew-seconds", "1", "--timeout-seconds", "18"] + optional_flags

    env = os.environ | {"CRAFT_FACTORY_RUNNER_TOKEN": tokens["first"], "ANTHROPIC_API_KEY": "fixture-key",
                        "FACTORY_TEST_SECRET": "fixture-host-secret"}

    def run(destination, key, collect=False):
        return subprocess.run(command(destination, key, collect), capture_output=True, text=True, timeout=22, env=env)

    destination = root / "claude-attempt"
    result = run(destination, request_id, collect=True)
    assert result.returncode == 0, result.stderr
    receipt = json.loads(result.stdout)
    assert receipt["work_id"] == order["id"] and receipt["knowledge_proposal"]["state"] == "recorded"
    knowledge = receipt["knowledge_proposal"]
    assert request("operations", "operator", "inspect_knowledge",
                   proposal_id=knowledge["id"])["proposal"]["proposal"]["location"] == "project:fixture-claude"
    request("operations", "operator", "review_knowledge", proposal_id=knowledge["id"],
            proposal_digest=knowledge["digest"], promote=False, owner="Fixture owners", evidence="fixture:review completed")
    assert (destination / "work" / "checkout" / "src" / "hello").read_text() == "changed by fixture Claude adapter"
    with closing(sqlite3.connect(destination / "state.db")) as db:
        note, = db.execute("SELECT note FROM events WHERE kind = 'runner_started'").fetchone()
        handoff, = db.execute("SELECT note FROM events WHERE kind = 'candidate_submitted'").fetchone()
    assert "claude" in note
    assert handoff == "Candidate from supervised Claude execution"
    audit = request("operations", "operator", "audit", work_id=order["id"])["audit"]
    assert audit["attempts"][0]["reported_tokens"] == 35
    assert audit["attempts"][0]["claude_approval_id"] == approval_fields["approval_id"]
    request("operator", "operator", "cancel", work_id=order["id"], reason="Claude fixture verified")

    malformed = order | {"id": "claude-malformed", "acceptance": ["Reject structured output"]}
    request("operator", "operator", "admit", repository="team/service", event_key=malformed["id"],
            work_order=malformed, token_limit=40)
    destination = root / "claude-malformed"
    failure = run(destination, str(uuid4()))
    assert failure.returncode != 0 and "valid final candidate or blocker" in failure.stderr
    assert not (destination / "submission.json").exists()
    request("operator", "operator", "cancel", work_id=malformed["id"], reason="invalid Claude output blocked")

    waiting = order | {"id": "claude-waiting", "acceptance": ["Wait for lease loss"]}
    request("operator", "operator", "admit", repository="team/service", event_key=waiting["id"],
            work_order=waiting, token_limit=40)
    destination = root / "claude-waiting"
    waiting_request = str(uuid4())
    revoke_fields = {"approval_id": approval_fields["approval_id"], "evidence": "fixture:withdrawn"}
    with subprocess.Popen(command(destination, waiting_request), stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True, env=env) as process:
        try:
            marker = destination / "work" / "checkout" / "src" / "claude-waiting"
            deadline = monotonic() + 10
            while not marker.exists() and process.poll() is None and monotonic() < deadline:
                sleep(0.05)
            assert marker.exists(), "Claude namespace fixture never started"
            revoked = request("operations", "operator", revoke, **revoke_fields)["approval"]
            assert revoked["revoked_by"] == "operator"
            _, stderr = process.communicate(timeout=8)
            assert process.returncode != 0 and "HTTP 409" in stderr
        finally:
            if process.poll() is None:
                process.terminate()
    assert not (destination / "submission.json").exists()
    assert request("operations", "operator", revoke, **revoke_fields)["approval"] == revoked
    assert request("operations", "operator", approve, **approval_fields)["approval"] == revoked
    audit = request("operations", "operator", "audit", work_id=waiting["id"])["audit"]
    assert audit["work"]["state"] == "queued" and audit["attempts"][0]["outcome"] == "expired"
    request("operations", "operator", approve, **(approval_fields | {"approval_id": "replacement"}))
    assert request("runner", "first", "claim", request_id=waiting_request, agent_adapter="claude")["claim"] is None
    request("runner", "first", "renew", 409, work_id=waiting["id"], generation=1)
    replacement = request("runner", "first", "claim", request_id=str(uuid4()), agent_adapter="claude")["claim"]
    assert replacement["work_id"] == waiting["id"] and replacement["generation"] == 2
    request("operator", "operator", "cancel", work_id=waiting["id"], reason="Claude lease loss verified")
    request("operations", "operator", revoke, **(revoke_fields | {"approval_id": "replacement"}))

    expiring = order | {"id": "claude-expiring"}
    request("operator", "operator", "admit", repository="team/service", event_key=expiring["id"],
            work_order=expiring, token_limit=40)
    request("operations", "operator", approve, **(approval_fields | {"approval_id": "short", "valid_seconds": 3}))
    short_request = str(uuid4())
    short = request("runner", "first", "claim", request_id=short_request, agent_adapter="claude")["claim"]
    assert short["work_id"] == expiring["id"]
    cutoff = monotonic() + 8
    while True:
        renewal = request("runner", "first", "renew", (200, 409), work_id=short["work_id"], generation=1)
        if renewal.get("error") == "lease_not_current":
            break
        assert renewal["expires_at"] == short["expires_at"]
        assert monotonic() < cutoff, "lease outlived its Claude approval"
        sleep(0.15)
    assert request("runner", "first", "claim", request_id=str(uuid4()), agent_adapter="claude")["claim"] is None
    audit = request("operations", "operator", "audit", work_id=expiring["id"])["audit"]
    assert audit["work"]["state"] == "queued" and audit["attempts"][0]["outcome"] == "expired"
    request("operator", "operator", "cancel", work_id=expiring["id"], reason="Claude approval expiry verified")
    print("Verified Claude routing, repository approval, revocation, namespace, structured output, and token usage.")


def exercise_team_skill(root, repo, mapping, home, template, request, launch, finish,
                        launcher, address, certificate, tokens):
    location = "team:fixture-release"
    markdown = "# Reviewed cross-repo skill"
    revision = f"sha256:{sha256(markdown.encode()).hexdigest()}"
    approval = "fixture:engineering-approval/1"
    release = {"location": location, "revision": revision, "markdown": markdown, "owner_name": "Fixture owners",
               "approval_reference": approval, "repositories": ["team/service", "team/other"]}
    cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
    env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    skill_path = root / "team-skill-approved.md"
    skill_path.write_bytes(markdown.encode())
    command = cli + ["register-team-skill", location, str(skill_path), "--revision", revision,
                     "--owner", release["owner_name"], "--repository", "team/service",
                     "--repository", "team/other", "--evidence", approval]
    request("operations", "reviewer", "register_team_skill", 403, release=release)
    saved = json.loads(subprocess.check_output(command, env=env, text=True, timeout=15))["release"]
    assert saved["registered_by"] == "operator" and saved["revision"] == revision and "markdown" not in saved
    assert request("operations", "operator", "register_team_skill", release=release)["release"] == saved
    assert request("operations", "operator", "team_skill_release",
                   location=location, revision=revision)["release"] == saved
    long_markdown = "#" * 32768
    long_pin = f"sha256:{sha256(long_markdown.encode()).hexdigest()}"
    large = request("operations", "operator", "register_team_skill", release=release | {
        "location": "team:fixture-large-skill", "markdown": long_markdown, "revision": long_pin,
    })["release"]
    assert large["revision"] == long_pin and "markdown" not in large
    skill_path.write_bytes(markdown.encode() + b"\n")
    rejected = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
    assert rejected.returncode != 0 and "differs from the approved revision" in rejected.stderr
    changed = markdown + "\n"
    request("operations", "operator", "register_team_skill", 409, release=release | {
        "markdown": changed, "revision": f"sha256:{sha256(changed.encode()).hexdigest()}",
        "repositories": ["team/service"],
    })
    second_checkout = root / "second-skill-repo"
    subprocess.run(["git", "clone", "--quiet", "--no-local", str(repo), str(second_checkout)], check=True)
    subprocess.run(["git", "-C", str(second_checkout), "-c", "user.name=Fixture",
                    "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-qm", "Other base"],
                   check=True)
    second_base = subprocess.check_output(["git", "-C", str(second_checkout), "rev-parse", "HEAD"]).decode().strip()
    mapping.write_text(json.dumps({
        "team/service": {"path": str(repo)}, "team/other": {"path": str(second_checkout)},
    }), encoding="utf-8")
    personal = home / "skills" / "fixture-release"
    personal.mkdir(parents=True)
    (personal / "SKILL.md").write_text("Unapproved personal skill", encoding="utf-8")
    pin = {"location": location, "revision": revision}
    first = template | {"id": "team-skill-first", "skill": pin}
    request("operator", "operator", "admit", 409, repository="team/private",
            event_key="skill:denied", work_order=first, token_limit=40)
    request("operator", "operator", "admit", 409, repository="team/service", event_key="skill:stale",
            work_order=first | {"skill": pin | {"revision": f"sha256:{'0' * 64}"}}, token_limit=40)
    for identity, repository, order in (
        ("first", "team/service", first),
        ("outside", "team/other", first | {"id": "team-skill-second", "base": second_base}),
    ):
        request("operator", "operator", "admit", repository=repository, event_key=f"skill:{identity}",
                work_order=order, token_limit=40)
        request("runner", identity, "team_skill", 409, work_id=order["id"], generation=1)
        attempt = root / order["id"]
        assert finish(launch(identity, attempt, str(uuid4())))["work_id"] == order["id"]
        with closing(sqlite3.connect(attempt / "state.db")) as db:
            note, = db.execute("SELECT note FROM events WHERE kind = 'team_skill_resolved'").fetchone()
        assert json.loads(note) == pin | {"owner": "Fixture owners", "approval_reference": approval}
        request("operator", "operator", "cancel", work_id=order["id"], reason="fixture skill resolved")

    held = first | {"id": "team-skill-held"}
    sql("UPDATE craft_queue.control SET review_limit = 2;")
    request("operator", "operator", "admit", repository="team/service", event_key="skill:held",
            work_order=held, token_limit=40)
    candidate = finish(launch("first", root / held["id"], str(uuid4())))
    sequence = request("verifier", "verifier", "record_verification",
                       work_id=held["id"], generation=candidate["generation"],
                       head=candidate["candidate"], digest=candidate["candidate_evidence"],
                       previous_sequence=0, passed=True, evidence="fixture:checked",
                       note="Fixture release checked independently")["verification_sequence"]
    publication = request("publication", "operator", "reserve", work_id=held["id"],
                          generation=candidate["generation"], head=candidate["candidate"],
                          verification_sequence=sequence)["publication"]

    revoked = first | {"id": "team-skill-revocation"}
    request("operator", "operator", "admit", repository="team/service", event_key="skill:leased",
            work_order=revoked, token_limit=40)
    lease = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
    binding = {"work_id": revoked["id"], "generation": lease["generation"]}
    assert request("runner", "first", "team_skill", **binding)["skill"]["markdown"] == markdown
    request("runner", "second", "team_skill", 409, **binding)
    request("runner", "outside", "team_skill", 409, **binding)
    request("runner", "operator", "team_skill", 403, **binding)
    withdrawal = {"location": location, "revision": revision, "evidence": "fixture:withdrawn"}
    response = json.loads(subprocess.check_output(
        cli + ["withdraw-team-skill", location, revision, "--evidence", withdrawal["evidence"]],
        env=env, text=True, timeout=15,
    ))["release"]
    assert response["revoked_by"] == "operator" and response["revoked_at"] and "markdown" not in response
    assert request("operations", "operator", "withdraw_team_skill", **withdrawal)["release"] == response
    request("operations", "operator", "register_team_skill", 409, release=release)
    assert request("operator", "operator", "status", work_id=revoked["id"])["work"]["state"] == "cancelled"
    request("runner", "first", "renew", 409, **binding)
    request("runner", "first", "team_skill", 409, **binding)
    request("runner", "first", "submit_candidate", 409, **binding, commit=CANDIDATE,
            evidence_ref="fixture:late", tokens=10)
    request("operator", "operator", "admit", 409, repository="team/service", event_key="skill:withdrawn",
            work_order=first | {"id": "team-skill-withdrawn"}, token_limit=40)
    request("publisher", "publisher", "begin", 409, action_key=publication["action_key"])
    request("operator", "operator", "cancel", 409, work_id=held["id"], reason="before reconciliation")
    request("publication", "operator", "reconcile", action_key=publication["action_key"], observation={
        "outcome": "absent", "head": candidate["candidate"],
        "evidence": "fixture:revocation fenced the reserved send",
    })
    request("operator", "operator", "cancel", work_id=held["id"], reason="reserved send reconciled")
    sql("UPDATE craft_queue.control SET review_limit = 1;")
    mapping.write_text(json.dumps({"team/service": {"path": str(repo)}}), encoding="utf-8")


def exercise_project_context(root, template, request, launch, finish, launcher, agent,
                             address, certificate, tokens):
    def release(location, excerpt, kind, source_revision):
        source = f"fixture:wiki/{kind}"
        approval = f"fixture:approved/{kind}/{source_revision}"
        canonical = "\n".join((kind, source, source_revision, approval, excerpt))
        revision = f"sha256:{sha256(canonical.encode()).hexdigest()}"
        sql(f"""
            INSERT INTO craft_queue.project_context_releases (
                repository, location, revision, excerpt, kind, source_reference,
                source_revision, approval_reference
            ) VALUES ('team/service', '{location}', '{revision}', '{excerpt}', '{kind}',
                '{source}', '{source_revision}', '{approval}');
        """)
        return {"location": location, "revision": revision}

    location = "project:fixture-routing"
    old = release(location, "Outdated routing decision", "decision", "4")
    runbook_release = {"repository": "team/service", "location": "project:fixture-recovery",
                       "kind": "runbook", "source_reference": "fixture:wiki/runbook", "source_revision": "12",
                       "approval_reference": "fixture:approved/runbook/12", "excerpt": "Approved runbook excerpt"}
    canonical = "\n".join(runbook_release[key] for key in (
        "kind", "source_reference", "source_revision", "approval_reference", "excerpt",
    ))
    runbook_release["revision"] = f"sha256:{sha256(canonical.encode()).hexdigest()}"
    created = request("operations", "operator", "register_project_context", release=runbook_release)["release"]
    assert created["registered_by"] == "operator" and created["replaces_revision"] is None
    runbook = {key: created[key] for key in ("location", "revision")}
    original = template | {"id": "context-original", "context": template["context"] + [old, runbook]}
    request("operator", "operator", "admit", 409, repository="team/other",
            event_key="context:other-repo", work_order=original, token_limit=40)
    request("operator", "operator", "admit", 409, repository="team/service",
            event_key="context:duplicate", work_order=original | {"context": [old, old]}, token_limit=40)
    request("operator", "operator", "admit", repository="team/service", event_key="context:original",
            work_order=original, token_limit=40)
    queued = original | {"id": "context-queued"}
    request("operator", "operator", "admit", repository="team/service", event_key="context:queued",
            work_order=queued, token_limit=40)
    request_id = str(uuid4())
    assert request("runner", "first", "claim", request_id=request_id)["claim"] is None
    lease = request("runner", "first", "claim", request_id=request_id, context_capable=True)["claim"]
    assert lease["work_id"] == original["id"]
    assert request("runner", "first", "claim", request_id=request_id)["claim"] is None
    binding = {"work_id": original["id"], "generation": lease["generation"]}
    request("runner", "second", "project_context", 409, **binding)
    request("runner", "outside", "project_context", 409, **binding)
    request("runner", "operator", "project_context", 403, **binding)
    context = request("runner", "first", "project_context", **binding)["context"]
    assert {item["kind"]: item["source_revision"] for item in context} == {"runbook": "12", "decision": "4"}

    policy = root / "confluence-context.json"
    provider_state = root / "confluence-craft-state"
    provider_state.mkdir()
    policy.write_text(json.dumps({"schema_version": 1, "craft": str(agent), "craft_state": str(provider_state),
                                  "craft_profile": "fixture-confluence", "cloud_id": "11111111-2222-3333-4444-555555555555",
                                  "space_id": "200", "page_id": "101", "repository": "team/service",
                                  "location": location, "kind": "decision"}), encoding="utf-8")
    excerpt = root / "confluence-excerpt.txt"
    excerpt.write_text("Approved replacement routing decision", encoding="utf-8")
    capture = [str(launcher.resolve()), "capture-confluence-context", str(policy), "--excerpt", str(excerpt)]
    preview = json.loads(subprocess.check_output(capture, text=True, timeout=10))
    approval_file = root / "confluence-approval.json"
    approval_file.write_text(json.dumps({"schema_version": 2, "approval_digest": preview["approval_digest"],
                                         "approval_reference": "fixture:approved/decision/5"}), encoding="utf-8")
    prepared = json.loads(subprocess.check_output(capture + ["--approval", str(approval_file)], text=True, timeout=10))
    payload = prepared["release"]
    request("operations", "reviewer", "register_project_context", 403, release=payload,
            expected_revision=old["revision"])
    request("operations", "operator", "register_project_context", 409, release=payload)
    request("operations", "operator", "register_project_context", 409, release=payload,
            expected_revision="sha256:" + "0" * 64)
    request("operations", "operator", "register_project_context", 409,
            release=payload | {"excerpt": "Unapproved replacement"}, expected_revision=old["revision"])
    assert request("runner", "first", "project_context", **binding)["context"] == context
    register = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                "register-confluence-context", str(policy), "--excerpt", str(excerpt),
                "--approval", str(approval_file), "--replace", old["revision"]]
    environment = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    check_source = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                    "check-confluence-context", str(policy)]
    different = json.loads(subprocess.check_output(check_source, env=environment, text=True, timeout=15))
    assert different["status"] == "different_binding" and different["registered"]["revision"] == old["revision"]
    changed = subprocess.run(register, env=environment | {"FACTORY_FIXTURE_CONFLUENCE_CHANGED": "1"},
                             capture_output=True, text=True, timeout=15)
    assert changed.returncode != 0 and "changed after approval" in changed.stderr
    original_excerpt = excerpt.read_bytes()
    excerpt.write_bytes(original_excerpt + b"\n")
    changed = subprocess.run(register, env=environment, capture_output=True, text=True, timeout=15)
    assert changed.returncode != 0 and "excerpt changed after approval" in changed.stderr
    excerpt.write_bytes(original_excerpt)
    current_pin = request("operations", "operator", "inspect_project_context",
                          repository="team/service", location=location)["catalog"]
    assert current_pin["items"][0]["revision"] == old["revision"]
    assert request("runner", "first", "project_context", **binding)["context"] == context
    with ThreadPoolExecutor(max_workers=2) as pool:
        submissions = [pool.submit(subprocess.check_output, register, env=environment, text=True, timeout=20)
                       for _ in range(2)]
        first, replay = [json.loads(submission.result())["release"] for submission in submissions]
    assert first == replay and first["registered_by"] == "operator" and first["registered_at"]
    assert first["revision"] == payload["revision"] and first["replaces_revision"] == old["revision"]
    assert "excerpt" not in first
    inspect = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
               "inspect-project-context", "team/service", "--location", location]
    active = json.loads(subprocess.check_output(inspect, env=environment, text=True, timeout=10))["catalog"]
    assert active["items"] == [first] and active["total"] == 1
    current_source = json.loads(subprocess.check_output(check_source + ["--require-current"], env=environment,
                                                        text=True, timeout=15))
    assert current_source["status"] == "current" and current_source["registered"]["revision"] == first["revision"]
    assert "excerpt" not in current_source["registered"]
    scope = {"repository": "team/service", "location": location}
    assert request("context-reader", "context-reader", None, **scope)["catalog"] == active
    wrong_scope = request("context-reader", "context-reader", None, 403, **(scope | {"repository": "team/other"}))
    assert wrong_scope["error"] == "context_repository_not_allowed"
    request("context-reader", "service-verifier", None, 403, **scope)
    request("context-reader", "context-reader", None, 422, **(scope | {"revision": old["revision"]}))
    request("operations", "context-reader", "inspect_project_context", 403, **scope)
    request("operations", "context-reader", "withdraw_project_context", 403,
            **scope, revision=first["revision"], evidence="fixture:denied")
    token_file = root / "scope-context-reader-token"
    with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as file:
        file.write(tokens["context-reader"] + "\n")
    readonly_env = {name: value for name, value in os.environ.items() if not name.startswith("CRAFT_FACTORY_")}
    readonly_command = [*check_source, "--context-reader", "--context-reader-token-file", str(token_file), "--require-current"]
    report_path = root / "last-context-report.json"
    scheduled_command = [*readonly_command, "--report-file", str(report_path)]
    assert json.loads(subprocess.check_output(scheduled_command, env=readonly_env, text=True, timeout=15)) == current_source
    assert json.loads(report_path.read_text(encoding="utf-8")) == current_source
    for protected_path in (token_file, certificate, policy, provider_state / "report.json"):
        previous_contents = protected_path.read_bytes() if protected_path.exists() else None
        refused = subprocess.run([*readonly_command, "--report-file", str(protected_path)],
                                 env=readonly_env, capture_output=True, text=True, timeout=15)
        assert refused.returncode != 0 and "context check report" in refused.stderr
        assert (protected_path.read_bytes() if protected_path.exists() else None) == previous_contents
    for fixture in ("FACTORY_FIXTURE_CONFLUENCE_CHANGED", "FACTORY_FIXTURE_CONFLUENCE_DIFF_BODY"):
        drift = subprocess.run(scheduled_command, env=readonly_env | {fixture: "1"},
                               capture_output=True, text=True, timeout=15)
        assert drift.returncode != 0
        assert json.loads(drift.stdout)["status"] == "source_changed"
        assert json.loads(report_path.read_text(encoding="utf-8")) == json.loads(drift.stdout)
    completed_report = report_path.read_bytes()
    failed = subprocess.run(scheduled_command, env=readonly_env | {"FACTORY_FIXTURE_CONFLUENCE_WRONG_SPACE": "1"},
                            capture_output=True, text=True, timeout=15)
    assert failed.returncode != 0 and not failed.stdout
    assert report_path.read_bytes() == completed_report
    assert json.loads(subprocess.check_output(inspect, env=environment, text=True, timeout=10))["catalog"] == active
    history = json.loads(subprocess.check_output(inspect + ["--revision", old["revision"]],
                                                env=environment, text=True, timeout=10))["catalog"]
    assert history["items"][0]["retired_at"] and history["items"][0]["revision"] == old["revision"]
    assert "excerpt" not in history["items"][0]
    assert request("operations", "operator", "inspect_project_context", repository="team/other")["catalog"]["total"] == 0
    request("operations", "reviewer", "inspect_project_context", 403, repository="team/service")
    replacement = {key: prepared["release"][key] for key in ("location", "revision")}
    request("runner", "first", "project_context", 409, **binding)
    request("runner", "first", "renew", 409, **binding)
    request("runner", "first", "submit_candidate", 409, **binding, commit=CANDIDATE,
            evidence_ref="fixture:late", tokens=10)
    assert request("operator", "operator", "status", work_id=queued["id"])["work"]["state"] == "cancelled"
    request("operator", "operator", "admit", 409, repository="team/service", event_key="context:stale",
            work_order=original | {"id": "context-stale"}, token_limit=40)
    current = original | {
        "id": "context-current", "context": template["context"] + [replacement, runbook],
    }
    request("operator", "operator", "admit", repository="team/service", event_key="context:current",
            work_order=current, token_limit=40)
    attempt = root / current["id"]
    assert finish(launch("first", attempt, str(uuid4())))["work_id"] == current["id"]
    with closing(sqlite3.connect(attempt / "state.db")) as db:
        note, = db.execute("SELECT note FROM events WHERE kind = 'project_context_resolved'").fetchone()
    provenance = json.loads(note)
    assert {item["revision"] for item in provenance} == {replacement["revision"], runbook["revision"]}
    assert all("excerpt" not in item for item in provenance)
    assert {item["approval_reference"] for item in provenance} == {
        "fixture:approved/decision/5", "fixture:approved/runbook/12",
    }
    confluence = next(item for item in provenance if item["revision"] == replacement["revision"])
    assert confluence["source_revision"] == f"5/{preview['source']['digest']}"
    withdrawal = {"repository": "team/service", "location": location, "revision": replacement["revision"],
                  "evidence": "fixture:withdraw-context"}
    request("operations", "reviewer", "withdraw_project_context", 403, **withdrawal)
    request("operations", "operator", "withdraw_project_context", 409, **(withdrawal | {"revision": old["revision"]}))
    withdraw = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
                "withdraw-project-context", "team/service", location, replacement["revision"],
                "--evidence", withdrawal["evidence"]]
    withdrawn = json.loads(subprocess.check_output(withdraw, env=environment, text=True, timeout=10))["release"]
    assert withdrawn["retired_by"] == "operator" and withdrawn["retired_at"] and "excerpt" not in withdrawn
    assert request("operations", "operator", "withdraw_project_context", **withdrawal)["release"] == withdrawn
    request("operations", "operator", "withdraw_project_context", 409,
            **(withdrawal | {"evidence": "fixture:different-withdrawal"}))
    assert request("operator", "operator", "status", work_id=current["id"])["work"]["state"] == "cancelled"
    assert request("operations", "operator", "inspect_project_context", repository="team/service",
                   location=location)["catalog"] == {"items": [], "total": 0}
    unregistered = json.loads(subprocess.check_output(check_source, env=environment, text=True, timeout=15))
    assert unregistered["status"] == "unregistered" and unregistered["registered"] is None
    assert request("operations", "operator", "inspect_project_context", repository="team/service",
                   location=location, revision=replacement["revision"])["catalog"]["items"] == [withdrawn]


def exercise_standing_pickup(root, template, request, launch, finish, launcher, address, certificate, tokens):
    sql("""
        INSERT INTO craft_queue.standing_rules (
            id, intake_id, source, project_key, repository, human_account, human_owner,
            required_label, eligible_status, allowed_paths, preferred_runner,
            fallback_runner, fallback_seconds, token_limit, approval_reference, enabled
        ) VALUES (
            'service-pickup', 'intake', 'jira:fixture', 'FIXTURE', 'team/service', 'human-123',
            'Fixture teammate', 'automate', 'Ready', ARRAY['src'], 'first',
            'second', 120, 40, 'fixture:delegation-approval', true
        );
    """)
    snapshot = {
        "source": "jira:fixture", "issue": "FIXTURE-77", "source_revision": 1,
        "observed_at": int(time()), "assignee": "human-123", "status": "Ready", "labels": ["automate"],
    }

    def observe(ticket=snapshot, order=template, identity="intake", expected=200):
        reply = request("intake", identity, "observe", expected, rule="service-pickup",
                        snapshot=ticket, work_order=order)
        return reply.get("decision")

    observe(identity="operator", expected=403)
    observe(identity="first", expected=403)
    observe(identity="other-intake", expected=403)
    assert observe(snapshot | {"observed_at": snapshot["observed_at"] - 301})["state"] == "invalid_observation"
    assert observe(snapshot | {"issue": "ELSE-77"})["state"] == "invalid_observation"
    assert observe(snapshot | {"issue": "FIXTURE-71", "labels": []})["state"] == "ineligible"
    assert observe(snapshot | {"issue": "FIXTURE-72"}, template | {"scope": ["elsewhere"]})["state"] == "scope_not_allowed"
    assert observe(snapshot | {"issue": "FIXTURE-73"}, template | {"acceptance": []})["state"] == "invalid_order"
    request("operator", "operator", "set_paused", paused=True)
    assert observe()["state"] == "paused"
    request("operator", "operator", "set_paused", paused=False)

    evidence = root / "standing-observation.json"
    evidence.write_text(json.dumps({"snapshot": snapshot, "work_order": template}), encoding="utf-8")
    result = subprocess.run(
        [str(launcher.resolve()), "observe-standing-ticket", str(evidence), "--queue", address,
         "--ca", str(certificate), "--rule", "service-pickup"],
        capture_output=True, text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_INTAKE_TOKEN": tokens["intake"]},
    )
    assert result.returncode == 0, result.stderr
    first = json.loads(result.stdout)["decision"]
    assert first["state"] == "admitted"
    work_id = first["work_id"]
    assert observe()["work_id"] == work_id and observe()["state"] == "already_admitted"
    assert observe(snapshot | {"issue": "FIXTURE-78"})["state"] == "review_capacity"
    request("operator", "operator", "admit", repository="team/service",
            event_key="standing:manual", work_order=template | {"id": "standing-manual"}, token_limit=40)
    summary = request("operations", "operator", "summary")["summary"]
    assert summary["queued_standing"] == summary["review_reserved"] == 1
    assert request("runner", "second", "claim", request_id=str(uuid4()))["claim"] is None
    request("operator", "operator", "cancel", work_id="standing-manual", reason="fixture capacity verified")

    sql("UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301;")
    assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
    assert observe(snapshot | {"observed_at": int(time())})["state"] == "already_admitted"
    preferred = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
    assert preferred["work_id"] == work_id and preferred["work_order"]["owner"] == "Fixture teammate"
    assert preferred["work_order"]["issue"] == snapshot["issue"]
    sql(f"""
        UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second'
        WHERE work_id = '{work_id}';
        UPDATE craft_queue.standing_work SET fallback_at = clock_timestamp() - interval '1 second'
        WHERE work_id = '{work_id}';
    """)
    assert request("runner", "outside", "claim", request_id=str(uuid4()))["claim"] is None
    fallback = finish(launch("second", root / "standing-fallback", str(uuid4())))
    assert fallback["work_id"] == work_id and fallback["generation"] == 2
    pack = request("operator", "operator", "download_pack", work_id=work_id)["candidate"]
    audit = request("operations", "operator", "audit", work_id=work_id)["audit"]
    sql("UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301 "
        "WHERE rule_id = 'service-pickup' AND issue_key = 'FIXTURE-77';")
    replay = request("runner", "second", "submit_pack", work_id=work_id, generation=2,
                     commit=pack["commit"], pack_base64=pack["pack_base64"],
                     tokens=audit["attempts"][0]["reported_tokens"])
    assert replay["candidate_evidence"] == pack["digest"]
    binding = request("operator", "operator", "status", work_id=work_id)["work"]["standing_delegation"]
    assert binding["human_account"] == "human-123"
    assert binding["approval_reference"] == "fixture:delegation-approval"
    assert observe(snapshot | {"assignee": "human-456"}, None)["state"] == "stale_or_conflicting"
    reassigned = snapshot | {"assignee": "human-456", "source_revision": 2}
    assert observe(reassigned, None)["state"] == "ineligible"
    assert request("operator", "operator", "status", work_id=work_id)["work"]["state"] == "cancelled"
    assert observe()["state"] == "stale_or_conflicting"

    next_ticket = snapshot | {"issue": "FIXTURE-78"}
    assert observe(next_ticket)["state"] == "admitted"
    working = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
    changed = next_ticket | {"source_revision": 2}
    new_intent = template | {"intent": "A materially different task"}
    assert observe(changed, new_intent)["state"] == "review_required"
    assert observe(changed, new_intent)["state"] == "review_required"
    request("runner", "first", "renew", 409, work_id=working["work_id"], generation=working["generation"])
    newest = changed | {"source_revision": 3}
    assert observe(newest, new_intent)["state"] == "review_required"
    assert sql("SELECT craft_queue.approve_standing_intent('service-pickup', 'FIXTURE-78', 2, 'fixture:obsolete');") == "f"
    assert sql("SELECT craft_queue.approve_standing_intent('service-pickup', 'FIXTURE-78', 3, 'fixture:approved/3');") == "t"
    resumed = observe(newest, new_intent)
    assert resumed["state"] == "admitted"
    approved = request("operator", "operator", "status", work_id=resumed["work_id"])["work"]["standing_delegation"]
    assert approved["intent_approval_reference"] == "fixture:approved/3"
    request("operator", "operator", "cancel", work_id=resumed["work_id"], reason="fixture approved intent")

    new = observe(snapshot | {"issue": "FIXTURE-79"})
    assert new["state"] == "admitted"
    leased = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
    assert leased["work_id"] == new["work_id"]
    assert sql("SELECT craft_queue.disable_standing_rule('service-pickup');") == "t"
    request("runner", "first", "renew", 409, work_id=leased["work_id"], generation=leased["generation"])
    assert observe(snapshot | {"issue": "FIXTURE-80"})["state"] == "disabled"


def exercise_jira_standing(root, template, request, launcher, agent, address, certificate, tokens, scrape):
    cloud_id = "11111111-2222-3333-4444-555555555555"
    sql(f"""
        INSERT INTO craft_queue.standing_rules (
            id, intake_id, source, project_key, repository, human_account, human_owner,
            required_label, eligible_status, allowed_paths, preferred_runner, token_limit,
            approval_reference, enabled
        ) VALUES ('jira-relay-rule', 'intake', 'jira:{cloud_id}', 'FIXTURE', 'team/service',
            'relay-human', 'Jira owner', 'automate', '10001', ARRAY['src'], 'first', 40,
            'fixture:jira-delegation', true);
    """)
    policy_path = root / "jira-standing.json"
    watch = {"issue": "FIXTURE-99", "issue_id": "1042", "rule": "jira-relay-rule", "work_order": template}
    configuration = {
        "schema_version": 1, "craft": str(agent), "craft_state": str(root),
        "craft_profile": "fixture-jira", "cloud_id": cloud_id, "watches": [watch],
    }
    credentials = root / "jira-relay-credentials"
    credentials.mkdir(mode=0o700)
    with os.fdopen(os.open(credentials / "CRAFT_FACTORY_INTAKE_TOKEN", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as token:
        token.write(tokens["intake"] + "\n")

    observation_file = root / "external-observation.json"
    external = [str(launcher.resolve()), "observe-standing-ticket", str(observation_file),
                "--queue", address, "--ca", str(certificate), "--rule", "jira-relay-rule",
                "--intake-token-file", str(credentials / "CRAFT_FACTORY_INTAKE_TOKEN")]
    candidate = {"source": f"jira:{cloud_id}", "issue": "FIXTURE-99", "source_revision": 1,
                 "observed_at": int(time()), "assignee": "relay-human", "status": "10001", "labels": ["automate"]}
    digest = "sha256:" + "a" * 64
    pinned_candidate = candidate | {"intent_digest": digest, "intent_approved": True}
    pin = {"location": f"jira-intent:{cloud_id}/1042", "revision": digest}
    invalid_orders = (
        template,
        template | {"context": [pin | {"revision": "sha256:" + "b" * 64}]},
        template | {"context": [pin | {"location": "jira-intent:22222222-2222-2222-2222-222222222222/1042"}]},
        template | {"context": [pin, pin]},
    )
    for snapshot, order, expected in [(candidate, template, "invalid_observation")] + [
        (pinned_candidate, order, "invalid_order") for order in invalid_orders
    ]:
        observation_file.write_text(json.dumps({"snapshot": snapshot, "work_order": order}), encoding="utf-8")
        reply = json.loads(subprocess.check_output(
            external, env=os.environ | {"CRAFT_FACTORY_INTAKE_TOKEN": "fixture-ambient-not-used"}, timeout=15))
        assert reply["decision"] == {"state": expected}
    assert sql("SELECT count(*) FROM craft_queue.standing_observations WHERE rule_id = 'jira-relay-rule';") == "0"

    def poll(*, queue=True, flag=None, updated="2024-09-05T10:00:00.002+1000", expected=0, full=False,
             budget=None, cursor=None, report=None, explicit_token=False):
        policy_path.write_text(json.dumps(configuration), encoding="utf-8")
        command = [str(launcher.resolve()), "poll-standing-jira", str(policy_path)]
        if queue:
            command += ["--queue", address, "--ca", str(certificate)]
        if budget is not None:
            command += ["--budget-seconds", str(budget)]
        if cursor is not None:
            command += ["--cursor-db", str(cursor)]
        if report is not None:
            command += ["--report-file", str(report)]
        if explicit_token:
            command += ["--intake-token-file", str(credentials / "CRAFT_FACTORY_INTAKE_TOKEN")]
        env = {key: value for key, value in os.environ.items()
               if key not in ("CRAFT_FACTORY_INTAKE_TOKEN", "CRAFT_FACTORY_INTAKE_TOKEN_FILE")} | {
            "CREDENTIALS_DIRECTORY": str(credentials),
            "FACTORY_FIXTURE_JIRA_OBSERVED": str(int(time() * 1000)),
            "FACTORY_FIXTURE_JIRA_UPDATED": updated,
        }
        if flag:
            env[f"FACTORY_FIXTURE_JIRA_{flag}"] = "1"
        if explicit_token:
            env["CRAFT_FACTORY_INTAKE_TOKEN"] = "wrong-ambient-queue-token-fixture"
        output = subprocess.run(command, env=env, capture_output=True, text=True, timeout=22)
        assert output.returncode == expected, output.stderr
        assert "Jira description" not in output.stdout + output.stderr
        payload = json.loads(output.stdout)
        assert payload["submitted"] == (queue and bool(configuration["watches"]))
        if expected:
            assert "see the JSON report" in output.stderr, output.stderr
            return payload
        assert payload["errors"] == []
        return payload if full else payload["observations"][0]

    wrong_issue = poll(flag="WRONG_ID", expected=1)
    assert wrong_issue["observations"] == []
    assert len(wrong_issue["errors"]) == 1
    assert "different Jira issue" in wrong_issue["errors"][0]["reason"]
    configuration["watches"] = [watch | {"issue": "FIXTURE-98", "issue_id": "1043"}, watch]
    for flag, reason in (("WATCH_OUTPUT_LIMIT", "output exceeded its byte limit"), ("WATCH_TIMEOUT", "timed out")):
        failed_read = poll(queue=False, flag=flag, expected=1)
        assert [item["snapshot"]["issue"] for item in failed_read["observations"]] == [watch["issue"]]
        [error] = failed_read["errors"]
        assert error["issue"] == "FIXTURE-98" and error["stage"] == "read" and reason in error["reason"]
    configuration["watches"] = [watch, watch | {"issue": "FIXTURE-98"},
                                watch | {"issue": "FIXTURE-97"}]
    configuration["discoveries"] = [{"rule": "jira-relay-rule", "jql": "project = FIXTURE AND labels = automate"}]
    cursor = root / "jira-relay.db"
    report = root / "jira-relay-last.json"
    bounded = poll(queue=False, flag="WATCH_TIMEOUT", expected=1, budget=18, cursor=cursor, report=report)
    assert json.loads(report.read_bytes()) == bounded
    assert [item["snapshot"]["issue"] for item in bounded["observations"]] == [watch["issue"]]
    assert [(error.get("issue"), error["stage"]) for error in bounded["errors"]] == [
        ("FIXTURE-98", "read"), ("FIXTURE-97", "budget"), (None, "budget"),
    ]
    assert bounded["discoveries"] == []
    with closing(sqlite3.connect(cursor)) as db:
        assert db.execute("SELECT watch_index FROM jira_relay_cursor").fetchone() == (2,)
    resumed_batch = poll(queue=False, full=True, budget=18, cursor=cursor, report=report)
    assert json.loads(report.read_bytes()) == resumed_batch
    assert [item["snapshot"]["issue"] for item in resumed_batch["observations"]] == [
        "FIXTURE-97", "FIXTURE-99", "FIXTURE-98",
    ]
    with closing(sqlite3.connect(cursor)) as db, db:
        assert db.execute("SELECT watch_index FROM jira_relay_cursor").fetchone() == (0,)
        db.execute("UPDATE jira_relay_cursor SET watch_index = 1")
    configuration["watches"].reverse()
    reordered = poll(queue=False, full=True, budget=18, cursor=cursor)
    assert reordered["observations"][0]["snapshot"]["issue"] == "FIXTURE-97"
    configuration["watches"] = [watch, watch | {"issue": "FIXTURE-98"}]
    deferred = poll(queue=False, flag="WATCH_TIMEOUT", expected=1, budget=18, cursor=cursor)
    assert deferred["discoveries"] == []
    with closing(sqlite3.connect(cursor)) as db:
        assert db.execute("SELECT watch_index FROM jira_relay_cursor").fetchone() == (2,)
    resumed_discovery = poll(queue=False, flag="WATCH_TIMEOUT", expected=1, budget=18, cursor=cursor)
    assert len(resumed_discovery["discoveries"]) == 1
    assert resumed_discovery["errors"][-1]["stage"] == "read"
    configuration["watches"] = [watch]
    del configuration["discoveries"]
    initial = poll(queue=False)["snapshot"]
    assert not initial["intent_approved"] and initial["source_revision"] == 1725494400002
    invalid_report = poll(queue=False, expected=1, report=policy_path)
    assert invalid_report["errors"][0]["stage"] == "report"
    assert json.loads(policy_path.read_bytes()) == configuration
    assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
    configuration["discoveries"] = [{"rule": "jira-relay-rule", "jql": "project = FIXTURE AND labels = automate"}]
    discovery = poll(queue=False, full=True)
    assert discovery["discoveries"] == [{"rule": "jira-relay-rule", "has_more": True,
                                         "suggestions": [{"issue": "FIXTURE-101", "issue_id": "1044"}]}]
    for flag, reason in (("WRONG_SEARCH_SITE", "different Jira command or Cloud site"),
                         ("DUPLICATE_SEARCH_ID", "duplicate issue ID")):
        failed = poll(queue=False, flag=flag, expected=1)
        assert failed["observations"][0]["snapshot"]["issue"] == watch["issue"]
        assert failed["discoveries"] == [] and failed["errors"][0]["stage"] == "discover"
        assert reason in failed["errors"][0]["reason"]
    configuration["watches"] = []
    discovery_only = poll(full=True)
    assert discovery_only["observations"] == [] and len(discovery_only["discoveries"][0]["suggestions"]) == 2
    assert sql("SELECT count(*) FROM craft_queue.standing_observations WHERE rule_id = 'jira-relay-rule';") == "0"
    configuration["watches"] = [watch]
    watch["approved_intent"] = initial["intent_digest"]
    configuration["watches"] = [
        watch | {"issue": "FIXTURE-98", "issue_id": "9999"},
        watch | {"rule": "unregistered-rule"},
        watch,
    ]
    mixed = poll(expected=1)
    configuration["watches"] = [watch]
    assert [(error["issue"], error["rule"], error["stage"]) for error in mixed["errors"]] == [
        ("FIXTURE-98", "jira-relay-rule", "read"),
        ("FIXTURE-99", "unregistered-rule", "submit"),
    ], mixed["errors"]
    failed_submit, first = mixed["observations"]
    assert failed_submit["snapshot"]["issue"] == "FIXTURE-99" and "decision" not in failed_submit
    assert first["rule"] == "jira-relay-rule" and first["decision"]["state"] == "admitted"
    assert mixed["discoveries"] == discovery["discoveries"]
    assert sql("SELECT count(*) FROM craft_queue.standing_observations WHERE issue_key = 'FIXTURE-101';") == "0"
    del configuration["discoveries"]
    replay = poll(explicit_token=True)
    assert replay["decision"]["state"] == "already_admitted"
    snapshot = replay["snapshot"]
    external_order = template | {
        "context": template["context"] + [pin | {"revision": snapshot["intent_digest"]}],
    }
    observation_file.write_text(json.dumps({"snapshot": snapshot, "work_order": external_order}), encoding="utf-8")
    assert json.loads(subprocess.check_output(external, timeout=15))["decision"] == replay["decision"]
    claim_args = {"request_id": str(uuid4())}
    lease = request("runner", "first", "claim", **claim_args)["claim"]
    assert lease["work_order"]["owner"] == "Jira owner"
    assert lease["work_order"]["context"][-1]["revision"] == initial["intent_digest"]
    binding = {"work_id": lease["work_id"], "generation": lease["generation"]}

    def owner_inbox(role="operations", token="operator"):
        return request(role, token, "review_inbox", owner="Jira owner")["inbox"]

    assert owner_inbox()["total"] == 0
    baseline = scrape("monitor")["standing_delegation_blocked"]
    sql("UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301 "
        "WHERE rule_id = 'jira-relay-rule' AND issue_key = 'FIXTURE-99';")
    assert scrape("monitor")["standing_delegation_blocked"] == baseline + 1
    assert request("operations", "operator", "summary")["summary"]["standing_delegation_blocked"] == baseline + 1
    inbox = owner_inbox()
    assert inbox["total"] == 1 and inbox["counts"] == {"check_standing_delegation": 1}
    [entry] = inbox["items"]
    assert entry["work_id"] == binding["work_id"] and entry["issue"] == "FIXTURE-99"
    assert entry["state"] == "leased" and entry["next_action"] == "check_standing_delegation"
    assert owner_inbox("reviewer", "reviewer")["total"] == 0
    grant = {"grant_id": "review-jira-blocker", "reviewer_id": "reviewer", "repository": "team/service",
             "evidence": "fixture:jira-blocker-review", "valid_seconds": 60}
    request("operations", "operator", "grant_review_inbox", **grant)
    assert owner_inbox("reviewer", "reviewer")["items"] == inbox["items"]
    request("operations", "operator", "revoke_review_inbox", grant_id=grant["grant_id"],
            evidence="fixture:jira-blocker-scope-verified")
    brief = subprocess.run(
        [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate),
         "review-inbox", "--owner", "Jira owner", "--brief"],
        env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
        capture_output=True, text=True, timeout=15,
    )
    assert brief.returncode == 0, brief.stderr
    assert "Check standing delegation" in brief.stdout and "FIXTURE-99" in brief.stdout
    assert request("runner", "first", "claim", **claim_args)["claim"] is None
    request("runner", "first", "renew", 409, **binding)
    request("runner", "first", "submit_candidate", 409, **binding, commit=CANDIDATE,
            evidence_ref="fixture:stale-standing", tokens=5)
    request("runner", "first", "submit_pack", 409, **binding, commit=CANDIDATE,
            pack_base64=base64.b64encode(b"PACK\0").decode(), tokens=5)
    assert poll()["decision"]["state"] == "already_admitted"
    request("runner", "first", "renew", **binding)
    assert owner_inbox()["total"] == 0
    assert scrape("monitor")["standing_delegation_blocked"] == baseline

    def intent_inbox(repository="team/service"):
        return request("operations", "operator", "operator_brief", repository=repository,
                       usage_hours=24, limit=1)["operator_brief"]["standing_intent_reviews"]

    newer = "2024-09-05T00:01:00.123Z"
    changed = poll(flag="CHANGED", updated=newer)
    assert changed["decision"]["state"] == "review_required"
    [pending_intent] = intent_inbox()["items"]
    assert pending_intent["tracker"] == "jira" and pending_intent["issue"] == watch["issue"]
    assert pending_intent["state"] == "confirm_relay_input" and "review_token" not in pending_intent
    assert intent_inbox("team/other")["items"] == []
    request("runner", "first", "renew", 409, **binding)
    revision = changed["snapshot"]["source_revision"]
    assert sql(f"SELECT craft_queue.approve_standing_intent('jira-relay-rule', 'FIXTURE-99', {revision}, 'fixture:generic');") == "f"
    first_token = changed["decision"]["review_token"]
    read_only = poll(queue=False, flag="CHANGED", updated=newer)
    watch["approved_intent"] = read_only["snapshot"]["intent_digest"]
    watch["work_order"] = template | {"intent": "Approved revised task"}
    revised = poll(flag="CHANGED", updated=newer)
    assert revised["decision"]["state"] == "review_required"
    review_token = revised["decision"]["review_token"]
    assert review_token != first_token
    [ready_intent] = intent_inbox()["items"]
    assert ready_intent["state"] == "ready_for_approval" and ready_intent["review_token"] == review_token
    sql("UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301 "
        "WHERE rule_id = 'jira-relay-rule' AND issue_key = 'FIXTURE-99';")
    [stale_intent] = intent_inbox()["items"]
    assert stale_intent["state"] == "refresh_observation" and "review_token" not in stale_intent

    approval = {"rule_id": "jira-relay-rule", "issue": "FIXTURE-99", "source_revision": revision,
                "review_token": review_token, "evidence": "fixture:reapproved-jira-intent"}
    request("operations", "operator", "approve_jira_standing_intent", 409, **approval)
    assert intent_inbox()["items"] == [stale_intent]
    assert poll(flag="CHANGED", updated=newer)["decision"] == revised["decision"]
    request("operations", "intake", "approve_jira_standing_intent", 403, **approval)
    request("operations", "operator", "approve_jira_standing_intent", 409,
            **(approval | {"review_token": first_token}))
    cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
    env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    command = cli + ["approve-jira-standing-intent", approval["rule_id"], "--issue", approval["issue"],
                     "--source-revision", str(revision), "--review-token", review_token,
                     "--evidence", approval["evidence"]]
    recorded = json.loads(subprocess.check_output(command, env=env, text=True, timeout=15))["intent_approval"]
    assert recorded["actor"] == "operator" and recorded["review_token"] == review_token
    assert intent_inbox()["items"] == []
    sql("UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301 "
        "WHERE rule_id = 'jira-relay-rule' AND issue_key = 'FIXTURE-99';")
    assert request("operations", "operator", "approve_jira_standing_intent", **approval)["intent_approval"] == recorded
    resumed = poll(flag="CHANGED", updated=newer)
    assert resumed["decision"]["state"] == "admitted"
    changed_work = resumed["decision"]["work_id"]
    reassigned = poll(flag="REASSIGNED", updated="2024-09-05T00:02:00.123Z")
    assert reassigned["decision"]["state"] == "ineligible"
    assert request("operator", "operator", "status", work_id=changed_work)["work"]["state"] == "cancelled"
    assert scrape("monitor")["standing_delegation_blocked"] == baseline
    evidence = "fixture:withdraw-the-standing-rule"
    disabled = json.loads(subprocess.check_output(
        cli + ["disable-jira-standing-rule", "jira-relay-rule", "--evidence", evidence],
        env=env, text=True, timeout=15,
    ))["standing_rule"]
    assert not disabled["enabled"] and disabled["disabled_by"] == "operator"
    assert request("operations", "operator", "disable_jira_standing_rule", rule_id="jira-relay-rule",
                   evidence=evidence)["standing_rule"] == disabled
    assert poll()["decision"]["state"] == "disabled"


def exercise_github_standing(root, template, request, launcher, agent, address, certificate, tokens):
    policy = {"id": "github-issue-relay", "intake_id": "intake", "source": "github:team/service",
              "project_key": "GITHUB", "repository": "team/service", "human_account": "36",
              "human_owner": "GitHub owner", "required_label": "automate", "eligible_status": "open",
              "allowed_paths": ["src"], "preferred_runner": "first", "token_limit": 40,
              "approval_reference": "fixture:standing-github"}
    path = root / "github-standing-rule.json"
    path.write_text(json.dumps(policy), encoding="utf-8")
    checksum = "sha256:" + sha256(path.read_bytes()).hexdigest()
    endpoints = ["--queue", address, "--ca", str(certificate)]
    ops = [str(launcher.resolve()), "shared-ops", *endpoints]
    operator = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    request("operations", "intake", "approve_standing_rule", 403, policy=policy, digest=checksum)
    request("operations", "operator", "approve_standing_rule", 400, policy=policy | {"project_key": "OTHER"}, digest=checksum)
    saved = json.loads(subprocess.check_output(
        [*ops, "approve-standing-rule", str(path), "--digest", checksum], env=operator, timeout=15))["standing_rule"]
    assert saved["source"] == "github:team/service" and saved["approved_by"] == "operator"

    issue = {"schema_version": 1, "gh": str(agent), "repository": "team/service", "number": 42, "issue_id": 707}
    native = {"repository_url": "https://api.github.com/repos/team/service",
              "html_url": "https://github.com/team/service/issues/42", "id": 707, "number": 42,
              "title": "Fixture issue", "body": "Private fixture issue body", "state": "open",
              "assignees": [{"id": 36}], "labels": [{"name": "automate"}], "updated_at": "2026-11-01T00:00:00Z"}
    relay_policy = {"rule": policy["id"], "issue": issue}
    order = template | {"id": "github-issue-relay", "issue": "team/service#42"}
    config, provider, order_file = (root / name for name in
                                    ("github-issue-relay.json", "native-github-issue.json", "github-issue-work.json"))
    token_file = root / "github-intake-token"
    with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as token:
        token.write(tokens["intake"] + "\n")
    relay = [str(launcher.resolve()), "poll-standing-github-issue", str(config), str(order_file)]
    fixture = os.environ | {"FACTORY_FIXTURE_GITHUB_ISSUE_JSON": str(provider),
                            "CRAFT_FACTORY_INTAKE_TOKEN": "fixture-unused-ambient-token"}
    report = root / "github-standing-last-report.json"
    scheduled = [*relay, *endpoints, "--intake-token-file", str(token_file), "--report-file"]

    def poll(item, *, submit=True):
        for file, data in ((config, relay_policy), (provider, item), (order_file, order)):
            file.write_text(json.dumps(data), encoding="utf-8")
        command = [*scheduled, report.name] if submit else relay
        result = json.loads(subprocess.check_output(command, cwd=root, env=fixture, timeout=15))
        if submit:
            assert json.loads(report.read_bytes()) == result
        assert "Private fixture issue body" not in json.dumps(result)
        return result

    def inbox(repository="team/service"):
        return request("operations", "operator", "operator_brief", repository=repository,
                       usage_hours=24, limit=1)["operator_brief"]["standing_intent_reviews"]

    preview = poll(native, submit=False)
    snapshot = preview["snapshot"]
    assert "decision" not in preview and not snapshot["intent_approved"]
    assert snapshot["source_issue_id"] == 707 and snapshot["issue"] == "GITHUB-42"
    missing_id = {key: value for key, value in snapshot.items() if key != "source_issue_id"}
    for sample, expected in ((missing_id, "invalid_observation"), (snapshot, "invalid_order")):
        decision = request("intake", "intake", "observe", rule=policy["id"], snapshot=sample, work_order=order)
        assert decision["decision"]["state"] == expected
    request("intake", "intake", "observe", 403, rule=policy["id"],
            snapshot=snapshot | {"source": "github:team/other"}, work_order=order)
    relay_policy["approved_intent"] = snapshot["intent_digest"]
    pending = poll(native)["decision"]
    assert pending["state"] == "review_required" and "work_id" not in pending
    before = inbox()
    assert before["total"] == 1 and before["ready"] == 0
    [item] = before["items"]
    assert item["issue"] == "GITHUB-42" and item["state"] == "confirm_relay_input"
    assert "review_token" not in item
    assert inbox("team/other") == {"total": 0, "ready": 0, "items": []}
    request("operations", "intake", "operator_brief", 403, repository="team/service", usage_hours=24, limit=1)
    approval = {"rule_id": policy["id"], "issue": snapshot["issue"], "source_revision": snapshot["source_revision"],
                "review_token": pending["review_token"], "evidence": "fixture:approved-first-github-issue"}
    request("operations", "operator", "approve_standing_intent", 409, **approval)
    assert poll(native)["decision"] == pending
    confirmed = inbox()
    assert confirmed["total"] == confirmed["ready"] == 1
    [item] = confirmed["items"]
    assert item["state"] == "ready_for_approval" and item["review_token"] == pending["review_token"]
    assert item["issue_url"] == native["html_url"] and "work_id" not in item
    cloud = "11111111-2222-3333-4444-555555555555"
    jira_policy = policy | {"id": "jira-mixed-inbox", "source": f"jira:{cloud}", "project_key": "FIXTURE"}
    jira_digest = "sha256:" + sha256(json.dumps(jira_policy).encode()).hexdigest()
    request("operations", "operator", "approve_jira_standing_rule", policy=jira_policy, digest=jira_digest)
    jira_snapshot = {"source": jira_policy["source"], "issue": "FIXTURE-123", "source_revision": 1,
                     "observed_at": int(time()), "assignee": "36", "status": policy["eligible_status"],
                     "labels": [policy["required_label"]], "intent_digest": "sha256:" + "a" * 64,
                     "intent_approved": False}
    jira_order = template | {"context": template["context"] + [{
        "location": f"jira-intent:{cloud}/123", "revision": jira_snapshot["intent_digest"],
    }]}

    def observe_jira(snapshot):
        return request("intake", "intake", "observe", rule=jira_policy["id"], snapshot=snapshot,
                       work_order=jira_order)["decision"]

    assert observe_jira(jira_snapshot)["state"] == "review_required"
    mixed = inbox()
    assert mixed == {"total": 2, "ready": 1, "items": [item]}
    all_items = request("operations", "operator", "operator_brief", repository="team/service",
                        usage_hours=24, limit=2)["operator_brief"]["standing_intent_reviews"]["items"]
    assert [entry["tracker"] for entry in all_items] == ["github", "jira"]
    assert observe_jira(jira_snapshot | {"source_revision": 2, "assignee": "different-account",
                                         "intent_approved": True})["state"] == "ineligible"
    assert inbox() == confirmed
    ineligible_token = sql("SELECT 'sha256:' || encode(fingerprint, 'hex') FROM craft_queue.standing_observations "
                           "WHERE rule_id = 'jira-mixed-inbox' AND issue_key = 'FIXTURE-123';")
    request("operations", "operator", "approve_jira_standing_intent", 409, rule_id=jira_policy["id"],
            issue=jira_snapshot["issue"], source_revision=2, review_token=ineligible_token,
            evidence="fixture:reject ineligible Jira issue")
    output = subprocess.check_output([*ops, "operator-brief", "--repository", "team/service"],
                                     env=operator, text=True, timeout=15)
    assert "Standing issue approvals | 1 ready" in output and pending["review_token"] in output
    request("operations", "intake", "approve_standing_intent", 403, **approval)
    def age_github_observation():
        sql("UPDATE craft_queue.standing_observations SET source_read_at = source_read_at - 301 "
            "WHERE rule_id = 'github-issue-relay' AND issue_key = 'GITHUB-42';")

    age_github_observation()
    request("operations", "operator", "approve_standing_intent", 409, **approval)
    [stale_item] = inbox()["items"]
    assert stale_item["state"] == "refresh_observation" and "review_token" not in stale_item
    assert poll(native)["decision"] == pending
    recorded = request("operations", "operator", "approve_standing_intent", **approval)
    age_github_observation()
    assert request("operations", "operator", "approve_standing_intent", **approval) == recorded
    assert inbox() == {"total": 0, "ready": 0, "items": []}
    original_intent = order["intent"]
    order["intent"] = "Different work order at the reviewed revision"
    assert poll(native)["decision"]["state"] == "stale_or_conflicting"
    order["intent"] = original_intent
    comment_update = native | {"updated_at": "2026-11-01T00:00:01Z"}
    first = poll(comment_update)["decision"]
    assert first["state"] == "admitted"
    assert poll(comment_update)["decision"] == {"state": "already_admitted", "work_id": first["work_id"]}
    admitted = request("operator", "operator", "status", work_id=first["work_id"])["work"]["standing_delegation"]
    assert admitted["intent_approval_reference"] == approval["evidence"]
    completed = report.read_bytes()
    for protected in (config, order_file, agent, certificate, token_file):
        before = Path(protected).read_bytes()
        refused = subprocess.run([*scheduled, str(protected)], cwd=root, env=fixture, text=True,
                                 capture_output=True, timeout=15)
        assert refused.returncode != 0 and "report path must be distinct" in refused.stderr
        assert Path(protected).read_bytes() == before
    provider.write_text(json.dumps(native | {"id": 999}), encoding="utf-8")
    failed_read = subprocess.run([*scheduled, report.name], cwd=root, env=fixture, capture_output=True, timeout=15)
    assert failed_read.returncode != 0 and not failed_read.stdout and report.read_bytes() == completed
    provider.write_text(json.dumps(native), encoding="utf-8")
    wrong_token = root / "wrong-github-intake-token"
    with os.fdopen(os.open(wrong_token, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as token:
        token.write("fixture-invalid-intake-credential\n")
    failed_queue = subprocess.run([*relay, *endpoints, "--intake-token-file", str(wrong_token), "--report-file", report.name],
                                  cwd=root, env=fixture, capture_output=True, timeout=15)
    assert failed_queue.returncode != 0 and not failed_queue.stdout and report.read_bytes() == completed
    assert poll(comment_update | {"assignees": [{"id": 36}, {"id": 99}]})["decision"]["state"] == "stale_or_conflicting"
    assert request("operator", "operator", "status", work_id=first["work_id"])["work"]["state"] == "cancelled"
    assert poll(comment_update)["decision"]["state"] == "stale_or_conflicting"

    revised = native | {"body": "New private requirement", "updated_at": "2026-11-01T00:00:02Z"}
    observed = poll(revised)
    assert observed["decision"]["state"] == "review_required"
    relay_policy["approved_intent"] = observed["snapshot"]["intent_digest"]
    reviewed = poll(revised)
    approval = {"rule_id": policy["id"], "issue": snapshot["issue"], "source_revision": reviewed["snapshot"]["source_revision"],
                "review_token": reviewed["decision"]["review_token"], "evidence": "fixture:reapproved-github-issue"}
    request("operations", "intake", "approve_standing_intent", 403, **approval)
    request("operations", "operator", "approve_standing_intent", **approval)
    resumed = poll(revised)["decision"]
    assert resumed["state"] == "admitted" and resumed["work_id"] != first["work_id"]
    closed = revised | {"state": "closed", "updated_at": "2026-11-01T00:00:03Z"}
    assert poll(closed)["decision"]["state"] == "ineligible"
    assert request("operator", "operator", "status", work_id=resumed["work_id"])["work"]["state"] == "cancelled"
    request("operations", "operator", "disable_standing_rule", rule_id=policy["id"], evidence="fixture:withdraw")
    assert poll(native)["decision"]["state"] == "disabled"
    print("Verified GitHub standing intake, first operator approval, exact replay, reapproval, closure and withdrawal.")


def exercise_release_series(root, repo, mapping, template, request, launch, finish, launcher, agent, address, certificate, tokens):
    downstream = root / "series-repo-two"
    subprocess.run(["git", "clone", "--quiet", "--no-local", str(repo), str(downstream)], check=True)
    subprocess.run(["git", "-C", str(downstream), "-c", "user.name=Fixture",
                    "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-qm", "Downstream base"],
                   check=True)
    second_base = subprocess.check_output(["git", "-C", str(downstream), "rev-parse", "HEAD"]).decode().strip()
    mapping.write_text(json.dumps({"team/service": {"path": str(repo)}, "team/other": {"path": str(downstream)}}),
                       encoding="utf-8")
    target = {"service": "upstream", "environment": "staging", "region": "fixture", "resource": "upstream-one"}
    next_target = target | {"service": "downstream", "resource": "downstream-one"}
    final_target = target | {"service": "follow-up", "resource": "follow-up-one"}
    upstream_order = template | {"id": "series-upstream", "owner": "Upstream owner"}
    downstream_order = template | {
        "id": "series-downstream", "owner": "Downstream owner", "base": second_base,
    }
    plan = {
        "id": "series-fixture", "owner": "Cross-repository release owner", "approval": "fixture:approved-rollout-order",
        "members": [
            {"repository": "team/service", "observer": "release-one", "target": target,
             "token_limit": 40, "work_order": upstream_order},
            {"repository": "team/other", "observer": "release-other", "target": next_target,
             "token_limit": 40, "work_order": downstream_order, "requires_rollout_proof": True},
            {"repository": "team/service", "observer": "release-one", "target": final_target,
             "token_limit": 40, "work_order": template | {"id": "series-final", "owner": "Follow-up owner"}},
        ],
    }
    invalid = plan | {"id": "series-invalid", "members": [plan["members"][0],
        plan["members"][1] | {"work_order": downstream_order | {"id": upstream_order["id"]}}]}
    request("series", "operator", "register", 409, **invalid)
    request("operator", "operator", "status", 404, work_id=upstream_order["id"])
    proposal = {"schema_version": 1, "id": plan["id"], "owner": plan["owner"],
                "source": {"location": "repo:decisions/fixture.md", "revision": second_base},
                "compatibility": "Verify the new API while keeping old consumers working.",
                "rollback": "Disable dependent consumers before withdrawing the API release.",
                "members": [member | {"work_order": member["work_order"] | {"repository": member["repository"]}}
                            for member in plan["members"]]}
    seed_path = root / "series-seed.json"
    seed_bytes = json.dumps(proposal).encode("utf-8")
    seed_path.write_bytes(seed_bytes)
    seed_digest = "sha256:" + sha256(seed_bytes).hexdigest()
    coordinator_home = root / "coordinator-home"
    coordinator_home.mkdir()
    draft_dir = root / "series-draft"
    drafted = subprocess.run([str(launcher.resolve()), "draft-series-proposal", str(seed_path),
                              "--seed-digest", seed_digest,
                              "--codex", str(agent), "--codex-home", str(coordinator_home),
                              "--destination", str(draft_dir), "--timeout-seconds", "10"],
                             env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
                             capture_output=True, text=True, timeout=15)
    assert drafted.returncode == 0, drafted.stderr
    draft = json.loads(drafted.stdout)
    assert draft["state"] == "awaiting_approval" and draft["usage"]["input_tokens"] == 13
    plan_path = draft_dir / "proposal.json"
    plan_bytes = plan_path.read_bytes()
    seed_guard = ["--against-seed", str(seed_path), "--seed-digest", seed_digest]
    checked = json.loads(subprocess.check_output(
        [str(launcher.resolve()), "review-series-proposal", str(plan_path), *seed_guard], text=True, timeout=15))
    digest = "sha256:" + sha256(plan_bytes).hexdigest()
    assert checked["state"] == "awaiting_approval" and checked["proposal_digest"] == draft["proposal_digest"] == digest
    assert checked["compatibility"] == "Preserve the upstream contract during the rollout."
    assert checked["seed_review"]["seed_compatible"] and checked["seed_review"]["seed_digest"] == seed_digest
    assert [member["waits_for"] for member in checked["members"]] == [None, upstream_order["id"], downstream_order["id"]]
    request("series", "operator", "status", 404, id=plan["id"])
    command = [str(launcher.resolve()), "shared-series", "--queue", address, "--ca", str(certificate)]
    register_command = command + ["register-proposal", str(plan_path), "--approval-reference", plan["approval"], *seed_guard]

    def register(identity, approved_digest=digest):
        return subprocess.run(register_command + ["--proposal-digest", approved_digest], capture_output=True, text=True, timeout=15,
                              env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens[identity]})

    plan_path.write_bytes(plan_bytes + b"\n")
    changed = register("operator")
    assert changed.returncode != 0 and "series proposal changed after approval" in changed.stderr
    request("series", "operator", "status", 404, id=plan["id"])
    expanded = json.loads(plan_bytes)
    expanded["members"][0]["token_limit"] += 1
    expanded_bytes = json.dumps(expanded).encode("utf-8")
    plan_path.write_bytes(expanded_bytes)
    changed = register("operator", "sha256:" + sha256(expanded_bytes).hexdigest())
    assert changed.returncode != 0 and "proposal changes approved constraints" in changed.stderr
    request("series", "operator", "status", 404, id=plan["id"])
    plan_path.write_bytes(plan_bytes)
    denied = register("first")
    assert denied.returncode != 0 and "HTTP 403" in denied.stderr
    request("series", "operator", "status", 404, id=plan["id"])
    registered = register("operator")
    assert registered.returncode == 0, registered.stderr
    series = json.loads(registered.stdout)["series"]
    assert series["approval_reference"] == f"{plan['approval']} [proposal {digest}] [seed {seed_digest}]"
    assert json.loads(register("operator").stdout)["series"] == series
    request("series", "operator", "register", 409, **(plan | {"approval": "fixture:other-plan"}))
    request("series", "first", "status", 403, id=plan["id"])
    assert series["members"][1]["waiting_on"] == upstream_order["id"]
    def observer_status():
        return request("release-observer", "release-other", "status",
                       work_id=downstream_order["id"])["observer_status"]

    request("release-observer", "operator", "status", 403, work_id=downstream_order["id"])
    forbidden = request("release-observer", "release-other", "status", 404, work_id=upstream_order["id"])
    assert forbidden == request("release-observer", "release-other", "status", 404, work_id="missing-series-order")
    waiting = observer_status()
    assert waiting["repository"] == "team/other" and waiting["target"] == next_target
    assert waiting["next_action"] == "prerequisite_not_current" and waiting["verification_sequence"] is None
    assert all(value not in json.dumps(waiting) for value in (upstream_order["id"], "team/service", plan["approval"]))
    observer_cli = subprocess.run(command + ["observer-status", downstream_order["id"]],
                                  capture_output=True, text=True, timeout=15,
                                  env=os.environ | {"CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN": tokens["release-other"]})
    assert observer_cli.returncode == 0, observer_cli.stderr
    assert json.loads(observer_cli.stdout)["observer_status"] == waiting
    request("release-observer", "operator", "inbox", 403, limit=20)
    request("release-observer", "release-one", "inbox", 400, limit=51)
    request("release-observer", "release-one", "inbox", 400, limit=1, after="../series")
    mine = request("release-observer", "release-other", "inbox", limit=20)["observer_inbox"]
    assert mine == {"items": [waiting], "next_after": None}
    other_repository = request("release-observer", "release-one", "inbox",
                               limit=20, repository="team/other")["observer_inbox"]
    assert other_repository == {"items": [], "next_after": None}
    own_repository = {"repository": "team/service", "limit": 1}
    first_page = request("release-observer", "release-one", "inbox",
                         after=downstream_order["id"], **own_repository)["observer_inbox"]
    assert first_page["next_after"] == first_page["items"][0]["work_id"] == "series-final"
    second_page = request("release-observer", "release-one", "inbox", **own_repository,
                          after=first_page["next_after"])["observer_inbox"]
    assert second_page["items"][0]["work_id"] == upstream_order["id"] and second_page["next_after"] is None
    inbox_cli = subprocess.run(
        command + ["observer-inbox", "--repository", "team/service", "--limit", "1", "--after", first_page["next_after"]],
        capture_output=True, text=True, timeout=15,
        env=os.environ | {"CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN": tokens["release-one"]},
    )
    assert inbox_cli.returncode == 0, inbox_cli.stderr
    assert json.loads(inbox_cli.stdout)["observer_inbox"] == second_page
    dispatch = request("operations", "operator", "dispatch_status", work_id=downstream_order["id"])["dispatch"]
    assert dispatch["waiting_on"] == upstream_order["id"] and "release_predecessor_not_ready" in dispatch["blockers"]
    assert request("runner", "outside", "claim", request_id=str(uuid4()))["claim"] is None

    def verify(candidate):
        return request("verifier", "verifier", "record_verification",
                       work_id=candidate["work_id"], generation=candidate["generation"],
                       head=candidate["candidate"], digest=candidate["candidate_evidence"],
                       previous_sequence=0, passed=True, evidence=f"fixture:verified/{candidate['work_id']}",
                       note="Fixture acceptance checks passed")["verification_sequence"]

    first = finish(launch("first", root / "series-upstream", str(uuid4())))
    upstream_receipt = {
        "work_id": first["work_id"], "generation": first["generation"], "head": first["candidate"],
        "verification_sequence": verify(first), "target": target,
        "artifact_digest": f"sha256:{'4' * 64}", "evidence": "fixture:upstream-staging",
    }
    request("release-observer", "operator", "observe", 403, **upstream_receipt)
    request("release-observer", "release-other", "observe", 409, **upstream_receipt)
    request("release-observer", "release-one", "observe", 409, **(upstream_receipt | {"target": next_target}))
    request("release-observer", "release-one", "observe", 409, **(upstream_receipt | {"verification_sequence": 0}))
    receipt_path = root / "series-release.json"
    receipt_path.write_text(json.dumps(upstream_receipt), encoding="utf-8")
    recorded = subprocess.run(command + ["observe", str(receipt_path)], capture_output=True, text=True, timeout=15,
                              env=os.environ | {"CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN": tokens["release-one"]})
    assert recorded.returncode == 0, recorded.stderr
    observed = json.loads(recorded.stdout)["release"]
    assert request("release-observer", "release-one", "observe", **upstream_receipt)["release"] == observed
    request("operator", "operator", "cancel", 409, work_id=first["work_id"], reason="fixture checks explicit rollback")
    partial = request("series", "operator", "status", id=plan["id"])["series"]
    assert partial["state"] == "partial" and partial["members"][1]["waiting_on"] is None
    assert observer_status()["next_action"] == "wait_for_candidate"
    assert request("operations", "operator", "summary")["summary"]["review_reserved"] == 0

    request_id = str(uuid4())
    second = request("runner", "outside", "claim", request_id=request_id)["claim"]
    assert second["work_id"] == downstream_order["id"]
    binding = request("series", "operator", "status", id=plan["id"])["series"]["members"][1]["predecessor_releases"]
    assert binding[first["work_id"]]["artifact_digest"] == upstream_receipt["artifact_digest"]

    def withdraw(observation, evidence):
        return request("release-observer", "release-one", "withdraw",
                       work_id=first["work_id"], sequence=observation["sequence"], evidence=evidence)["release"]

    def observe_changes(**overrides):
        return request("release-observer", "release-one", "observe",
                       **(upstream_receipt | overrides))["release"]

    assert withdraw(observed, "fixture:upstream-rollback")["withdrawn_at"] is not None
    assert observer_status()["next_action"] == "prerequisite_not_current"
    downstream_lease = {"work_id":second["work_id"], "generation":second["generation"]}
    request("runner", "outside", "renew", 409, **downstream_lease)
    request("runner", "outside", "submit_candidate", 409, **downstream_lease, commit=CANDIDATE,
            evidence_ref="fixture:stale-output", tokens=10)
    incompatible = observe_changes(artifact_digest=f"sha256:{'5' * 64}", evidence="fixture:other-artifact")
    assert observer_status()["next_action"] == "prerequisite_changed"
    assert request("series", "operator", "status", id=plan["id"])["series"]["members"][1]["waiting_on"] == first["work_id"]
    request("runner", "outside", "renew", 409, **downstream_lease)
    withdraw(incompatible, "fixture:rollback-other-artifact")
    resumed = observe_changes(evidence="fixture:original-artifact-back")
    request("runner", "outside", "renew", **downstream_lease)
    candidate = finish(launch("outside", root / "series-downstream", request_id))
    assert candidate["work_id"] == second["work_id"]
    assert observer_status()["next_action"] == "wait_for_verification"
    downstream_verdict = verify(candidate)
    assert observer_status()["next_action"] == "wait_for_rollout_approval"
    sql("INSERT INTO craft_queue.draft_policies VALUES ('team/other','main','publisher',true);")
    pending = request("publication", "operator", "reserve", **downstream_lease,
                      head=candidate["candidate"], verification_sequence=downstream_verdict)["publication"]
    assert observer_status()["next_action"] == "reconcile_publication"
    withdraw(resumed, "fixture:pause-publisher")
    [operator_item] = request("operations", "operator", "review_inbox", repository="team/other")["inbox"]["items"]
    assert operator_item["waiting_on"] == first["work_id"] and operator_item["dependency_blocked"]
    grant_id = "review-downstream-only"
    request("operations", "operator", "grant_review_inbox", grant_id=grant_id, reviewer_id="reviewer",
            repository="team/other", evidence="fixture:downstream-review", valid_seconds=3600)
    [reviewer_item] = request("reviewer", "reviewer", "review_inbox")["inbox"]["items"]
    assert reviewer_item["dependency_blocked"] and reviewer_item["next_action"] == "wait_for_predecessor"
    assert reviewer_item["waiting_on"] is None and first["work_id"] not in json.dumps(reviewer_item)
    request("operations", "operator", "revoke_review_inbox", grant_id=grant_id, evidence="fixture:review-complete")
    request("publisher", "publisher", "begin", 409, action_key=pending["action_key"])
    request("publication", "operator", "reconcile", action_key=pending["action_key"], observation={
        "outcome": "absent", "head": candidate["candidate"], "evidence": "fixture:reserved send never began",
    })
    observe_changes(evidence="fixture:upstream-restored-for-rollout")
    strict_release = downstream_lease | {
        "head": candidate["candidate"], "verification_sequence": downstream_verdict,
    }
    request("release-observer", "release-other", "observe", 409, **strict_release,
            target=next_target, artifact_digest=f"sha256:{'6' * 64}", evidence="fixture:manual-observation")
    sql("UPDATE craft_queue.control SET review_limit = 2;")
    assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
    verify_series_rollout(root, request, command, tokens, strict_release, downstream_order["issue"], next_target)
    sql("UPDATE craft_queue.control SET review_limit = 1;")
    final = finish(launch("first", root / "series-final", str(uuid4())))
    assert final["work_id"] == "series-final"
    request("release-observer", "release-one", "observe", work_id=final["work_id"],
            generation=final["generation"], head=final["candidate"],
            verification_sequence=verify(final), target=final_target,
            artifact_digest=f"sha256:{'7' * 64}", evidence="fixture:follow-up-deployed")
    complete = request("series", "operator", "status", id=plan["id"])["series"]
    assert complete["state"] == "released" and complete["released_members"] == 3
    delivered = observer_status()
    assert delivered["next_action"] == "released" and delivered["release_current"]
    assert delivered["verification_sequence"] == downstream_verdict
    assert delivered["latest_release"]["artifact_digest"] == "sha256:" + "6" * 64
    open_items = request("release-observer", "release-other", "inbox", limit=20)["observer_inbox"]
    assert open_items == {"items": [], "next_after": None}
    history = request("release-observer", "release-other", "inbox",
                      limit=20, include_closed=True)["observer_inbox"]
    assert history == {"items": [delivered], "next_after": None}
    mapping.write_text(json.dumps({"team/service": {"path": str(repo)}}), encoding="utf-8")


def verify_series_rollout(root, request, command, tokens, binding, work_item, target):
    policy = json.loads((ROOT.parent / "examples/remediation-policy.json").read_text())
    evidence = json.loads((ROOT.parent / "examples/release-evidence.json").read_text())
    artifact = f"sha256:{'6' * 64}"
    policy["artifact_digest"] = artifact
    policy["required_targets"][0]["target"] = target
    policy["change"].update(repository="team/other", source_commit=binding["head"], work_item=work_item)
    evidence["inventory"][0]["target"] = target
    evidence["deployments"][0]["target"] = target
    evidence["inventory"][0]["running"]["artifact_digest"] = artifact
    evidence["deployments"][0]["version"]["artifact_digest"] = artifact
    evidence["deployments"][0]["repository"] = "team/other"
    evidence["builds"][0].update(repository="team/other", artifact_digest=artifact, source_commit=binding["head"])
    evidence["changes"][0].update(repository="team/other", result_commit=binding["head"], work_item=work_item)
    now = int(time())
    evidence["inventory"][0]["observation"]["observed_at"] = now - 10
    evidence["deployments"][0]["observation"]["observed_at"] = now - 15
    evidence["deployments"][0]["deployed_at"] = now - 30
    evidence["builds"][0]["observation"]["observed_at"] = now - 45
    evidence["builds"][0]["built_at"] = now - 60
    evidence["changes"][0]["observation"]["observed_at"] = now - 70
    evidence["changes"][0]["merged_at"] = now - 80
    policy_bytes = json.dumps(policy).encode()
    evidence_bytes = json.dumps(evidence).encode()
    policy_digest = f"sha256:{sha256(policy_bytes).hexdigest()}"
    evidence_digest = f"sha256:{sha256(evidence_bytes).hexdigest()}"
    body = binding | {
        "policy_base64": base64.b64encode(policy_bytes).decode(),
        "evidence_base64": base64.b64encode(evidence_bytes).decode(),
    }
    request("series-rollout", "release-other", None, 409, **body)
    approval = binding | {"policy_digest": policy_digest, "approval": "fixture:approved-rollout-policy"}
    request("series", "release-other", "approve_rollout", 403, **approval)
    request("series", "operator", "approve_rollout", 409, **(approval | {"verification_sequence": 0}))
    approval_path = root / "strict-rollout-approval.json"
    approval_path.write_text(json.dumps(approval), encoding="utf-8")
    approved = subprocess.run(command + ["approve-rollout", str(approval_path)],
                              capture_output=True, text=True, timeout=15,
                              env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]})
    assert approved.returncode == 0, approved.stderr
    assert json.loads(approved.stdout)["rollout_approval"]["policy_digest"] == policy_digest
    observer = request("release-observer", "release-other", "status", work_id=binding["work_id"])["observer_status"]
    assert observer["next_action"] == "check_rollout_evidence" and observer["policy_digest"] == policy_digest
    request("series-rollout", "release-one", None, 409, **body)
    request("series-rollout", "operator", None, 403, **body)
    request("series-rollout", "release-other", None, 409, **(body | {
        "policy_base64": base64.b64encode(policy_bytes + b" ").decode(),
    }))
    request("series-rollout", "release-other", None, 400, **(body | {"as_of": now - 1000}))
    evidence["inventory"][0]["observation"]["observed_at"] = now - 400
    request("series-rollout", "release-other", None, 409, **(body | {
        "evidence_base64": base64.b64encode(json.dumps(evidence).encode()).decode(),
    }))

    policy_path = root / "strict-rollout-policy.json"
    release_path = root / "strict-release-evidence.json"
    policy_path.write_bytes(policy_bytes)
    release_path.write_bytes(evidence_bytes)
    verified = subprocess.run(command + [
        "observe-rollout", "--work-id", binding["work_id"], "--generation", str(binding["generation"]),
        "--head", binding["head"], "--verification-sequence", str(binding["verification_sequence"]),
        "--policy", str(policy_path), "--release", str(release_path),
    ], capture_output=True, text=True, timeout=15,
       env=os.environ | {"CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN": tokens["release-other"]})
    assert verified.returncode == 0, verified.stderr
    receipt = json.loads(verified.stdout)["release"]
    assert receipt["rollout_policy_digest"] == policy_digest
    assert receipt["rollout_evidence_digest"] == evidence_digest
    assert request("series-rollout", "release-other", None, **body)["release"] == receipt


def exercise_archived_rollout(server, launcher, root, certificate, key, config, ca, tokens, database_url, *, github=False):
    original = root / "incident-archive"
    prefix = "github-archive" if github else "archive"
    archive = root / f"{prefix}-rollout-originals"
    archive.mkdir()
    now = int(time())

    def stamp(seconds):
        return datetime.fromtimestamp(now + seconds, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")

    def save(name, value):
        raw = json.dumps(value).encode()
        (archive / name).write_bytes(raw)
        return {"path": name, "sha256": f"sha256:{sha256(raw).hexdigest()}"}

    def load(name):
        return json.loads((original / name).read_bytes())

    manifest = load("sources.json")
    approval = load("approval.json")
    repo = "team/github-release" if github else f"{approval['workspace']}/{approval['repository']}"
    provider = "github" if github else "bitbucket"
    source_repository = f"{provider}:{repo}"
    work_item, reviewed, merged = (approval[field] for field in ("work_item", "approved_source_commit", "result_commit"))
    github_pr = f"https://github.com/{repo}/pull/42"
    base = "e" * 40
    pack_digest = "sha256:" + sha256(b"PACK").hexdigest()
    manifest["as_of"] = now - 5
    kube = manifest["kubernetes"]
    kube["inventory"]["observed_at"] = now - 5
    kube["deployment"]["replica_set_observed_at"] = now - 7
    target = {name: kube["inventory"][name] for name in ("service", "environment", "region", "resource")}
    pod = load("pod.json")
    pod["metadata"]["creationTimestamp"] = stamp(-40)
    pod["status"]["containerStatuses"][0]["state"]["running"]["startedAt"] = stamp(-30)
    replica_set = load("replica-set.json")
    replica_set["metadata"]["creationTimestamp"] = stamp(-45)
    for metadata in (pod["metadata"], replica_set["spec"]["template"]["metadata"]):
        metadata["annotations"]["craft.factory/source-repository"] = source_repository
    kube["deployment"]["repository"] = source_repository
    kube["pod"], kube["replica_set"] = save("pod.json", pod), save("replica-set.json", replica_set)
    envelope = load("build.dsse.json")
    statement = json.loads(base64.b64decode(envelope["payload"]))
    statement["predicate"]["runDetails"]["metadata"]["finishedOn"] = stamp(-60)
    build_policy = load("build-policy.json")
    if github:
        build_policy.update(repository=source_repository, source_uri=f"git+https://github.com/{repo}@refs/heads/main")
        [dependency] = statement["predicate"]["buildDefinition"]["resolvedDependencies"]
        dependency["uri"] = build_policy["source_uri"]
    payload = json.dumps(statement).encode()
    input_path = archive / "signing-input"
    payload_type = envelope["payloadType"]
    input_path.write_bytes(f"DSSEv1 {len(payload_type)} {payload_type} {len(payload)} ".encode() + payload)
    signature = subprocess.check_output([
        "openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(root / "fixture-build-signer.pem"),
        "-in", str(input_path),
    ], timeout=10)
    envelope["payload"] = base64.b64encode(payload).decode()
    envelope["signatures"] = [{"sig": base64.b64encode(signature).decode()}]
    manifest["build"] = {"attestation": save("build.dsse.json", envelope),
                         "policy": save("build-policy.json", build_policy), "observed_at": now - 55}
    approval.update(approved_at=now - 100, merged_at=now - 80)
    if github:
        identity = {"id": 100, "full_name": repo}
        pull_request = {"number": 42, "html_url": github_pr, "state": "closed", "merged": True,
                        "created_at": stamp(-120), "merged_at": stamp(-80), "merge_commit_sha": merged,
                        "head": {"sha": reviewed, "repo": identity}, "base": {"ref": "main", "repo": identity}}
        approval = {field: approval[field] for field in (
            "schema_version", "approved_source_commit", "result_commit", "work_item", "intent_reference",
            "intent_revision", "approval_reference", "approved_at", "merged_at",
        )} | {"repository": repo, "repository_id": 100, "destination_branch": "main", "pull_request_number": 42}
    else:
        pull_request = load("pull-request.json")
        pull_request.update(created_on=stamp(-120), updated_on=stamp(-70))
    manifest["change"] = {"pull_request": save("pull-request.json", pull_request),
                          "approval": save("approval.json", approval), "observed_at": now - 65}
    if github:
        manifest["change"]["provider"] = "github"
    manifest_pin = save("sources.json", manifest)
    assert reviewed != merged
    artifact = "sha256:" + statement["subject"][0]["digest"]["sha256"]
    rollout = save("policy.json", {
        "schema_version": 1, "artifact_digest": artifact, "release_max_age_seconds": 120,
        "required_targets": [{"target": target, "config_revision": pod["metadata"]["annotations"]["craft.factory/config-revision"]}],
        "change": {"repository": source_repository, "source_commit": merged,
                   "pull_request": github_pr if github else f"https://bitbucket.org/{repo}/pull-requests/{approval['pull_request_id']}",
                   **{field: approval[field] for field in ("work_item", "intent_reference", "intent_revision", "approval_reference")}},
    })
    prepare = [str(launcher.resolve()), "prepare-rollout-archive", str(archive / rollout["path"]),
               "--policy-digest", rollout["sha256"], "--work-id", f"{prefix}-rollout",
               "--reviewed-head", reviewed, "--as-of", str(now),
               "--release", str(archive / manifest_pin["path"]), manifest_pin["sha256"],
               "--destination", str(archive / "index.json")]
    prepared = json.loads(subprocess.check_output(prepare, text=True, timeout=10))
    catalog_entry = prepared["catalog_entry"]
    assert prepared["verification"]["rollout_verified"]
    sources = {"path": catalog_entry["sources"], "sha256": catalog_entry["sources_digest"]}
    catalog = archive / "catalog.json"
    cases = []
    bindings = []
    dependency_intake = "sha256:" + "d" * 64
    for work_id, head in [(f"{prefix}-rollout", reviewed), (f"{prefix}-mismatch", reviewed if github else merged)]:
        dependency = not github and work_id == "archive-rollout"
        payload = {"issue": work_item, "base": base}
        if dependency:
            payload.update(dependency_intake_digest=dependency_intake, require_native_checks=True)
        sql(f"""
            INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens, state, generation, candidate, candidate_evidence)
                VALUES ('{work_id}', '{repo}', '{work_id}', '{json.dumps(payload)}',
                    16, 'candidate', 1, '{head}', '{pack_digest}');
            INSERT INTO craft_queue.attempts (work_id, generation, runner_id, request_id, expires_at, started_at, token_limit, outcome)
                VALUES ('{work_id}', 1, 'first', '{uuid4()}', now(), now(), 16, 'candidate');
            INSERT INTO craft_queue.candidate_packs VALUES ('{work_id}', 1, '{pack_digest}', decode('5041434b', 'hex'));
            INSERT INTO craft_queue.release_series (id, owner_name, approval_reference, registered_by, plan_digest)
                VALUES ('{work_id}', 'Fixture operator', 'fixture:series', 'operator', 'fixture');
            INSERT INTO craft_queue.series_members (series_id, position, work_id, observer_id, release_target, requires_rollout_proof)
                VALUES ('{work_id}', 1, '{work_id}', 'release-one', '{json.dumps(target)}', true);
        """)
        sequence = None if dependency else int(sql(f"""INSERT INTO craft_queue.verifications
            (work_id, generation, head, digest, verifier, passed, evidence, note)
            VALUES ('{work_id}', 1, '{head}', '{pack_digest}', 'verifier', true, 'fixture:independent', 'passed')
            RETURNING sequence;"""))
        cases.append(catalog_entry | {"work_id": work_id})
        bindings.append({"work_id": work_id, "generation": 1, "head": head, "verification_sequence": sequence})
    if github:
        sql(f"""INSERT INTO craft_queue.draft_policies (repository, base_branch, publisher_id, enabled, forge)
            VALUES ('{repo}', 'main', 'publisher', true, 'github');""")
        sql("UPDATE craft_queue.control SET paused = false;")
    catalog.write_text(json.dumps(cases), encoding="utf-8")
    with run_service(server, certificate, key, config, database_url, "--rollout-archives", str(catalog)) as (address, _process):
        request = api_client(address, ca, tokens)
        binding, wrong_revision = bindings
        observer_env = os.environ | {"CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN": tokens["release-one"]}

        def observer_command(coordinates):
            return [str(launcher.resolve()), "shared-series", "--queue", address, "--ca", str(ca),
                    "observe-archived-rollout", "--work-id", coordinates["work_id"], "--generation",
                    str(coordinates["generation"]), "--head", coordinates["head"], "--verification-sequence",
                    str(coordinates["verification_sequence"])]

        if not github:
            candidate = {name: binding[name] for name in ("work_id", "generation", "head")}
            approved_pins = {"intake_digest": dependency_intake, "policy_digest": "sha256:" + "a" * 64,
                             "baseline_run_digest": "sha256:" + "b" * 64, "candidate_run_digest": "sha256:" + "c" * 64,
                             "scanner_fresh_through": now + 300}
            request("operations", "operator", "approve_dependency_evidence", 409, approval=candidate | {
                "id": "archive-stale-dependency-proof", "pins": approved_pins | {"scanner_fresh_through": now - 1},
                "evidence": "fixture:stale-dependency-ci"})
            request("operations", "operator", "approve_dependency_evidence", approval=candidate | {
                "id": "archive-dependency-proof", "pins": approved_pins, "evidence": "fixture:dependency-ci"})
            def pin_deadline(value):
                sql(f"""UPDATE craft_queue.dependency_evidence_approvals
                    SET pins = jsonb_set(pins, '{{scanner_fresh_through}}', '{value}')
                    WHERE id = 'archive-dependency-proof';""")

            def attest_deadline(pins, expected=200):
                return request("verifier", "verifier", "record_dependency_verification", expected, **candidate,
                               digest=pack_digest, previous_sequence=0, passed=True, approval_id="archive-dependency-proof",
                               proof=pins | {"report_digest": "sha256:" + "e" * 64})

            pin_deadline(now - 1)
            attest_deadline(approved_pins | {"scanner_fresh_through": now - 1}, 409)
            pin_deadline(now + 300)
            result = attest_deadline(approved_pins)
            binding["verification_sequence"] = result["verification_sequence"]
            def expire_scan(value):
                sql(f"""UPDATE craft_queue.dependency_verifications
                    SET proof = jsonb_set(proof, '{{scanner_fresh_through}}', '{value}')
                    WHERE verification_sequence = {binding['verification_sequence']};""")

            assert sql(f"""SELECT craft_queue.approve_series_rollout(
                'operator', '{binding['work_id']}', 1, '{reviewed}', {binding['verification_sequence']},
                '{rollout['sha256']}', 'fixture:unarchived', NULL::text) IS NULL;""") == "t"
        request("series-rollout", "release-one", None, 409, archived=True, **binding)
        check_args = {"archived": True, "check_only": True}
        unavailable = request("series-rollout", "release-one", None, **check_args, **binding)["preflight"]
        assert not unavailable["evidence_ready"] and unavailable["blocker"] == "rollout_binding_unavailable"
        request("series", "operator", "approve_rollout", 409, **binding,
                policy_digest=rollout["sha256"], approval="fixture:archive-policy")
        request("series", "operator", "approve_rollout", 409, **binding,
                policy_digest=rollout["sha256"], approval="fixture:archive-policy", archive_index_digest="sha256:" + "0" * 64)
        approval_fields = {"policy_digest": rollout["sha256"], "approval": "fixture:archive-policy",
                           "archive_index_digest": sources["sha256"]}
        if not github:
            expire_scan(now - 1)
            request("series", "operator", "approve_rollout", 409, **binding, **approval_fields)
            expire_scan(now + 300)
        for item in bindings:
            approved = request("series", "operator", "approve_rollout", **item, **approval_fields)["rollout_approval"]
            assert approved["archive_index_digest"] == sources["sha256"]
            assert request("series", "operator", "approve_rollout", **item, **approval_fields)["rollout_approval"] == approved
        if not github:
            expire_scan(now - 1)
            expired = subprocess.run(observer_command(binding) + ["--check-only"], env=observer_env,
                                     capture_output=True, text=True, timeout=10)
            assert expired.returncode != 0, expired.stdout
            preview = json.loads(expired.stdout)["preflight"]
            assert not preview["evidence_ready"] and preview["blocker"] == "dependency_scan_expired"
            denied = request("series-rollout", "release-other", None, **check_args, **binding)["preflight"]
            assert denied == unavailable
            wrong_head = request("series-rollout", "release-one", None, **check_args,
                                 **(binding | {"head": "e" * 40}))["preflight"]
            assert not wrong_head["evidence_ready"] and wrong_head["blocker"] == "rollout_binding_unavailable"
            request("series-rollout", "release-one", None, 409, archived=True, **binding)
            expired_binding = binding.copy()
            refreshed_pins = approved_pins | {"scanner_refresh_digest": "sha256:" + "f" * 64,
                                               "scanner_fresh_through": now + 600}
            approval_id = "archive-refreshed-dependency-proof"
            refreshed_approval = request("operations", "operator", "approve_dependency_evidence", approval=candidate | {
                "id": approval_id, "previous_approval_id": "archive-dependency-proof", "pins": refreshed_pins,
                "evidence": "fixture:dependency-scan-refresh"})
            assert refreshed_approval["approval"]["pins"] == refreshed_pins
            current = request("verifier", "verifier", "status", work_id=binding["work_id"])["work"]
            assert not current["independent_checks_passed"]
            refreshed = request("verifier", "verifier", "record_dependency_verification", **candidate,
                                digest=pack_digest, previous_sequence=current["verification"]["sequence"],
                                passed=True, approval_id=approval_id,
                                proof=refreshed_pins | {"report_digest": "sha256:" + "e" * 64})
            binding["verification_sequence"] = refreshed["verification_sequence"]
            request("series-rollout", "release-one", None, 409, archived=True, **expired_binding)
            superseded = request("series-rollout", "release-one", None, **check_args, **expired_binding)["preflight"]
            assert not superseded["evidence_ready"] and superseded["blocker"] == "rollout_binding_unavailable"
            request("series-rollout", "release-one", None, 409, archived=True, **binding)
            unavailable = request("series-rollout", "release-one", None, **check_args, **binding)["preflight"]
            assert not unavailable["evidence_ready"] and unavailable["blocker"] == "rollout_binding_unavailable"
            renewed = request("series", "operator", "approve_rollout", **binding, **approval_fields)["rollout_approval"]
            assert renewed["archive_index_digest"] == sources["sha256"]
        if github:
            request("series-rollout", "release-one", None, 409, archived=True, **binding)
            pin = {"policy_file_digest": "sha256:" + "1" * 64, "protection_digest": "sha256:" + "2" * 64}
            digest = "sha256:" + sha256(
                f"github-shadow-v1\n{pin['policy_file_digest']}\n{pin['protection_digest']}\n".encode()
            ).hexdigest()
            sql(f"""INSERT INTO craft_queue.shadow_policies VALUES ('{repo}', '{digest}', true,
                'operator', 'fixture:approved-github-protection', to_timestamp({now - 98}), to_timestamp({now + 120}));""")
            for item, url, result_commit in (
                (binding, github_pr, merged),
                (wrong_revision, f"https://github.com/{repo}/pull/43", "f" * 40),
            ):
                work_id = item["work_id"]
                request("publication", "operator", "bind_github_review", **item,
                        observation={"native_url": url, "head": reviewed, "base": base,
                                     "source_branch": f"craft-factory/{work_id}", "base_branch": "main"})
                receipt_id = f"{work_id}-shadow"
                report_digest = "sha256:" + sha256(receipt_id.encode()).hexdigest()
                request("operations", "operator", "record_shadow_merge", **item, receipt_id=receipt_id,
                        report_digest=report_digest, pack_digest=pack_digest,
                        assessment={"github": pin, "repository": repo, "base": base, "pull_request": url,
                                    "policy_digest": digest, "observation_digest": report_digest, "as_of": now - 90,
                                    "eligible": False, "native_verified": False, "blockers": ["github_pr_not_ready"]})
                request("series-rollout", "release-one", None, 409, archived=True, **item)
                request("verifier", "verifier", "confirm_github_shadow_merge", receipt_id=receipt_id,
                        head=reviewed, report_digest=report_digest, observation={
                            "pull_request": url, "head": reviewed, "pinned_base": base, "destination_branch": "main",
                            "result_commit": result_commit, "destination_tip": result_commit,
                            "merged_at": now - 80, "actor": "github:user:88",
                            "reference": f"https://github.com/{repo}/commit/{result_commit}",
                        })
                request("operations", "operator", "close_github_work", 409,
                        work_id=binding["work_id"], merge_receipt_id=receipt_id, head=reviewed,
                        evidence="fixture:series-requires-rollout-evidence")
            assert sql(f"""SELECT craft_queue.record_series_verified_rollout(
                'release-one', '{binding['work_id']}', 1, '{reviewed}', {binding['verification_sequence']},
                '{rollout['sha256']}', '{sources['sha256']}', '{artifact}', {now + 113}, '{sources['sha256']}',
                '{{"pull_request":"{github_pr}","result_commit":"{'f' * 40}"}}'::jsonb) IS NULL;""") == "t"
        assert sql(f"""SELECT craft_queue.record_series_verified_rollout(
            'release-one', '{binding['work_id']}', 1, '{reviewed}', {binding['verification_sequence']},
            '{rollout['sha256']}', '{sources['sha256']}', '{artifact}', {now + 100}) IS NULL;""") == "t"
        assert sql(f"""SELECT craft_queue.record_series_verified_rollout(
            'release-one', '{binding['work_id']}', 1, '{reviewed}', {binding['verification_sequence']},
            '{rollout['sha256']}', 'sha256:{'0' * 64}', '{artifact}', {now + 100}, '{sources['sha256']}') IS NULL;""") == "t"
        request("series-rollout", "release-one", None, 409, policy_base64="e30=", evidence_base64="e30=", **binding)
        request("series-rollout", "release-one", None, 400, archived=True, evidence_base64="e30=", **binding)
        request("series-rollout", "release-other", None, 409, archived=True, **binding)
        request("series-rollout", "release-one", None, 409, archived=True, **wrong_revision)
        denied = request("series-rollout", "release-other", None, **check_args, **binding)["preflight"]
        assert denied == unavailable
        mismatch = request("series-rollout", "release-one", None, **check_args, **wrong_revision)["preflight"]
        assert not mismatch["evidence_ready"] and mismatch["blocker"] == "candidate_or_artifact_mismatch"
        index = archive / "index.json"
        original_bytes = index.read_bytes()
        index.write_bytes(original_bytes + b" ")
        request("series-rollout", "release-one", None, 409, archived=True, **binding)
        observe = observer_command(binding)
        damaged = subprocess.run(observe + ["--check-only"], env=observer_env, capture_output=True, text=True, timeout=10)
        assert damaged.returncode != 0
        assert json.loads(damaged.stdout)["preflight"]["blocker"] == "invalid_evidence"
        index.write_bytes(original_bytes)
        preview = json.loads(subprocess.check_output(observe + ["--check-only"], env=observer_env, text=True,
                                                     timeout=10))["preflight"]
        assert preview["evidence_ready"] and preview["archive_index_digest"] == sources["sha256"]
        assert preview["artifact_digest"] == artifact and preview["fresh_through"] == now + 113
        unchanged = request("series", "operator", "status", id=binding["work_id"])["series"]["members"][0]
        assert unchanged["state"] == "candidate" and unchanged["latest_release"] is None
        checked = subprocess.run(observe, env=observer_env, capture_output=True, text=True, timeout=10)
        assert checked.returncode == 0, checked.stderr
        receipt = json.loads(checked.stdout)["release"]
        assert receipt["head"] == reviewed and receipt["artifact_digest"] == artifact
        assert receipt["rollout_policy_digest"] == rollout["sha256"]
        assert receipt["rollout_evidence_digest"] == sources["sha256"]
        assert receipt["rollout_fresh_through"] == now + 113
        if github:
            assert receipt["github_merge"] == {"pull_request": github_pr, "result_commit": merged}
        else:
            print("Verified dependency scan refresh fences the old rollout approval and permits the first rollout with new approval.")
            expire_scan(now - 1)
        assert request("series-rollout", "release-one", None, archived=True, **binding)["release"] == receipt
        replay_with_new_expiry = sql(f"""SELECT craft_queue.record_series_verified_rollout(
            'release-one', '{binding['work_id']}', 1, '{reviewed}', {binding['verification_sequence']},
            '{rollout['sha256']}', '{sources['sha256']}', '{artifact}', {now + 300}, '{sources['sha256']}') IS NULL;""")
        assert replay_with_new_expiry == "t"
        runner_id = f"{prefix}-fixture-runner"
        dependent_id = f"{prefix}-dependent"
        sql(f"""
            UPDATE craft_queue.control SET paused = false, review_limit = 99;
            INSERT INTO craft_queue.runner_repositories VALUES ('{runner_id}','team/{dependent_id}');
            INSERT INTO craft_queue.work (id, repository, event_key, payload, reserved_tokens)
                VALUES ('{dependent_id}', 'team/{dependent_id}', 'fixture:{dependent_id}', '{{"issue":"DEP-1"}}', 16);
            INSERT INTO craft_queue.series_members (series_id, position, work_id, observer_id, release_target)
                VALUES ('{binding['work_id']}', 2, '{dependent_id}', 'release-one', '{json.dumps(target)}');
        """)
        claim = uuid4()
        assert sql(f"SELECT work_id FROM craft_queue.claim('{runner_id}', '{claim}');") == dependent_id
        status = request("series", "operator", "status", id=binding["work_id"])["series"]
        assert status["members"][0]["release_current"] and status["members"][1]["waiting_on"] is None
        sql(f"UPDATE craft_queue.series_observations SET rollout_fresh_through = {now - 1} WHERE sequence = {receipt['sequence']};")
        status = request("series", "operator", "status", id=binding["work_id"])["series"]
        assert status["released_members"] == 0 and not status["members"][0]["release_current"]
        assert status["members"][0]["latest_release"]["sequence"] == receipt["sequence"]
        assert status["members"][1]["waiting_on"] == binding["work_id"]
        expired = request("release-observer", "release-one", "status",
                          work_id=binding["work_id"])["observer_status"]
        assert expired["next_action"] == "refresh_rollout_evidence" and not expired["release_current"]
        assert expired["latest_release"]["rollout_fresh_through"] == now - 1
        expired_inbox = request("release-observer", "release-one", "inbox", limit=50,
                                repository=repo)["observer_inbox"]
        assert expired in expired_inbox["items"]
        assert not sql(f"SELECT work_id FROM craft_queue.claim('{runner_id}', '{claim}');")
        assert sql(f"SELECT craft_queue.renew('{runner_id}', '{dependent_id}', 1) IS NULL;") == "t"
        sql(f"UPDATE craft_queue.series_observations SET rollout_fresh_through = {receipt['rollout_fresh_through']} WHERE sequence = {receipt['sequence']};")
        def replace_catalog(entries):
            raw = json.dumps(entries).encode()
            replacement = catalog.with_suffix(".next.json")
            replacement.write_bytes(raw)
            replacement.replace(catalog)
            return f"sha256:{sha256(raw).hexdigest()}"

        # Reject invalid replacements while the approved snapshot can still verify.
        pinned = replace_catalog(cases + [cases[0]])
        request("series", "release-one", "reload_rollout_catalog", 403, digest=pinned)
        request("series", "operator", "reload_rollout_catalog", 400, digest="invalid")
        request("series", "operator", "reload_rollout_catalog", 409, digest=pinned)
        wrong_pin = replace_catalog([cases[0] | {"sources_digest": "sha256:" + "0" * 64}])
        request("series", "operator", "reload_rollout_catalog", 409, digest=wrong_pin)
        assert request("series-rollout", "release-one", None, **check_args, **binding)["preflight"]["evidence_ready"]

        # A new index requires approval even when its runtime evidence cannot extend the receipt.
        if github:
            stale_manifest = save("stale-sources.json", manifest | {"max_age_seconds": 1})
            changed_bytes = json.dumps({"schema_version": 1, "releases": [stale_manifest]}).encode()
        else:
            changed_bytes = (archive / "index.json").read_bytes() + b" "
        (archive / "next-index.json").write_bytes(changed_bytes)
        new_index_digest = f"sha256:{sha256(changed_bytes).hexdigest()}"
        for entry in cases:
            entry.update(sources="next-index.json", sources_digest=new_index_digest)
        catalog_digest = replace_catalog(cases)
        request("series", "operator", "reload_rollout_catalog", 409, digest=pinned)
        series_cli = [str(launcher.resolve()), "shared-series", "--queue", address, "--ca", str(ca)]
        operator_env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
        reloaded = json.loads(subprocess.check_output(series_cli + ["reload-rollout-catalog", "--digest", catalog_digest],
                                                     env=operator_env, text=True, timeout=10))["rollout_catalog"]
        assert reloaded == {"changed": True, "digest": catalog_digest, "entries": len(cases)}
        assert request("series", "operator", "reload_rollout_catalog", digest=catalog_digest)["rollout_catalog"] == (
            reloaded | {"changed": False})
        request("series-rollout", "release-one", None, 409, archived=True, **binding)
        request("series", "operator", "approve_rollout", 409, **binding,
                **(approval_fields | {"archive_index_digest": new_index_digest}))
        refresh = {"work_id": binding["work_id"], "refresh_id": f"{prefix}-equivalent-rollout-index",
                   "observation_sequence": receipt["sequence"], "policy_digest": rollout["sha256"],
                   "archive_index_digest": new_index_digest, "approval": "fixture:approve-refresh"}
        request("series", "release-one", "approve_rollout_refresh", 403, **refresh)
        request("series", "operator", "approve_rollout_refresh", **refresh)
        request("series-rollout", "release-one", None, 409, archived=True, refresh_id=refresh["refresh_id"], **binding)
        if github:
            stale = request("series-rollout", "release-one", None, **check_args, refresh_id=refresh["refresh_id"],
                            **binding)["preflight"]
            assert stale["blocker"] == "targets_not_verified"
            assert stale["pending_targets"] == [{"target": target, "status": "evidence_gap"}]

        manifest["as_of"] = now - 1
        kube["inventory"]["observed_at"] = now - 1
        kube["deployment"]["replica_set_observed_at"] = now - 2
        renewed_sources = save("refresh-index.json", {"schema_version": 1, "releases": [save("refreshed-sources.json", manifest)]})
        for entry in cases:
            entry.update(sources=renewed_sources["path"], sources_digest=renewed_sources["sha256"])
        catalog_digest = replace_catalog(cases)
        assert request("series", "operator", "reload_rollout_catalog", digest=catalog_digest)["rollout_catalog"]["changed"]
        sql(f"UPDATE craft_queue.series_observations SET rollout_fresh_through = {now - 1} WHERE sequence = {receipt['sequence']};")
        refresh.update(refresh_id=f"{prefix}-fresh-runtime-rollout-index", archive_index_digest=renewed_sources["sha256"])
        request("series-rollout", "release-one", None, 409, archived=True, refresh_id=refresh["refresh_id"], **binding)
        approval_file = archive / "refresh-approval.json"
        approval_file.write_text(json.dumps(refresh), encoding="utf-8")
        approved = subprocess.run(series_cli + ["approve-rollout-refresh", str(approval_file)],
            env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, capture_output=True, text=True, timeout=10)
        assert approved.returncode == 0, approved.stderr
        assert json.loads(approved.stdout)["rollout_refresh"]["verified_at"] is None
        request("series", "operator", "approve_rollout_refresh", 409,
                **(refresh | {"refresh_id": f"{prefix}-duplicate-refresh"}))
        preview = request("series-rollout", "release-one", None, **check_args, refresh_id=refresh["refresh_id"],
                          **binding)["preflight"]
        assert preview["evidence_ready"] and preview["archive_index_digest"] == renewed_sources["sha256"]
        still_expired = request("series", "operator", "status", id=binding["work_id"])["series"]["members"][0]
        assert not still_expired["release_current"] and still_expired["latest_rollout_refresh"] is None
        transfer = {
            "transfer_id": f"{prefix}-observer-handover", "work_id": binding["work_id"],
            "expected_revision": 0, "observer_id": "release-other", "evidence": "fixture:observer handover approved",
        }
        assert still_expired["observer_revision"] == 0
        request("series", "release-one", "transfer_observer", 403, **transfer)
        request("series", "operator", "transfer_observer", 404, **(transfer | {"observer_id": "first"}))
        request("series", "operator", "transfer_observer", 409, **(transfer | {"expected_revision": 1}))
        cmd = series_cli + ["transfer-observer", transfer["transfer_id"], "--work-id", transfer["work_id"],
                            "--expected-revision", "0", "--observer-id", "release-other",
                            "--evidence", transfer["evidence"]]
        run = subprocess.run(cmd, env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
                             capture_output=True, text=True, timeout=10)
        assert run.returncode == 0, run.stderr
        handover = json.loads(run.stdout)["observer_transfer"]
        assert handover["previous_observer"] == "release-one" and handover["next_observer"] == "release-other"
        assert handover["transferred_by"] == "operator" and handover["revision"] == 1
        assert request("series", "operator", "transfer_observer", **transfer)["observer_transfer"] == handover
        request("series", "operator", "transfer_observer", 409, **(transfer | {"evidence": "fixture:changed"}))
        request("release-observer", "release-one", "status", 404, work_id=binding["work_id"])
        inbox = request("release-observer", "release-other", "inbox", limit=50, repository=repo)["observer_inbox"]
        assert any(item["work_id"] == binding["work_id"] for item in inbox["items"])
        request("series-rollout", "release-one", None, 409, archived=True, refresh_id=refresh["refresh_id"], **binding)
        request("release-observer", "release-one", "withdraw", 409, work_id=binding["work_id"],
                sequence=receipt["sequence"], evidence="fixture:denied former observer")
        verified = subprocess.run(series_cli + ["observe-archived-rollout", "--work-id", binding["work_id"],
            "--generation", "1", "--head", reviewed, "--verification-sequence", str(binding["verification_sequence"]),
            "--refresh-id", refresh["refresh_id"]], env=os.environ | {"CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN": tokens["release-other"]},
            capture_output=True, text=True, timeout=10)
        assert verified.returncode == 0, verified.stderr
        refresh_receipt = json.loads(verified.stdout)["rollout_refresh"]
        assert refresh_receipt["rollout_fresh_through"] == now + 118 and refresh_receipt["verified_by"] == "release-other"
        assert request("series-rollout", "release-other", None, archived=True,
                       refresh_id=refresh["refresh_id"], **binding)["rollout_refresh"] == refresh_receipt
        status = request("series", "operator", "status", id=binding["work_id"])["series"]
        prior, dependent = status["members"]
        assert prior["release_current"] and dependent["waiting_on"] is None
        assert prior["observer"] == "release-other" and prior["observer_revision"] == 1
        assert prior["latest_observer_transfer"] == handover
        refreshed = request("release-observer", "release-other", "status",
                            work_id=binding["work_id"])["observer_status"]
        assert refreshed["next_action"] == "released" and refreshed["release_current"]
        assert refreshed["latest_release"]["rollout_fresh_through"] == now + 118
        fresh_inbox = request("release-observer", "release-other", "inbox", limit=50,
                               repository=repo)["observer_inbox"]
        assert binding["work_id"] not in [item["work_id"] for item in fresh_inbox["items"]]
        assert prior["latest_release"]["sequence"] == receipt["sequence"] and prior["latest_release"]["withdrawn_at"] is None
        assert prior["latest_release"]["observer_id"] == "release-one"
        assert prior["latest_rollout_refresh"] == refresh_receipt
        withdrawn = request("release-observer", "release-other", "withdraw", work_id=binding["work_id"],
                            sequence=receipt["sequence"], evidence="fixture:confirmed-rollback")["release"]
        assert withdrawn["observer_id"] == "release-one" and withdrawn["withdrawn_by"] == "release-other"
        request("series-rollout", "release-other", None, 409, archived=True, refresh_id=refresh["refresh_id"], **binding)
        print(f"Verified {prefix} rollout catalog replacement and approval without restarting the queue.")


def exercise_draft_policy_setup(request, launcher, address, ca, tokens):
    repo = "team/draft-config"
    policy = {"repository": repo, "base_branch": "release/main", "publisher_id": "publisher",
              "forge": "github", "expected_revision": 0, "enabled": True, "evidence": "fixture:draft setup"}
    request("operations", "operator", "draft_policy", 404, repository=repo)
    request("operations", "publisher", "configure_draft_policy", 403, **policy)
    request("operations", "operator", "configure_draft_policy", 409, **(policy | {"publisher_id": "first"}))
    request("operations", "operator", "configure_draft_policy", 400, **(policy | {"base_branch": "release/a..b"}))
    args = [str(launcher), "shared-ops", "--queue", address, "--ca", str(ca)]
    result = subprocess.run(args + ["configure-draft-policy", repo, "--base-branch", policy["base_branch"],
                            "--publisher", "publisher", "--forge", "github", "--expected-revision", "0",
                            "--enabled", "true", "--evidence", policy["evidence"]],
                            capture_output=True, text=True, timeout=10,
                            env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]})
    assert result.returncode == 0, result.stderr
    enabled = json.loads(result.stdout)["policy"]
    assert enabled["enabled"] and enabled["revision"] == 1 and enabled["decision"]["actor"] == "operator"
    assert enabled == request("operations", "operator", "draft_policy", repository=repo)["policy"]
    assert enabled == request("operations", "operator", "configure_draft_policy", **policy)["policy"]
    request("operations", "operator", "configure_draft_policy", 409,
            **(policy | {"expected_revision": 1, "enabled": False, "forge": "bitbucket"}))


def set_draft_policy(request, enabled, evidence, expected=200):
    current = request("operations", "operator", "draft_policy", repository="team/service")["policy"]
    return request("operations", "operator", "configure_draft_policy", expected,
                   repository=current["repository"], base_branch=current["base_branch"],
                   publisher_id=current["publisher_id"], forge=current["forge"],
                   expected_revision=current["revision"], enabled=enabled, evidence=evidence)


def exercise_publication(request, candidate, reconcile):
    order_id = candidate["id"]
    reservation = {
        "work_id": order_id, "generation": candidate["generation"], "head": candidate["candidate"],
        "verification_sequence": candidate["verification"]["sequence"],
    }
    request("publication", "first", "reserve", 403, **reservation)
    request("publication", "operator", "reserve", 409, **(reservation | {"generation": 0}))
    request("publication", "operator", "reserve", 409, **(reservation | {"verification_sequence": 0}))
    request("publication", "operator", "reserve", 409, **reservation)
    set_draft_policy(request, True, "fixture:approve test publishing")
    action = request("publication", "operator", "reserve", **reservation)["publication"]
    assert action["state"] == "reserved" and action["head"] == candidate["candidate"]
    assert action["base_branch"] == "main" and action["source_branch"] == f"craft-factory/{order_id}"
    assert request("publication", "operator", "reserve", **reservation)["publication"] == action
    key = action["action_key"]
    [entry] = request("operations", "operator", "review_inbox")["inbox"]["items"]
    assert entry["next_action"] == "inspect_reservation" and entry["publication"]["action_key"] == key
    request("publisher", "first", "prepare", 403, action_key=key)
    request("publisher", "publisher-other", "prepare", 409, action_key=key)
    prepared = request("publisher", "publisher", "prepare", action_key=key)["publication"]
    assert prepared["state"] == "reserved" and prepared["pack_digest"] == candidate["candidate_evidence"]
    assert base64.b64decode(prepared["pack_base64"]).startswith(b"PACK")
    assert "repository" not in prepared["work_order"]
    request("publisher", "first", "begin", 403, action_key=key)
    request("publisher", "publisher-other", "begin", 409, action_key=key)
    request("operator", "operator", "set_paused", paused=True)
    request("publisher", "publisher", "begin", 409, action_key=key)
    request("operator", "operator", "set_paused", paused=False)
    with ThreadPoolExecutor(max_workers=2) as pool:
        def begin(_):
            reply = request("publisher", "publisher", "begin", (200, 409), action_key=key)
            return reply.get("publication", {}).get("state", reply.get("error"))
        assert sorted(pool.map(begin, range(2))) == ["publication_not_eligible", "sending"]
    request("publisher", "publisher", "begin", 409, action_key=key)
    missing_ack = request("publication", "operator", "status", action_key=key)["publication"]
    assert missing_ack["state"] == "sending"
    set_draft_policy(request, False, "fixture:hold new draft sends")
    set_draft_policy(request, True, "fixture:unresolved send prevents activation", 409)
    [entry] = request("operations", "operator", "review_inbox")["inbox"]["items"]
    assert entry["next_action"] == "reconcile_publication" and entry["publication"]["state"] == "sending"
    request("publisher", "publisher", "prepare", 409, action_key=key)
    request("operator", "operator", "cancel", 409, work_id=order_id, reason="Ambiguous draft")
    sql(f"UPDATE craft_queue.attempts SET expires_at = clock_timestamp() - interval '1 second' WHERE work_id = '{order_id}';")
    request("publisher", "publisher", "begin", 409, action_key=key)

    fake_forge_draft = {
        "outcome": "confirmed", "head": action["head"],
        "native_url": "https://bitbucket.org/team/service/pull-requests/101",
        "source_branch": action["source_branch"], "base_branch": action["base_branch"],
        "evidence": "fixture:operator observed the native draft after a lost acknowledgement",
    }
    request("publication", "operator", "reconcile", 409, action_key=key,
            observation=fake_forge_draft | {"source_branch": "unrelated"})
    request("publication", "operator", "reconcile", 409, action_key=key,
            observation=fake_forge_draft | {"native_url": "https://bitbucket.org/team/other/pull-requests/101"})
    wrong = reconcile(key, wrong_head=True)
    assert wrong.returncode != 0 and "exact open draft" in wrong.stderr
    assert request("publication", "operator", "status", action_key=key)["publication"]["state"] == "sending"
    result = reconcile(key)
    assert result.returncode == 0, result.stderr
    confirmed = json.loads(result.stdout)["publication"]
    assert confirmed["result_url"] == fake_forge_draft["native_url"]
    [entry] = request("operations", "operator", "review_inbox")["inbox"]["items"]
    assert entry["next_action"] == "inspect_draft"
    assert entry["publication"]["head"] == candidate["candidate"]
    assert entry["publication"]["result_url"] == confirmed["result_url"]
    assert [event["state"] for event in confirmed["events"]] == ["reserved", "sending", "confirmed"]
    assert json.loads(reconcile(key).stdout)["publication"] == confirmed
    request("publisher", "publisher", "begin", 409, action_key=key)
    set_draft_policy(request, True, "fixture:approve following reconciliation")
    return key


def exercise_stale_publication(request, candidate):
    verification = {
        "work_id": candidate["work_id"], "generation": candidate["generation"],
        "head": candidate["candidate"], "digest": candidate["candidate_evidence"],
        "previous_sequence": 0, "passed": True, "evidence": "test:verified",
        "note": "Synthetic independent verification for publication policy",
    }
    seq = request("verifier", "verifier", "record_verification", **verification)["verification_sequence"]
    action = request("publication", "operator", "reserve", work_id=verification["work_id"],
                     generation=verification["generation"], head=verification["head"],
                     verification_sequence=seq)["publication"]
    request("verifier", "verifier", "record_verification", **(verification | {
        "previous_sequence": seq, "passed": False, "evidence": "test:invalidated",
    }))
    request("publisher", "publisher", "begin", 409, action_key=action["action_key"])
    set_draft_policy(request, False, "fixture:withdraw draft send before begin")
    held = request("publication", "operator", "status", action_key=action["action_key"])["publication"]
    assert held["state"] == "uncertain" and held["events"][-1]["evidence"] == "fixture:withdraw draft send before begin"
    request("publisher", "publisher", "prepare", 409, action_key=action["action_key"])
    set_draft_policy(request, True, "fixture:unreconciled reservation prevents activation", 409)
    request("publisher", "publisher", "begin", 409, action_key=action["action_key"])
    observation = {"outcome": "absent", "head": action["head"], "evidence": "fixture:send never started"}
    request("publisher", "publisher", "record", 409, action_key=action["action_key"], observation=observation)
    assert request("publication", "operator", "reconcile", action_key=action["action_key"],
                   observation=observation)["publication"]["state"] == "absent"
    request("publisher", "publisher", "begin", 409, action_key=action["action_key"])
    set_draft_policy(request, True, "fixture:approve after recording send absence")
    print("Verified audited draft policy setup, withdrawal, and reconciliation for reserved and in-flight drafts.")


def exercise_cutover(address, certificate, tokens, request, launcher, root, order):
    database = root / "local.db"
    epoch = str(uuid4())

    def local(*arguments):
        return subprocess.run(
            [str(launcher.resolve()), "--state", str(database), *arguments],
            capture_output=True, text=True, timeout=15,
            env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
        )

    def work_order(name):
        path = root / f"{name}.json"
        path.write_text(json.dumps(order | {"id": name, "repository": str(root / "repo")}),
                        encoding="utf-8")
        return str(path)

    def cutover(*arguments):
        return local("cutover-shared", "--queue", address, "--ca", str(certificate),
                     "--epoch", epoch, *arguments)

    terminal = work_order("local-before-cutover")
    new_work = work_order("local-after-cutover")
    assert local("delegate", terminal).returncode == 0
    blocked = cutover()
    assert blocked.returncode != 0 and "drain local work" in blocked.stderr
    assert request("cutover", "operator", "status", epoch=epoch)["cutover"] is None
    assert local("cancel", "local-before-cutover", "--reason", "Drain the local fixture").returncode == 0

    # A failed remote activation must leave local work frozen and replay safe.
    interrupted = cutover()
    assert interrupted.returncode != 0 and "HTTP 409" in interrupted.stderr
    assert "frozen for shared cutover" in local("delegate", new_work).stderr
    assert local("status", "local-before-cutover").returncode == 0
    request("operator", "operator", "set_paused", paused=True)
    switched = cutover()
    assert switched.returncode == 0, switched.stderr
    shared = json.loads(switched.stdout)
    assert shared["authority"] == "shared" and shared["cutover"]["archived_orders"] == 1
    assert json.loads(cutover().stdout) == shared
    raw = request("cutover", "operator", "archive", epoch=epoch)["archive"]
    assert f"sha256:{sha256(raw.encode()).hexdigest()}" == shared["cutover"]["digest"]
    archive = json.loads(raw)
    assert archive["receipts"][0]["state"] == "cancelled"
    assert archive["receipts"][0]["work_order"]["id"] == "local-before-cutover"
    request("cutover", "first", "archive", 403, epoch=epoch)
    request("operator", "operator", "admit", 409, repository="team/service",
            event_key="cutover:duplicate", work_order=order | {"id": "local-before-cutover"}, token_limit=30)
    request("operator", "operator", "admit", repository="team/service",
            event_key="cutover:shared", work_order=order | {"id": "shared-during-cutover"}, token_limit=30)

    draining = cutover("--rollback")
    assert draining.returncode == 0, draining.stderr
    assert json.loads(draining.stdout)["authority"] == "shared_draining"
    assert "frozen for shared cutover" in local("delegate", new_work).stderr
    request("operator", "operator", "set_paused", 409, paused=False)
    assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
    request("operator", "operator", "cancel", work_id="shared-during-cutover", reason="Drain shared work")
    returned = cutover("--rollback")
    assert returned.returncode == 0, returned.stderr
    assert json.loads(returned.stdout)["authority"] == "local"
    assert json.loads(cutover("--rollback").stdout) == json.loads(returned.stdout)
    request("operator", "operator", "set_paused", 409, paused=False)
    assert "already archived from the shared queue" in local(
        "delegate", work_order("shared-during-cutover")).stderr
    assert local("delegate", new_work).returncode == 0
    assert "released" in cutover().stderr


@contextmanager
def run_service(binary, certificate, key, config, database_url, *additional, credentials=None):
    env = os.environ.copy()
    if credentials is None:
        env["CRAFT_DATABASE_URL"] = database_url
    else:
        env.pop("CRAFT_DATABASE_URL", None)
        env["CREDENTIALS_DIRECTORY"] = str(credentials)
    with subprocess.Popen(
        [str(binary.resolve()), "--bind", "127.0.0.1:0",
         "--certificate", str(certificate), "--private-key", str(key), "--identities", str(config),
         *additional],
        env=env,
        stdout=subprocess.PIPE, text=True,
    ) as server:
        try:
            address = server.stdout.readline().strip()
            assert address.startswith("https://127.0.0.1:"), address
            yield address, server
        finally:
            if server.poll() is None:
                server.terminate()


def exercise_shutdown(server, certificate, key, config, database_url, ca, tokens):
    if os.name != "posix":
        return
    paused = sql("SELECT paused FROM craft_queue.control;") == "t"
    try:
        with run_service(server, certificate, key, config, database_url) as (address, process):
            target = urlsplit(address)
            trust = ssl.create_default_context(cafile=str(ca))
            body = json.dumps({"action": "set_paused", "paused": not paused}).encode()
            connection = http.client.HTTPSConnection(target.hostname, target.port, context=trust, timeout=5)
            with closing(connection) as client:
                client.putrequest("POST", "/v1/operator")
                client.putheader("Authorization", f"Bearer {tokens['operator']}")
                client.putheader("Content-Type", "application/json")
                client.putheader("Content-Length", str(len(body)))
                client.endheaders(body[:-1])
                process.terminate()
                deadline = monotonic() + 3
                while True:
                    try:
                        with socket.create_connection((target.hostname, target.port), timeout=0.2):
                            pass
                    except ConnectionRefusedError:
                        break
                    assert monotonic() < deadline, "queue still accepts connections during shutdown"
                    sleep(0.02)
                assert process.poll() is None, "queue exited before finishing the admitted request"
                client.send(body[-1:])
                response = client.getresponse()
                assert response.status == 200 and json.loads(response.read())["paused"] is (not paused)
            assert process.wait(timeout=5) == 0, "queue did not finish shutdown cleanly"
        assert (sql("SELECT paused FROM craft_queue.control;") == "t") is (not paused)
    finally:
        sql(f"UPDATE craft_queue.control SET paused = {'true' if paused else 'false'};")


def exercise_identity_reload(server, launcher, certificate, key, config, ca, tokens, database_url):
    original = config.read_bytes()
    roster = json.loads(original)
    replacement_token = secrets.token_urlsafe(32)

    def replace(data):
        pending = config.with_suffix(".next.json")
        pending.write_bytes(data)
        pending.replace(config)
        return "sha256:" + sha256(data).hexdigest()

    with run_service(server, certificate, key, config, database_url) as (address, process):
        request = api_client(address, ca, tokens | {"replacement": replacement_token})
        assert request("operations", "operator", "recovery_status")["recovery"] is None
        pid = process.pid
        request("operations", "first", "reload_identities", 403, digest="sha256:" + "0" * 64)
        request("operations", "operator", "reload_identities", 400, digest="invalid")
        try:
            wrong_role = [entry | {"verifier_repositories": ["team/service"]} if entry["id"] == "first"
                          else entry for entry in roster]
            duplicate_scope = [entry | {"verifier_repositories": ["team/service", "team/service"]}
                               if entry["id"] == "service-verifier" else entry for entry in roster]
            null_scope = [entry | {"verifier_repositories": None} if entry["id"] == "service-verifier"
                          else entry for entry in roster]
            missing_context_scope = [
                {key: value for key, value in entry.items() if key != "context_repositories"}
                if entry["id"] == "context-reader" else entry for entry in roster
            ]
            for invalid in (roster + [roster[0]], [entry for entry in roster if entry["role"] != "operator"],
                            wrong_role, duplicate_scope, null_scope, missing_context_scope):
                digest = replace(json.dumps(invalid).encode())
                request("operations", "operator", "reload_identities", 409, digest=digest)
                request("operations", "operator", "summary")

            rotated = [entry | {"token_sha256": sha256(replacement_token.encode()).hexdigest()}
                       if entry["id"] == "operator" else
                       entry | {"verifier_repositories": ["team/other"]} if entry["id"] == "service-verifier"
                       else entry | {"context_repositories": ["team/other"]} if entry["id"] == "context-reader"
                       else entry for entry in roster]
            digest = replace(json.dumps(rotated).encode())
            request("operations", "operator", "reload_identities", 409, digest="sha256:" + "0" * 64)
            request("operations", "replacement", "summary", 401)
            if launcher is None:
                updated = request("operations", "operator", "reload_identities", digest=digest)
            else:
                updated = json.loads(subprocess.check_output([
                    str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(ca),
                    "reload-identities", "--digest", digest,
                ], env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, text=True, timeout=15))
            assert updated["identities"] == {"configured_identities": len(roster), "digest": digest, "changed": True}
            request("operations", "operator", "summary", 401)
            request("operations", "operator", "reload_identities", 401, digest=digest)
            request("operations", "replacement", "summary")
            request("verifier", "service-verifier", "pending_merge_checks", 403, repository="team/service", limit=1)
            assert not request("verifier", "service-verifier", "pending_merge_checks",
                               repository="team/other", limit=1)["merge_checks"]["items"]
            request("context-reader", "context-reader", None, 403, repository="team/service", location="project:routing")
            assert request("context-reader", "context-reader", None, repository="team/other",
                           location="project:routing")["catalog"]["items"] == []
            replay = request("operations", "replacement", "reload_identities", digest=digest)["identities"]
            assert replay == updated["identities"] | {"changed": False}
            assert process.pid == pid and process.poll() is None
        finally:
            replace(original)
    print("Verified queue credential rotation, malformed replacement isolation, and immediate revocation for new requests.")


def exercise_queue_preflight(server, certificate, key, wrong_key, config, database_url):
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        occupied.listen()
        bind = f"127.0.0.1:{occupied.getsockname()[1]}"
        command = [str(server.resolve()), "--bind", bind,
                   "--certificate", str(certificate), "--identities", str(config)]

        def run(*args, private_key=key, database=database_url):
            return subprocess.run(command + ["--private-key", str(private_key), *args], capture_output=True,
                                  text=True, timeout=10, env=os.environ | {"CRAFT_DATABASE_URL": database})

        ready = run("--check-config")
        assert ready.returncode == 0, ready.stderr
        report = json.loads(ready.stdout)
        assert report == {"schema_version": 1, "ready": True,
                          "configured_identities": len(json.loads(config.read_text())), "operator_configured": True,
                          "database": {"schema": "craft_queue", "paused": True}}

        credential_file = config.parent / "database url.txt"
        credential_file.touch(mode=0o600, exist_ok=False)
        credential_file.write_bytes((database_url + "\r\n").encode())
        credential_env = {name: value for name, value in os.environ.items() if name != "CRAFT_DATABASE_URL"}
        credential_env.update(CRAFT_DATABASE_URL_FILE=str(credential_file),
                              CREDENTIALS_DIRECTORY=str(config.parent / "missing-credentials"))
        preflight = command + ["--private-key", str(key), "--check-config"]

        def file_check(**overrides):
            return subprocess.run(preflight, capture_output=True, text=True, timeout=10,
                                  env=credential_env | overrides)

        ready_from_file = file_check()
        assert ready_from_file.returncode == 0, ready_from_file.stderr
        assert json.loads(ready_from_file.stdout) == report
        missing_path = str(config.parent / "missing-database-credential")
        assert file_check(CRAFT_DATABASE_URL=database_url, CRAFT_DATABASE_URL_FILE=missing_path).returncode == 0
        missing_file = file_check(CRAFT_DATABASE_URL_FILE=missing_path)
        assert missing_file.returncode != 0 and "could not read the CRAFT_DATABASE_URL credential file" in missing_file.stderr
        for invalid in (b"", b"\xff", b"fixture-private-marker\nsecond-line", b"x" * (16 * 1024 + 1)):
            credential_file.write_bytes(invalid)
            rejected = file_check()
            assert rejected.returncode != 0 and rejected.stdout == ""
            assert "CRAFT_DATABASE_URL credential file" in rejected.stderr and "fixture-private-marker" not in rejected.stderr
        credential_file.unlink()

        empty = f"preflight_empty_{uuid4().hex}"
        subprocess.run(["createdb", empty], check=True, timeout=20)
        empty_url = urlsplit(database_url)._replace(path=f"/{empty}").geturl()
        missing_schema = run("--check-config", database=empty_url)
        assert missing_schema.returncode != 0 and "schema is incomplete" in missing_schema.stderr

        original_digest = sql("SELECT digest FROM craft_migration.queue_history WHERE number = 1;")
        drift_epoch = uuid4()
        sql(f"UPDATE craft_migration.queue_history SET digest = '{'0' * 64}' WHERE number = 1;")
        try:
            for flags in (("--check-config",), ("--recover-restored-database", str(drift_epoch))):
                drift = run(*flags)
                assert drift.returncode != 0 and not drift.stdout
                assert "migration history differs from this build" in drift.stderr
        finally:
            sql(f"UPDATE craft_migration.queue_history SET digest = '{original_digest}' WHERE number = 1;")
        assert sql(f"SELECT count(*) FROM craft_queue.recoveries WHERE epoch = '{drift_epoch}';") == "0"
        sql("INSERT INTO craft_migration.queue_history (number, filename, digest) VALUES (999, '999_fixture.sql', 'future');")
        try:
            ahead = run("--check-config")
            assert ahead.returncode != 0 and "migration history differs from this build" in ahead.stderr
        finally:
            sql("DELETE FROM craft_migration.queue_history WHERE number = 999;")
        assert run("--check-config").returncode == 0

        signature = "craft_queue.diagnostic_grant_audit(text)"
        sql(f"REVOKE EXECUTE ON FUNCTION {signature} FROM craft_test_service;")
        try:
            missing_grant = run("--check-config")
            assert missing_grant.returncode != 0 and "missing queue function grants" in missing_grant.stderr
        finally:
            sql(f"GRANT EXECUTE ON FUNCTION {signature} TO craft_test_service;")

        legacy = "craft_queue.claim(text,uuid)"
        blocked_epoch = str(uuid4())
        sql(f"GRANT EXECUTE ON FUNCTION {legacy} TO PUBLIC;")
        try:
            for args in (("--check-config",), ("--recover-restored-database", blocked_epoch)):
                over_granted = run(*args)
                assert over_granted.returncode != 0 and "excess queue function grants" in over_granted.stderr
        finally:
            sql(f"REVOKE EXECUTE ON FUNCTION {legacy} FROM PUBLIC;")
        assert sql(f"SELECT count(*) FROM craft_queue.recoveries WHERE epoch = '{blocked_epoch}';") == "0"

        sql("GRANT SELECT ON craft_queue.work TO craft_test_service;")
        try:
            over_granted = run("--check-config")
            assert over_granted.returncode != 0 and "direct queue table access" in over_granted.stderr
        finally:
            sql("REVOKE SELECT ON craft_queue.work FROM craft_test_service;")

        sql("""
            CREATE ROLE fixture_queue_group;
            CREATE ROLE fixture_queue_maintainer;
            ALTER ROLE craft_test_service NOINHERIT;
            GRANT fixture_queue_group TO craft_test_service;
            GRANT fixture_queue_maintainer TO fixture_queue_group;
        """)
        try:
            assert run("--check-config").returncode == 0
            sql("GRANT USAGE ON SCHEMA craft_queue TO fixture_queue_maintainer;")
            sql("GRANT SELECT ON craft_queue.work TO fixture_queue_maintainer;")
            assert sql("SELECT has_table_privilege('craft_test_service', 'craft_queue.work', 'SELECT');") == "f"
            for flags in (("--check-config",), ("--recover-restored-database", blocked_epoch)):
                over_granted = run(*flags)
                assert over_granted.returncode != 0 and not over_granted.stdout
                assert "direct queue table access" in over_granted.stderr, over_granted.stderr
            assert sql(f"SELECT count(*) FROM craft_queue.recoveries WHERE epoch = '{blocked_epoch}';") == "0"
            sql("REVOKE SELECT ON craft_queue.work FROM fixture_queue_maintainer;")
            sql(f"GRANT EXECUTE ON FUNCTION {legacy} TO fixture_queue_maintainer;")
            over_granted = run("--check-config")
            assert over_granted.returncode != 0 and "excess queue function grants" in over_granted.stderr
        finally:
            sql(f"""
                REVOKE EXECUTE ON FUNCTION {legacy} FROM fixture_queue_maintainer;
                REVOKE SELECT ON craft_queue.work FROM fixture_queue_maintainer;
                REVOKE USAGE ON SCHEMA craft_queue FROM fixture_queue_maintainer;
                REVOKE fixture_queue_maintainer FROM fixture_queue_group;
                REVOKE fixture_queue_group FROM craft_test_service;
                ALTER ROLE craft_test_service INHERIT;
                DROP ROLE fixture_queue_group, fixture_queue_maintainer;
            """)
        assert run("--check-config").returncode == 0

        epoch = uuid4()
        invalid = run("--recover-restored-database", str(epoch), private_key=wrong_key)
        assert invalid.returncode != 0 and not invalid.stdout
        assert sql(f"SELECT count(*) FROM craft_queue.recoveries WHERE epoch = '{epoch}';") == "0"


def diagnostic_manifest(root):
    release = ROOT.parent / "examples" / "release-evidence.json"
    telemetry = ROOT.parent / "examples" / "diagnostic-telemetry.json"
    policy = root / "diagnostic-policy.json"
    manifest = root / "diagnostic-cases.json"
    policy.write_text(json.dumps({
        "schema_version": 1, "target": json.loads(telemetry.read_text())["target"],
        "approval_reference": "fixture:approved-staging-case",
        "release_digest": f"sha256:{sha256(release.read_bytes()).hexdigest()}",
        "telemetry_digest": f"sha256:{sha256(telemetry.read_bytes()).hexdigest()}",
        "authorized_window": {"from": 1795238300, "to": 1795238400},
        "max_window_seconds": 100, "release_max_age_seconds": 120, "max_records": 5,
        "log_codes": ["UPSTREAM_TIMEOUT"], "metrics": [{"name": "request_latency_p95", "unit": "ms"}],
    }), encoding="utf-8")
    cases = [
        {"id": name, "policy": str(policy.resolve()), "release": str(release.resolve()),
         "telemetry": str(telemetry.resolve())}
        for name in ("staging-incident", "other-incident")
    ]
    archive_case, archive_digest, archive_files = archive_diagnostic_case(root, telemetry, policy)
    cases.append(archive_case)
    manifest.write_text(json.dumps(cases), encoding="utf-8")
    return manifest, f"sha256:{sha256(policy.read_bytes()).hexdigest()}", archive_digest, archive_files


def exercise_build_capture(launcher, root, certificate, key, ca):
    archive = root / "incident-archive"
    signing_policy = archive / "build-policy.json"
    envelope = json.loads((archive / "build.dsse.json").read_bytes())
    statement = json.loads(base64.b64decode(envelope["payload"]))
    statement["predicate"]["runDetails"]["metadata"]["finishedOn"] = "2020-01-01T00:00:00Z"
    payload = json.dumps(statement).encode()
    payload_type = envelope["payloadType"]
    signing_input = root / "capture-signing-input"
    signing_input.write_bytes(f"DSSEv1 {len(payload_type)} {payload_type} {len(payload)} ".encode() + payload)
    signature = subprocess.check_output([
        "openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(root / "fixture-build-signer.pem"),
        "-in", str(signing_input),
    ], timeout=10)
    envelope["payload"] = base64.b64encode(payload).decode()
    envelope["signatures"] = [{"sig": base64.b64encode(signature).decode()}]
    raw = json.dumps(envelope, indent=2).encode()
    token = secrets.token_urlsafe(32)
    writer = secrets.token_urlsafe(32)
    requests = []
    puts = []
    objects = {}

    class Archive(BaseHTTPRequestHandler):
        def do_GET(self):
            authorization = self.headers.get("Authorization")
            if self.path.startswith("/releases/"):
                if authorization not in (f"Bearer {writer}", f"Bearer {token}"):
                    self.send_error(401)
                elif self.path not in objects:
                    self.send_error(404)
                else:
                    self.send_body(objects[self.path])
                return
            requests.append((self.path, self.headers.get("Authorization") == f"Bearer {token}"))
            if not requests[-1][1]:
                self.send_error(401)
            elif self.path == "/build.dsse.json":
                self.send_body(raw)
            elif self.path == "/redirect":
                self.send_response(302)
                self.send_header("Location", "/build.dsse.json")
                self.end_headers()
            else:
                self.send_error(404)

        def do_PUT(self):
            puts.append(self.path)
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            expected = f"/releases/{sha256(body).hexdigest()}.dsse.json"
            if self.headers.get("Authorization") != f"Bearer {writer}":
                self.send_error(401)
            elif self.headers.get("If-None-Match") != "*" or self.path != expected:
                self.send_error(400)
            elif self.path in objects:
                self.send_response(412)
                self.end_headers()
            else:
                objects[self.path] = body
                self.send_response(201)
                self.end_headers()

        def send_body(self, body):
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    def digest(data):
        return f"sha256:{sha256(data).hexdigest()}"

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certificate, key)
    with ThreadingHTTPServer(("127.0.0.1", 0), Archive) as server:
        server.socket = context.wrap_socket(server.socket, server_side=True)
        serving = Thread(target=server.serve_forever)
        serving.start()
        try:
            endpoint = f"https://127.0.0.1:{server.server_port}"
            target = root / "build-capture"
            policy = root / "build-capture-policy.json"
            expected_ca = digest(ca.read_bytes())

            def capture_policy(scope, destination):
                pin = json.dumps(scope).encode()
                policy.write_bytes(pin)
                return subprocess.run([
                    str(launcher.resolve()), "collect-build-provenance", str(policy),
                    "--policy-digest", digest(pin), "--signing-policy", str(signing_policy),
                    "--archive-ca", str(ca), "--destination", str(destination),
                ], env=os.environ | {"CRAFT_FACTORY_BUILD_EVIDENCE_TOKEN": token},
                    text=True, capture_output=True, timeout=10)

            def capture(path, ca_digest):
                return capture_policy({
                    "schema_version": 1, "envelope_url": endpoint + path,
                    "envelope_digest": digest(raw), "signing_policy_digest": digest(signing_policy.read_bytes()),
                    "archive_ca_digest": ca_digest, "source_commit": "b" * 40,
                    "artifact_digest": "sha256:" + statement["subject"][0]["digest"]["sha256"],
                }, target)

            wrong_ca = capture("/build.dsse.json", digest(b"another CA"))
            assert wrong_ca.returncode != 0 and "CA does not match" in wrong_ca.stderr, wrong_ca
            assert requests == [] and not target.exists()
            redirected = capture("/redirect", expected_ca)
            assert redirected.returncode != 0 and not target.exists(), redirected
            assert requests == [("/redirect", True)], requests
            collected = capture("/build.dsse.json", expected_ca)
            assert collected.returncode == 0, collected.stderr
            assert requests == [("/redirect", True), ("/build.dsse.json", True)], requests
            assert (target / "build.dsse.json").read_bytes() == raw
            assert (target / "signing-policy.json").read_bytes() == signing_policy.read_bytes()
            assert json.loads(collected.stdout)["source_digest"] == digest((target / "build-source.json").read_bytes())

            secret_key = root / "fixture-signer.der"
            subprocess.run(["openssl", "pkey", "-in", str(root / "fixture-build-signer.pem"),
                            "-outform", "DER", "-out", str(secret_key)], check=True, timeout=10)
            other_key = root / "other-fixture-signer.der"
            subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-outform", "DER",
                            "-out", str(other_key)], check=True, timeout=10)
            build_input = root / "build-input.json"
            build_input.write_text(json.dumps({
                "schema_version": 1, "artifact_digest": "sha256:" + statement["subject"][0]["digest"]["sha256"],
                "source_commit": "b" * 40, "invocation_id": "fixture-ci-publication", "finished_on": "2020-01-01T00:00:00Z",
            }), encoding="utf-8")
            publish_policy = root / "build-archive-policy.json"
            publish_policy.write_text(json.dumps({
                "schema_version": 1, "archive_prefix": endpoint + "/releases/",
                "signing_policy_digest": digest(signing_policy.read_bytes()), "archive_ca_digest": expected_ca,
            }), encoding="utf-8")

            def publish(signing_key):
                return subprocess.run([
                    str(launcher.resolve()), "publish-build-provenance", str(build_input),
                    "--archive-policy", str(publish_policy), "--archive-policy-digest", digest(publish_policy.read_bytes()),
                    "--signing-policy", str(signing_policy), "--signing-key", str(signing_key), "--archive-ca", str(ca),
                ], env=os.environ | {"CRAFT_FACTORY_BUILD_PUBLISH_TOKEN": writer}, text=True, capture_output=True, timeout=10)

            wrong_key = publish(other_key)
            assert wrong_key.returncode != 0 and "does not match the approved build signing key" in wrong_key.stderr
            assert not puts
            published = publish(secret_key)
            assert published.returncode == 0, published.stderr
            scope = json.loads(published.stdout)["capture_policy"]
            object_path = urlsplit(scope["envelope_url"]).path
            assert puts == [object_path] and scope["envelope_digest"] == digest(objects[object_path])
            downloaded = root / "published-build-capture"
            fetched = capture_policy(scope, downloaded)
            assert fetched.returncode == 0, fetched.stderr
            assert (downloaded / "build.dsse.json").read_bytes() == objects[object_path]
            replayed = publish(secret_key)
            assert replayed.returncode == 0 and replayed.stdout == published.stdout, replayed.stderr
            objects[object_path] = b"{}"
            conflict = publish(secret_key)
            assert conflict.returncode != 0 and "does not match the signed envelope" in conflict.stderr
            assert objects[object_path] == b"{}"
        finally:
            server.shutdown()
            serving.join()


def archive_diagnostic_case(root, telemetry_file, base_policy_file):
    archive = root / "incident-archive"
    archive.mkdir()
    manifest = archive / "sources.json"
    telemetry = json.loads(telemetry_file.read_text())
    version = telemetry["logs"][0]["version"]
    digest = version["artifact_digest"]
    repo = "bitbucket:example/sample-api"
    full_name = repo.removeprefix("bitbucket:")
    head = "c" * 40
    merge = "b" * 40
    container = {"pod_uid": "pod-uid", "container_name": "api", "container_id": "a" * 64}
    annotations = {"craft.factory/deployment-id": version["deployment_id"],
                   "craft.factory/config-revision": version["config_revision"],
                   "craft.factory/source-repository": repo}
    labels = {"pod-template-hash": "fixture"}
    containers = [{"name": "api", "image": f"registry.example/api@{digest}"}]

    def write(name, data):
        raw = json.dumps(data).encode()
        (archive / name).write_bytes(raw)
        return {"path": name, "sha256": f"sha256:{sha256(raw).hexdigest()}"}

    def owner(kind, name, uid):
        return [{"apiVersion": "apps/v1", "kind": kind, "name": name, "uid": uid, "controller": True}]

    pod = write("pod.json", {
        "apiVersion": "v1", "kind": "Pod",
        "metadata": {"namespace": "staging", "name": "api-fixture", "uid": "pod-uid",
                     "creationTimestamp": "2026-11-21T05:15:00Z", "annotations": annotations, "labels": labels,
                     "ownerReferences": owner("ReplicaSet", "api-fixture-rs", "rs-uid")},
        "spec": {"containers": containers},
        "status": {"phase": "Running", "containerStatuses": [
            {"name": "api", "imageID": digest, "containerID": "containerd://" + container["container_id"],
             "state": {"running": {"startedAt": "2026-11-21T05:16:30Z"}}}
        ]},
    })
    replica_set = write("replica-set.json", {
        "apiVersion": "apps/v1", "kind": "ReplicaSet",
        "metadata": {"namespace": "staging", "name": "api-fixture-rs", "uid": "rs-uid",
                     "creationTimestamp": "2026-11-21T05:14:55Z",
                     "ownerReferences": owner("Deployment", "api", "deployment-uid")},
        "spec": {"template": {"metadata": {"annotations": annotations, "labels": labels},
                              "spec": {"containers": containers}}},
    })
    key = root / "fixture-build-signer.pem"
    subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(key)], check=True, timeout=10)
    public_key = subprocess.check_output(
        ["openssl", "pkey", "-in", str(key), "-pubout", "-outform", "DER"], timeout=10,
    )
    assert len(public_key) == 44 and public_key[:12] == bytes.fromhex("302a300506032b6570032100")
    builder = "https://ci.example/builders/release"
    build_type = "https://ci.example/build-types/container"
    source_uri = "git+https://bitbucket.org/example/sample-api@refs/heads/main"
    build_policy = write("build-policy.json", {
        "schema_version": 1, "repository": repo, "source_uri": source_uri,
        "artifact_name": "registry.example/api", "builder_id": builder, "build_type": build_type,
        "external_parameters": {}, "public_key_base64": base64.b64encode(public_key[-32:]).decode(),
        "approval_reference": "fixture:builder-approval",
    })
    payload_type = "application/vnd.in-toto+json"
    payload = json.dumps({
        "_type": "https://in-toto.io/Statement/v1", "predicateType": "https://slsa.dev/provenance/v1",
        "subject": [{"name": "registry.example/api", "digest": {"sha256": digest.removeprefix("sha256:")}}],
        "predicate": {
            "buildDefinition": {"buildType": build_type, "externalParameters": {},
                                "resolvedDependencies": [{"uri": source_uri, "digest": {"gitCommit": merge}}]},
            "runDetails": {"builder": {"id": builder}, "metadata": {
                "invocationId": "fixture-build", "finishedOn": "2026-11-21T05:14:20Z"}},
        },
    }).encode()
    signing_input = root / "fixture-signing-input"
    signing_input.write_bytes(f"DSSEv1 {len(payload_type)} {payload_type} {len(payload)} ".encode() + payload)
    signature = subprocess.check_output(
        ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(key), "-in", str(signing_input)], timeout=10,
    )
    attestation = write("build.dsse.json", {"payloadType": payload_type, "payload": base64.b64encode(payload).decode(),
                                            "signatures": [{"sig": base64.b64encode(signature).decode()}]})
    uuid = "a9876543-4321-1234-abcd-fedcba987654"
    pull_request = write("pull-request.json", {
        "type": "pullrequest", "id": 16, "state": "MERGED", "source": {"commit": {"hash": head}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": full_name, "uuid": uuid}},
        "merge_commit": {"hash": merge}, "created_on": "2026-11-21T05:10:00Z", "updated_on": "2026-11-21T05:13:30Z",
    })
    approval = write("approval.json", {
        "schema_version": 1, "workspace": "example", "repository": "sample-api", "repository_uuid": uuid,
        "destination_branch": "main", "pull_request_id": 16, "approved_source_commit": head, "result_commit": merge,
        "work_item": "SAMPLE-42", "intent_reference": "fixture:intent", "intent_revision": "r3",
        "approval_reference": "fixture:release-approval", "approved_at": 1795237990,
        "merge_reference": "fixture:merge", "merged_at": 1795238000,
    })
    manifest.write_text(json.dumps({
        "schema_version": 1, "as_of": 1795238400, "max_age_seconds": 120,
        "kubernetes": {"pod": pod, "replica_set": replica_set,
                       "inventory": telemetry["target"] | {"cluster": "staging-one", "namespace": "staging",
                           "pod": "api-fixture", "pod_uid": "pod-uid", "container": "api",
                           "owner": "Fixture team", "observed_at": 1795238340},
                       "deployment": {"replica_set_observed_at": 1795238320, "deployment_name": "api",
                                      "deployment_uid": "deployment-uid", "repository": repo}},
        "build": {"attestation": attestation, "policy": build_policy, "observed_at": 1795238090},
        "change": {"pull_request": pull_request, "approval": approval, "observed_at": 1795238030},
    }), encoding="utf-8")
    target = telemetry["target"]
    attributes = [
        {"key": key, "value": {"stringValue": value}}
        for key, value in {
            "service.name": target["service"], "service.instance.id": "pod-uid/api",
            "deployment.environment.name": target["environment"], "cloud.region": target["region"],
            "craft.resource.id": target["resource"], "craft.deployment.id": version["deployment_id"],
            "craft.config.revision": version["config_revision"], "craft.artifact.digest": digest,
            "k8s.pod.uid": container["pod_uid"], "k8s.container.name": container["container_name"],
            "container.id": container["container_id"],
        }.items()
    ]
    unrelated_attributes = [
        item if item["key"] != "container.id" else {"key": "container.id", "value": {"stringValue": "b" * 64}}
        for item in attributes
    ]
    log_file = write("otlp-logs.json", {"resourceLogs": [
        {"resource": {"attributes": attributes}, "scopeLogs": [
            {"scope": {"name": "api.events"}, "logRecords": [
                {"timeUnixNano": "1795238350000000000", "severityNumber": 13, "eventName": "UPSTREAM_TIMEOUT",
                 "body": {"stringValue": "private fixture log body"}},
                {"timeUnixNano": "1795238352000000000", "severityNumber": 13, "eventName": "INTERNAL_ONLY"},
            ]},
        ]},
        {"resource": {"attributes": unrelated_attributes}, "scopeLogs": [
            {"scope": {"name": "api.events"}, "logRecords": [
                {"timeUnixNano": "1795238351000000000", "severityNumber": 13, "eventName": "UPSTREAM_TIMEOUT"},
            ]},
        ]},
    ]})
    metric_file = write("otlp-metrics.json", {"resourceMetrics": [
        {"resource": {"attributes": attributes}, "scopeMetrics": [
            {"scope": {"name": "api.metrics"}, "metrics": [
                {"name": "request_latency_p95", "unit": "ms", "gauge": {"dataPoints": [
                    {"timeUnixNano": "1795238360000000000", "asDouble": 425.0},
                ]}},
            ]},
        ]},
    ]})
    import_policy = write("otlp-policy.json", {
        "schema_version": 1, "target": target, "version": version, "service_instance_id": "pod-uid/api",
        "kubernetes_container": container,
        "log_scope": "api.events", "metric_scope": "api.metrics", "coverage": telemetry["coverage"],
        "observed_at": 1795238405, "approval_reference": "fixture:otlp-approval",
        "logs_digest": log_file["sha256"], "metrics_digest": metric_file["sha256"], "max_records": 3,
        "log_codes": ["UPSTREAM_TIMEOUT"], "metrics": [{"name": "request_latency_p95", "unit": "ms"}],
    })
    policy = archive / "policy.json"
    policy.write_text(json.dumps(json.loads(base_policy_file.read_text()) | {
        "release_basis": "source_manifest",
        "release_digest": f"sha256:{sha256(manifest.read_bytes()).hexdigest()}",
        "telemetry_basis": "otlp_archive",
        "telemetry_digest": import_policy["sha256"],
    }), encoding="utf-8")
    policy_digest = f"sha256:{sha256(policy.read_bytes()).hexdigest()}"
    return ({
        "id": "archived-incident", "policy": str(policy.resolve()), "policy_digest": policy_digest,
        "release_manifest": str(manifest.resolve()),
        "otlp": {"policy": str(archive / import_policy["path"]), "logs": str(archive / log_file["path"]),
                 "metrics": str(archive / metric_file["path"])},
    }, policy_digest, (archive / "pod.json", archive / log_file["path"]))


def diagnostic_request(address, certificate, tokens, identity, body, method="POST"):
    parsed = urlsplit(address)
    connection = http.client.HTTPSConnection(
        parsed.hostname, parsed.port, context=ssl.create_default_context(cafile=str(certificate)), timeout=5
    )
    headers = {"Content-Type": "application/json"}
    if identity:
        headers["Authorization"] = f"Bearer {tokens[identity]}"
    try:
        connection.request(method, "/v1/diagnostics", json.dumps(body), headers)
        result = connection.getresponse()
        payload = result.read()
        return result.status, result.getheader("Cache-Control"), payload
    finally:
        connection.close()


def exercise_diagnostics(address, certificate, tokens, launcher, policy_digest):
    wrong_digest = f"sha256:{'0' * 64}"
    sql(f"""
        INSERT INTO craft_queue.diagnostic_grants (
            identity_id, case_id, policy_digest, allowed_from, allowed_to, expires_at, max_reads, approval_reference
        ) VALUES
            ('diagnosis-one', 'staging-incident', '{policy_digest}', 1795238330, 1795238400,
                clock_timestamp() + interval '1 hour', 2, 'fixture:viewer-approval'),
            ('diagnosis-peer', 'staging-incident', '{wrong_digest}', 1795238330, 1795238400,
                clock_timestamp() + interval '1 hour', 2, 'fixture:unrelated-digest');
    """)
    body = {"case_id": "staging-incident", "from": 1795238330, "to": 1795238400}

    def send(identity="diagnosis-one", request=body, method="POST"):
        return diagnostic_request(address, certificate, tokens, identity, request, method)

    assert send(identity=None)[0] == 401
    assert send(identity="operator")[0] == 403
    assert send(identity="diagnosis-peer")[0] == 403
    assert send(request=body | {"case_id": "other-incident"})[0] == 403
    assert send(request=body | {"from": 1795238329})[0] == 403
    assert send(request=body | {"destination": "https://example.invalid"})[0] == 422
    assert send(method="DELETE")[0] == 405
    assert sql("SELECT count(*) FROM craft_queue.diagnostic_reads;") == "0"
    if launcher is not None:
        output = subprocess.run(
            [str(launcher.resolve()), "diagnose-shared-release", "--queue", address,
             "--ca", str(certificate), "--case", body["case_id"],
             "--from", str(body["from"]), "--to", str(body["to"])],
            env=os.environ | {"CRAFT_FACTORY_DIAGNOSTIC_TOKEN": tokens["diagnosis-one"]},
            capture_output=True, text=True, timeout=15,
        )
        assert output.returncode == 0, output.stderr
        assert json.loads(output.stdout)["report"]["release"]["change"]["work_item"] == "SAMPLE-42"
    else:
        assert send()[0] == 200

    with ThreadPoolExecutor(max_workers=2) as pool:
        responses = list(pool.map(lambda _: send(), range(2)))
    assert sorted(status for status, _, _ in responses) == [200, 403]
    status, cache_control, payload = next(response for response in responses if response[0] == 200)
    assert status == 200 and cache_control == "no-store"
    report = json.loads(payload)["report"]
    assert report["telemetry_correlated"] and len(report["logs"]) == 1 and len(report["metrics"]) == 1
    assert send()[0] == 403
    assert sql("SELECT count(*) FROM craft_queue.diagnostic_reads WHERE approval_reference = 'fixture:viewer-approval';") == "2"

    sql(f"""
        UPDATE craft_queue.diagnostic_grants
        SET policy_digest = '{policy_digest}', expires_at = clock_timestamp() - interval '1 second'
        WHERE identity_id = 'diagnosis-peer';
    """)
    assert send(identity="diagnosis-peer")[0] == 403
    sql("""
        UPDATE craft_queue.diagnostic_grants SET expires_at = clock_timestamp() + interval '1 hour',
            revoked_at = clock_timestamp() WHERE identity_id = 'diagnosis-peer';
    """)
    assert send(identity="diagnosis-peer")[0] == 403
    assert sql("SELECT count(*) FROM craft_queue.diagnostic_reads;") == "2"


def exercise_archive_diagnostics(address, certificate, tokens, archive_digest):
    sql(f"""
        INSERT INTO craft_queue.diagnostic_grants (
            identity_id, case_id, policy_digest, allowed_from, allowed_to, expires_at, max_reads, approval_reference
        ) VALUES
            ('diagnosis-one', 'archived-incident', '{archive_digest}', 1795238330, 1795238400,
                clock_timestamp() + interval '1 hour', 1, 'fixture:archive-grant'),
            ('diagnosis-peer', 'archived-incident', 'sha256:{'0' * 64}', 1795238330, 1795238400,
                clock_timestamp() + interval '1 hour', 1, 'fixture:wrong-archive-pin');
    """)
    body = {"case_id": "archived-incident", "from": 1795238330, "to": 1795238400}

    def send(identity="diagnosis-one", request=body):
        return diagnostic_request(address, certificate, tokens, identity, request)

    assert send(identity="operator")[0] == 403
    assert send(identity="diagnosis-peer")[0] == 403
    assert send(request=body | {"to": 1795238399})[0] == 403
    assert sql("SELECT count(*) FROM craft_queue.diagnostic_reads WHERE case_id = 'archived-incident';") == "0"
    status, cache_control, payload = send()
    assert status == 200 and cache_control == "no-store"
    report = json.loads(payload)["report"]
    assert report["release_basis"] == "source_manifest" and report["release"]["gap"] is None
    assert report["telemetry_basis"] == "otlp_archive"
    assert report["telemetry_observation"]["source"] == "opentelemetry-otlp-json"
    assert b"private fixture log body" not in payload and b"INTERNAL_ONLY" not in payload
    assert report["release"]["inventory"]["observation"]["source"] == "kubernetes-pod"
    assert report["release"]["inventory"]["kubernetes_container"] == {
        "pod_uid": "pod-uid", "container_name": "api", "container_id": "a" * 64,
    }
    assert report["release"]["build"]["observation"]["source"] == "verified-slsa-dsse"
    assert report["release"]["change"]["approval_reference"] == "fixture:release-approval"
    assert report["telemetry_correlated"] and len(report["logs"]) == 1 and len(report["metrics"]) == 1
    assert send()[0] == 403
    assert sql("SELECT count(*) FROM craft_queue.diagnostic_reads WHERE case_id = 'archived-incident';") == "1"


def reject_wrong_otlp_binding(manifest, reject_startup, section, field, attribute_key, different, reason):
    entries = json.loads(manifest.read_text())
    entry = next(item for item in entries if item["id"] == "archived-incident")
    policy = Path(entry["policy"])
    import_policy = Path(entry["otlp"]["policy"])
    logs = Path(entry["otlp"]["logs"])
    metrics = Path(entry["otlp"]["metrics"])
    originals = {path: path.read_bytes() for path in (manifest, policy, import_policy, logs, metrics)}
    try:
        import_config = json.loads(originals[import_policy])
        if field is None:
            del import_config[section]
        else:
            previous = import_config[section][field]
            import_config[section][field] = different
            for signal, path, resources in (("logs", logs, "resourceLogs"), ("metrics", metrics, "resourceMetrics")):
                data = json.loads(originals[path])
                attribute = next(item for item in data[resources][0]["resource"]["attributes"]
                                 if item["key"] == attribute_key)
                assert attribute["value"]["stringValue"] == previous
                attribute["value"]["stringValue"] = different
                path.write_text(json.dumps(data), encoding="utf-8")
                import_config[f"{signal}_digest"] = f"sha256:{sha256(path.read_bytes()).hexdigest()}"
        import_policy.write_text(json.dumps(import_config), encoding="utf-8")
        config = json.loads(originals[policy])
        config["telemetry_digest"] = f"sha256:{sha256(import_policy.read_bytes()).hexdigest()}"
        policy.write_text(json.dumps(config), encoding="utf-8")
        entry["policy_digest"] = f"sha256:{sha256(policy.read_bytes()).hexdigest()}"
        manifest.write_text(json.dumps(entries), encoding="utf-8")
        reject_startup(reason)
    finally:
        for path, raw in originals.items():
            path.write_bytes(raw)


def exercise_managed_diagnostics(address, certificate, tokens, launcher, archive_digest):
    request = api_client(address, certificate, tokens)
    fields = {
        "grant_id": "incident-first-grant", "diagnostician_id": "diagnosis-one", "case_id": "archived-incident",
        "policy_digest": archive_digest, "from": 1795238330, "to": 1795238400, "valid_seconds": 3600,
        "max_reads": 2, "evidence": "fixture:managed-access",
    }
    action = "grant_diagnostic_access"
    request("operations", "diagnosis-one", action, 403, **fields)
    request("operations", "operator", action, 404, **(fields | {"diagnostician_id": "first"}))
    request("operations", "operator", action, 409, **(fields | {"policy_digest": "sha256:" + "0" * 64}))
    request("operations", "operator", action, 409, **(fields | {"to": 1795238399}))
    if launcher is not None:
        cli = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(certificate)]
        env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
        args = ["grant-diagnostic-access", fields["grant_id"]]
        for key, value in fields.items():
            if key != "grant_id":
                args.extend((f"--{key.replace('_', '-')}", str(value)))
        result = subprocess.run(cli + args, env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, result.stderr
        grant = json.loads(result.stdout)["grant"]
    else:
        grant = request("operations", "operator", action, **fields)["grant"]
    assert grant["issued_by"] == "operator" and grant["reads"] == 0
    assert request("operations", "operator", action, **fields)["grant"] == grant
    replacement = fields | {"grant_id": "incident-replacement-grant"}
    request("operations", "operator", action, 409, **replacement)
    request("operations", "operator", action, 409, **(fields | {"evidence": "fixture:different request"}))

    body = {key: fields[key] for key in ("case_id", "from", "to")}

    def read():
        return diagnostic_request(address, certificate, tokens, "diagnosis-one", body)

    assert read()[0] == 200
    request("operations", "diagnosis-one", "audit_diagnostic_access", 403, grant_id=fields["grant_id"])
    audit = request("operations", "operator", "audit_diagnostic_access", grant_id=fields["grant_id"])["audit"]
    assert audit["grant"]["reads"] == 1
    [receipt] = audit["receipts"]
    assert receipt["sequence"] == 1 and receipt["requested_to"] == body["to"]
    revoke = {"grant_id": fields["grant_id"], "evidence": "fixture:incident closed"}
    request("operations", "diagnosis-one", "revoke_diagnostic_access", 403, **revoke)
    revoked = request("operations", "operator", "revoke_diagnostic_access", **revoke)["grant"]
    assert revoked["revoked_by"] == "operator" and revoked["revocation_reference"] == revoke["evidence"]
    assert request("operations", "operator", "revoke_diagnostic_access", **revoke)["grant"] == revoked
    assert request("operations", "operator", action, **fields)["grant"] == revoked
    assert read()[0] == 403
    request("operations", "operator", action, **replacement)
    # A report already started under the revoked grant cannot redeem the replacement.
    assert sql(f"SELECT craft_queue.consume_diagnostic_grant('{fields['grant_id']}', 'diagnosis-one', "
               f"'archived-incident', '{archive_digest}', 1795238330, 1795238400);") == "f"
    assert read()[0] == 200
    audit = request("operations", "operator", "audit_diagnostic_access", grant_id=replacement["grant_id"])["audit"]
    assert audit["grant"]["reads"] == 1 and len(audit["receipts"]) == 1


def exercise_diagnostic_catalog_reload(server, launcher, certificate, key, config, ca, tokens,
                                       database_url, manifest, policy_digest, archive_digest):
    original = manifest.read_bytes()
    *normalized_cases, archived = json.loads(original)
    for case in normalized_cases:
        case["policy_digest"] = policy_digest
    normalized = normalized_cases[0]
    with (run_service(server, certificate, key, config, database_url, "--diagnostic-cases", str(manifest)) as (address, _),
          run_service(server, certificate, key, config, database_url, "--diagnostic-cases", str(manifest)) as (peer, _)):
        request = api_client(address, ca, tokens)
        stale_peer = api_client(peer, ca, tokens)
        action = "reload_diagnostic_catalog"
        evidence = "fixture:rotate diagnostic case"
        grant_fields = {
            "diagnostician_id": "diagnosis-peer", "from": 1795238330, "to": 1795238400,
            "valid_seconds": 3600, "max_reads": 10, "evidence": "fixture:catalog access",
        }
        old_grant = grant_fields | {
            "grant_id": "catalog-old-grant", "case_id": archived["id"], "policy_digest": archive_digest,
        }
        unchanged = grant_fields | {
            "grant_id": "catalog-unchanged-grant", "case_id": normalized["id"], "policy_digest": policy_digest,
        }

        def read(endpoint, grant):
            body = {key: grant[key] for key in ("case_id", "from", "to")}
            return diagnostic_request(endpoint, ca, tokens, "diagnosis-peer", body)

        def stage(entries):
            raw = json.dumps(entries).encode()
            replacement = manifest.with_suffix(".replacement")
            replacement.write_bytes(raw)
            replacement.replace(manifest)
            return f"sha256:{sha256(raw).hexdigest()}"

        try:
            for grant in (old_grant, unchanged):
                request("operations", "operator", "grant_diagnostic_access", **grant)
                assert read(address, grant)[0] == 200

            updated_policy = manifest.parent / "updated-diagnostic-policy.json"
            document = json.loads(Path(archived["policy"]).read_text()) | {"approval_reference": evidence}
            raw = json.dumps(document).encode()
            updated_policy.write_bytes(raw)
            updated_digest = f"sha256:{sha256(raw).hexdigest()}"
            updated = archived | {"policy": str(updated_policy.resolve()), "policy_digest": updated_digest}
            wrong = "sha256:" + "0" * 64
            invalid = stage([*normalized_cases, updated | {"policy_digest": wrong}])
            request("operations", "operator", action, 409, digest=invalid, evidence=evidence)
            assert read(address, old_grant)[0] == 200
            approved = stage([*normalized_cases, updated])
            request("operations", "diagnosis-peer", action, 403, digest=approved, evidence=evidence)
            request("operations", "operator", action, 409, digest=wrong, evidence=evidence)
            if launcher is not None:
                cmd = [str(launcher.resolve()), "shared-ops", "--queue", address, "--ca", str(ca),
                       "reload-diagnostic-catalog", "--digest", approved, "--evidence", evidence]
                env = os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
                run = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=15)
                assert run.returncode == 0, run.stderr
                receipt = json.loads(run.stdout)["diagnostic_catalog"]
            else:
                receipt = request("operations", "operator", action, digest=approved, evidence=evidence)["diagnostic_catalog"]
            assert receipt["digest"] == approved and receipt["changed"] and receipt["configured_cases"] == 3
            assert receipt["retirement"]["retired_cases"] == 1 and receipt["retirement"]["revoked_grants"] >= 1
            replay = request("operations", "operator", action, digest=approved, evidence=evidence)["diagnostic_catalog"]
            assert not replay["changed"] and replay["retirement"] == {"retired_cases": 0, "revoked_grants": 0}
            audit = request("operations", "operator", "audit_diagnostic_access", grant_id=old_grant["grant_id"])["audit"]
            assert audit["grant"]["revocation_reference"] == evidence and len(audit["receipts"]) == 2
            assert read(address, old_grant)[0] == 403 and read(peer, old_grant)[0] == 403
            stale_peer("operations", "operator", "grant_diagnostic_access", 409,
                       **(old_grant | {"grant_id": "stale-peer-new-grant"}))
            assert read(address, unchanged)[0] == 200 and read(peer, unchanged)[0] == 200

            new_grant = old_grant | {"grant_id": "catalog-new-grant", "policy_digest": updated_digest}
            request("operations", "operator", "grant_diagnostic_access", **new_grant)
            status, _, body = read(address, new_grant)
            assert status == 200 and json.loads(body)["report"]["approval_reference"] == evidence
            assert read(peer, new_grant)[0] == 403

            rollback = stage([*normalized_cases, archived])
            request("operations", "operator", action, 409, digest=rollback, evidence="fixture:retired")
            assert read(address, new_grant)[0] == 200
            inspect = "diagnostic_case_access"
            case = {"case_id": archived["id"]}
            request("operations", "diagnosis-peer", inspect, 403, **case, limit=1)
            request("operations", "operator", inspect, 400, **case, limit=51)
            first = stale_peer("operations", "operator", inspect, **case, limit=1)["case_access"]
            [first_grant] = first["grants"]
            assert first["next_after"] == first_grant["grant_id"]
            following = stale_peer("operations", "operator", inspect, **case,
                                   after=first["next_after"], limit=1)["case_access"]
            assert following["grants"][0]["grant_id"] > first["next_after"]
            pinned = case | {"policy_digest": updated_digest}
            active = stale_peer("operations", "operator", inspect, **pinned, limit=1)["case_access"]
            assert active["retirement"] is None and active["grants"][0]["active"]
            assert active["grants"][0]["grant_id"] == new_grant["grant_id"]
            assert "receipts" not in active["grants"][0] and "approval_reference" not in active["grants"][0]
            close = pinned | {"evidence": "fixture:retire from a stale catalog"}
            stale_peer("operations", "diagnosis-peer", "retire_diagnostic_case", 403, **close)
            if launcher is not None:
                run = subprocess.run(
                    [str(launcher.resolve()), "shared-ops", "--queue", peer, "--ca", str(ca),
                     "retire-diagnostic-case", archived["id"], "--policy-digest", updated_digest,
                     "--evidence", close["evidence"]],
                    env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]},
                    capture_output=True, text=True, timeout=15,
                )
                assert run.returncode == 0, run.stderr
                retired = json.loads(run.stdout)["case_retirement"]
            else:
                retired = stale_peer("operations", "operator", "retire_diagnostic_case", **close)["case_retirement"]
            assert retired["revoked_grants"] == 1 and retired["retirement"]["retired_by"] == "operator"
            assert retired["retirement"]["replacement_catalog_digest"] is None
            replay = stale_peer("operations", "operator", "retire_diagnostic_case", **close)["case_retirement"]
            assert replay["revoked_grants"] == 0 and replay["retirement"] == retired["retirement"]
            after = stale_peer("operations", "operator", inspect, **pinned, limit=1)["case_access"]
            assert not after["grants"][0]["active"] and after["retirement"] == retired["retirement"]
            assert read(address, new_grant)[0] == 403 and read(address, unchanged)[0] == 200
            request("operations", "operator", "grant_diagnostic_access", 409,
                    **(new_grant | {"grant_id": "retired-new-grant"}))
            removed = stage(normalized_cases)
            request("operations", "operator", action, digest=removed, evidence="fixture:close incident")
            assert read(address, new_grant)[0] == 403 and read(address, unchanged)[0] == 200
            request("operations", "operator", "grant_diagnostic_access", 409,
                    **(new_grant | {"grant_id": "removed-new-grant"}))
        finally:
            manifest.write_bytes(original)
    print("Verified catalog reload, cross-process case inspection and retirement, and rejected rollback.")


def exercise_restore(server, launcher, certificate, key, config, ca, tokens, root, manifest, policy_digest):
    # Use generated databases in the same disposable cluster; the original test database stays intact.
    source_db = f"craft_backup_{uuid4().hex}"
    restore_db = f"craft_restore_{uuid4().hex}"
    for database in (source_db, restore_db):
        subprocess.run(["createdb", "--template=template0", database], check=True, timeout=20)
    diagnostics_upgrade = ROOT / "migrations" / "033_managed_diagnostic_access.sql"
    for migration in sorted((ROOT / "migrations").glob("*.sql")):
        if migration >= diagnostics_upgrade:
            break
        sql(migration.read_text(encoding="utf-8"), source_db)
    action_key = sql(f"""
        UPDATE craft_queue.control SET paused = false, review_limit = 3;
        INSERT INTO craft_queue.runner_repositories VALUES ('first','team/service'), ('second','team/service');
        INSERT INTO craft_queue.draft_policies VALUES ('team/service','main','publisher',true);
        GRANT USAGE ON SCHEMA craft_queue TO craft_test_service;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA craft_queue TO craft_test_service;
        DO $$
        DECLARE
            pack_digest text;
            verdict bigint;
        BEGIN
            PERFORM craft_queue.admit('restore-draft','team/service','restore:draft',
                '{{"schema_version":1,"id":"restore-draft","base":"{BASE}","intent":"private-prompt-fixture"}}'::jsonb, 40);
            PERFORM craft_queue.claim('first','10000000-0000-0000-0000-000000000000');
            pack_digest := craft_queue.submit_candidate_pack('first','restore-draft',1,'{CANDIDATE}',
                decode('5041434b00','hex'),20);
            verdict := craft_queue.record_verification('verifier','restore-draft',1,'{CANDIDATE}',pack_digest,0,true,
                'fixture:targeted-test','fixture checks');
            PERFORM craft_queue.reserve_draft('operator','restore-draft',1,'{CANDIDATE}',verdict);
            PERFORM craft_queue.admit('restore-lease','team/service','restore:lease',
                '{{"schema_version":1,"id":"restore-lease","base":"{BASE}"}}'::jsonb, 40);
            PERFORM craft_queue.claim('second','20000000-0000-0000-0000-000000000000');
            PERFORM craft_queue.admit('restore-verification','team/service','restore:verification',
                '{{"schema_version":1,"id":"restore-verification","base":"{BASE}"}}'::jsonb, 40);
            PERFORM craft_queue.claim('first','30000000-0000-0000-0000-000000000000');
            PERFORM craft_queue.submit_candidate_pack('first','restore-verification',1,'{CANDIDATE}',
                decode('5041434b00','hex'),20);
        END;
        $$;
        SELECT action_key FROM craft_queue.publications;
    """, source_db)
    assert action_key.startswith("draft:")
    sql(f"""
        INSERT INTO craft_queue.diagnostic_grants (
            identity_id, case_id, policy_digest, allowed_from, allowed_to, expires_at, max_reads, approval_reference
        ) VALUES ('diagnosis-one', 'staging-incident', '{policy_digest}', 1795238330, 1795238400,
            clock_timestamp() + interval '1 hour', 2, 'fixture:restore-grant');
        SELECT craft_queue.consume_diagnostic('diagnosis-one', 'staging-incident', '{policy_digest}', 1795238330, 1795238400);
        SELECT craft_queue.admit('restore-fallback','team/service','restore:fallback',
            '{{"schema_version":1,"id":"restore-fallback","base":"{BASE}","execution_profile":{{
                "id":"routine","model":"fixture-mini","reasoning_effort":"low","timeout_seconds":8,
                "max_attempts":1,"approval_reference":"fixture:initial-profile"
            }}}}'::jsonb, 40);
        SELECT craft_queue.approve_profile_fallback('operator','restore-fallback','restore-fallback-next',
            '{{"id":"deep","model":"fixture-mini","reasoning_effort":"high","timeout_seconds":8,
                "max_attempts":1,"approval_reference":"fixture:fallback-profile"}}'::jsonb,
            'fixture:restore-fallback', 3600);
        SELECT craft_queue.grant_review_inbox('operator','restore-review','reviewer','team/service',
            'fixture:restore-review', 3600);
        SELECT craft_queue.approve_claude_repository('operator','team/service','restore-claude',
            'fixture:restore-claude', 3600);
    """, source_db)
    unmanaged = run_queue_migrations(database=source_db)
    assert unmanaged.returncode != 0 and "explicit verified migration baseline" in unmanaged.stderr
    baseline = str(int(diagnostics_upgrade.name[:3]) - 1)
    migrated = run_queue_migrations("--baseline-through", baseline, database=source_db)
    assert migrated.returncode == 0, migrated.stderr
    assert sql("SELECT count(*) FROM craft_migration.queue_history WHERE external_baseline;", source_db) == baseline
    upgraded_id = sql("SELECT grant_id FROM craft_queue.diagnostic_reads;", source_db)
    assert upgraded_id and sql(f"SELECT grant_id FROM craft_queue.diagnostic_grants;", source_db) == upgraded_id
    assert sql("SELECT count(*) FROM craft_queue.claim('first', gen_random_uuid(), true, true, "
               "ARRAY[]::text[], 'codex');", source_db) == "0"
    assert sql("SELECT count(*) FROM craft_queue.runner_claim_polls;", source_db) == "1"
    shadow_pin = "sha256:" + "1" * 64
    restore_policy = json.dumps({
        "id": "restore-enqueue-policy", "repository": "team/service", "base_branch": "main",
        "publisher": "publisher", "shadow_policy_digest": shadow_pin, "allowed_paths": ["src/hello"],
        "max_files": 1, "max_commits": 1, "max_changed_lines": 20, "daily_limit": 1, "valid_seconds": 600,
        "evidence": "fixture:standing policy before restore",
    })
    sql(f"""
        INSERT INTO craft_queue.shadow_policies VALUES ('team/service', '{shadow_pin}', true, 'operator',
            'fixture:restore-shadow', clock_timestamp(), clock_timestamp() + interval '1 hour');
        SELECT craft_queue.approve_github_enqueue_policy('operator', '{restore_policy}'::jsonb);
        INSERT INTO craft_queue.shadow_merge_receipts (
            id, work_id, generation, verification_sequence, publication_key, head, pack_digest, repository,
            policy_digest, report_digest, assessment, actor
        ) SELECT 'restore-enqueue-receipt', p.work_id, p.generation, p.verification_sequence,
            p.action_key, p.head, p.pack_digest, p.repository, '{shadow_pin}', '{shadow_pin}', '{{}}', 'operator'
        FROM craft_queue.publications AS p WHERE p.action_key = '{action_key}';
        INSERT INTO craft_queue.github_enqueue_intents (
            id, receipt_id, work_id, repository, pull_request, head, base_branch, pull_request_node_id,
            publisher, approved_by, approval, approved_at, expires_at, state
        ) VALUES ('restore-enqueue', 'restore-enqueue-receipt', 'restore-draft', 'team/service',
            'https://github.com/team/service/pull/201', '{CANDIDATE}', 'main', 'PR_restore_fixture',
            'publisher', 'operator', 'fixture:enqueue approval snapshot',
            clock_timestamp(), clock_timestamp() + interval '5 minutes', 'approved'),
            ('restore-enqueue-old-revision', 'restore-enqueue-receipt', 'restore-lease', 'team/service',
            'https://github.com/team/service/pull/202', '{CANDIDATE}', 'main', 'PR_restore_older_fixture',
            'publisher', 'operator', 'fixture:approval before the work advanced',
            clock_timestamp(), clock_timestamp() + interval '5 minutes', 'approved');
    """, source_db)
    pickup = json.loads(sql("SELECT craft_queue.claim_pending_verification("
                            "'service-verifier','team/service',NULL);", source_db))
    assert pickup["work_id"] == "restore-verification"
    archive = root / "queue.dump"
    subprocess.run(["pg_dump", "--format=custom", "--file", str(archive), source_db], check=True, timeout=30)
    # The forge request could have happened after the recoverable backup point.
    assert sql(f"SELECT craft_queue.begin_draft('publisher','{action_key}')->>'state';", source_db) == "sending"
    subprocess.run(["pg_restore", "--exit-on-error", "--dbname", restore_db, str(archive)], check=True, timeout=30)

    epoch = str(uuid4())
    database_url = urlsplit(os.environ["CRAFT_TEST_DATABASE_URL"])._replace(path=f"/{restore_db}").geturl()
    recovery_args = ("--recover-restored-database", epoch, "--diagnostic-cases", str(manifest))
    verdict = {
        "work_id": "restore-verification", "generation": 1, "head": CANDIDATE,
        "digest": "sha256:" + sha256(b"PACK\0").hexdigest(), "previous_sequence": 0,
        "passed": True, "evidence": "fixture:restored independent checks", "note": "Checked the restored candidate.",
    }

    def reject_recovery_start(recovery_epoch, reason):
        result = subprocess.run([str(server.resolve()), "--bind", "127.0.0.1:0", "--certificate", str(certificate),
                                 "--private-key", str(key), "--identities", str(config),
                                 "--recover-restored-database", recovery_epoch],
                                env=os.environ | {"CRAFT_DATABASE_URL": database_url},
                                text=True, capture_output=True, timeout=10)
        assert result.returncode != 0 and not result.stdout and reason in result.stderr, result.stderr

    with run_service(server, certificate, key, config, database_url, *recovery_args) as (address, _process):
        request = api_client(address, ca, tokens)
        parsed = urlsplit(address)
        health = http.client.HTTPSConnection(
            parsed.hostname, parsed.port, context=ssl.create_default_context(cafile=str(ca)), timeout=5
        )
        try:
            health.request("GET", "/healthz")
            reply = health.getresponse()
            assert reply.status == 200 and json.loads(reply.read()) == {"schema_version": 1, "ready": True}
        finally:
            health.close()

        request("operations", "first", "summary", 403)
        request("operations", "first", "recovery_status", 403)
        recovery = request("operations", "operator", "recovery_status")["recovery"]
        assert recovery["epoch"] == epoch and recovery["finished_at"] is None
        if launcher is not None:
            result = subprocess.check_output([str(launcher.resolve()), "shared-ops", "--queue", address, "--ca",
                                              str(ca), "recovery-status"], text=True, timeout=10,
                                             env=os.environ | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]})
            assert json.loads(result)["recovery"] == recovery
        assert diagnostic_request(address, ca, tokens, "diagnosis-one", {
            "case_id": "staging-incident", "from": 1795238330, "to": 1795238400,
        })[0] == 403
        assert sql("SELECT count(*) FROM craft_queue.diagnostic_reads;", restore_db) == "1"
        assert sql("SELECT count(*) FROM craft_queue.runner_claim_polls;", restore_db) == "0"
        assert sql("SELECT revoked_at IS NOT NULL FROM craft_queue.diagnostic_grants;", restore_db) == "t"
        managed = request("operations", "operator", "audit_diagnostic_access", grant_id=upgraded_id)["audit"]
        assert managed["grant"]["revocation_reference"] == f"restore:{epoch}" and len(managed["receipts"]) == 1
        fallback = request("operations", "operator", "audit", work_id="restore-fallback")["audit"]["profile_fallback"]
        assert fallback["revoked_by"] == f"restore:{epoch}" and "applied_at" not in fallback
        assert request("reviewer", "reviewer", "review_inbox")["inbox"]["total"] == 0
        inbox = request("operations", "operator", "review_inbox")["inbox"]
        assert {item["work_id"] for item in inbox["items"]} == {"restore-draft", "restore-verification"}
        assert sql("SELECT revocation_reference FROM craft_queue.review_inbox_grants "
                   "WHERE grant_id = 'restore-review';", restore_db) == f"restore:{epoch}"
        assert sql("SELECT revocation_reference FROM craft_queue.claude_repository_approvals "
                   "WHERE grant_id = 'restore-claude';", restore_db) == f"restore:{epoch}"
        summary = request("operations", "operator", "summary")["summary"]
        assert summary["paused"] and summary["recovery_active"]
        assert summary["queued"] == 2 and summary["review_reserved"] == 2
        assert summary["live_leases"] == 0 and summary["held_publications"] == 1
        assert summary["tokens_charged_or_reserved"] == 80
        assert summary["oldest_queued_seconds"] >= 0
        assert "private-prompt-fixture" not in json.dumps(summary)
        with ThreadPoolExecutor(max_workers=8) as pool:
            summaries = list(pool.map(lambda _: request("operations", "operator", "summary")["summary"], range(24)))
        assert all(row["paused"] and row["held_publications"] == 1 for row in summaries)
        audit = request("operations", "operator", "audit", work_id="restore-draft")["audit"]
        assert audit["publications"][0]["state"] == "uncertain"
        assert audit["verifications"][0]["evidence"] == "fixture:targeted-test"
        assert "private-prompt-fixture" not in json.dumps(audit)
        request("verifier", "service-verifier", "record_verification", 409, **verdict)
        request("verifier", "service-verifier", "record_verification", 409, **verdict, pickup_id=pickup["pickup_id"])
        assert request("operations", "operator", "audit",
                       work_id=verdict["work_id"])["audit"]["verifications"] == []
        assert request("operations", "operator", "held_publications")["holds"][0]["action_key"] == action_key
        request("runner", "second", "renew", 409, work_id="restore-lease", generation=1)
        assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
        request("publisher", "publisher", "begin", 409, action_key=action_key)
        request("operator", "operator", "set_paused", 409, paused=False)
        request("operations", "operator", "finish_recovery", 409, epoch=epoch, evidence="fixture:reconciled")

    reject_recovery_start(str(uuid4()), "could not fence the restored PostgreSQL queue")
    with run_service(server, certificate, key, config, database_url, *recovery_args) as (address, _process):
        request = api_client(address, ca, tokens)
        assert request("operations", "operator", "recovery_status")["recovery"] == recovery
        assert request("operations", "operator", "held_publications")["holds"][0]["action_key"] == action_key
        request("operations", "operator", "set_runner_enrollment", 409, runner_id="first",
                repository="team/new-recovery-enrollment", expected_revision=0, enrolled=True,
                evidence="fixture:new-access-during-recovery")
        assert request("runner", "first", "claim", request_id=str(uuid4()))["claim"] is None
        request("publication", "operator", "reconcile", action_key=action_key, observation={
            "outcome": "confirmed", "head": CANDIDATE,
            "native_url": "https://bitbucket.org/team/service/pull-requests/201",
            "source_branch": "craft-factory/restore-draft", "base_branch": "main",
            "evidence": "fixture:matching native draft observed after restore",
        })
        [enqueue, older] = request("publication", "operator", "github_enqueue_holds")["github_enqueue_holds"]
        assert enqueue["id"] == "restore-enqueue" and enqueue["state"] == "uncertain" and enqueue["send_possible"]
        assert enqueue["resolution_evidence"] == f"Restore recovery {epoch}"
        assert older["id"] == "restore-enqueue-old-revision" and older["state"] == "uncertain"
        assert sql("SELECT revocation_evidence FROM craft_queue.github_enqueue_policies "
                   "WHERE id = 'restore-enqueue-policy';", restore_db) == f"Restore recovery {epoch}"
        assert request("operator", "operator", "status", work_id="restore-lease")["work"]["state"] == "queued"
        request("publisher", "publisher", "begin_github_enqueue", 409, intent_id=enqueue["id"])
        request("operations", "operator", "finish_recovery", 409, epoch=epoch, evidence="fixture:reconciled")
        for intent in (enqueue, older):
            request("publication", "operator", "resolve_github_enqueue", intent_id=intent["id"], resolution={
                "outcome": "abandoned", "queue_entry_id": None, "evidence": "fixture:stopped sender and reconciled native state",
            })
        finished = request("operations", "operator", "finish_recovery", epoch=epoch, evidence="fixture:reconciled")
        assert finished == request("operations", "operator", "finish_recovery",
                                   epoch=epoch, evidence="fixture:reconciled")
        summary = request("operations", "operator", "summary")["summary"]
        assert summary["paused"] and not summary["recovery_active"] and summary["held_publications"] == 0
        policy = request("operations", "operator", "draft_policy", repository="team/service")["policy"]
        assert not policy["enabled"] and policy["revision"] == 2
        request("operations", "operator", "configure_draft_policy", 409,
                repository="team/service", base_branch="main", publisher_id="publisher", forge="bitbucket",
                expected_revision=1, enabled=True, evidence="fixture:stale approval from before restore")

    reject_recovery_start(epoch, "this restore recovery epoch already finished")
    with run_service(server, certificate, key, config, database_url) as (address, _process):
        request = api_client(address, ca, tokens)
        assert request("operations", "operator", "recovery_status")["recovery"] == finished["recovery"]
        assert request("operations", "operator", "summary")["summary"]["paused"]
        request("verifier", "service-verifier", "record_verification", 409, **verdict, pickup_id=pickup["pickup_id"])
        recorded = request("verifier", "service-verifier", "record_verification", **verdict)
        assert request("verifier", "service-verifier", "record_verification", **verdict) == recorded
        request("operator", "operator", "set_paused", paused=False)
        claim = request("runner", "first", "claim", request_id=str(uuid4()))["claim"]
        assert claim["work_id"] == "restore-lease" and claim["generation"] == 2


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("server", type=Path)
    parser.add_argument("--launcher", type=Path)
    parser.add_argument("--install-assets", type=Path, default=ROOT)
    args = parser.parse_args()
    sql("CREATE ROLE craft_test_service LOGIN PASSWORD 'disposable-fixture';")
    install_assets = args.install_assets.resolve(strict=True)
    exercise_queue_installer(install_assets)
    exercise_standing_intent_upgrades(install_assets)
    exercise_dependency_approval_upgrade(install_assets)
    sql("""
        INSERT INTO craft_queue.runner_repositories VALUES
            ('first', 'team/service'), ('second', 'team/service'), ('outside', 'team/other');
        INSERT INTO craft_queue.draft_policies (repository, base_branch, publisher_id)
            VALUES ('team/service', 'main', 'publisher');
        DO $$
        BEGIN
            IF has_table_privilege('craft_test_service', 'craft_queue.work', 'SELECT, INSERT, UPDATE') THEN
                RAISE EXCEPTION 'service login has direct table access';
            END IF;
        END;
        $$;
    """)
    verifier_scopes = {"service-verifier": ["team/service"], "peer-verifier": ["team/service"], "other-verifier": ["team/other"]}
    names = ("operator", "verifier", "first", "second", "outside", "publisher", "publisher-other",
             "intake", "other-intake", "diagnosis-one", "diagnosis-peer", "release-one", "release-other", "scheduler",
             "reviewer", "reviewer-other", "monitor", "context-reader", *verifier_scopes)
    tokens = {name: secrets.token_urlsafe(32) for name in names}
    with TemporaryDirectory() as directory:
        root = Path(directory)
        certificate, key, config = root / "server.pem", root / "key.pem", root / "identities.json"
        ca, ca_key = root / "ca.pem", root / "ca-key.pem"
        csr = root / "server.csr"
        extensions = root / "server-ext.cnf"
        extensions.write_text(
            "basicConstraints=critical,CA:FALSE\n"
            "keyUsage=critical,digitalSignature,keyEncipherment\n"
            "extendedKeyUsage=serverAuth\n"
            "subjectKeyIdentifier=hash\n"
            "authorityKeyIdentifier=keyid\n"
            "subjectAltName=IP:127.0.0.1\n",
            encoding="utf-8",
        )

        def openssl(*arguments):
            subprocess.run(["openssl", *map(str, arguments)], capture_output=True, check=True, timeout=10)

        openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-keyout", ca_key, "-out", ca, "-subj", "/CN=Craft fixture CA",
                "-addext", "basicConstraints=critical,CA:TRUE",
                "-addext", "keyUsage=critical,keyCertSign,cRLSign")
        openssl("req", "-new", "-newkey", "rsa:2048", "-nodes",
                "-keyout", key, "-out", csr, "-subj", "/CN=localhost")
        openssl("x509", "-req", "-in", csr, "-CA", ca, "-CAkey", ca_key,
                "-CAcreateserial", "-out", certificate, "-days", "1", "-extfile", extensions)
        identities = [
            {"id": name, "role": "release_observer" if name.startswith("release-") else "diagnostician" if name.startswith("diagnosis-") else "publisher" if name.startswith("publisher") else (
                "profile_scheduler" if name == "scheduler" else
                "metrics_reader" if name == "monitor" else
                "reviewer" if name.startswith("reviewer") else
                "intake" if name.endswith("intake") else name if name in ("operator", "verifier") else "runner"),
             "token_sha256": sha256(tokens[name].encode()).hexdigest()}
            | ({"role": "verifier", "verifier_repositories": verifier_scopes[name]} if name in verifier_scopes else {})
            | ({"role": "context_reader", "context_repositories": ["team/service"]} if name == "context-reader" else {})
            for name in names
        ]
        if args.launcher is not None:
            token_file = root / "issued-monitor-token"
            issue = [str(args.launcher.resolve()), "issue-queue-identity", "monitor", "--role", "metrics-reader",
                     "--token-file", str(token_file)]
            issued = subprocess.run(issue, check=True, text=True, capture_output=True, timeout=10)
            raw = token_file.read_bytes()
            token = raw.rstrip(b"\r\n")
            assert re.fullmatch(rb"[A-Za-z0-9_-]{43}", token) and raw == token + b"\n"
            assert token not in issued.stdout.encode() and token not in issued.stderr.encode()
            if os.name != "nt":
                assert token_file.stat().st_mode & 0o077 == 0
            identity = json.loads(issued.stdout)
            assert identity == {"id": "monitor", "role": "metrics_reader", "token_sha256": sha256(token).hexdigest()}
            duplicate = subprocess.run(issue, text=True, capture_output=True, timeout=10)
            assert duplicate.returncode != 0 and not duplicate.stdout and token_file.read_bytes() == raw
            tokens["monitor"] = token.decode()
            identities[names.index("monitor")] = identity
            context_token = root / "issued-context-token"
            issue_context = [str(args.launcher.resolve()), "issue-queue-identity", "context-reader",
                             "--role", "context-reader", "--token-file", str(context_token)]
            invalid = subprocess.run(issue_context, text=True, capture_output=True, timeout=10)
            assert invalid.returncode != 0 and not context_token.exists()
            issued = subprocess.check_output([*issue_context, "--context-repository", "team/service"], text=True, timeout=10)
            token = context_token.read_bytes().rstrip(b"\r\n")
            identity = json.loads(issued)
            assert token not in issued.encode()
            assert identity == {"id": "context-reader", "role": "context_reader",
                                "token_sha256": sha256(token).hexdigest(), "context_repositories": ["team/service"]}
            tokens["context-reader"] = token.decode()
            identities[names.index("context-reader")] = identity
        config.write_text(json.dumps(identities), encoding="utf-8")
        manifest, policy_digest, archive_digest, archive_files = diagnostic_manifest(root)
        if args.launcher is not None:
            exercise_build_capture(args.launcher, root, certificate, key, ca)
        archived_pod, archived_logs = archive_files
        original_pod = archived_pod.read_bytes()
        original_logs = archived_logs.read_bytes()
        database_url = os.environ["CRAFT_TEST_DATABASE_URL"]
        exercise_identity_reload(args.server, args.launcher, certificate, key, config, ca, tokens, database_url)
        exercise_queue_preflight(args.server, certificate, key, ca_key, config, database_url)
        exercise_shutdown(args.server, certificate, key, config, database_url, ca, tokens)
        credentials = root / "queue-credentials"
        credentials.mkdir(mode=0o700)
        credential = credentials / "CRAFT_DATABASE_URL"
        credential.write_text(database_url + "\n", encoding="utf-8")
        credential.chmod(0o600)
        with run_service(args.server, certificate, key, config, database_url,
                         "--diagnostic-cases", str(manifest), credentials=credentials) as (address, _process):
            exercise_diagnostics(address, ca, tokens, args.launcher, policy_digest)
            archived_pod.write_bytes(b"{}")
            exercise_archive_diagnostics(address, ca, tokens, archive_digest)
            exercise_managed_diagnostics(address, ca, tokens, args.launcher, archive_digest)
            exercise(address, ca, tokens, args.launcher)
        def reject_changed_archive(reason):
            invalid = subprocess.run(
                [str(args.server.resolve()), "--bind", "127.0.0.1:0", "--certificate", str(certificate),
                 "--private-key", str(key), "--identities", str(config), "--diagnostic-cases", str(manifest)],
                env=os.environ | {"CRAFT_DATABASE_URL": database_url},
                capture_output=True, text=True, timeout=10,
            )
            assert invalid.returncode != 0 and invalid.stdout == "", invalid.stdout
            assert reason in invalid.stderr, invalid.stderr

        try:
            reject_changed_archive("Kubernetes Pod does not match its pinned digest")
        finally:
            archived_pod.write_bytes(original_pod)
        try:
            archived_logs.write_bytes(b"{}")
            reject_changed_archive("OTLP export does not match the approved digest")
        finally:
            archived_logs.write_bytes(original_logs)
        reject_wrong_otlp_binding(manifest, reject_changed_archive, "version", "deployment_id",
                                  "craft.deployment.id", "other-rollout-same-artifact",
                                  "OTLP collection version does not match the observed deployment")
        for field, attribute, wrong_value in (("pod_uid", "k8s.pod.uid", "other-pod"),
                                               ("container_name", "k8s.container.name", "sidecar"),
                                               ("container_id", "container.id", "c" * 64)):
            reject_wrong_otlp_binding(manifest, reject_changed_archive, "kubernetes_container",
                                      field, attribute, wrong_value,
                                      "OTLP collection container does not match the observed Kubernetes container")
        reject_wrong_otlp_binding(manifest, reject_changed_archive, "kubernetes_container", None, None, None,
                                  "OTLP diagnosis of a Kubernetes deployment requires an approved container identity")
        exercise_diagnostic_catalog_reload(args.server, args.launcher, certificate, key, config, ca, tokens,
                                           database_url, manifest, policy_digest, archive_digest)
        if args.launcher is not None:
            exercise_archived_rollout(args.server, args.launcher, root, certificate, key, config, ca, tokens, database_url)
            exercise_archived_rollout(args.server, args.launcher, root, certificate, key, config, ca, tokens, database_url,
                                      github=True)
        exercise_restore(args.server, args.launcher, certificate, key, config, ca, tokens, root, manifest, policy_digest)
    print("Verified serialized queue install, drift rejection, transactional failure recovery, and a baselined upgrade.")
    print("Verified read-only preflight, exact service function permissions, and startup fencing before recovery.")
    print("Verified TLS, scoped execution, draft repair, publication holds, cutover, and backup recovery.")
    print("Verified raw OTLP diagnosis bound to the running container, scoped grants, and receipt migration.")
    if args.launcher is not None:
        print("Verified signed build publication and capture over scoped HTTPS, conditional replay, and refusal to follow redirects.")
        print("Verified shared skill pins, repository scope, and revocation of leases and reserved sends.")
        print("Verified approved decision and runbook pins, incompatible runners, and context supersession.")
        print("Verified standing slot reservation, runner fallback, stale observations, reassignment, and approval.")
        print("Verified Jira relay identity, intent drift, replacement approval, and reassignment end to end.")
        print("Verified ordered cross-repo release, observer inbox and status, prerequisite drift, and recovery fencing.")
        print("Verified server-side rollout policy pins, fresh evidence, observer scope, and dependent pickup.")
        print("Verified ledger-pinned rollout archives, exact reviewed head, and rejection of legacy writes and catalog drift.")
        print("Verified archive rollout freshness expires downstream authorization and survives receipt replay.")
        print("Verified approved evidence refresh restored downstream eligibility while preserving the original release receipt.")


if __name__ == "__main__":
    main()
