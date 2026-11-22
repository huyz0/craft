"""Run the required delivery workflows against an extracted Factory launcher."""

import argparse
import os
from pathlib import Path
import subprocess


SOURCE = Path(__file__).resolve().parents[1]
SCENARIOS = (
    "coordinator_drafts_only_reviewable_work_with_the_approved_constraints",
    "coordinator_profile_comparison_counts_attempts_and_binds_reviews",
    "coordinator_review_brief_highlights_changes_and_blocks_constraint_drift",
    "jira_capture_binds_reviewed_intent_and_prepares_the_supervised_order",
    "confluence_context_requires_approval_for_the_same_source_destination_and_excerpt",
    "github_failure_cycle_pins_inputs_and_saves_the_last_report",
    "github_failure_pins_the_selected_attempt_through_native_verification",
    "github_issue_capture_guards_supervised_delegation_and_draft_binding",
    "github_failure_poll_reuses_pause_admission_limit_and_receipts",
    "bitbucket_failure_pins_the_live_attempt_and_rejects_duplicate_admission",
    "interactive_handoff_reuses_pinned_skill_and_imports_scoped_repair_history",
)


def qualify_launcher(launcher, env=None):
    launcher = launcher.resolve(strict=True)
    if not launcher.is_file():
        raise SystemExit(f"The extracted launcher is not a file: {launcher}")
    test_env = (os.environ if env is None else env) | {"CRAFT_FACTORY_TEST_LAUNCHER": str(launcher)}
    result = subprocess.run(
        ["cargo", "test", "--locked", "--offline", "--test", "workflow", "--", "--exact", *SCENARIOS],
        cwd=SOURCE,
        env=test_env,
        stdout=subprocess.PIPE,
        text=True,
        check=False,
    )
    print(result.stdout, end="", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)
    passed = set(result.stdout.splitlines())
    missing = [scenario for scenario in SCENARIOS if f"test {scenario} ... ok" not in passed]
    if missing:
        raise SystemExit(f"Required launcher scenarios did not pass: {', '.join(missing)}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launcher", type=Path)
    qualify_launcher(parser.parse_args().launcher)
