"""Inspect and validate Craft's local specification and implementation plan."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHANGE = Path("openspec/changes/build-cloud-cli")
FIELDS = ("Goal", "Spec", "Depends on", "Build", "Verify", "Done when", "Evidence")
LABEL = re.compile(r"\b(?:M\d+(?:[._-]\d+)*|(?:TASK|TICKET|CRAFT)[_-]\d+|T\d{2,})\b")
IMPLEMENTATION_SUFFIXES = {
    ".rs", ".py", ".toml", ".json", ".yaml", ".yml", ".sh", ".ps1", ".txt", ".snap", ".csv"
}


def emit(value: object) -> None:
    print(json.dumps(value, ensure_ascii=False, indent=2))


def parse_tasks(content: str) -> list[dict]:
    headings = list(re.finditer(r"^### ([a-z][a-z0-9-]+)\s*$", content, re.M))
    tasks = []
    for index, match in enumerate(headings):
        end = headings[index + 1].start() if index + 1 < len(headings) else len(content)
        section = content[match.end():end]
        values: dict[str, str] = {}
        for field, value in re.findall(r"^\*\*([^*]+):\*\* (.+)$", section, re.M):
            if field in values:
                raise ValueError(f"{match[1]}: duplicate field {field}")
            values[field] = value.strip()
        checks = re.findall(r"^- \[([ x])\] Deliver this task\.$", section, re.M)
        if len(checks) != 1:
            raise ValueError(f"{match[1]}: expected one completion checkbox")
        missing = set(FIELDS) - values.keys()
        extra = values.keys() - set(FIELDS)
        if missing or extra:
            raise ValueError(f"{match[1]}: missing={sorted(missing)}, unknown={sorted(extra)}")
        if any(not value or len(value) < 3 for value in values.values()):
            raise ValueError(f"{match[1]}: empty or incomplete task field")
        dependency_text = values["Depends on"]
        dependencies = [] if dependency_text == "none" else dependency_text.split(", ")
        tasks.append({
            "name": match[1], "complete": checks[0] == "x",
            "dependencies": dependencies, "fields": values,
        })
    if not tasks:
        raise ValueError("No task sections found")
    return tasks


def read_requirements(root: Path) -> dict[str, set[str]]:
    specs = root / CHANGE / "specs"
    return {
        path.parent.name: set(re.findall(r"^### Requirement: (.+)$", path.read_text(encoding="utf-8"), re.M))
        for path in sorted(specs.glob("*/spec.md"))
    }


def validate_tasks(tasks: list[dict], requirements: dict[str, set[str]], root: Path) -> list[str]:
    errors = []
    by_name = {task["name"]: task for task in tasks}
    if len(by_name) != len(tasks):
        errors.append("Duplicate task names")
    covered: set[tuple[str, str]] = set()
    for task in tasks:
        name = task["name"]
        if len(set(task["dependencies"])) != len(task["dependencies"]):
            errors.append(f"{name}: repeated dependency")
        for dependency in task["dependencies"]:
            if dependency not in by_name:
                errors.append(f"{name}: unknown dependency {dependency}")
            elif task["complete"] and not by_name[dependency]["complete"]:
                errors.append(f"{name}: completed with incomplete dependency {dependency}")
        parts = task["fields"]["Spec"].split(" / ", 1)
        if len(parts) != 2 or parts[1] not in requirements.get(parts[0], set()):
            errors.append(f"{name}: invalid capability requirement reference")
        else:
            covered.add((parts[0], parts[1]))
        evidence = task["fields"]["Evidence"]
        if task["complete"] and evidence == "none":
            errors.append(f"{name}: completed without evidence")
        if evidence != "none":
            path = (root / evidence).resolve()
            permitted = (root / "docs/evidence").resolve()
            if not path.is_relative_to(permitted) or not path.is_file():
                errors.append(f"{name}: evidence is not an existing local evidence record")
    for capability, names in requirements.items():
        if not names:
            errors.append(f"{capability}: no requirements")
        for name in names:
            if (capability, name) not in covered:
                errors.append(f"{capability}: uncovered requirement {name}")
    active: set[str] = set()
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in active:
            errors.append(f"Dependency cycle at {name}")
            return
        if name in visited or name not in by_name:
            return
        active.add(name)
        for dependency in by_name[name]["dependencies"]:
            visit(dependency)
        active.remove(name)
        visited.add(name)

    for name in by_name:
        visit(name)
    return errors


def outside_fences(content: str) -> tuple[str, list[str], int]:
    fence = None
    language = ""
    block = []
    visible = []
    errors = []
    examples = 0
    for line in content.splitlines():
        marker = re.match(r"^\s*(`{3,}|~{3,})(.*)$", line)
        if fence is None and marker:
            fence, language = marker[1], marker[2].strip()
            block = []
        elif fence:
            if marker and marker[1][0] == fence[0] and len(marker[1]) >= len(fence) and not marker[2].strip():
                if language == "json":
                    try:
                        json.loads("\n".join(block))
                        examples += 1
                    except ValueError as error:
                        errors.append(f"Invalid JSON example: {error}")
                fence = None
            else:
                block.append(line)
        else:
            visible.append(line)
    if fence:
        errors.append("Unclosed code fence")
    return "\n".join(visible), errors, examples


def source_files(root: Path, excluded: set[str]):
    def fail(error):
        raise error

    # Prune before descent so build output cannot make source checks grow with the cache.
    for directory, children, files in os.walk(root, topdown=True, onerror=fail):
        children[:] = sorted(child for child in children if child not in excluded)
        for name in sorted(files):
            path = Path(directory) / name
            if path.is_file():
                yield path


def scan_implementation(root: Path) -> list[str]:
    errors = []
    for path in sorted(source_files(root, {"openspec", ".git", "target", ".venv", "__pycache__"})):
        relative = path.relative_to(root)
        if path.suffix not in IMPLEMENTATION_SUFFIXES:
            continue
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if LABEL.search(line):
                errors.append(f"{relative}:{line_number}: planning label in implementation text")
    return errors


def check_skill(path: Path) -> list[str]:
    """Check the two plain-scalar fields used by the local contributor skill."""
    content = path.read_text(encoding="utf-8")
    header = re.match(r"\A---\n(.*?)\n---\n", content, re.S)
    if header is None:
        return ["Contributor skill has no valid frontmatter boundary"]
    fields = {}
    for line in header[1].splitlines():
        pair = line.split(": ", 1)
        if len(pair) != 2 or pair[0] in fields:
            return ["Contributor skill must use unique plain name/description fields"]
        fields[pair[0]] = pair[1]
    if set(fields) != {"name", "description"}:
        return ["Contributor skill must declare name and description only"]
    name = fields["name"]
    if len(name) > 64 or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", name):
        return ["Contributor skill name is invalid"]
    description = fields["description"]
    if not description or len(description) > 1024 or any(char in description for char in "<>\n"):
        return ["Contributor skill description is invalid"]
    if "[TODO:" in content:
        return ["Contributor skill contains an unfinished placeholder"]
    return []


def check(root: Path) -> dict:
    required = [
        Path("README.md"), Path("AGENTS.md"), Path("SDD.md"), Path("STATE.md"),
        Path("docs/coding-standard.md"), Path("docs/repository-audit.md"),
        Path("docs/validation.md"), Path("docs/decisions.md"),
        Path("openspec/config.yaml"), Path("openspec/specs/project-workflow/spec.md"),
        Path(".agents/skills/craft-sdd/SKILL.md"),
        CHANGE / "proposal.md", CHANGE / "design.md", CHANGE / "tasks.md",
    ]
    errors = [f"Missing required file: {path}" for path in required if not (root / path).is_file()]
    if errors:
        return {"ok": False, "errors": errors}
    config = (root / "openspec/config.yaml").read_text(encoding="utf-8")
    if not re.search(r"^schema: spec-driven$", config, re.M):
        errors.append("Expected the applied spec-driven OpenSpec configuration")
    try:
        tasks = parse_tasks((root / CHANGE / "tasks.md").read_text(encoding="utf-8"))
    except ValueError as error:
        return {"ok": False, "errors": [str(error)]}
    requirements = read_requirements(root)
    if not requirements:
        errors.append("No product capability specs")
    errors.extend(validate_tasks(tasks, requirements, root))
    examples = 0
    for path in sorted(source_files(root, {"target", ".git", ".venv"})):
        if path.suffix != ".md":
            continue
        content = path.read_text(encoding="utf-8")
        visible, fence_errors, count = outside_fences(content)
        examples += count
        errors.extend(f"{path.relative_to(root)}: {error}" for error in fence_errors)
        for destination in re.findall(r"\]\(([^)]+)\)", visible):
            if re.match(r"^[a-z]+:", destination) or destination.startswith("#"):
                continue
            target = destination.split("#", 1)[0]
            if not (path.parent / target).exists():
                errors.append(f"{path.relative_to(root)}: missing link {destination}")
    errors.extend(scan_implementation(root))
    errors.extend(check_skill(root / ".agents/skills/craft-sdd/SKILL.md"))
    return {
        "ok": not errors, "tasks": len(tasks), "complete": sum(task["complete"] for task in tasks),
        "capabilities": len(requirements), "requirements": sum(map(len, requirements.values())),
        "json_examples": examples, "errors": errors,
    }


def ready_tasks(tasks: list[dict]) -> list[dict]:
    done = {task["name"] for task in tasks if task["complete"]}
    return [task for task in tasks if not task["complete"] and set(task["dependencies"]) <= done]


def selftest() -> dict:
    cases = []
    requirements = {"sample": {"Observable behavior"}}

    def example(name: str, depends: str = "none", complete: bool = False) -> str:
        values = {
            "Goal": "Produce observable behavior", "Spec": "sample / Observable behavior",
            "Depends on": depends, "Build": "Build the smallest complete behavior",
            "Verify": "Observe the accepted and rejected outcomes", "Done when": "The behavior is verified",
            "Evidence": "none",
        }
        return (f"### {name}\n\n- [{'x' if complete else ' '}] Deliver this task.\n\n" +
                "\n\n".join(f"**{key}:** {value}" for key, value in values.items()) + "\n\n")

    with tempfile.TemporaryDirectory(prefix=".plan-check-", dir=ROOT) as temporary:
        root = Path(temporary).resolve()
        if not root.is_relative_to(ROOT.resolve()):
            raise ValueError("Self-test directory escaped the intended workspace")
        valid = example("first-behavior") + example("dependent-behavior", "first-behavior")
        cases.append(("valid_graph", not validate_tasks(parse_tasks(valid), requirements, root)))
        cases.append(("ready_only", len(ready_tasks(parse_tasks(valid))) == 1))
        for name, content, expected in [
            ("unknown_dependency", example("first-behavior", "absent-behavior"), "unknown dependency"),
            ("cycle", example("first-behavior", "dependent-behavior") + example("dependent-behavior", "first-behavior"), "Dependency cycle"),
            ("duplicate_task", example("first-behavior") * 2, "Duplicate task"),
            ("false_completion", example("first-behavior", complete=True), "completed without evidence"),
            ("invalid_requirement", example("first-behavior").replace("sample / Observable behavior", "sample / Missing behavior"), "invalid capability requirement"),
        ]:
            failures = validate_tasks(parse_tasks(content), requirements, root)
            cases.append((name, any(expected in failure for failure in failures)))
        missing = valid.replace("**Verify:** Observe the accepted and rejected outcomes\n\n", "", 1)
        try:
            parse_tasks(missing)
            cases.append(("missing_field", False))
        except ValueError:
            cases.append(("missing_field", True))
        coverage_errors = validate_tasks(parse_tasks(valid), {"sample": {"Observable behavior", "Other behavior"}}, root)
        cases.append(("uncovered_requirement", any("uncovered requirement Other behavior" in error for error in coverage_errors)))
        source = root / "sample.rs"
        source.write_text("// " + "M" + str(71) + ": delivery note\n", encoding="utf-8")
        cases.append(("label_leak", bool(scan_implementation(root))))
        source.write_text("// Preserve the unreturned records before advancing.\n", encoding="utf-8")
        cases.append(("semantic_comment", not scan_implementation(root)))
        from unittest.mock import patch
        excluded = {"target", ".git", ".venv", "openspec", "__pycache__"}
        for name in excluded:
            (root / name / "nested").mkdir(parents=True)
            (root / name / "nested" / "ignored.rs").write_bytes(b"\xff")
        original_scandir = os.scandir

        def guarded_scandir(path):
            if set(Path(path).relative_to(root).parts) & excluded:
                raise AssertionError("Excluded directory was traversed")
            return original_scandir(path)

        with patch("os.scandir", guarded_scandir):
            cases.append(("pruned_source_traversal", list(source_files(root, excluded)) == [source]))
            cases.append(("pruned_implementation_scan", not scan_implementation(root)))
        _, invalid, _ = outside_fences("```json\n{broken}\n```\n")
        cases.append(("invalid_json_example", bool(invalid)))
        _, unclosed, _ = outside_fences("```rust\nfn main() {}\n")
        cases.append(("unclosed_fence", bool(unclosed)))
        skill = root / "SKILL.md"
        skill.write_text("---\nname: sample-skill\ndescription: Use for the sample workflow.\n---\nRead only relevant inputs.\n", encoding="utf-8")
        cases.append(("valid_skill_metadata", not check_skill(skill)))
        skill.write_text("---\nname: invalid name\ndescription: Use for the sample workflow.\n---\n", encoding="utf-8")
        cases.append(("invalid_skill_metadata", bool(check_skill(skill))))
    failed = [name for name, passed in cases if not passed]
    return {"ok": not failed, "checks": len(cases), "failed": failed}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "status", "next", "show", "coverage", "selftest"))
    parser.add_argument("task", nargs="?")
    parser.add_argument("--limit", type=int, default=5)
    args = parser.parse_args()
    if args.limit < 1 or args.limit > 100:
        parser.error("--limit must be between 1 and 100")
    if args.command == "selftest":
        result = selftest()
    else:
        checked = check(ROOT)
        if not checked["ok"] or args.command == "check":
            emit(checked)
            return 0 if checked["ok"] else 1
        tasks = parse_tasks((ROOT / CHANGE / "tasks.md").read_text(encoding="utf-8"))
        ready = ready_tasks(tasks)
        if args.command == "status":
            result = {key: checked[key] for key in ("tasks", "complete", "capabilities", "requirements")}
            result.update({"ready": len(ready), "runtime_status": "No runtime acceptance inferred from planning status"})
        elif args.command == "next":
            result = {"ready": [{"name": task["name"], "goal": task["fields"]["Goal"]} for task in ready[:args.limit]], "more": len(ready) > args.limit}
        elif args.command == "show":
            task = next((task for task in tasks if task["name"] == args.task), None)
            if task is None:
                emit({"ok": False, "error": "Unknown semantic task name"})
                return 1
            result = task
        else:
            result = {}
            for task in tasks:
                result.setdefault(task["fields"]["Spec"], []).append(task["name"])
    emit(result)
    return 0 if result.get("ok", True) else 1


if __name__ == "__main__":
    sys.exit(main())
