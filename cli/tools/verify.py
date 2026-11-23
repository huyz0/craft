"""Run local development gates without claiming native or live-provider qualification."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import dependencies
import plan
import verification_scope


ROOT = Path(__file__).resolve().parents[1]
STATE = ROOT / "target" / "verification-state.json"
PROGRESS = ROOT / "target" / "verification-progress.json"


def environment_digest():
    value = [sys.executable, sys.version, sys.platform, str(ROOT), sorted(os.environ.items())]
    return hashlib.sha256(json.dumps(value, ensure_ascii=True).encode()).hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as output:
        temporary = Path(output.name)
        json.dump(value, output, indent=2)
        output.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def checkpoint_passes(value, hashes, environment):
    candidates = value["passed"]
    previous = value["hashes"]
    if (value["format"] != 1 or value["environment"] != environment
            or not isinstance(previous, dict)
            or not all(isinstance(path, str) and isinstance(digest, str)
                       for path, digest in previous.items())
            or not isinstance(candidates, list)
            or not all(isinstance(command, list) and command
                       and all(isinstance(arg, str) for arg in command) for command in candidates)):
        return []
    changed = changed_paths(hashes, previous)
    if not changed:
        return candidates
    # Only these prose paths are known not to feed the compiled Rust checks.
    # Other commands can read planning documents, so retain no tooling passes.
    if not all(path in {"STATE.md", "HISTORY.md"}
               or path.startswith("docs/") and path.endswith(".md") for path in changed):
        return []
    return [command for command in candidates
            if command[:2] in [["cargo", "fmt"], ["cargo", "clippy"], ["cargo", "test"]]]


def run_checks(commands, hashes, fresh=False, progress=PROGRESS, observe=None, cwd=ROOT, environment=None):
    observe = source_hashes if observe is None else observe
    environment = environment_digest() if environment is None else environment
    passed = []
    if not fresh:
        try:
            value = json.loads(progress.read_text(encoding="utf-8"))
            passed = checkpoint_passes(value, hashes, environment)
        except (OSError, ValueError, KeyError, TypeError):
            pass
    executed = reused = 0
    observed_changes = set()

    def stable_now():
        changes = changed_paths(hashes, observe())
        observed_changes.update(changes)
        return not changes

    for command in commands:
        stable = stable_now()
        if stable and command in passed:
            reused += 1
            print(json.dumps({"reused": command}), flush=True)
            continue
        subprocess.run(command, cwd=cwd, check=True)
        executed += 1
        # Finish the selected matrix after drift so the caller can retain stable inputs.
        after_stable = stable_now()
        if stable and after_stable:
            passed.append(command)
            write_json(progress, {"format": 1, "hashes": hashes, "environment": environment, "passed": passed})
    return executed, reused, sorted(observed_changes)


def changed_paths(current, previous):
    return sorted(path for path in set(current) | set(previous) if current.get(path) != previous.get(path))


def stable_baseline(initial, final, previous):
    stable = dict(initial)
    for path in changed_paths(initial, final):
        if path in previous:
            stable[path] = previous[path]
        else:
            stable.pop(path, None)
    return stable


def checked_baseline(initial, final, previous, observed_changes):
    unstable = changed_paths(initial, final)
    all_changes = sorted(set(observed_changes) | set(unstable))
    if all_changes:
        if not unstable or verification_scope.select(all_changes)["scope"] != "planning":
            raise ValueError("Inputs changed during verification; qualification remains pending.")
    return stable_baseline(initial, final, previous), unstable


def source_hashes():
    paths = set()
    for name in ["crates", "tools", "schemas", "skills", "openspec", "docs", ".cargo", ".github", ".agents"]:
        directory = ROOT / name
        if directory.exists():
            paths.update(path for path in directory.rglob("*") if path.is_file()
                         and "__pycache__" not in path.parts and path.suffix != ".pyc")
    paths.update(ROOT / name for name in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "AGENTS.md", "SDD.md", "STATE.md"])
    paths.update(ROOT.glob("*.md"))
    return {path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(paths) if path.exists()}


def previous_hashes():
    try:
        value = json.loads(STATE.read_text(encoding="utf-8"))
        if value["format"] == 1 and isinstance(value["hashes"], dict):
            return value["hashes"]
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return None


def save_hashes(hashes, scope, commands):
    write_json(STATE, {"format": 1, "hashes": hashes, "scope": scope, "commands": commands})


def focused_commands(selection):
    checks = set(selection["checks"])
    commands = [[sys.executable, "tools/plan.py", "check"]]
    runtime_tests = []
    if "verifier" in checks:
        commands.append([sys.executable, "tools/verify.py", "--selftest"])
    if "planning-selftest" in checks:
        commands.append([sys.executable, "tools/plan.py", "selftest"])
    if "dependencies" in checks:
        commands.append([sys.executable, "tools/dependencies.py"])
    if "providers" in checks:
        commands.extend([[sys.executable, "tools/providers.py", mode] for mode in ["check", "selftest"]])
        commands.append(["cargo", "run", "--quiet", "--locked", "--offline", "-p", "craft-cli", "--example", "provider_routes"])
    if "contracts" in checks:
        commands.extend([[sys.executable, "tools/contracts.py", mode] for mode in ["check", "selftest"]])
    if "runtime-skill" in checks:
        commands.extend([[sys.executable, "tools/runtime_skill.py", mode] for mode in ["check", "selftest"]])
        commands.append([sys.executable, "-m", "unittest", "discover", "-s", "tools", "-p", "skills*_test.py"])
        commands.append([sys.executable, "tools/release_test.py"])
    if "bundle" in checks:
        commands.append([sys.executable, "tools/bundle_test.py"])
    if "jira-conformance" in checks:
        commands.append([sys.executable, "tools/jira_conformance.py", "selftest"])
    packages = selection["packages"]
    if packages:
        arguments = [part for package in packages for part in ["-p", package]]
        commands.extend([
            ["cargo", "fmt", "--all", "--check"],
            ["cargo", "clippy", *arguments, "--all-targets", "--locked", "--offline", "--", "-D", "warnings"],
        ])
        runtime_tests.append(["cargo", "test", *arguments, "--locked", "--offline"])
    if selection["hosts"]:
        arguments = [part for host in selection["hosts"] for part in ["--test", host]]
        commands.append(["cargo", "clippy", "-p", "craft-cli", "--all-targets", "--features", "synthetic-acceptance", "--locked", "--offline", "--", "-D", "warnings"])
        runtime_tests.append(["cargo", "test", "-p", "craft-cli", *arguments, "--features", "synthetic-acceptance", "--locked", "--offline"])
    commands.extend(runtime_tests)
    if "isolation" in checks:
        commands.append([sys.executable, "tools/synthetic-acceptance.py"])
    return commands


def layer_errors(records):
    local = {record["name"] for record in records if record["source"] == "workspace"}
    errors = []
    for record in records:
        name = record["name"]
        if name not in local:
            continue
        for dependency in record["dependencies"]:
            target = dependency["package"]
            production = bool(set(dependency["kinds"]) & {"normal", "build"})
            if not production:
                continue
            if target == "craft-test-support":
                errors.append(f"{name}: test support must be dev-only")
            if name == "craft-core" and target in local:
                errors.append(f"{name}: cannot depend on workspace infrastructure {target}")
            if name == "craft-core" and target in {"reqwest", "tokio", "keyring-core", "rusqlite"}:
                errors.append(f"{name}: cannot own runtime I/O dependency {target}")
            if name == "craft-platform" and target in local:
                errors.append(f"{name}: native leaf cannot depend on {target}")
            if name == "craft-runtime" or name.startswith("craft-provider-"):
                allowed = {"craft-core", "craft-platform"} if name == "craft-runtime" else {"craft-core"}
                if target in local and target not in allowed:
                    errors.append(f"{name}: cannot depend on composition layer {target}")
            if name == "craft-workflows" and target in {"craft-runtime", "craft-cli"}:
                errors.append(f"{name}: must use core invocation ports instead of {target}")
    return errors


def checkpoint_selftest(directory):
    root = Path(directory)
    progress = root / "progress.json"
    counter = root / "counter"
    success = [sys.executable, "-c", "from pathlib import Path; p=Path('counter'); "
               "p.write_text(p.read_text()+'x' if p.exists() else 'x')"]
    failure = [sys.executable, "-c", "raise SystemExit(7)"]
    hashes = {"source": "initial"}
    options = {"progress": progress, "observe": lambda: dict(hashes), "cwd": root,
               "environment": "same-environment"}
    try:
        run_checks([success, failure], dict(hashes), **options)
        raise AssertionError("Failed subprocess was accepted")
    except subprocess.CalledProcessError as error:
        assert error.returncode == 7
    assert json.loads(progress.read_text())["passed"] == [success]
    assert run_checks([success], dict(hashes), **options) == (0, 1, [])
    assert counter.read_text() == "x"
    hashes["source"] = "changed"
    assert run_checks([success], dict(hashes), **options) == (1, 0, [])
    assert run_checks([success], dict(hashes), fresh=True, **options) == (1, 0, [])
    options["environment"] = "different-environment"
    assert run_checks([success], dict(hashes), **options) == (1, 0, [])
    progress.write_text('{"format":1,"passed":[null]}')
    assert run_checks([success], dict(hashes), **options) == (1, 0, [])
    assert counter.read_text() == "xxxxx"
    before = progress.read_bytes()
    unstable = [dict(hashes), {"source": "concurrent-change"}]
    options["observe"] = lambda: unstable.pop(0)
    assert run_checks([success], dict(hashes), fresh=True, **options) == (1, 0, ["source"])
    assert progress.read_bytes() == before
    initial = {"source": "changed", "STATE.md": "before"}
    final = {**initial, "STATE.md": "after"}
    # A documentation edit must not abort the remaining selected commands.
    observations = [initial, initial, initial, final, final, final]
    options["observe"] = lambda: observations.pop(0)
    later = [*success, "later"]
    assert run_checks([success, later, failure[:2] + ["pass"]], initial,
                      fresh=True, **options) == (3, 0, ["STATE.md"])
    assert json.loads(progress.read_text())["passed"] == [success]
    retained, pending = checked_baseline(initial, final, {"STATE.md": "older"}, ["STATE.md"])
    assert pending == ["STATE.md"]
    assert changed_paths(final, retained) == ["STATE.md"]
    observations = [initial, final, initial, initial]
    options["observe"] = lambda: observations.pop(0)
    assert run_checks([success, later], initial, fresh=True, **options) == (2, 0, ["STATE.md"])
    for last in [initial, {**initial, "crates/craft-core/src/lib.rs": "changed"}]:
        try:
            checked_baseline(initial, last, {}, ["STATE.md"])
            raise AssertionError("Observed source drift qualified untested inputs")
        except ValueError:
            pass
    temporary_code = {**final, "crates/craft-core/src/lib.rs": "temporary"}
    observations = [initial, temporary_code, final, final]
    options["observe"] = lambda: observations.pop(0)
    _, _, observed = run_checks([success, later], initial, fresh=True, **options)
    assert observed == ["STATE.md", "crates/craft-core/src/lib.rs"]
    try:
        checked_baseline(initial, final, {}, observed)
        raise AssertionError("Reverted implementation drift qualified through a remaining doc edit")
    except ValueError:
        pass


def prose_checkpoint_selftest(directory):
    from unittest.mock import patch

    progress = Path(directory) / "prose-progress.json"
    compiled = [["cargo", "fmt", "--all", "--check"],
                ["cargo", "clippy", "--workspace"], ["cargo", "test", "--workspace"]]
    tooling = [sys.executable, "tools/plan.py", "check"]
    commands = [*compiled, tooling]
    initial = {"crates/craft-cli/src/lib.rs": "same", "STATE.md": "before"}
    current = {**initial, "STATE.md": "after", "docs/evidence/result.md": "new"}
    value = {"format": 1, "hashes": initial, "environment": "same", "passed": commands}
    assert checkpoint_passes(value, current, "same") == compiled
    assert checkpoint_passes(value, {"crates/craft-cli/src/lib.rs": "same"}, "same") == compiled
    assert checkpoint_passes(value, current, "different") == []
    assert checkpoint_passes({**value, "hashes": []}, current, "same") == []
    assert checkpoint_passes({**value, "hashes": {"STATE.md": None}}, current, "same") == []
    assert checkpoint_passes({**value, "passed": [None]}, current, "same") == []
    for path in ["crates/craft-cli/src/lib.rs", "tools/verify.py", "Cargo.lock",
                 "skills/craft-cloud/SKILL.md", "openspec/specs/project-workflow/spec.md",
                 "docs/evidence/dependency-inventory.json", "crates/fixture.md", "unknown.md"]:
        assert checkpoint_passes(value, {**current, path: "changed"}, "same") == []
    write_json(progress, value)
    options = {"progress": progress, "observe": lambda: dict(current), "environment": "same"}
    # Exercise scheduling and checkpoint rollover without launching Cargo.
    with patch.object(subprocess, "run", side_effect=subprocess.CalledProcessError(7, tooling)) as runner:
        try:
            run_checks(commands, current, **options)
            raise AssertionError("Failed tooling was accepted after prose-only reuse")
        except subprocess.CalledProcessError as error:
            assert error.returncode == 7
        runner.assert_called_once_with(tooling, cwd=ROOT, check=True)
    assert json.loads(progress.read_text()) == value
    with patch.object(subprocess, "run") as runner:
        assert run_checks(commands, current, **options) == (1, 3, [])
        runner.assert_called_once_with(tooling, cwd=ROOT, check=True)
    assert json.loads(progress.read_text())["hashes"] == current
    with patch.object(subprocess, "run") as runner:
        assert run_checks(commands, current, **options) == (0, 4, [])
        runner.assert_not_called()
    with patch.object(subprocess, "run") as runner:
        assert run_checks(commands, current, fresh=True, **options) == (4, 0, [])
        assert runner.call_count == 4


def selftest():
    verification_scope.selftest()
    scheduled = focused_commands({"checks": [], "packages": ["craft-cli"], "hosts": ["jira_create_host"]})
    tests = [index for index, command in enumerate(scheduled) if command[:2] == ["cargo", "test"]]
    lints = [index for index, command in enumerate(scheduled) if command[:2] == ["cargo", "clippy"]]
    assert len(tests) == len(lints) == 2 and max(lints) < min(tests)
    initial = {"crates/craft-cli/src/policies.rs": "tested", "STATE.md": "before"}
    final = {**initial, "STATE.md": "after"}
    recorded = stable_baseline(initial, final, {"STATE.md": "older"})
    assert verification_scope.select(changed_paths(final, recorded))["scope"] == "planning"
    assert stable_baseline({"new.rs": "before"}, {"new.rs": "after"}, {}) == {}
    assert changed_paths({"removed.rs": "before"}, {}) == ["removed.rs"]
    records = [
        {"name": "craft-core", "source": "workspace", "dependencies": []},
        {"name": "craft-runtime", "source": "workspace", "dependencies": []},
    ]
    if layer_errors(records):
        raise ValueError("Independent core rejected")
    records[0]["dependencies"] = [{"package": "craft-runtime", "kinds": ["normal"]}]
    if not layer_errors(records):
        raise ValueError("Core-layer negative control escaped")
    records[0]["dependencies"] = [{"package": "craft-test-support", "kinds": ["dev"]}]
    if layer_errors(records):
        raise ValueError("Dev-only support rejected")
    records[0]["dependencies"][0]["kinds"] = ["dev", "build"]
    if not layer_errors(records):
        raise ValueError("Build-time support negative control escaped")
    native_records = [
        {"name": "craft-core", "source": "workspace", "dependencies": []},
        {"name": "craft-platform", "source": "workspace", "dependencies": []},
        {"name": "craft-runtime", "source": "workspace", "dependencies": [
            {"package": "craft-platform", "kinds": ["normal"]},
            {"package": "craft-core", "kinds": ["normal"]},
        ]},
        {"name": "craft-provider-example", "source": "workspace", "dependencies": [
            {"package": "craft-core", "kinds": ["normal"]},
        ]},
    ]
    if layer_errors(native_records):
        raise ValueError("Native leaf dependency direction rejected")
    native_records[1]["dependencies"] = [{"package": "craft-core", "kinds": ["normal"]}]
    if not layer_errors(native_records):
        raise ValueError("Native leaf negative control escaped")
    native_records[1]["dependencies"] = []
    native_records[3]["dependencies"] = [{"package": "craft-platform", "kinds": ["normal"]}]
    if not layer_errors(native_records):
        raise ValueError("Provider native bypass negative control escaped")
    temporary_root = ROOT / "target" / "tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="craft-quality-", dir=temporary_root) as directory:
        root = Path(directory)
        checkpoint_selftest(directory)
        prose_checkpoint_selftest(directory)
        source = root / "sample.rs"
        source.write_text("// Keep confirmed resources before reconciliation.\n", encoding="utf-8")
        if plan.scan_implementation(root):
            raise ValueError("Semantic comment rejected")
        source.write_text("// " + "TASK" + "_123" + " complete\n", encoding="utf-8")
        if not plan.scan_implementation(root):
            raise ValueError("Artificial-label negative control escaped")
    print(json.dumps({"ok": True, "checks": 11, "impact_selection": "passed",
                      "command_resume": "passed", "prose_resume": "passed"}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--full", action="store_true", help="Force complete local qualification for release, broad changes or explicit review")
    parser.add_argument("--plan", action="store_true", help="Show selected checks without running them or advancing verification state")
    arguments = parser.parse_args()
    try:
        if arguments.selftest:
            selftest()
            return 0
        hashes = source_hashes()
        previous = previous_hashes()
        changed = changed_paths(hashes, previous or {})
        selection = verification_scope.select(changed, arguments.full or previous is None)
        full_commands = [
            [sys.executable, "tools/plan.py", "check"],
            [sys.executable, "tools/runtime_skill.py", "check"],
            [sys.executable, "tools/runtime_skill.py", "selftest"],
            [sys.executable, "-m", "unittest", "discover", "-s", "tools", "-p", "skills*_test.py"],
            [sys.executable, "tools/release_test.py"],
            [sys.executable, "tools/bundle_test.py"],
            [sys.executable, "tools/dependencies.py"],
            [sys.executable, "tools/providers.py", "check"],
            [sys.executable, "tools/providers.py", "selftest"],
            [sys.executable, "tools/jira_conformance.py", "selftest"],
            ["cargo", "run", "--quiet", "--locked", "--offline", "-p", "craft-cli", "--example", "provider_routes"],
            ["cargo", "fmt", "--all", "--check"],
            [sys.executable, "tools/contracts.py", "check"],
            [sys.executable, "tools/contracts.py", "selftest"],
            ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
            ["cargo", "clippy", "--workspace", "--all-targets", "--features", "craft-cli/synthetic-acceptance", "--locked", "--offline", "--", "-D", "warnings"],
            ["cargo", "test", "--workspace", "--locked"],
            ["cargo", "test", "-p", "craft-cli", "--test", "confluence_context_host", "--test", "jira_create_host", "--test", "jira_project_list_host", "--test", "jira_issue_reads_host", "--test", "jira_project_context_host", "--test", "jira_field_reads_host", "--test", "jira_issue_context_host", "--test", "jira_permissions_host", "--test", "jira_issue_mutation_host", "--features", "synthetic-acceptance", "--locked", "--offline"],
            ["cargo", "test", "-p", "craft-runtime", "--lib", "--features", "synthetic-acceptance", "--locked", "--offline"],
            [sys.executable, "tools/synthetic-acceptance.py"],
        ]
        commands = full_commands if selection["scope"] == "full" else focused_commands(selection) if selection["scope"] != "unchanged" else []
        print(json.dumps({"scope": selection["scope"], "changed_count": len(changed),
                          "changed": changed[:50], "packages": selection["packages"],
                          "hosts": selection["hosts"], "commands": commands,
                          "planned": arguments.plan}), flush=True)
        if arguments.plan:
            return 0
        if selection["scope"] == "full" or "dependencies" in selection["checks"]:
            errors = layer_errors(dependencies.inventory()["packages"])
            if errors:
                raise ValueError("; ".join(errors))
        executed, reused, observed_changes = run_checks(commands, hashes, fresh=arguments.full)
        final_hashes = source_hashes()
        baseline, unstable = checked_baseline(hashes, final_hashes, previous or {}, observed_changes)
        if unstable:
            save_hashes(baseline, "partial", commands)
            raise ValueError("Inputs changed during verification: " + ", ".join(unstable[:10])
                             + ". Only stable inputs were recorded; the next run selects the remaining changes.")
        if commands:
            save_hashes(hashes, selection["scope"], commands)
        print(json.dumps({"ok": True, "scope": selection["scope"], "checks_run": executed,
                          "checks_reused": reused,
                          "full_qualification": selection["scope"] == "full" and reused == 0,
                          "native_and_live_checks": "not_run"}))
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
