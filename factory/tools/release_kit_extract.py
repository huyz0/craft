"""Extract the regular files of a verified Factory kit into a staging directory."""

from hashlib import file_digest
import os
from pathlib import Path
import re
import shutil
import stat
import tarfile
from tempfile import TemporaryDirectory
import zipfile


WINDOWS_DEVICE = re.compile(r"(?i)(?:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|(?:COM|LPT)[1-9¹²³])")
PLATFORM_ARCHIVES = {
    "windows": "craft-factory-shared-windows-x64.zip",
    "linux": "craft-factory-shared-linux-x64.tar.gz",
}


def archive_record(archive):
    with archive.open("rb") as stream:
        digest = file_digest(stream, "sha256").hexdigest()
        return {"name": archive.name, "bytes": os.fstat(stream.fileno()).st_size, "sha256": digest}


def extract_entry(root, name, directory, mode, contents, paths):
    if directory:
        name = name.removesuffix("/")
    parts = name.split("/")
    if any(not part or part in {".", ".."} or part.endswith((" ", "."))
           or re.search(r'[<>:"\\|?*\x00-\x1f]', part)
           or WINDOWS_DEVICE.fullmatch(part.split(".", 1)[0].rstrip())
           for part in parts):
        raise ValueError(f"Unsafe kit archive member: {name!r}")
    for length in range(1, len(parts) + 1):
        prefix = "/".join(parts[:length])
        if paths.setdefault(prefix.casefold(), prefix) != prefix:
            raise ValueError(f"Conflicting kit archive path: {name!r}")

    target = root.joinpath(*parts)
    if directory:
        target.mkdir(parents=True, exist_ok=True)
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        with contents() as source, target.open("xb") as output:
            shutil.copyfileobj(source, output)
        if os.name != "nt":
            target.chmod(0o755 if mode & 0o111 else 0o644)


def extract_archive(archive, stage, platform):
    paths = {}
    suffix = ".exe" if platform == "windows" else ""
    if platform == "windows":
        with zipfile.ZipFile(archive) as kit:
            for entry in kit.infolist():
                mode = entry.external_attr >> 16
                expected_type = stat.S_IFDIR if entry.is_dir() else stat.S_IFREG
                if stat.S_IFMT(mode) not in (0, expected_type):
                    raise ValueError(f"Unsupported ZIP member type: {entry.orig_filename!r}")
                extract_entry(stage, entry.orig_filename, entry.is_dir(), mode, lambda: kit.open(entry), paths)
    else:
        with tarfile.open(archive, "r:gz") as kit:
            for entry in kit:
                if not (entry.isfile() or entry.isdir()):
                    raise ValueError(f"Unsupported tar member type: {entry.name!r}")
                extract_entry(stage, entry.name, entry.isdir(), entry.mode, lambda: kit.extractfile(entry), paths)
    for filename in ("QUICKSTART.md", f"craft-factory{suffix}", f"shared/craft-factory-queue{suffix}",
                     "shared/migrate_queue.py"):
        if not (stage / filename).is_file():
            raise ValueError(f"Kit is missing {filename}")


def install_archive(archive, platform, destination, expected):
    if destination.name in {"", ".."}:
        raise ValueError("Provide a new destination directory")
    destination = destination.parent.resolve(strict=True) / destination.name
    if os.path.lexists(destination):
        raise ValueError("The kit destination already exists")
    with TemporaryDirectory(prefix=".craft-install-", dir=destination.parent) as temporary:
        root = Path(temporary)
        snapshot = root / archive.name
        shutil.copyfile(archive, snapshot)
        if archive_record(snapshot) != expected:
            raise ValueError("The selected archive changed after bundle verification")
        stage = root / "kit"
        stage.mkdir()
        try:
            extract_archive(snapshot, stage, platform)
        except (tarfile.TarError, zipfile.BadZipFile, RuntimeError) as error:
            raise ValueError("The verified kit is not a readable archive") from error

        destination.mkdir(mode=0o700)
        moved = []
        try:
            for entry in sorted(stage.iterdir()):
                moved.append(entry)
                entry.rename(destination / entry.name)
        except BaseException:
            for entry in reversed(moved):
                if not entry.exists():
                    (destination / entry.name).rename(entry)
            destination.rmdir()
            raise
