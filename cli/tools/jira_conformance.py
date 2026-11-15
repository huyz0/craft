"""Run the composed Jira host suites and report which outcomes each public Jira command reached.

Every envelope those suites observe is validated against the command's published response
schema (see crates/craft-cli/tests/support/conformance.rs) and recorded as
`<command>__<outcome>`. This tool requires, for every Jira command the contract publishes:

- a validated success envelope;
- a validated permission refusal (native permission, local policy or authentication);
- for reads, a validated bound or transient failure (deadline, cancellation, rate limit,
  unavailable service or output budget), because reads may retry and must stop;
- for writes, a validated unknown outcome and a validated refusal to send again.

Commands whose host results are internal tuples rather than envelopes are reported as
missing instead of being assumed covered.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "schemas/local-contracts.json"
SUITES = [
    "jira_create_host",
    "jira_field_reads_host",
    "jira_issue_context_host",
    "jira_issue_mutation_host",
    "jira_issue_reads_host",
    "jira_permissions_host",
    "jira_project_context_host",
    "jira_project_list_host",
]
DENIAL = {"PERMISSION_DENIED", "POLICY_DENIED", "AUTH_REQUIRED"}
BOUNDED = {"DEADLINE_EXCEEDED", "CANCELLED", "RATE_LIMITED", "REMOTE_UNAVAILABLE", "OUTPUT_BUDGET_TOO_SMALL"}
UNKNOWN = {"WRITE_OUTCOME_UNKNOWN"}
NO_RESEND = {"OPERATION_ID_REUSED", "APPLY_REQUIRED"}


def jira_commands():
    operations = json.loads(CONTRACT.read_text(encoding="utf-8"))["operations"]
    return {
        command: entry["descriptor"]["effect"]
        for command, entry in operations.items()
        if command.startswith("jira.")
    }


def read_records(directory):
    outcomes = {}
    for path in Path(directory).iterdir():
        command, _, outcome = path.name.partition("__")
        tests = {line for line in path.read_text(encoding="utf-8").splitlines() if line}
        outcomes.setdefault(command, {})[outcome] = tests
    return outcomes


def requirements(effect):
    required = [("success", {"ok"}), ("permission refusal", DENIAL)]
    if effect == "read":
        required.append(("bounded failure", BOUNDED))
    else:
        required += [("unknown outcome", UNKNOWN), ("no resend", NO_RESEND)]
    return required


def evaluate(commands, outcomes):
    rows, gaps = [], []
    for command in sorted(commands):
        seen = outcomes.get(command, {})
        row = {"command": command, "effect": commands[command], "outcomes": sorted(seen)}
        for label, codes in requirements(commands[command]):
            if not codes & set(seen):
                gaps.append(f"{command}: no validated {label} ({' | '.join(sorted(codes))})")
        rows.append(row)
    return rows, gaps


def run_suites(directory, threads):
    environment = dict(os.environ, CRAFT_CONFORMANCE_RECORD=str(directory))
    command = ["cargo", "test", "-p", "craft-cli", "--features", "synthetic-acceptance", "--no-fail-fast"]
    for suite in SUITES:
        command += ["--test", suite]
    if threads:
        command += ["--", f"--test-threads={threads}"]
    return subprocess.run(command, cwd=ROOT, env=environment, check=False).returncode


def report(rows, gaps, as_json):
    if as_json:
        print(json.dumps({"commands": rows, "gaps": gaps}, indent=2))
        return
    for row in rows:
        print(f"{row['command']:<28} {row['effect']:<10} {', '.join(row['outcomes']) or '-'}")
    for gap in gaps:
        print(f"GAP {gap}")
    print(f"{len(rows)} commands, {len(gaps)} gaps")


def selftest():
    commands = {"jira.a.get": "read", "jira.a.create": "write"}
    complete = {
        "jira.a.get": {"ok": {"t"}, "POLICY_DENIED": {"t"}, "DEADLINE_EXCEEDED": {"t"}},
        "jira.a.create": {
            "ok": {"t"},
            "PERMISSION_DENIED": {"t"},
            "WRITE_OUTCOME_UNKNOWN": {"t"},
            "OPERATION_ID_REUSED": {"t"},
        },
    }
    assert evaluate(commands, complete)[1] == []
    missing = {"jira.a.get": {"ok": {"t"}}}
    gaps = evaluate(commands, missing)[1]
    assert any("jira.a.get: no validated permission refusal" in gap for gap in gaps)
    assert any("jira.a.create: no validated success" in gap for gap in gaps)
    with tempfile.TemporaryDirectory() as directory:
        Path(directory, "jira.a.get__ok").write_text("one\ntwo\n", encoding="utf-8")
        assert read_records(directory) == {"jira.a.get": {"ok": {"one", "two"}}}
    print("jira_conformance selftest passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("mode", choices=["run", "report", "selftest"])
    parser.add_argument("--records", help="Record directory; report reads it, run writes it")
    parser.add_argument("--threads", type=int, help="Test threads per suite")
    parser.add_argument("--json", action="store_true")
    arguments = parser.parse_args()
    if arguments.mode == "selftest":
        selftest()
        return 0
    if arguments.mode == "run":
        directory = Path(arguments.records or tempfile.mkdtemp(prefix="craft-jira-conformance-"))
        if directory.exists() and any(directory.iterdir()):
            print(f"{directory} is not empty; records must come from one run", file=sys.stderr)
            return 2
        directory.mkdir(parents=True, exist_ok=True)
        status = run_suites(directory, arguments.threads)
        if status:
            print("Jira host suites failed; outcomes below are partial", file=sys.stderr)
    else:
        if not arguments.records:
            parser.error("report needs --records")
        directory, status = Path(arguments.records), 0
    rows, gaps = evaluate(jira_commands(), read_records(directory))
    report(rows, gaps, arguments.json)
    return 1 if status or gaps else 0


if __name__ == "__main__":
    sys.exit(main())
