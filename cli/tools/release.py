"""Check release metadata, package a platform archive, verify and install it.

check     versions agree (Cargo.toml, skill compatibility, CHANGELOG.md, optional --tag)
package   pack a built binary, contract, all skills and license inventory into
          craft-<version>-<os>-<arch>.zip (Windows) or .tar.gz (other), plus SHA256SUMS
verify    check an archive against a digest and its manifest
install   extract a verified archive to a new directory; --skills-dir also copies the skills
Archives are preview builds: no signing, no redistribution license, hosts unqualified.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PRODUCT = "Craft AI Factory CLI"
CHANNEL = "preview"
MAX_ARCHIVE = 256 * 1024 * 1024
MAX_FILE = 128 * 1024 * 1024
SEMVER = r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def target():
    system = {"win32": "windows", "linux": "linux", "darwin": "macos"}.get(sys.platform)
    require(system, "Unsupported platform " + sys.platform)
    machine = {"amd64": "x86_64", "x86_64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64"}.get(
        platform.machine().lower())
    require(machine, "Unsupported architecture " + platform.machine())
    return system, machine


def version():
    return tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]


def check_metadata(tag=None):
    current = version()
    require(re.fullmatch(SEMVER, current), "Cargo version is not semantic: " + current)
    compatibility = json.loads((ROOT / "skills/craft-cloud/compatibility.json").read_text(encoding="utf-8"))
    require(compatibility["binary_version"] == current, "Skill compatibility names another binary version")
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    require(re.search(rf"^## {re.escape(current)}\b", changelog, re.M),
            "CHANGELOG.md has no section for " + current)
    require(tag is None or tag == "v" + current, f"Tag {tag} does not match version {current}")
    return {"version": current, "channel": CHANNEL}


def payload(binary):
    files = {}
    name = "bin/craft.exe" if sys.platform == "win32" else "bin/craft"
    files[name] = binary.read_bytes()
    files["schemas/local-contracts.json"] = (ROOT / "schemas/local-contracts.json").read_bytes()
    inventory = ROOT / "docs/evidence/dependency-inventory.json"
    files["dependency-inventory.json"] = inventory.read_bytes()
    require(json.loads(files["dependency-inventory.json"])["lock_sha256"]
            == digest((ROOT / "Cargo.lock").read_bytes()), "Dependency inventory does not match Cargo.lock")
    for path in sorted((ROOT / "skills").rglob("*")):
        if path.is_file():
            files[path.relative_to(ROOT).as_posix()] = path.read_bytes()
    files["CHANGELOG.md"] = (ROOT / "CHANGELOG.md").read_bytes()
    files["README.txt"] = (
        f"{PRODUCT} {version()} ({CHANNEL}).\n"
        "Run bin/craft version and bin/craft --help. Skills are under skills/; copy them into a host's\n"
        "skill directory yourself or use `release.py install --skills-dir`.\n"
        "Preview build: unsigned, no redistribution license selected, host skill loading unqualified.\n"
        "SHA-256 in SHA256SUMS detects corruption against an independently retained digest; it is not a signature.\n"
        "Native vault and live provider behavior is qualified only where docs/install.md says so.\n"
    ).encode("utf-8")
    return files


def manifest_for(files):
    system, machine = target()
    return {"format": 1, "product": PRODUCT, "version": version(), "channel": CHANNEL,
            "platform": {"os": system, "machine": machine},
            "files": {n: {"size": len(d), "sha256": digest(d)} for n, d in sorted(files.items())}}


def archive_name():
    system, machine = target()
    return f"craft-{version()}-{system}-{machine}" + (".zip" if system == "windows" else ".tar.gz")


def package(binary, output_dir, tag):
    check_metadata(tag)
    probe = subprocess.run([str(binary), "version"], capture_output=True, timeout=30, check=True)
    identity = json.loads(probe.stdout)
    require(identity["ok"] and identity["data"]["product"] == PRODUCT
            and identity["data"]["version"] == version(), "Executable identity differs from Cargo.toml")
    files = payload(binary)
    manifest = manifest_for(files)
    members = {**files, "manifest.json": encoded(manifest)}
    out = io.BytesIO()
    if sys.platform == "win32":
        with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
            for name, data in sorted(members.items()):
                entry = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
                entry.external_attr = (stat.S_IFREG | 0o644) << 16
                entry.compress_type = zipfile.ZIP_DEFLATED
                zipped.writestr(entry, data)
    else:
        with tarfile.open(fileobj=out, mode="w:gz", format=tarfile.PAX_FORMAT) as tarred:
            for name, data in sorted(members.items()):
                entry = tarfile.TarInfo(name)
                entry.size, entry.mtime = len(data), 1767225600
                entry.mode = 0o755 if name == "bin/craft" else 0o644
                tarred.addfile(entry, io.BytesIO(data))
    data = out.getvalue()
    require(len(data) <= MAX_ARCHIVE, "Archive exceeds limit")
    output_dir.mkdir(parents=True, exist_ok=True)
    path = output_dir / archive_name()
    with path.open("xb") as handle:
        handle.write(data)
    sums = output_dir / "SHA256SUMS"
    with sums.open("a", encoding="utf-8", newline="\n") as handle:
        handle.write(f"{digest(data)}  {path.name}\n")
    return {"archive": str(path), "sha256": digest(data), "bytes": len(data)}


def read_archive(path, expected):
    require(re.fullmatch(r"[a-f0-9]{64}", expected) is not None, "Invalid digest")
    data = path.read_bytes()
    require(len(data) <= MAX_ARCHIVE and digest(data) == expected, "Archive integrity mismatch")
    files = {}
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(data)) as zipped:
            for entry in zipped.infolist():
                files[entry.filename] = zipped.read(entry) if entry.file_size <= MAX_FILE else None
                require(files[entry.filename] is not None, "Member exceeds limit")
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tarred:
            for entry in tarred.getmembers():
                require(entry.isfile() and entry.size <= MAX_FILE, "Unsupported or oversized member")
                files[entry.name] = tarred.extractfile(entry).read()
    for name in files:
        parts = Path(name).parts
        require(not Path(name).is_absolute() and ".." not in parts and "\\" not in name,
                "Unsafe member name: " + name)
    manifest = json.loads(files.pop("manifest.json"))
    require(manifest["product"] == PRODUCT and manifest["format"] == 1, "Unsupported manifest")
    require(set(manifest["files"]) == set(files), "Manifest does not match archive members")
    for name, data in files.items():
        entry = manifest["files"][name]
        require(entry["size"] == len(data) and entry["sha256"] == digest(data), "Payload mismatch: " + name)
    for needed in ("schemas/local-contracts.json", "skills/craft-cloud/SKILL.md",
                   "skills/confluence-pages/SKILL.md", "skills/atlassian-design/SKILL.md"):
        require(needed in files, "Missing " + needed)
    compat = json.loads(files["skills/craft-cloud/compatibility.json"])
    require(compat["binary_version"] == manifest["version"]
            and compat["contract_snapshot_sha256"]
            == digest(files["schemas/local-contracts.json"].replace(b"\r\n", b"\n")),
            "Skill compatibility does not match the bundled contract")
    return files, manifest


def install(path, expected, destination, skills_dir):
    files, manifest = read_archive(path, expected)
    require(destination.is_absolute() and destination.parent.is_dir() and not destination.exists(),
            "Destination must be a new absolute directory with an existing parent")
    require(skills_dir is None or skills_dir.is_absolute(), "Skills directory must be absolute")
    if skills_dir:
        for name in files:
            if name.startswith("skills/"):
                existing = skills_dir / name.removeprefix("skills/")
                require(not existing.exists(), "Skill file already exists: " + str(existing))
    destination.mkdir()
    for name, data in sorted({**files, "manifest.json": encoded(manifest)}.items()):
        out = destination / name
        out.parent.mkdir(parents=True, exist_ok=True)
        with out.open("xb") as handle:
            handle.write(data)
        if name == "bin/craft":
            out.chmod(0o755)
    copied = []
    if skills_dir:
        for name, data in files.items():
            if name.startswith("skills/"):
                out = skills_dir / name.removeprefix("skills/")
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_bytes(data)
                copied.append(str(out))
    return {"destination": str(destination), "version": manifest["version"], "skill_files": len(copied)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check").add_argument("--tag")
    pack = commands.add_parser("package")
    pack.add_argument("--binary", type=Path, required=True)
    pack.add_argument("--output-dir", type=Path, required=True)
    pack.add_argument("--tag")
    for name in ("verify", "install"):
        command = commands.add_parser(name)
        command.add_argument("--archive", type=Path, required=True)
        command.add_argument("--sha256", required=True)
        if name == "install":
            command.add_argument("--destination", type=Path, required=True)
            command.add_argument("--skills-dir", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "check":
            result = check_metadata(args.tag)
        elif args.command == "package":
            result = package(args.binary, args.output_dir, args.tag)
        elif args.command == "verify":
            files, manifest = read_archive(args.archive, args.sha256)
            result = {"verified": True, "files": len(files), "version": manifest["version"],
                      "platform": manifest["platform"]}
        else:
            result = install(args.archive, args.sha256, args.destination, args.skills_dir)
        print(json.dumps({"ok": True, **result}))
        return 0
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile, tarfile.TarError,
            subprocess.SubprocessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
