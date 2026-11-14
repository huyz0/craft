"""Produce a reproducible source/license inventory from locked Cargo metadata."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "docs" / "evidence" / "dependency-inventory.json"


def inventory():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
        cwd=ROOT, text=True, encoding="utf-8",
    ))
    lock_bytes = (ROOT / "Cargo.lock").read_bytes()
    locked = tomllib.loads(lock_bytes.decode("utf-8"))
    checksums = {
        (package["name"], package["version"], package.get("source")): package.get("checksum")
        for package in locked["package"]
    }
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    packages = {package["id"]: package for package in metadata["packages"]}
    records = []
    for package_id, node in nodes.items():
        package = packages[package_id]
        source = package["source"]
        local = source is None
        if local and package_id not in metadata["workspace_members"]:
            raise ValueError("Unreviewed non-workspace path dependency")
        checksum = checksums[(package["name"], package["version"], source)]
        if not local and (not source.startswith("registry+") or not checksum):
            raise ValueError("Dependency needs a registry source and locked checksum")
        if not local and not (package["license"] or package["license_file"]):
            raise ValueError("Dependency has no declared license metadata")
        manifest = Path(package["manifest_path"])
        manifest_hash = hashlib.sha256(manifest.read_bytes()).hexdigest()
        dependencies = []
        for dependency in node["deps"]:
            target = packages[dependency["pkg"]]
            dependencies.append({
                "package": target["name"], "version": target["version"],
                "kinds": sorted({kind["kind"] or "normal" for kind in dependency["dep_kinds"]}),
            })
        records.append({
            "name": package["name"], "version": package["version"],
            "source": source or "workspace", "checksum": checksum,
            "manifest_sha256": manifest_hash, "license": package["license"],
            "license_file": Path(package["license_file"]).name if package["license_file"] else None,
            "rust_version": package["rust_version"], "features": sorted(node["features"]),
            "dependencies": sorted(dependencies, key=lambda item: (item["package"], item["version"])),
        })
    return {
        "format_version": 1,
        "lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
        "packages": sorted(records, key=lambda item: (item["name"], item["version"])),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="Refresh the checked-in inventory")
    arguments = parser.parse_args()
    try:
        current = inventory()
        if arguments.write:
            INVENTORY.write_text(json.dumps(current, indent=2) + "\n", encoding="utf-8")
        elif json.loads(INVENTORY.read_text(encoding="utf-8")) != current:
            raise ValueError("Dependency inventory differs; review changes before --write")
        print(json.dumps({"ok": True, "packages": len(current["packages"])}))
        return 0
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
