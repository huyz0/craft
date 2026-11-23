"""Qualify every skill under skills/: static checks, optional binary checks, evidence freshness.

check      offline static checks for all skills (frontmatter, links, command examples, budgets)
           --binary PATH also runs the executable: version, every cited command resolves through
           `craft schema`, and the whiteboard scene example encodes, checks and decodes.
baseline   record the content hash of each skill as the version last qualified by A/B or live
           evidence (run only after that evidence is recorded in docs/evidence)
Skills changed since the baseline are reported as stale; --strict turns that into a failure.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
import runtime_skill  # noqa: E402
import skills  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
SKILLS = ROOT / "skills"
CONTRACT = ROOT / "schemas/local-contracts.json"
BASELINE = ROOT / "docs/evidence/skills-qualification.json"
ENTRY_BUDGET = 16 * 1024         # SKILL.md is loaded whole: keep it small
DESCRIPTION_BUDGET = 1024
SCENE_REFERENCE = SKILLS / "craft-cloud/references/whiteboard-scenes.md"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def skill_dirs():
    return sorted(path.parent for path in SKILLS.glob("*/SKILL.md"))


def content_hash(directory):
    """Hash names and bytes with LF endings so CRLF and LF checkouts agree."""
    digest = hashlib.sha256()
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        digest.update(path.relative_to(directory).as_posix().encode())
        digest.update(b"\0" + path.read_bytes().replace(b"\r\n", b"\n") + b"\0")
    return digest.hexdigest()


def fenced_lines(text):
    """Yield (language, line) for each line inside a fenced block."""
    language = None
    for line in text.splitlines():
        if line.startswith("```"):
            language = None if language is not None else line[3:].strip() or ""
            continue
        if language is not None:
            yield language, line


def anchors(text):
    """GitHub-style heading slugs."""
    slugs = set()
    for _, heading in re.findall(r"^(#{1,6}) +(.+?) *$", text, re.M):
        slugs.add(re.sub(r"[^a-z0-9 -]", "", heading.lower()).replace(" ", "-"))
    return slugs


def command_path(words):
    path = []
    for word in words:
        if word.startswith("-") or not re.fullmatch(r"[a-z][a-z0-9-]*", word):
            break
        path.append(word)
    return ".".join(path)


def check_skill(directory, contract):
    name = directory.name
    entry = (directory / "SKILL.md").read_text(encoding="utf-8")
    frontmatter = re.match(r"\A---\nname: ([a-z0-9-]+)\ndescription: ([^\n]+)\n---\n", entry)
    require(frontmatter and frontmatter[1] == name, f"{name}: invalid two-field frontmatter")
    require(len(frontmatter[2]) <= DESCRIPTION_BUDGET, f"{name}: description over budget")
    require(len(entry.encode()) <= ENTRY_BUDGET, f"{name}: SKILL.md over {ENTRY_BUDGET} bytes")
    commands, examples, links = set(), 0, 0
    files = sorted(directory.rglob("*.md"))
    for path in files:
        text = path.read_text(encoding="utf-8")
        for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", text):
            if re.match(r"[a-z]+://", target):
                continue
            file_part, _, anchor = target.partition("#")
            resolved = (path.parent / file_part if file_part else path).resolve()
            require(resolved.is_relative_to(SKILLS.resolve()) and resolved.is_file(),
                    f"{name}: broken or escaping link {target} in {path.name}")
            require(not anchor or anchor in anchors(resolved.read_text(encoding="utf-8")),
                    f"{name}: missing heading anchor {target} in {path.name}")
            links += 1
        for cited in re.findall(r"`([a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)+)`", text):
            if cited in contract["operations"]:
                commands.add(cited)
        for span in re.findall(r"`(craft [^`]+)`", text):
            words = span.split()[1:]
            if words and words[0] in {"schema", "help", "version"} or len(words) == 0:
                continue
            candidate = command_path(words)
            require(candidate in contract["operations"],
                    f"{name}: inline example names an unknown command: {span}")
            commands.add(candidate)
        for _, line in fenced_lines(text):
            if line.startswith("craft "):
                commands.add(runtime_skill.command_example(line, contract))
                examples += 1
    return {"name": name, "markdown_files": len(files), "entry_bytes": len(entry.encode()),
            "links": links, "examples": examples, "commands": sorted(commands),
            "content_sha256": content_hash(directory)}


def run(binary, *arguments, timeout=30, cwd=None):
    result = subprocess.run([str(binary), *arguments], capture_output=True, timeout=timeout, cwd=cwd)
    return json.loads(result.stdout or b"{}"), result.returncode


def scene_example():
    text = SCENE_REFERENCE.read_text(encoding="utf-8")
    match = re.search(r"```json\n(.*?)\n```", text, re.S)
    require(match, "Whiteboard scene example not found")
    return json.loads(match[1])


def check_binary(binary, reports):
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    version, _ = run(binary, "version")
    require(version.get("ok") and version["data"]["version"] == cargo["workspace"]["package"]["version"],
            "Executable version differs from Cargo.toml")
    contract_bytes = CONTRACT.read_bytes().replace(b"\r\n", b"\n")
    cited = sorted({c for report in reports for c in report["commands"]})
    for command in cited:
        document, code = run(binary, "schema", "--command", command, "--max-bytes", "4000000")
        require(code == 0 and document.get("ok"), f"Executable does not resolve cited command {command}")
    with tempfile.TemporaryDirectory(prefix="craft-skill-scene-") as temporary:
        scene = Path(temporary) / "scene.json"
        scene.write_text(json.dumps(scene_example()), encoding="utf-8")
        for verb in ("check", "encode"):
            document, code = run(binary, "whiteboard", verb, "--file", scene.name, "--max-bytes", "1048576",
                                cwd=temporary)
            require(code == 0 and document.get("ok"), f"Whiteboard scene example fails {verb}")
        encoded, _ = run(binary, "whiteboard", "encode", "--file", scene.name,
                         "--fields", "clipboard_html", "--max-bytes", "1048576", cwd=temporary)
        html = Path(temporary) / "clipboard.html"
        html.write_text(encoded["data"]["clipboard_html"], encoding="utf-8")
        decoded, code = run(binary, "whiteboard", "decode", "--file", html.name, "--max-bytes", "1048576",
                              cwd=temporary)
        require(code == 0 and decoded.get("ok"), "Encoded scene example does not decode")
    return {"version": version["data"]["version"], "commands_resolved": len(cited),
            "scene_roundtrip": "encode+check+decode", "contract_sha256": hashlib.sha256(contract_bytes).hexdigest()}


def check(binary=None, strict=False):
    skills.references(SKILLS)
    contract = json.loads(CONTRACT.read_bytes())
    reports = [check_skill(directory, contract) for directory in skill_dirs()]
    require({r["name"] for r in reports} >= {"craft-cloud", "confluence-pages", "atlassian-design"},
            "An expected skill is missing")
    runtime_skill.check()
    result = {"ok": True, "skills": reports, "host_qualification": "not_run",
              "validation_network_requests": 0}
    if binary:
        result["binary"] = check_binary(binary, reports)
    baseline = json.loads(BASELINE.read_text(encoding="utf-8")) if BASELINE.exists() else {"skills": {}}
    stale = [r["name"] for r in reports if baseline["skills"].get(r["name"]) != r["content_sha256"]]
    result["changed_since_evidence"] = stale
    require(not (strict and stale), "Skills changed since their recorded A/B or live evidence: "
            + ", ".join(stale))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["check", "baseline"], nargs="?", default="check")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--strict", action="store_true")
    arguments = parser.parse_args()
    try:
        if arguments.mode == "baseline":
            hashes = {d.name: content_hash(d) for d in skill_dirs()}
            BASELINE.write_text(json.dumps({"format": 1, "skills": hashes}, indent=2) + "\n",
                                encoding="utf-8", newline="\n")
            result = {"ok": True, "baseline": hashes}
        else:
            result = check(arguments.binary, arguments.strict)
        print(json.dumps(result, indent=2))
        return 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
