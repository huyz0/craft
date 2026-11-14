"""Check that composed acceptance remains an explicit debug-only development feature."""

import json
from pathlib import Path
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
FEATURE = "synthetic-acceptance"
REFUSAL = "synthetic-acceptance requires debug assertions and cannot be distributed"


def main():
    try:
        manifests = {
            name: tomllib.loads((ROOT / "crates" / name / "Cargo.toml").read_text())
            for name in ["craft-cli", "craft-runtime"]
        }
        for name, manifest in manifests.items():
            if manifest["features"].get("default", []):
                raise ValueError(f"{name}: review changed default features before accepting fixture isolation")
        for name in ["confluence_context_host", "jira_create_host", "jira_project_list_host", "jira_issue_reads_host", "jira_project_context_host", "jira_field_reads_host", "jira_issue_context_host", "jira_permissions_host", "jira_issue_mutation_host"]:
            target = next(item for item in manifests["craft-cli"]["test"] if item["name"] == name)
            if target.get("required-features") != [FEATURE]:
                raise ValueError("Composed integration target must require its explicit development feature")
        tree = subprocess.check_output(
            ["cargo", "tree", "-p", "craft-cli", "--locked", "--offline", "--edges", "normal", "--prefix", "none", "--format", "{p}"],
            cwd=ROOT, text=True, encoding="utf-8",
        )
        if any(line.startswith(("rcgen ", "craft-test-support ")) for line in tree.splitlines()):
            raise ValueError("Fixture-only dependencies reached the normal CLI dependency tree")
        result = subprocess.run(
            ["cargo", "rustc", "-p", "craft-runtime", "--lib", "--features", FEATURE, "--locked", "--offline", "--", "-C", "debug-assertions=no"],
            cwd=ROOT, text=True, encoding="utf-8", capture_output=True,
        )
        log = ROOT / "target" / "synthetic-acceptance-nondebug-output.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text(result.stdout + result.stderr, encoding="utf-8")
        if result.returncode == 0 or REFUSAL not in result.stderr:
            raise ValueError(f"Expected non-debug feature refusal missing; inspect {log.relative_to(ROOT)}")
        print(json.dumps({"ok": True, "explicit_feature": True, "normal_tree_isolated": True, "nondebug_refused": True}))
        return 0
    except (OSError, ValueError, KeyError, StopIteration, subprocess.CalledProcessError) as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
