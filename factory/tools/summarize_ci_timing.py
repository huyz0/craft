"""Report timing for one Factory GitHub Actions run attempt."""

import argparse
from datetime import datetime
import json
import sys


SCOPE = "Select Factory checks"
WINDOWS = "Offline checks (windows-latest)"
LINUX = "Shared PostgreSQL queue"
TRACKED = (SCOPE, WINDOWS, LINUX)


def timestamp(value):
    if not isinstance(value, str):
        return None
    try:
        result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return result if result.tzinfo is not None else None


def seconds(start, end):
    if start is None or end is None or end < start:
        return None
    return (end - start).total_seconds()


def duration(item):
    if item.get("status") != "completed" or item.get("conclusion") == "skipped":
        return None
    return seconds(timestamp(item.get("started_at")), timestamp(item.get("completed_at")))


def display(value):
    return f"{value:g} s" if value is not None else "unavailable"


def cell(value):
    # The API can return arbitrary step names. Keep them inside the Markdown cell as plain text.
    text = " ".join(str(value).split())
    return "".join(f"&#{ord(char)};" if char in "&<>|`\\*_[]!" else char for char in text)


def render(payload, run_id, attempt, lane, cache_states):
    jobs = payload.get("jobs") if isinstance(payload, dict) else None
    if not isinstance(jobs, list) or len(jobs) != payload.get("total_count"):
        raise ValueError("expected a complete page of workflow jobs")
    tracked = {}
    for job in jobs:
        if not isinstance(job, dict):
            raise ValueError("invalid workflow job")
        name = job.get("name")
        if not isinstance(name, str) or name not in TRACKED:
            continue
        if job.get("run_id") != run_id or name in tracked:
            raise ValueError("workflow job has a different run ID or a duplicate name")
        tracked[name] = job
    lines = [
        "## Factory CI timing", "",
        f"Run {run_id}, attempt {attempt}, {lane} lane. Durations come from GitHub job and step timestamps.", "",
        "| Job | Result | Runtime | After scope job | Cargo cache |",
        "| --- | --- | ---: | ---: | --- |",
    ]
    scope = tracked.get(SCOPE, {})
    scope_end = timestamp(scope.get("completed_at")) if duration(scope) is not None else None
    for name in TRACKED:
        job = tracked.get(name, {})
        outcome = job.get("conclusion") or job.get("status") or "unavailable"
        runtime = duration(job)
        gap = seconds(scope_end, timestamp(job.get("started_at"))) if name != SCOPE and runtime is not None else None
        cache = cache_states.get(name, "unavailable") if runtime is not None else "unavailable"
        lines.append(f"| {name} | {cell(outcome)} | {display(runtime)} | {display(gap)} | {cache} |")
    for name in TRACKED:
        steps = tracked.get(name, {}).get("steps")
        if not isinstance(steps, list):
            continue
        measured = [(duration(step), step) for step in steps if isinstance(step, dict)]
        slowest = sorted((pair for pair in measured if pair[0] is not None), key=lambda pair: -pair[0])[:5]
        if not slowest:
            continue
        lines.extend(["", f"### Slowest measured steps for {name}", "",
                      "| Step | Result | Runtime |", "| --- | --- | ---: |"])
        for elapsed, step in slowest:
            lines.append(f"| {cell(step.get('name', 'Unnamed step'))} | "
                         f"{cell(step.get('conclusion') or 'unavailable')} | {display(elapsed)} |")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", required=True, type=int)
    parser.add_argument("--attempt", required=True, type=int)
    parser.add_argument("--lane", required=True, choices=("docs", "full"))
    cache_choices = ("hit", "no exact hit", "unavailable")
    parser.add_argument("--windows-cache", choices=cache_choices, default="unavailable")
    parser.add_argument("--linux-cache", choices=cache_choices, default="unavailable")
    args = parser.parse_args()
    try:
        print(render(json.load(sys.stdin), args.run_id, args.attempt, args.lane,
                     {WINDOWS: args.windows_cache, LINUX: args.linux_cache}), end="")
    except ValueError as error:
        parser.exit(1, f"Factory timing: {error}\n")


if __name__ == "__main__":
    main()
