"""Check an extracted factory kit against disposable PostgreSQL and loopback HTTPS."""

import argparse
import base64
from contextlib import contextmanager
import csv
from datetime import datetime
from hashlib import sha256
from http.client import HTTPSConnection
import json
import os
from pathlib import Path
from queue import Empty, Queue
import secrets
import shutil
import socket
import ssl
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Thread
from urllib.parse import urlencode, urlsplit
from uuid import uuid4

from disposable_tls import create_tls_files


def allow_windows_fixture_user(root):
    # An elevated Python process can create an owner-only directory owned by Administrators.
    # PostgreSQL drops that group before it reads the initdb password and creates the cluster.
    system = Path(os.environ["SystemRoot"]) / "System32"
    identity = subprocess.check_output(
        [system / "whoami.exe", "/user", "/fo", "csv", "/nh"], text=True, timeout=10)
    _, sid = next(csv.reader(identity.splitlines()))
    subprocess.run([system / "icacls.exe", root, "/grant", f"*{sid}:(OI)(CI)M"],
                   stdout=subprocess.DEVNULL, check=True, timeout=10)


@contextmanager
def private_postgres(root, env, tools):
    data = root / "postgres"
    pg = env | {"PGUSER": "craft_smoke_admin", "PGDATABASE": "postgres"}
    initdb = [tools["initdb"], "--no-locale", "--encoding=UTF8", "--pgdata", data]
    password_file = None
    if os.name == "nt":
        allow_windows_fixture_user(root)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = str(listener.getsockname()[1])
        pg.update(PGHOST="127.0.0.1", PGPORT=port, PGPASSWORD=secrets.token_urlsafe(32),
                  PGUSER="craft_smoke_" + secrets.token_hex(8))
        password_file = root / "initdb-password"
        password_file.write_text(pg["PGPASSWORD"] + "\n", encoding="ascii")
        initdb += ["--auth=scram-sha-256", "--pwfile", password_file]
        options = f"-h 127.0.0.1 -p {port}"
    else:
        socket_dir = root / "socket"
        socket_dir.mkdir(mode=0o700)
        pg.update(PGHOST=str(socket_dir), PGPORT="5432")
        initdb += ["--auth-local=trust", "--auth-host=reject"]
        options = f"-c listen_addresses='' -c unix_socket_directories={socket_dir} -c unix_socket_permissions=0700"

    try:
        subprocess.run(initdb + ["--username", pg["PGUSER"]], env=pg, cwd=root,
                       stdout=subprocess.DEVNULL, check=True, timeout=60)
    finally:
        if password_file is not None:
            password_file.unlink(missing_ok=True)
    command = [tools["pg_ctl"], "--pgdata", data, "--timeout", "20", "--wait"]
    try:
        subprocess.run(command + ["--log", root / "postgres.log", "--options", options, "start"],
                       env=pg, cwd=root, stdout=subprocess.DEVNULL, check=True, timeout=25)
        directory = subprocess.check_output(
            [tools["psql"], "-XAt", "--no-password", "--command", "SHOW data_directory"],
            env=pg, cwd=root, text=True, timeout=10).strip()
        if Path(directory).resolve() != data.resolve():
            raise RuntimeError("The database connection did not reach the disposable cluster.")
        yield pg
    finally:
        running = subprocess.run(command + ["status"], env=pg, cwd=root, capture_output=True, timeout=5)
        if running.returncode == 0:
            subprocess.run(command + ["--mode", "fast", "stop"], env=pg, cwd=root,
                           stdout=subprocess.DEVNULL, check=True, timeout=25)


@contextmanager
def queue_server(command, root, env, *, host="127.0.0.1"):
    with subprocess.Popen(command, cwd=root, env=env, stdout=subprocess.PIPE, text=True) as server:
        announcement = Queue()
        reader = Thread(target=lambda: announcement.put(server.stdout.readline(128)), daemon=True)
        reader.start()
        try:
            try:
                origin = announcement.get(timeout=10).strip()
            except Empty as error:
                raise RuntimeError("The disposable queue did not announce its address.") from error
            address = urlsplit(origin)
            if address.scheme != "https" or address.hostname != host or not address.port:
                raise RuntimeError("The disposable queue did not open the expected HTTPS address.")
            yield origin
        finally:
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
            reader.join(timeout=5)


@contextmanager
def disposable_queue_database(root, postgres_bin, identity_options, *, queue_host="127.0.0.1"):
    env = {name: os.environ[name] for name in ("PATH", "LD_LIBRARY_PATH", "SYSTEMROOT", "WINDIR", "COMSPEC")
           if name in os.environ}
    env.update(HOME=str(root), TMPDIR=str(root), LC_ALL="C")
    if os.name == "nt":
        env.update(TEMP=str(root), TMP=str(root))
    env["PATH"] = os.pathsep.join((str(postgres_bin), env.get("PATH", os.defpath))) if postgres_bin else env.get("PATH", os.defpath)
    tools = {name: shutil.which(name, path=env["PATH"]) for name in ("initdb", "pg_ctl", "createdb", "psql")}
    missing = [name for name, path in tools.items() if path is None]
    if missing:
        raise RuntimeError("Required PostgreSQL tools are missing: " + ", ".join(missing))
    suffix = ".exe" if os.name == "nt" else ""
    ca = root / "ca.pem"
    create_tls_files(root, env=env, address=queue_host)
    tokens = {role: secrets.token_urlsafe(32) for role in identity_options}
    identities = root / "identities.json"
    identities.write_text(json.dumps([
        {"id": f"smoke-{role}", "role": role, "token_sha256": sha256(token.encode()).hexdigest(),
         **identity_options[role]}
        for role, token in tokens.items()
    ]), encoding="utf-8")

    with private_postgres(root, env, tools) as pg:
        role = "CREATE ROLE craft_smoke_api LOGIN"
        if os.name == "nt":
            password = secrets.token_urlsafe(32)
            role += f" PASSWORD '{password}'"
            database_url = f"postgres://craft_smoke_api:{password}@127.0.0.1:{pg['PGPORT']}/craft_smoke?sslmode=disable"
        else:
            database_url = "postgres://craft_smoke_api@localhost/craft_smoke?" + urlencode({"host": pg["PGHOST"]})
        subprocess.run([tools["psql"], "-X", "--no-password", "--set", "ON_ERROR_STOP=1"],
                       input=role + ";\n", text=True, env=pg, cwd=root,
                       stdout=subprocess.DEVNULL, check=True, timeout=10)
        subprocess.run([tools["createdb"], "--template=template0", "craft_smoke"],
                       env=pg, cwd=root, check=True, timeout=10)
        database_env = pg | {"PGDATABASE": "craft_smoke"}

        def migrate(kit):
            return subprocess.check_output(
                [sys.executable, "-B", kit / "shared/migrate_queue.py", "--service-role", "craft_smoke_api"],
                env=database_env, cwd=root, text=True, timeout=90)

        def configure(review_limit, lease_seconds):
            subprocess.run([tools["psql"], "-X", "--no-password", "--set", "ON_ERROR_STOP=1",
                            "--set", f"review_limit={review_limit}", "--set", f"lease_seconds={lease_seconds}"],
                           input=("UPDATE craft_queue.control SET review_limit = :'review_limit'::integer, "
                                  "lease_seconds = :'lease_seconds'::integer;\n"),
                           text=True, env=database_env, cwd=root,
                           stdout=subprocess.DEVNULL, check=True, timeout=10)

        connector = root / "database-url"
        with os.fdopen(os.open(connector, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="utf-8") as stream:
            stream.write(database_url + "\n")
        service_env = env | {"CRAFT_DATABASE_URL_FILE": str(connector)}

        def command(kit):
            return [kit / f"shared/craft-factory-queue{suffix}", "--bind", f"{queue_host}:0",
                    "--certificate", root / "server.pem", "--private-key", root / "server.key", "--identities", identities]

        yield migrate, configure, command, service_env, ca, tokens, env


@contextmanager
def disposable_queue(kit, root, postgres_bin, identity_options, *, review_limit=1, lease_seconds=120,
                     queue_host="127.0.0.1"):
    with disposable_queue_database(root, postgres_bin, identity_options, queue_host=queue_host) as database:
        migrate, configure, command, service_env, ca, tokens, env = database
        migrate(kit)
        if review_limit != 1 or lease_seconds != 120:
            configure(review_limit, lease_seconds)
        service = command(kit)
        config = json.loads(subprocess.check_output(service + ["--check-config"], cwd=root, env=service_env, timeout=15))
        if config.get("ready") is not True or config.get("operator_configured") is not True:
            raise RuntimeError("The extracted queue failed database and identity preflight.")
        with queue_server(service, root, service_env, host=queue_host) as origin:
            yield origin, ca, tokens, env


def smoke_handoff(launcher, root, origin, ca, tokens, env, repository):
    git_env = env | {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
    source = root / "trusted source"
    worker = root / "worker checkout"
    greeting = Path("src/greeting.txt")

    def git(checkout, *args, input=None):
        return subprocess.check_output(["git", "-C", checkout, *args], env=git_env, input=input, timeout=15)

    def commit(checkout, note):
        git(checkout, "add", ".")
        git(checkout, "-c", "user.name=Craft fixture", "-c", "user.email=craft-fixture@example.invalid",
            "-c", "commit.gpgSign=false", "commit", "--quiet", "-m", note)
        return git(checkout, "rev-parse", "HEAD").decode("ascii").strip()

    source.mkdir()
    git(source, "init", "--quiet", "--template=")
    git(source, "config", "core.autocrlf", "false")
    (source / greeting.parent).mkdir()
    (source / greeting).write_bytes(b"original greeting\n")
    (source / "skill").mkdir()
    (source / "skill/SKILL.md").write_bytes(b"# Fixture skill\nCheck exactly one changed greeting.\n")
    base = commit(source, "Fixture base")
    git(root, "clone", "--no-local", "--quiet", source, worker)
    (worker / greeting).write_bytes(b"verified greeting\n")
    candidate = commit(worker, "Fixture candidate")
    pack = git(worker, "pack-objects", "--stdout", "--revs", input=f"{candidate}\n^{base}\n".encode("ascii"))
    work_id = "installed-kit-smoke"
    order_file = root / "approved order.json"
    order_file.write_bytes(json.dumps({
        "schema_version": 1, "id": work_id, "issue": "FIXTURE-1", "owner": "Fixture owner",
        "repository": str(source), "base": base, "intent": "Update the fixture greeting.",
        "acceptance": ["Only src/greeting.txt contains the new greeting."], "scope": ["src"],
        "skill": {"location": "repo:skill/SKILL.md", "revision": base},
        "context": [{"location": "fixture:approved-greeting", "revision": base}],
    }).encode("utf-8"))
    mapping = root / "verifier map.json"
    mapping.write_text(json.dumps({repository: {"path": str(source)}}), encoding="utf-8")

    def call(command, *args, role="operator"):
        credentials = env | ({f"CRAFT_FACTORY_{role.upper()}_TOKEN": tokens[role]} if role else {})
        return subprocess.run([launcher, command, "--queue", origin, "--ca", ca, *args],
                              cwd=root, env=credentials, timeout=15, capture_output=True)

    def cli(command, *args, role="operator"):
        result = call(command, *args, role=role)
        if result.returncode:
            raise RuntimeError(result.stderr.decode(errors="replace"))
        return json.loads(result.stdout)

    address = urlsplit(origin)
    trust = ssl.create_default_context(cafile=str(ca))

    def runner(action, **binding):
        connection = HTTPSConnection(address.hostname, address.port, context=trust, timeout=10)
        try:
            connection.request("POST", "/v1/runner", json.dumps({"action": action, **binding}),
                               {"Authorization": "Bearer " + tokens["runner"], "Content-Type": "application/json"})
            response = connection.getresponse()
            if response.status != 200:
                raise RuntimeError(f"Installed runner handoff failed at {action} with HTTP {response.status}.")
            return json.loads(response.read())
        finally:
            connection.close()

    cli("shared-ops", "set-runner-enrollment", "smoke-runner", repository, "--expected-revision", "0",
        "--decision", "grant", "--evidence", "fixture:installed runner enrollment")
    cli("shared-work", "resume")
    admitted = cli("shared-work", "admit", order_file, "--repository", repository,
                   "--order-digest", "sha256:" + sha256(order_file.read_bytes()).hexdigest(),
                   "--event-key", "fixture:installed-kit-smoke", "--token-limit", "25")
    lease = runner("claim", request_id=str(uuid4()), available_repositories=[repository])["claim"]
    if not lease or admitted["work_order_id"] != work_id or lease["work_id"] != work_id or lease["generation"] != 1:
        raise RuntimeError("The installed runner did not claim the approved work order.")
    binding = {"work_id": work_id, "generation": lease["generation"]}
    runner("start_attempt", **binding)
    upload = runner("submit_pack", **binding, commit=candidate, pack_base64=base64.b64encode(pack).decode("ascii"), tokens=5)
    inbox = cli("shared-ops", "pending-verifications", repository, role="verifier")["verification_inbox"]
    [pending] = inbox["items"]
    if (pending["work_id"] != work_id or pending["generation"] != binding["generation"]
            or pending["head"] != candidate or pending["pack_digest"] != upload["candidate_evidence"]):
        raise RuntimeError("The installed verifier inbox did not identify the submitted candidate.")
    verifier_token = root / "verifier token"
    with os.fdopen(os.open(verifier_token, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as file:
        file.write(tokens["verifier"] + "\n")
    verification = root / "independent verification"
    handoff = cli("prepare-next-verification", "--repository", repository, "--repository-map", mapping,
                  "--destination", verification, "--verifier-token-file", verifier_token, role=None)
    if (not handoff.get("pickup_id") or not handoff.get("pickup_expires_at")
            or cli("shared-ops", "pending-verifications", repository, role="verifier")["verification_inbox"]["items"]):
        raise RuntimeError("The installed verifier did not reserve its pending candidate.")
    renewal = cli("shared-ops", "renew-verification-pickup", repository, handoff["pickup_id"],
                  "--verifier-token-file", verifier_token, role=None)["verification_pickup"]
    if (renewal["pickup_id"] != handoff["pickup_id"]
            or datetime.fromisoformat(renewal["pickup_expires_at"]) <= datetime.fromisoformat(handoff["pickup_expires_at"])):
        raise RuntimeError("The installed verifier did not extend the same pickup.")
    brief = call("shared-ops", "operator-brief", "--repository", repository)
    if (brief.returncode or b"Verification in progress" not in brief.stdout
            or b"Verifier smoke-verifier" not in brief.stdout):
        raise RuntimeError("The installed operator brief did not show the verifier pickup.")
    detached = Path(handoff["checkout"])
    if (handoff["work_id"] != work_id or handoff["generation"] != binding["generation"]
            or handoff["head"] != candidate or handoff["pack_digest"] != upload["candidate_evidence"]
            or handoff["pack_digest"] != "sha256:" + sha256(pack).hexdigest()
            or git(detached, "rev-parse", "HEAD").decode("ascii").strip() != candidate
            or git(detached, "diff", "--name-only", base, candidate).decode("ascii").strip() != greeting.as_posix()
            or (detached / greeting).read_bytes() != b"verified greeting\n"
            or (source / greeting).read_bytes() != b"original greeting\n"):
        raise RuntimeError("The independent checkout did not contain the exact approved change.")
    attestation = ("attest-shared-verification", "--handoff", verification / "handoff.json", "--result", "passed",
                   "--evidence", "fixture:installed verification", "--note", "Checked the exact detached candidate.",
                   "--verifier-token-file", verifier_token)

    def reject_checkout(expected):
        rejected = call(*attestation, role=None)
        if not rejected.returncode or expected not in rejected.stderr:
            detail = rejected.stderr.decode(errors="replace").strip()
            raise RuntimeError(f"The installed verifier did not reject the changed checkout as expected: {detail}")

    (detached / greeting).write_bytes(b"unverified edit\n")
    reject_checkout(b"verification checkout has tracked changes")
    git(detached, "add", "--", greeting)
    reject_checkout(b"verification checkout has tracked changes")
    git(detached, "restore", "--source=HEAD", "--staged", "--worktree", "--", greeting)
    git(detached, "checkout", "--quiet", "--detach", base)
    reject_checkout(b"verification checkout HEAD differs")
    if cli("shared-work", "status", work_id)["work"]["verification"] is not None:
        raise RuntimeError("The queue recorded a verdict from a changed checkout.")
    git(detached, "checkout", "--quiet", "--detach", candidate)
    (detached / "fixture-build-output.txt").write_bytes(b"disposable test output\n")
    recorded = cli(*attestation, role=None)
    verdict = cli("shared-work", "status", work_id)["work"]["verification"]
    if not verdict["passed"] or verdict["verifier"] != "smoke-verifier":
        raise RuntimeError("The installed queue did not record the independent verdict.")
    brief = call("shared-ops", "operator-brief", "--repository", repository)
    if brief.returncode or b"Verification in progress" in brief.stdout:
        raise RuntimeError("The installed operator brief still shows a completed verifier pickup.")
    explicit = cli("prepare-shared-verification", "--verifier", "--work-id", work_id, "--head", candidate,
                   "--repository-map", mapping, "--destination", root / "explicit verification",
                   "--verifier-token-file", verifier_token, role=None)
    if explicit.get("pickup_id") or explicit["verification_sequence"] != recorded["verification_sequence"]:
        raise RuntimeError("Explicit verification did not reuse the protected credential to read the current verdict.")
    if cli("shared-ops", "pending-verifications", repository, role="verifier")["verification_inbox"]["items"]:
        raise RuntimeError("The installed verifier inbox still lists a verified candidate.")
    empty_destination = root / "no pending verification"
    idle = cli("prepare-next-verification", "--repository", repository, "--repository-map", mapping,
               "--destination", empty_destination, role="verifier")
    if idle["status"] != "idle" or empty_destination.exists():
        raise RuntimeError("The installed verifier did not leave the empty inbox idle.")
    cli("shared-work", "cancel", work_id, "--reason", "fixture finished")
    cli("shared-work", "pause")


def smoke_confluence(launcher, root, origin, ca, tokens, env, repository, fixture):
    state = root / "craft state"
    state.mkdir()
    policy = root / "confluence policy.json"
    location = "project:routing"
    config = {
        "schema_version": 1, "craft": str(fixture), "craft_state": str(state),
        "craft_profile": "fixture-confluence", "cloud_id": "11111111-2222-3333-4444-555555555555",
        "space_id": "200", "page_id": "101", "repository": repository, "location": location, "kind": "decision",
    }
    policy.write_text(json.dumps(config), encoding="utf-8")
    excerpt = root / "approved excerpt"
    excerpt.write_text("Use the approved fixture route.", encoding="utf-8")
    approval = root / "owner approval.json"
    preview = json.loads(subprocess.check_output(
        [launcher, "capture-confluence-context", policy, "--excerpt", excerpt], cwd=root, env=env, timeout=15))
    approval.write_text(json.dumps({"schema_version": 2, "approval_digest": preview["approval_digest"],
                                    "approval_reference": "fixture:owner approved routing"}), encoding="utf-8")
    ops = [launcher, "shared-ops", "--queue", origin, "--ca", ca]
    operator = env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
    registered = json.loads(subprocess.check_output(
        [*ops, "register-confluence-context", policy, "--excerpt", excerpt, "--approval", approval],
        cwd=root, env=operator, timeout=15))["release"]
    token_file = root / "context reader token"
    with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="ascii") as file:
        file.write(tokens["context_reader"] + "\n")

    report = root / "last-report.json"
    command = [*ops, "check-confluence-context", policy, "--context-reader", "--context-reader-token-file",
               token_file, "--require-current", "--report-file"]
    current = json.loads(subprocess.check_output([*command, report.name], cwd=root, env=env, timeout=15))
    if (current["status"] != "current" or current["registered"]["revision"] != registered["revision"]
            or "excerpt" in current["registered"] or json.loads(report.read_bytes()) != current):
        raise RuntimeError("The installed context reader failed to save the current approved pin.")

    for protected in (token_file, ca, policy, state / report.name):
        before = protected.read_bytes() if protected.exists() else None
        rejected = subprocess.run([*command, protected], cwd=root, env=env, capture_output=True, timeout=15)
        if (not rejected.returncode or b"context check report" not in rejected.stderr
                or (protected.read_bytes() if protected.exists() else None) != before):
            raise RuntimeError("The installed context check did not protect its input paths.")

    drift = subprocess.run([*command, report.name], cwd=root,
                           env=env | {"FACTORY_FIXTURE_CONFLUENCE_CHANGED": "1"}, capture_output=True, timeout=15)
    saved = report.read_bytes()
    drift_report = json.loads(saved)
    if (not drift.returncode or not drift.stdout or drift_report["status"] != "source_changed"
            or json.loads(drift.stdout) != drift_report):
        raise RuntimeError("The installed context check did not retain the detected source drift.")

    broken = subprocess.run([*command, report.name], cwd=root,
                             env=env | {"FACTORY_FIXTURE_CONFLUENCE_WRONG_SPACE": "1"}, capture_output=True, timeout=15)
    if not broken.returncode or broken.stdout or report.read_bytes() != saved:
        raise RuntimeError("The installed context check replaced the report after an invalid source response.")

    policy.write_text(json.dumps(config | {"repository": "fixture/other"}), encoding="utf-8")
    forbidden = subprocess.run([*command, report.name], cwd=root, env=env, capture_output=True, timeout=15)
    if (not forbidden.returncode or b"HTTP 403" not in forbidden.stderr
            or forbidden.stdout or report.read_bytes() != saved):
        raise RuntimeError("The installed context reader did not enforce its repository scope.")
    withdrawal = subprocess.run(
        [*ops, "withdraw-project-context", repository, location, registered["revision"], "--evidence", "fixture:denied"],
        cwd=root, env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["context_reader"]},
        capture_output=True, timeout=15)
    if not withdrawal.returncode or b"HTTP 403" not in withdrawal.stderr or withdrawal.stdout:
        raise RuntimeError("The installed queue gave the context reader operator write access.")


def smoke(kit, root, postgres_bin, confluence_fixture=None):
    repository = "fixture/service"
    identities = {"operator": {}, "runner": {}, "verifier": {"verifier_repositories": [repository]}}
    if confluence_fixture is not None:
        identities["context_reader"] = {"context_repositories": [repository]}
    with disposable_queue(kit, root, postgres_bin, identities) as (origin, ca, tokens, env):
        launcher = kit / ("craft-factory.exe" if os.name == "nt" else "craft-factory")
        cli = [launcher, "shared-ops", "--queue", origin, "--ca", ca]
        operator = env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}
        summary = json.loads(subprocess.check_output(cli + ["summary"], cwd=root, env=operator, timeout=10))
        if summary["summary"].get("paused") is not True:
            raise RuntimeError("The fresh queue did not start paused.")
        forbidden = subprocess.run(cli + ["summary"], cwd=root, capture_output=True, timeout=10,
                                   env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["runner"]})
        if forbidden.returncode == 0 or b"HTTP 403" not in forbidden.stderr or forbidden.stdout:
            raise RuntimeError("The queue did not deny operator access to the runner credential.")
        smoke_handoff(launcher, root, origin, ca, tokens, env, repository)
        if confluence_fixture is not None:
            smoke_confluence(launcher, root, origin, ca, tokens, env, repository, confluence_fixture)
    result = {"schema_version": 1, "passed": True, "migrations": "installed",
              "preflight": "passed", "queue": "paused", "operator_tls": "verified", "runner_operator_access": "denied",
              "candidate_handoff": "independently verified"}
    if confluence_fixture is not None:
        result["confluence_drift_report"] = "verified"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--postgres-bin", type=Path, help="Directory containing the PostgreSQL binaries")
    parser.add_argument("--confluence-fixture", type=Path, help="Compiled disposable Craft connector fixture for CI")
    parser.add_argument("--scratch-parent", type=Path, help="Existing directory for the disposable fixture")
    args = parser.parse_args()
    if sys.platform not in {"linux", "win32"} or (sys.platform == "linux" and os.geteuid() == 0):
        parser.error("run the smoke test on Windows or as a non-root Linux user")
    postgres_bin = args.postgres_bin.resolve(strict=True) if args.postgres_bin else None
    confluence_fixture = args.confluence_fixture.resolve(strict=True) if args.confluence_fixture else None
    parent = args.scratch_parent.resolve(strict=True) if args.scratch_parent else (
        "/tmp" if sys.platform == "linux" else None)
    with TemporaryDirectory(prefix="craft-smoke-", dir=parent) as scratch:
        print(json.dumps(smoke(Path(__file__).resolve().parents[1], Path(scratch), postgres_bin, confluence_fixture)))


if __name__ == "__main__":
    main()
