"""Qualify an extracted Linux kit with a private, disposable PostgreSQL queue."""

import argparse
from hashlib import file_digest
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import tomllib
from urllib.parse import urlencode

from package_linux import SOURCE, qualify
from qualify_launcher import qualify_launcher


def require_tools(env):
    for name in ("initdb", "pg_ctl", "createdb", "pg_dump", "pg_restore", "psql", "git", "openssl",
                 "rustc", "cargo", "bwrap", "systemd-analyze", "prometheus", "promtool"):
        if shutil.which(name, path=env["PATH"]) is None:
            raise SystemExit(f"Required qualification tool is missing: {name}")


def local_environment(root, postgres_bin):
    home = root / "home"
    home.mkdir()
    env = {name: os.environ[name] for name in ("PATH", "LD_LIBRARY_PATH", "LANG", "TZ", "CARGO_TARGET_DIR")
           if name in os.environ}
    env.update(HOME=str(home), TMPDIR=str(root), LC_ALL="C", GIT_CONFIG_NOSYSTEM="1",
               CARGO_HOME=os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
               RUSTUP_HOME=os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
               RUSTUP_TOOLCHAIN=tomllib.loads((SOURCE / "rust-toolchain.toml").read_text())["toolchain"]["channel"])
    if postgres_bin is not None:
        env["PATH"] = os.pathsep.join((str(postgres_bin), env.get("PATH", os.defpath)))
    else:
        env.setdefault("PATH", os.defpath)
    return env


def check_deployment_assets(kit, root, env):
    units = root / "units"
    units.mkdir()
    expected_units = ("craft-factory-queue.service", "craft-factory-runner.service",
                      "craft-factory-jira-relay.service", "craft-factory-jira-relay.timer",
                      "craft-factory-ci-repair.service", "craft-factory-ci-repair.timer",
                      "craft-factory-shared-ci.service", "craft-factory-shared-ci.timer")
    expected_units += tuple(
        f"craft-factory-{forge}-merge-check@.{extension}"
        for forge in ("github", "bitbucket") for extension in ("service", "timer")
    )
    expected_units += ("craft-factory-github-enqueue@.service", "craft-factory-github-enqueue@.timer")
    expected_units += ("craft-factory-github-readiness@.service", "craft-factory-github-readiness@.timer")
    expected_units += ("craft-factory-context-check@.service", "craft-factory-context-check@.timer")
    expected_units += ("craft-factory-github-issue-relay@.service", "craft-factory-github-issue-relay@.timer")
    verified_units = []
    for name in expected_units:
        unit = kit / "deploy" / "systemd" / name
        installed = units / name.replace("@.", "@fixture.")
        installed.write_text(unit.read_text(encoding="utf-8").replace("/opt/craft-factory/", f"{kit}/"),
                             encoding="utf-8")
        verified_units.append(installed)
    subprocess.run(["systemd-analyze", "verify", "--man=no", *verified_units],
                   env=env, check=True, timeout=20)
    monitoring = kit / "deploy" / "prometheus"
    subprocess.run(["promtool", "check", "rules", "craft-factory.rules.yml"],
                   cwd=monitoring, env=env, check=True, timeout=20)
    subprocess.run(["promtool", "test", "rules", "craft-factory.rules.test.yml"],
                   cwd=monitoring, env=env, check=True, timeout=20)


def check_queue(kit, root, env):
    socket = root / "socket"
    socket.mkdir(mode=0o700)
    data = root / "postgres"
    log = root / "postgres.log"
    pg = env | {"PGHOST": str(socket), "PGPORT": "5432", "PGUSER": "craft_fixture_admin"}
    subprocess.run(["initdb", "--no-locale", "--encoding=UTF8", "--auth=trust", "--username", pg["PGUSER"],
                    "--pgdata", data], env=pg, check=True, timeout=30, stdout=subprocess.DEVNULL)
    command = ["pg_ctl", "--pgdata", data, "--timeout", "20"]
    try:
        subprocess.run(command + ["--log", log, "--options",
                       f"-c listen_addresses='' -c unix_socket_directories={socket} -c unix_socket_permissions=0700",
                       "--wait", "start"], env=pg, check=True)
        for database in ("craft_factory_sql_fixture", "craft_factory_https_fixture"):
            subprocess.run(["createdb", "--template=template0", database], env=pg, check=True, timeout=20)
        database_url = ("postgres://craft_test_service:disposable-fixture@localhost/craft_factory_https_fixture?"
                        + urlencode({"host": str(socket)}))
        subprocess.run([sys.executable, "-B", SOURCE / "shared/test_queue.py"], cwd=root,
                       env=pg | {"PGDATABASE": "craft_factory_sql_fixture"}, check=True)
        subprocess.run([sys.executable, "-B", SOURCE / "shared/test_service.py", kit / "shared/craft-factory-queue",
                        "--launcher", kit / "craft-factory", "--install-assets", kit / "shared"], cwd=root,
                       env=pg | {"PGDATABASE": "craft_factory_https_fixture", "CRAFT_TEST_DATABASE_URL": database_url,
                                 "FACTORY_REQUIRE_CLAUDE_FIXTURE": "1",
                                 "CRAFT_TEST_PROMETHEUS": shutil.which("prometheus", path=pg["PATH"])}, check=True)
    except subprocess.CalledProcessError:
        if log.is_file():
            print("\n".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-25:]), file=sys.stderr)
        raise
    finally:
        active = subprocess.run(command + ["status"], env=pg, capture_output=True, check=False)
        if active.returncode == 0:
            subprocess.run(command + ["--mode", "fast", "--wait", "stop"], env=pg, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="Linux shared-service kit .tar.gz")
    parser.add_argument("--postgres-bin", type=Path, help="Directory containing the PostgreSQL test binaries")
    args = parser.parse_args()
    if sys.platform != "linux" or os.geteuid() == 0:
        parser.error("run qualification as a non-root Linux user")
    archive = args.archive.resolve(strict=True)
    postgres_bin = args.postgres_bin.resolve(strict=True) if args.postgres_bin else None
    # A short private directory keeps PostgreSQL's socket path below the Unix limit.
    with TemporaryDirectory(prefix="craft-qualify-", dir="/tmp") as scratch:
        root = Path(scratch)
        kit = root / "kit"
        env = local_environment(root, postgres_bin)
        require_tools(env)
        snapshot = root / "shared-kit.tar.gz"
        shutil.copyfile(archive, snapshot)
        with snapshot.open("rb") as stream:
            digest = file_digest(stream, "sha256").hexdigest()
        qualify(snapshot, kit, root / "smoke", env=env)
        packaged_smoke = json.loads(subprocess.check_output(
            [sys.executable, "-B", kit / "tools/smoke_shared_queue.py"], cwd=root, env=env, timeout=180,
        ))
        if packaged_smoke.get("passed") is not True or packaged_smoke.get("candidate_handoff") != "independently verified":
            raise SystemExit("The packaged shared queue smoke test failed.")
        check_deployment_assets(kit, root, env)
        qualify_launcher(kit / "craft-factory", env=env)
        check_queue(kit, root, env)
    print(f"Qualified Linux shared-service kit sha256:{digest}")


if __name__ == "__main__":
    main()
