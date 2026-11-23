"""Run the packaged Windows queue HTTPS suite in a disposable PostgreSQL cluster."""

import argparse
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

from disposable_tls import find_openssl
from smoke_shared_queue import private_postgres


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kit", type=Path, help="Extracted Windows shared-service kit")
    parser.add_argument("--postgres-bin", required=True, type=Path, help="PostgreSQL tools directory")
    parser.add_argument("--scratch-parent", type=Path, help="Existing directory for the disposable fixture")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("run qualification on Windows")
    kit = args.kit.resolve(strict=True)
    postgres_bin = args.postgres_bin.resolve(strict=True)
    tools = {name: postgres_bin / f"{name}.exe" for name in
             ("initdb", "pg_ctl", "createdb", "psql", "pg_dump", "pg_restore")}
    missing = [name for name, path in tools.items() if not path.is_file()]
    if missing:
        parser.error("required PostgreSQL tools are missing: " + ", ".join(missing))
    launcher = kit / "craft-factory.exe"
    shared = kit / "shared"
    server = shared / "craft-factory-queue.exe"
    if not launcher.is_file() or not server.is_file():
        parser.error("the extracted kit must include the Windows launcher and queue")
    suite = Path(__file__).resolve().parents[1] / "shared/test_service.py"
    env = {name: value for name, value in os.environ.items() if not name.upper().startswith(("PG", "CRAFT_"))}
    env["PATH"] = os.pathsep.join((str(postgres_bin), str(Path(find_openssl(env)).parent), env["PATH"]))
    env["PYTHONUNBUFFERED"] = "1"
    parent = args.scratch_parent.resolve(strict=True) if args.scratch_parent else None
    with TemporaryDirectory(prefix="craft-windows-queue-", dir=parent) as scratch:
        root = Path(scratch)
        with private_postgres(root, env, tools) as pg:
            database = "craft_factory_https_fixture"
            subprocess.run([tools["createdb"], "--template=template0", database],
                           cwd=root, env=pg, check=True, timeout=20)
            url = f"postgres://craft_test_service:disposable-fixture@127.0.0.1:{pg['PGPORT']}/{database}?sslmode=disable"
            subprocess.run([sys.executable, "-B", suite, server, "--launcher", launcher, "--install-assets", shared],
                           cwd=root, env=pg | {"PGDATABASE": database, "CRAFT_TEST_DATABASE_URL": url},
                           check=True, timeout=480)


if __name__ == "__main__":
    main()
