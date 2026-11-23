"""Qualify native runner takeover across isolated Linux network namespaces."""

import argparse
from base64 import b64encode
from contextlib import ExitStack, contextmanager, suppress
from hashlib import file_digest, sha256
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Event

from qualify_native_coordinator import native_codex_fixture
from qualify_native_worker import git, prepare_order
from qualify_native_wsl_handoff import (
    CRASH_LEASE_SECONDS, REPOSITORY, TARGET, checked, clean_environment, native_attempt, protected_file,
    runner_command, wait_until,
)
from smoke_shared_queue import disposable_queue


QUEUE_IP = "198.18.20.1"
ISOLATE = ["unshare", "--user", "--map-current-user", "--keep-caps", "--net", "--"]
DROP_CAPS = ["setpriv", "--bounding-set=-all", "--inh-caps=-all", "--ambient-caps=-all", "--"]


def ip(*args, prefix=()):
    subprocess.run([*prefix, "ip", *args], check=True, stdout=subprocess.DEVNULL, timeout=10)


def net_id(pid="self"):
    return os.readlink(f"/proc/{pid}/ns/net")


@contextmanager
def namespace_clients():
    # Check the real namespace before touching interfaces. Only the newly created namespace may be configured.
    links = json.loads(subprocess.check_output(["ip", "-j", "link", "show"], timeout=10))
    if [link["ifname"] for link in links] != ["lo"]:
        raise RuntimeError("The partition fixture requires a fresh network namespace with only loopback.")
    ip("link", "set", "lo", "up")
    ip("address", "add", f"{QUEUE_IP}/32", "dev", "lo")
    with ExitStack() as processes:
        clients = []
        for number in (1, 2):
            anchor = subprocess.Popen(["unshare", "--net", "--", "sleep", "300"])

            def stop(process=anchor):
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=5)

            processes.callback(stop)
            wait_until(lambda: anchor.poll() is None and net_id(anchor.pid) != net_id(),
                       5, "The worker's network namespace did not start.")
            name, peer = f"factory{number}", f"worker{number}"
            inside = ["nsenter", f"--net=/proc/{anchor.pid}/ns/net", "--"]
            ip("link", "add", name, "type", "veth", "peer", "name", peer)
            ip("link", "set", peer, "netns", str(anchor.pid))
            ip("address", "add", f"198.18.{number}.1/30", "dev", name)
            ip("link", "set", name, "up")
            ip("link", "set", "lo", "up", prefix=inside)
            ip("address", "add", f"198.18.{number}.2/30", "dev", peer, prefix=inside)
            ip("link", "set", peer, "up", prefix=inside)
            ip("route", "add", f"{QUEUE_IP}/32", "via", f"198.18.{number}.1", prefix=inside)
            clients.append((inside, name, net_id(anchor.pid)))
        assert len({net_id(), *(client[2] for client in clients)}) == 3
        yield clients


def native_client(args):
    status = Path("/proc/self/status").read_text(encoding="utf-8").splitlines()
    if int(next(line.split()[1] for line in status if line.startswith("CapEff:")), 16):
        raise RuntimeError("Run the native worker without the network setup capabilities.")
    if args.mode == "replace":
        return checked(native_attempt(args.launcher, args.codex, args.origin, args.ca, args.directory,
                                      args.mapping, args.order, args.token, TARGET))
    model = args.directory / "model"
    model.mkdir()
    disconnected = Event()

    def hold_connection(connection):
        (args.directory / "model-started").touch(exist_ok=False)
        connection.settimeout(30)
        try:
            if not connection.recv(1):
                disconnected.set()
        except ConnectionResetError:
            disconnected.set()
        except TimeoutError:
            pass

    with native_codex_fixture(args.codex, model, None, hold_connection=hold_connection) as (shim, home, requests):
        command = runner_command(args.launcher, args.origin, args.ca, args.mapping,
                                 args.directory / "attempt", args.token)
        result = subprocess.run([*command, "--codex", shim, "--codex-home", home, "--renew-seconds", "1",
                                 "--timeout-seconds", "60"], env=clean_environment(),
                                capture_output=True, text=True, timeout=65)
        assert result.returncode and "queue" in result.stderr.lower(), result.stderr
        assert disconnected.wait(2) and len(requests) == 1 and not (home / "auth.json").exists()
        return {"launcher_exit": result.returncode, "model_connection": "closed"}


def qualify(args, root, clients):
    launcher = args.kit / "craft-factory"
    order = prepare_order(root)
    order["execution_profile"]["max_attempts"] = 2
    work_id = order["id"]
    source = Path(order["repository"])
    order_file = root / "order.json"
    order_bytes = json.dumps(order).encode("utf-8")
    order_file.write_bytes(order_bytes)
    mapper = {}
    for name in ("original", "replacement", "verifier"):
        directory = root / name
        directory.mkdir()
        checkout = directory / "source"
        git(root, "clone", "--quiet", "--no-local", source, checkout)
        mapping = directory / "map.json"
        mapping.write_text(json.dumps({REPOSITORY: {"path": str(checkout)}}), encoding="utf-8")
        mapper[name] = (directory, checkout, mapping)
    database = root / "db"
    database.mkdir()
    roles = {"operator": {}, "runner": {}, "replacement": {"role": "runner"},
             "verifier": {"verifier_repositories": [REPOSITORY]}}
    (original, first_link, first_ns), (replacement, _, second_ns) = clients
    with disposable_queue(args.kit, database, args.postgres_bin, roles, lease_seconds=CRASH_LEASE_SECONDS,
                          queue_host=QUEUE_IP) as (origin, ca, tokens, env):
        tokens_path = {}
        for name, role in (("original", "runner"), ("replacement", "replacement"), ("verifier", "verifier")):
            tokens_path[name] = mapper[name][0] / "queue-token"
            protected_file(tokens_path[name], tokens[role])

        def operator(command, *flags):
            return json.loads(subprocess.check_output(
                [launcher, command, "--queue", origin, "--ca", ca, *flags],
                env=env | {"CRAFT_FACTORY_OPERATOR_TOKEN": tokens["operator"]}, timeout=15))

        def client_command(network, mode, name):
            directory, _, mapping = mapper[name]
            return [*network, *DROP_CAPS, sys.executable, "-B", Path(__file__).resolve(), "worker", mode,
                    launcher, args.codex, origin, ca, directory, mapping, order_file, tokens_path[name]]

        for runner in ("smoke-runner", "smoke-replacement"):
            operator("shared-ops", "set-runner-enrollment", runner, REPOSITORY, "--expected-revision", "0",
                     "--decision", "grant", "--evidence", "fixture:partition enrollment")
        operator("shared-work", "resume")
        operator("shared-work", "admit", order_file, "--repository", REPOSITORY,
                 "--order-digest", "sha256:" + sha256(order_bytes).hexdigest(),
                 "--event-key", "fixture:network partition", "--token-limit", "400")

        def live_lease():
            return operator("shared-ops", "dispatch-status", work_id)["dispatch"]["live_lease"]

        initial_root = mapper["original"][0]
        initial_env = clean_environment()
        with subprocess.Popen(client_command(original, "hold", "original"), env=initial_env, start_new_session=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as first:
            try:
                wait_until(lambda: first.poll() is not None or (initial_root / "model-started").is_file(),
                           25, "The original runner did not reach its model.")
                if first.poll() is not None:
                    raise RuntimeError(first.communicate()[1])
                assert live_lease()
                ip("link", "set", first_link, "down")
                second_root, _, second_map = mapper["replacement"]
                home = second_root / "unclaimed-home"
                home.mkdir()
                probe = runner_command(launcher, origin, ca, second_map,
                                       second_root / "premature", tokens_path["replacement"])
                idle = checked(subprocess.run(
                    [*replacement, *DROP_CAPS, *probe, "--codex", args.codex, "--codex-home", home], env=initial_env,
                    capture_output=True, text=True, timeout=15))
                assert idle == {"schema_version": 1, "claim": None}
                assert not (second_root / "premature").exists()
                stdout, stderr = first.communicate(timeout=18)
                assert first.returncode == 0, stderr
                stopped = json.loads(stdout)
                assert stopped["model_connection"] == "closed"
                wait_until(lambda: not live_lease(), CRASH_LEASE_SECONDS + 5,
                           "The partitioned runner's lease did not expire.")
            finally:
                if first.poll() is None:
                    with suppress(ProcessLookupError):
                        os.killpg(first.pid, signal.SIGKILL)

        assert not (initial_root / "attempt" / "submission.json").exists()
        candidate = checked(subprocess.run(client_command(replacement, "replace", "replacement"), env=initial_env,
                                           capture_output=True, text=True, timeout=125))
        head = candidate["candidate"]
        assert candidate["generation"] == 2 and candidate["work_id"] == work_id
        pack = (second_root / "attempt" / "work" / "candidate.pack").read_bytes()
        assert candidate["candidate_evidence"] == "sha256:" + sha256(pack).hexdigest()

        # Reconnect the original interface before attempting a stale submission over actual TLS.
        ip("link", "set", first_link, "up")
        authorization = initial_root / "late-header"
        protected_file(authorization, "Authorization: Bearer " + tokens["runner"])
        late = {"action": "submit_pack", "work_id": work_id, "generation": 1, "commit": head,
                "pack_base64": b64encode(pack).decode("ascii"), "tokens": 300}
        status = subprocess.check_output(
            [*original, *DROP_CAPS, "curl", "--disable", "--noproxy", "*", "--silent", "--show-error", "--cacert", ca,
             "--header", f"@{authorization}", "--header", "Content-Type: application/json",
             "--data-binary", "@-", "--output", os.devnull, "--write-out", "%{http_code}", origin + "/v1/runner"],
            input=json.dumps(late), text=True, timeout=15)
        assert status == "409", status

        def verifier(command, *flags):
            return json.loads(subprocess.check_output(
                [*replacement, *DROP_CAPS, launcher, command, "--queue", origin, "--ca", ca,
                 "--verifier-token-file", tokens_path["verifier"], *flags], env=initial_env, timeout=25))

        verification = mapper["verifier"][0] / "verification"
        handoff = verifier("prepare-next-verification", "--repository", REPOSITORY, "--repository-map",
                           mapper["verifier"][2], "--destination", verification)
        assert handoff["head"] == head and handoff["pack_digest"] == candidate["candidate_evidence"]
        assert git(verification / "checkout", "diff", "--name-only", order["base"], head) == TARGET
        assert (verification / "checkout" / TARGET).read_text().strip() == "changed greeting"
        verdict = verifier("attest-shared-verification", "--handoff", verification / "handoff.json",
                           "--result", "passed", "--evidence", "fixture:network partition",
                           "--note", "Verified the exact candidate transferred across the isolated link.")
        audit = operator("shared-ops", "audit", work_id)["audit"]
        assert audit["work"]["verification"]["passed"]
        assert audit["work"]["verification"]["sequence"] == verdict["verification_sequence"]
        attempts = sorted(audit["attempts"], key=lambda item: item["generation"])
        assert [(a["generation"], a["outcome"], a["reported_tokens"], a["worker_stop_reported_at"]) for a in attempts] == [
            (1, "expired", None, None), (2, "candidate", 300, None)]
    digests = {}
    for name, path in (("launcher", launcher), ("queue", args.kit / "shared" / "craft-factory-queue"),
                       ("codex", args.codex)):
        with path.open("rb") as stream:
            digests[name] = file_digest(stream, "sha256").hexdigest()
    return {"schema_version": 1, "mode": "single_machine_isolated_linux_networks", "model": "scripted_loopback",
            "binary_sha256": digests, "network_namespaces": {"queue": net_id(), "original": first_ns,
                                                            "replacement": second_ns},
            "lease_seconds": CRASH_LEASE_SECONDS, "live_lease_takeover": "denied",
            "partitioned_runner": stopped, "stale_submission_http_status_after_reconnect": int(status),
            "replacement_commit": head, "candidate_pack_sha256": handoff["pack_digest"],
            "verification_sequence": verdict["verification_sequence"],
            "attempts": [{key: a[key] for key in ("generation", "outcome", "reported_tokens", "worker_stop_reported_at")}
                         for a in attempts]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    rehearsal = commands.add_parser("qualify")
    for name in ("kit", "codex", "postgres-bin"):
        rehearsal.add_argument("--" + name, type=Path, required=True)
    rehearsal.add_argument("--parent-net-fd", type=int, help=argparse.SUPPRESS)
    worker = commands.add_parser("worker")
    worker.add_argument("mode", choices=("hold", "replace"))
    for name in ("launcher", "codex", "origin", "ca", "directory", "mapping", "order", "token"):
        worker.add_argument(name, type=str if name == "origin" else Path)
    args = parser.parse_args()
    if not sys.platform.startswith("linux") or os.geteuid() == 0:
        parser.error("run as a non-root Linux user with unprivileged user namespaces")
    if args.command == "worker":
        print(json.dumps(native_client(args)))
    elif args.parent_net_fd is None:
        with open("/proc/self/ns/net", "rb") as previous:
            fd = previous.fileno()
            result = subprocess.run([*ISOLATE, sys.executable, "-B", Path(__file__).resolve(), *sys.argv[1:],
                                     "--parent-net-fd", str(fd)], pass_fds=(fd,))
        sys.exit(result.returncode)
    else:
        fd = args.parent_net_fd
        parent = os.readlink(f"/proc/self/fd/{fd}")
        if not parent.startswith("net:[") or parent == net_id():
            parser.error("could not enter a new network namespace")
        os.close(fd)
        for name in ("kit", "codex", "postgres_bin"):
            setattr(args, name, getattr(args, name).resolve(strict=True))
        with TemporaryDirectory(prefix="craft-partition-", dir="/tmp") as scratch, namespace_clients() as clients:
            print(json.dumps(qualify(args, Path(scratch), clients), indent=2))


if __name__ == "__main__":
    main()
