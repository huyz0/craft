"""Search bundled Craft skill references, install them, or diagnose an install."""

import argparse
from contextlib import suppress
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
HOSTS = ("codex", "claude-code")
SKILL_NAMES = ("atlassian-design", "confluence-pages", "craft-cloud")
STOP_WORDS = set("a an and by can for from how i in it me my of on the this to use with".split())
GENERIC_ALIASES = set(
    "approval approved backlog cells clipboard columns nodes permissions plan policy preview rendered "
    "rendering repository revision rows table timed verify widths".split()
)


def terms(text):
    normalized = re.sub(r"\bpull[ -]+requests?\b", "pullrequest", text.casefold())
    return set(re.findall(r"[a-z0-9]+", normalized)) - STOP_WORDS


def references(bundle):
    path = bundle / "craft-cloud/reference-index.json"
    index = json.loads(path.read_text(encoding="utf-8"))
    expected = {
        path.relative_to(bundle).as_posix()
        for skill in bundle.glob("*/SKILL.md")
        for path in (skill.parent / "references").rglob("*.md")
    }
    seen = set()
    for item in index:
        if set(item) != {"path", "title", "description", "aliases"}:
            raise ValueError("Invalid reference metadata")
        path = item["path"]
        if path not in expected or path in seen:
            raise ValueError(f"Invalid or duplicate reference path: {path}")
        if not all(isinstance(item[key], str) and item[key].strip()
                   for key in ("title", "description", "aliases")):
            raise ValueError(f"Empty reference metadata: {path}")
        seen.add(path)
    if seen != expected:
        raise ValueError(f"References missing from catalog: {sorted(expected - seen)}")
    return index


def find(bundle, query, skill):
    words = terms(query)
    matches = []
    for item in references(bundle):
        owner, relative = item["path"].split("/", 1)
        if skill and owner != skill:
            continue
        title_terms = terms(item["title"])
        title = words & title_terms
        aliases = words & terms(item["aliases"])
        description = words & terms(item["description"])
        exact_title = title == title_terms
        has_signal = aliases and (len(words) == 1 or len(aliases) > 1 or aliases - GENERIC_ALIASES)
        if not exact_title and not has_signal:
            continue
        score = 100 * exact_title + 3 * len(title) + 4 * len(aliases) + len(description)
        matches.append((score, item["path"], {
            "skill": owner, "reference": relative, "title": item["title"],
            "description": item["description"],
        }))
    matches.sort(key=lambda match: (-match[0], match[1]))
    return {"matches": [match[2] for match in matches[:3]]}


def compatible_bundle(root):
    bundle = root / "skills"
    if not all((bundle / name / "SKILL.md").is_file() for name in SKILL_NAMES):
        raise ValueError("The bundle is missing a required skill")
    compatibility = json.loads((bundle / "craft-cloud/compatibility.json").read_text(encoding="utf-8"))
    snapshot = (root / "schemas/local-contracts.json").read_bytes().replace(b"\r\n", b"\n")
    if (hashlib.sha256(snapshot).hexdigest() != compatibility["contract_snapshot_sha256"]
            or json.loads(snapshot)["contract_version"] != compatibility["contract_version"]):
        raise ValueError("Bundled skills and contract snapshot disagree")
    references(bundle)
    return bundle, compatibility


def path_type(path):
    # Junctions are directories to is_dir() on Windows.
    try:
        info = path.lstat()
    except FileNotFoundError:
        return "missing"
    if getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        return "redirected"
    if stat.S_ISDIR(info.st_mode):
        return "directory"
    if stat.S_ISREG(info.st_mode):
        return "file"
    return "redirected" if stat.S_ISLNK(info.st_mode) else "unsupported"


def targets(hosts, scope, project):
    for host in hosts:
        if scope == "project":
            base = project.resolve(strict=True)
            folder = ".agents" if host == "codex" else ".claude"
        else:
            claude_config = os.environ.get("CLAUDE_CONFIG_DIR")
            base = Path.home().resolve(strict=True)
            folder = ".agents" if host == "codex" else ".claude"
            if host == "claude-code" and claude_config:
                config = Path(claude_config)
                if not config.is_absolute():
                    raise ValueError("CLAUDE_CONFIG_DIR must be absolute for user installation")
                base, folder = config.parent.resolve(strict=True), config.name
        if not base.is_dir():
            raise ValueError(f"Host base path is not a directory: {base}")
        config = base / folder
        yield host, config, config / "skills"


def inventory(directory):
    result = {}
    for path in sorted(directory.iterdir()):
        kind = path_type(path)
        if kind == "directory":
            result.update({Path(path.name) / name: content for name, content in inventory(path).items()})
        elif kind == "file":
            result[Path(path.name)] = path.read_bytes()
        else:
            raise ValueError(f"Unsupported skill entry ({kind}): {path}")
    return result


def installations(bundle, destinations):
    sources = {
        path.parent.name: inventory(path.parent)
        for path in sorted(bundle.glob("*/SKILL.md"))
    }
    for host, config, skills in destinations:
        directory_error = None
        for directory in (config, skills):
            kind = path_type(directory)
            if kind not in ("missing", "directory"):
                directory_error = f"Unsafe {host} discovery directory ({kind}): {directory}"
                break
        for name, expected in sources.items():
            directory = skills / name
            detail = {"host": host, "skill": name, "path": str(directory)}
            if directory_error:
                yield {**detail, "status": "unsafe", "error": directory_error}, directory, expected
                continue
            kind = path_type(directory)
            detail["status"] = kind
            if kind == "directory":
                try:
                    actual = inventory(directory)
                except (OSError, ValueError) as error:
                    detail.update(status="unsafe", error=str(error))
                else:
                    detail["status"] = "installed" if actual == expected else "conflict"
                    if detail["status"] == "conflict":
                        detail["differences"] = [
                            path.as_posix() for path in sorted(actual.keys() | expected.keys())
                            if actual.get(path) != expected.get(path)
                        ]
            yield detail, directory, expected


def install(entries):
    for detail, directory, _ in entries:
        if detail["status"] not in ("missing", "installed"):
            raise ValueError(detail.get("error") or f"Existing skill conflicts with the bundle: {directory}")
    copied = set()
    for detail, directory, files in entries:
        if directory in copied:
            detail["status"] = "installed"
        if detail["status"] == "installed":
            continue
        directory.parent.mkdir(parents=True, exist_ok=True)
        directory.mkdir()
        created_dirs = [directory]
        created_files = []
        try:
            for name, content in files.items():
                output = directory / name
                for parent in reversed(output.parents):
                    if parent != directory and parent.is_relative_to(directory) and parent not in created_dirs:
                        parent.mkdir()
                        created_dirs.append(parent)
                with output.open("xb") as handle:
                    created_files.append(output)
                    handle.write(content)
        except OSError:
            for path in reversed(created_files):
                with suppress(OSError):
                    path.unlink()
            for path in reversed(created_dirs):
                with suppress(OSError):
                    path.rmdir()
            raise
        detail["status"] = "installed"
        copied.add(directory)


def binary_status(root, selected, compatibility):
    if selected is None:
        bundled = root / "bin" / ("craft.exe" if sys.platform == "win32" else "craft")
        selected = str(bundled) if bundled.is_file() else shutil.which("craft")
    if selected is None:
        return {"status": "missing"}
    binary = Path(selected).resolve()
    result = {"path": str(binary)}
    try:
        probe = subprocess.run([str(binary), "version"], capture_output=True, timeout=10, check=True)
        response = json.loads(probe.stdout)
        identity = response["data"]
        if response["ok"] is not True or not isinstance(identity, dict):
            raise ValueError("Invalid version response")
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        return {**result, "status": "probe_failed"}
    expected = {"product": "Craft AI Factory CLI", "version": compatibility["binary_version"],
                "contract": compatibility["contract_version"]}
    differences = [key for key, value in expected.items() if identity.get(key) != value]
    return {**result, "status": "mismatch", "differences": differences} if differences else {
        **result, "status": "compatible", "version": expected["version"], "contract": expected["contract"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    discover = commands.add_parser("find")
    discover.add_argument("query")
    discover.add_argument("--skill", choices=SKILL_NAMES)
    for name in ("install", "doctor"):
        command = commands.add_parser(name)
        command.add_argument("--host", choices=(*HOSTS, "both"), required=True)
        command.add_argument("--scope", choices=("project", "user"), required=True)
        command.add_argument("--project", type=Path)
        if name == "doctor":
            command.add_argument("--binary")
    args = parser.parse_args()
    try:
        if args.command == "find":
            result = {"ok": True, **find(ROOT / "skills", args.query, args.skill)}
        else:
            bundle, compatibility = compatible_bundle(ROOT)
            if args.project is not None and args.scope != "project":
                raise ValueError("--project requires project scope")
            hosts = HOSTS if args.host == "both" else (args.host,)
            entries = list(installations(bundle, targets(hosts, args.scope, args.project or Path.cwd())))
            result = {"ok": True, "scope": args.scope, "skills": [detail for detail, _, _ in entries]}
            if args.command == "install":
                install(entries)
            else:
                binary = binary_status(ROOT, args.binary, compatibility)
                result["binary"] = binary
                result["ok"] = binary["status"] == "compatible" and all(
                    detail["status"] == "installed" for detail, _, _ in entries)
        print(json.dumps(result))
        return 0 if result["ok"] else 1
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
