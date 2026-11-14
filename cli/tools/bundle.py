"""Prepare, verify and install an internal Windows development bundle without PATH changes."""

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
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
MAX_ARCHIVE = 256 * 1024 * 1024
MAX_FILE = 128 * 1024 * 1024
MAX_TOTAL = 240 * 1024 * 1024
SKILL_FILES = {"SKILL.md", "compatibility.json", "references/lists.md",
               "references/writes.md", "references/plan-review.md", "references/recovery.md"}
PAYLOAD = {"bin/craft.exe", "schemas/local-contracts.json", "dependency-inventory.json",
           "README.txt"} | {"skills/craft-cloud/" + name for name in SKILL_FILES}
PRODUCT = "Craft AI Factory CLI"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate JSON member")
        result[key] = value
    return result


def document(data):
    return json.loads(data, object_pairs_hook=unique_object)


def host():
    return {"os": sys.platform, "machine": platform.machine().lower()}


def check_payload(files, manifest):
    require(set(files) == PAYLOAD, "Unexpected or missing payload")
    require(isinstance(manifest, dict) and set(manifest) == {
        "format", "product", "version", "status", "platform", "files"}, "Invalid manifest shape")
    require(type(manifest["format"]) is int and manifest["format"] == 1
            and manifest["product"] == PRODUCT and manifest["status"] == "development",
            "Unsupported bundle identity")
    require(isinstance(manifest["version"], str)
            and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", manifest["version"]), "Invalid version")
    require(manifest["platform"] == host() and sys.platform == "win32",
            "Bundle platform does not match this Windows host")
    require(isinstance(manifest["files"], dict) and set(manifest["files"]) == PAYLOAD,
            "Manifest payload mismatch")
    for name, data in files.items():
        entry = manifest["files"][name]
        require(isinstance(entry, dict) and set(entry) == {"size", "sha256"}
                and type(entry["size"]) is int and entry["size"] == len(data)
                and entry["sha256"] == digest(data), "Payload integrity mismatch: " + name)
    contract = document(files["schemas/local-contracts.json"])
    compatibility = document(files["skills/craft-cloud/compatibility.json"])
    require(set(compatibility) == {"format_version", "skill_version", "status", "binary_version",
                                  "contract_version", "contract_snapshot_sha256", "providers",
                                  "hosts", "live_provider_qualification"}, "Invalid compatibility metadata")
    require(type(compatibility["format_version"]) is int and compatibility["format_version"] == 1
            and isinstance(compatibility["skill_version"], str)
            and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", compatibility["skill_version"])
            and compatibility["hosts"] == {"codex": "unqualified", "claude-code": "unqualified"}
            and compatibility["live_provider_qualification"] == "not_established_by_skill_validation",
            "Unsupported qualification claims")
    require(compatibility["contract_snapshot_sha256"] == digest(files["schemas/local-contracts.json"])
            and compatibility["binary_version"] == manifest["version"]
            and compatibility["contract_version"] == contract["contract_version"]
            and compatibility["status"] == "development"
            and set(compatibility["providers"]) == {"confluence", "jira"}
            and isinstance(contract["operations"], dict), "Incompatible skill/contract")


def prepare(binary, binary_sha256, output):
    require(sys.platform == "win32", "Only Windows development bundles are implemented")
    binary_data = binary.read_bytes()
    require(digest(binary_data) == binary_sha256, "Selected binary digest mismatch")
    require(binary.resolve() == (ROOT / "target/debug/craft.exe").resolve(),
            "Select the normal workspace development executable")
    # Probe the pinned bytes rather than a workspace path a concurrent build can replace.
    with tempfile.TemporaryDirectory(prefix="craft-bundle-probe-", dir=ROOT / "target") as temporary:
        probe = Path(temporary) / "craft.exe"
        with probe.open("xb") as target:
            target.write(binary_data)
        result = subprocess.run([str(probe), "version"], check=True,
                                capture_output=True, timeout=20)
    version = document(result.stdout)
    expected = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    require(version["ok"] is True and version["data"]["product"] == PRODUCT
            and version["data"]["version"] == expected, "Unexpected executable identity")
    files = {"bin/craft.exe": binary_data,
             "schemas/local-contracts.json": (ROOT / "schemas/local-contracts.json").read_bytes(),
             "dependency-inventory.json": (ROOT / "docs/evidence/dependency-inventory.json").read_bytes()}
    inventory = document(files["dependency-inventory.json"])
    require(inventory["lock_sha256"] == digest((ROOT / "Cargo.lock").read_bytes()),
            "Dependency inventory does not match Cargo.lock")
    for name in SKILL_FILES:
        files["skills/craft-cloud/" + name] = (ROOT / "skills/craft-cloud" / name).read_bytes()
    files["README.txt"] = (
        "Craft AI Factory CLI. Internal local development bundle.\n"
        "Run bin/craft.exe version and bin/craft.exe --help from this directory.\n"
        "The progressive companion skill is in skills/craft-cloud. Host loading is unqualified.\n"
        "No PATH, credentials, profile, cloud resource or host skill directory is changed.\n"
        "Windows synthetic checks do not qualify native vaults, live providers or other platforms.\n"
        "Dependency license metadata is an inventory, not complete redistribution notices.\n"
        "No project redistribution license has been selected. Do not redistribute this bundle.\n"
        "SHA-256 detects changes against an independently retained digest; it is not a signature.\n"
        "Upgrade/uninstall automation and release qualification remain unfinished.\n"
    ).encode("utf-8")
    manifest = {"format": 1, "product": PRODUCT, "version": expected,
                "status": "development", "platform": host(),
                "files": {name: {"size": len(data), "sha256": digest(data)}
                          for name, data in files.items()}}
    check_payload(files, manifest)
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as target:
        for name, data in sorted({**files, "manifest.json": encoded(manifest)}.items()):
            entry = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            entry.external_attr = (stat.S_IFREG | 0o644) << 16
            target.writestr(entry, data)
    data = archive.getvalue()
    require(len(data) <= MAX_ARCHIVE, "Bundle exceeds archive limit")
    with output.open("xb") as target:
        target.write(data)
    return {"archive": str(output.resolve()), "sha256": digest(data), "status": "development"}


def verified(archive, expected_digest):
    require(re.fullmatch(r"[a-f0-9]{64}", expected_digest) is not None, "Invalid archive digest")
    # Hash and inspect the same bounded bytes, never reopen the archive after validation.
    with archive.open("rb") as source:
        data = source.read(MAX_ARCHIVE + 1)
    require(len(data) <= MAX_ARCHIVE and digest(data) == expected_digest, "Archive integrity mismatch")
    files = {}
    with zipfile.ZipFile(io.BytesIO(data)) as source:
        entries = source.infolist()
        require(len(entries) == len(PAYLOAD) + 1, "Unexpected archive member count")
        total = 0
        for entry in entries:
            require(entry.filename in PAYLOAD | {"manifest.json"} and entry.filename not in files,
                    "Unexpected, unsafe or duplicate archive member")
            mode = entry.external_attr >> 16
            require(not entry.is_dir() and stat.S_IFMT(mode) in {0, stat.S_IFREG}
                    and not entry.flag_bits & 1, "Linked or encrypted member is unsupported")
            require(0 <= entry.file_size <= MAX_FILE, "Archive member exceeds limit")
            total += entry.file_size
            require(total <= MAX_TOTAL, "Expanded payload exceeds limit")
            if entry.filename == "manifest.json":
                require(entry.file_size <= 65536, "Manifest exceeds limit")
            files[entry.filename] = source.read(entry)
    manifest = document(files.pop("manifest.json"))
    check_payload(files, manifest)
    return files, manifest


def install(archive, expected_digest, destination):
    files, manifest = verified(archive, expected_digest)
    # A destination is an explicit private directory, never an inferred PATH entry.
    require(destination.is_absolute(), "Installation requires an absolute destination")
    require(destination.parent.is_dir() and not destination.exists(),
            "Destination must be new with an existing parent")
    destination.mkdir()
    for name, data in sorted({**files, "manifest.json": encoded(manifest)}.items()):
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as target:
            target.write(data)
    receipt = {"format": 1, "archive_sha256": expected_digest,
               "destination": str(destination), "status": "development"}
    with (destination / "install-receipt.json").open("xb") as target:
        target.write(encoded(receipt))
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("prepare")
    create.add_argument("--binary", type=Path, required=True)
    create.add_argument("--binary-sha256", required=True)
    create.add_argument("--output", type=Path, required=True)
    for name in ["verify", "install"]:
        command = commands.add_parser(name)
        command.add_argument("--archive", type=Path, required=True)
        command.add_argument("--sha256", required=True)
        if name == "install":
            command.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            result = prepare(args.binary, args.binary_sha256, args.output)
        elif args.command == "verify":
            files, manifest = verified(args.archive, args.sha256)
            result = {"verified": True, "files": len(files), "version": manifest["version"],
                      "status": manifest["status"]}
        else:
            result = install(args.archive, args.sha256, args.destination)
        print(json.dumps({"ok": True, **result}))
        return 0
    except (OSError, ValueError, KeyError, TypeError, RecursionError, zipfile.BadZipFile,
            subprocess.SubprocessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
