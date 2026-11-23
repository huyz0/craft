"""Build and check a Linux shared-service kit using release or CI binaries."""

import argparse
import json
import os
from hashlib import file_digest
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory


TOOLS = Path(__file__).resolve().parent
SOURCE = TOOLS.parent


def stage_kit(stage, launcher, queue):
    for source, destination in ((launcher, "craft-factory"), (queue, "shared/craft-factory-queue")):
        binary = stage / destination
        binary.parent.mkdir(exist_ok=True)
        shutil.copyfile(source, binary)
        binary.chmod(0o755)

    shutil.copytree(SOURCE / "skills", stage / "skills")
    shutil.copytree(SOURCE / "deploy/systemd", stage / "deploy/systemd")
    shutil.copytree(SOURCE / "deploy/prometheus", stage / "deploy/prometheus")
    shutil.copyfile(SOURCE / "pilot-handoff.md", stage / "pilot-handoff.md")
    shutil.copyfile(SOURCE / "QUICKSTART-LINUX.md", stage / "QUICKSTART.md")
    guide = (SOURCE / "README.md").read_text(encoding="utf-8")
    before, heading, instructions = guide.partition("## Try the supervised workflow\n")
    if not heading:
        raise SystemExit("The Factory workflow section is missing from the guide.")
    introduction = before.split("## Verify and install\n", 1)[0]
    instructions = "".join(line for line in instructions.splitlines(keepends=True)
                           if not line.startswith("See the [factory delivery plan](../"))
    (stage / "README.md").write_text(
        introduction + "## Verify and install\n\nFollow the [Linux kit quickstart](QUICKSTART.md).\n\n"
        + heading + instructions, encoding="utf-8",
    )
    for name in ("README.md", "migrate_queue.py", "service-functions.txt"):
        shutil.copyfile(SOURCE / "shared" / name, stage / "shared" / name)
    shutil.copytree(SOURCE / "shared/migrations", stage / "shared/migrations")

    (stage / "examples").mkdir()
    (stage / "schemas").mkdir()
    (stage / "tools").mkdir()
    for name in ("summarize_scorecard.py", "smoke_shared_queue.py", "disposable_tls.py"):
        shutil.copyfile(TOOLS / name, stage / "tools" / name)
    shutil.copyfile(SOURCE / "examples/pilot-scorecard.csv", stage / "examples/pilot-scorecard.csv")
    for kind in (TOOLS / "kit-schemas.txt").read_text(encoding="utf-8").splitlines():
        schema = subprocess.check_output([launcher, "schema", kind])
        (stage / "schemas" / f"{kind}.schema.json").write_bytes(schema)
        example = SOURCE / "examples" / f"{kind}.json"
        if example.is_file():
            shutil.copyfile(example, stage / "examples" / example.name)


def qualify(archive, extracted, smoke, env=None):
    with tarfile.open(archive, "r:gz") as package:
        package.extractall(extracted, filter="data")
    subprocess.run([sys.executable, "-B", TOOLS / "check_ci_scope.py", "validate-docs", "--kit", extracted],
                   env=env, check=True)
    subprocess.run([sys.executable, "-B", extracted / "tools/summarize_scorecard.py",
                    extracted / "examples/pilot-scorecard.csv"], env=env, check=True, stdout=subprocess.DEVNULL)
    subprocess.run([extracted / "shared/craft-factory-queue", "--help"], env=env, check=True, stdout=subprocess.DEVNULL)
    result = json.loads(subprocess.check_output([extracted / "craft-factory", "smoke", "--destination", smoke], env=env))
    if result.get("passed") is not True or result.get("state") != "closed":
        raise SystemExit("The extracted Linux launcher failed its offline smoke test.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--launcher", required=True, type=Path)
    parser.add_argument("--queue-service", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if not output.name.endswith(".tar.gz") or output.exists():
        parser.error("--output must be a new .tar.gz file")
    with TemporaryDirectory(prefix="craft-package-", dir=output.parent) as temporary:
        root = Path(temporary)
        stage = root / "stage"
        stage.mkdir()
        stage_kit(stage, args.launcher.resolve(strict=True), args.queue_service.resolve(strict=True))
        archive = root / "kit.tar.gz"
        with tarfile.open(archive, "w:gz", dereference=True) as package:
            for path in sorted(stage.iterdir()):
                package.add(path, arcname=path.name)
        qualify(archive, root / "fresh install", root / "smoke")
        os.link(archive, output)
    with output.open("rb") as stream:
        print(f"{file_digest(stream, 'sha256').hexdigest()}  {output}")


if __name__ == "__main__":
    main()
