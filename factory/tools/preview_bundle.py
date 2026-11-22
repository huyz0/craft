"""Assemble and install a checksum-pinned local Factory preview."""

import argparse
from pathlib import Path
import re
import shutil
import sys
from tempfile import TemporaryDirectory
import zipfile

from release_kit_extract import PLATFORM_ARCHIVES, archive_record, install_archive


INSTALLER_MODULES = ("preview_bundle.py", "release_kit_extract.py")
ENTRY_POINT = """import sys
from preview_bundle import main

main(["install", sys.argv[0], *sys.argv[1:]])
"""


def sha256(value):
    if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
        raise argparse.ArgumentTypeError("provide a full SHA-256 digest")
    return value.lower()


def assemble(output, windows, windows_sha256, linux, linux_sha256):
    with TemporaryDirectory(prefix="craft-preview-") as temporary:
        root = Path(temporary)
        sources = {"windows": (windows, windows_sha256), "linux": (linux, linux_sha256)}
        for platform, (source, expected) in sources.items():
            snapshot = root / PLATFORM_ARCHIVES[platform]
            shutil.copyfile(source, snapshot)
            if archive_record(snapshot)["sha256"] != expected:
                raise ValueError(f"The {platform} archive does not match its expected SHA-256")

        stream = output.open("xb")
        try:
            with stream, zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_STORED) as bundle:
                for name in PLATFORM_ARCHIVES.values():
                    bundle.write(root / name, arcname=name)
                for name in INSTALLER_MODULES:
                    bundle.write(Path(__file__).with_name(name), arcname=name)
                bundle.writestr("__main__.py", ENTRY_POINT)
        except BaseException:
            output.unlink()
            raise
    return archive_record(output)["sha256"]


def install(bundle_path, expected_sha256, platform, destination):
    with TemporaryDirectory(prefix="craft-preview-") as temporary:
        root = Path(temporary)
        snapshot = root / "preview.zip"
        shutil.copyfile(bundle_path, snapshot)
        if archive_record(snapshot)["sha256"] != expected_sha256:
            raise ValueError("The preview bundle does not match its expected SHA-256")

        archive = root / PLATFORM_ARCHIVES[platform]
        with zipfile.ZipFile(snapshot) as bundle:
            names = sorted(bundle.namelist())
            bare = sorted(PLATFORM_ARCHIVES.values())
            standalone = sorted([*bare, *INSTALLER_MODULES, "__main__.py"])
            if names not in (bare, standalone):
                raise ValueError("A Factory preview must contain both platform archives and only the expected installer files")
            with bundle.open(archive.name) as source, archive.open("xb") as target:
                shutil.copyfileobj(source, target)
        install_archive(archive, platform, destination, archive_record(archive))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    writer = commands.add_parser("assemble")
    writer.add_argument("output", type=Path)
    for platform in PLATFORM_ARCHIVES:
        writer.add_argument(f"--{platform}", type=Path, required=True)
        writer.add_argument(f"--{platform}-sha256", type=sha256, required=True)
    installer = commands.add_parser("install")
    installer.add_argument("bundle", type=Path)
    installer.add_argument("destination", type=Path)
    installer.add_argument("--sha256", type=sha256, required=True)
    installer.add_argument("--platform", choices=PLATFORM_ARCHIVES,
                           default={"win32": "windows", "linux": "linux"}.get(sys.platform))
    args = parser.parse_args(argv)
    if args.action == "install" and args.platform is None:
        parser.error("Specify --platform when preparing a kit on a different operating system")
    try:
        if args.action == "assemble":
            digest = assemble(args.output, args.windows, args.windows_sha256, args.linux, args.linux_sha256)
            print(f"{digest}  {args.output.name}")
        else:
            install(args.bundle, args.sha256, args.platform, args.destination)
            print(f"Installed verified {args.platform} preview in {args.destination}")
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Factory preview: {error}\n")


if __name__ == "__main__":
    main()
