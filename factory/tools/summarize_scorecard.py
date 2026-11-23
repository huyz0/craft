"""Summarize Factory evaluation cases without treating missing measurements as zero."""

import argparse
from collections import Counter, defaultdict
import csv
from decimal import Decimal, InvalidOperation
import json
from pathlib import Path


IDENTITY = ("comparison_path", "work_item", "task_class", "outcome", "review_kind")
COUNTS = ("started_attempts", "finished_attempts", "abandoned_attempts", "repair_cycles",
          "input_tokens", "cached_input_tokens", "output_tokens", "material_misses")
HUMAN = ("human_specification_minutes", "human_steering_minutes", "human_review_minutes", "human_repair_minutes")
MEASURES = ("queue_minutes", "elapsed_minutes", *HUMAN, "metered_cost")
COLUMNS = set((*IDENTITY, *COUNTS, *MEASURES, "currency", "acceptance_evidence"))
OUTCOMES = {"accepted", "verified draft", "rejected", "abandoned", "in progress"}
REVIEWS = {"human", "simulated"}


def read_cases(path):
    paths = {}
    seen = set()
    with path.open(encoding="utf-8-sig", newline="") as stream:
        reader = csv.DictReader(stream)
        headers = reader.fieldnames or []
        if set(headers) != COLUMNS or len(headers) != len(COLUMNS):
            raise ValueError("columns must match the Factory scorecard")
        for raw in reader:
            try:
                if None in raw or any(value is None for value in raw.values()):
                    raise ValueError("the row must contain one value per column")
                row = {field: value.strip() for field, value in raw.items()}
                lane, task, task_class, outcome, review = (row[field] for field in IDENTITY)
                if lane and not any(value for field, value in row.items() if field != "comparison_path"):
                    paths.setdefault(lane, [])
                    continue
                if not lane or not task or not task_class or outcome not in OUTCOMES or review not in REVIEWS:
                    raise ValueError("provide the path, work item, task class, and a supported outcome and review kind")
                if (lane, task) in seen:
                    raise ValueError("duplicate work item within a comparison path")
                if outcome == "accepted" and (review != "human" or not row["acceptance_evidence"]):
                    raise ValueError("accepted work requires human review and an acceptance evidence reference")
                for field in (*COUNTS, *MEASURES):
                    row[field] = number(field, row[field])
                validate_measurements(row)
                paths.setdefault(lane, []).append(row)
                seen.add((lane, task))
            except (InvalidOperation, ValueError) as error:
                raise ValueError(f"CSV line {reader.line_num}: {error}") from error
    return paths


def number(field, value):
    if not value:
        return None
    if len(value) > 32 or not all(char.isascii() and (char.isdigit() or char == ".") for char in value):
        raise ValueError(f"{field} must be a nonnegative number")
    try:
        result = Decimal(value)
    except InvalidOperation as error:
        raise ValueError(f"{field} must be a nonnegative number") from error
    if field in COUNTS:
        if result != result.to_integral_value():
            raise ValueError(f"{field} must be a whole number")
        return int(result)
    return result


def validate_measurements(row):
    started = row["started_attempts"]
    completed = [row[key] for key in ("finished_attempts", "abandoned_attempts") if row[key] is not None]
    if started is not None and sum(completed) > started:
        raise ValueError("finished and abandoned attempts cannot exceed started attempts")
    input_tokens = row["input_tokens"]
    cached_tokens = row["cached_input_tokens"]
    if input_tokens is not None and cached_tokens is not None and cached_tokens > input_tokens:
        raise ValueError("cached input tokens cannot exceed input tokens")
    currency = row["currency"]
    if (row["metered_cost"] is None) != (not currency) or (
        currency and (len(currency) != 3 or not currency.isascii() or not currency.isalpha() or not currency.isupper())
    ):
        raise ValueError("provide a metered cost and a three-letter uppercase currency together")


def format_value(value):
    return format(value, "f") if isinstance(value, Decimal) else value


def coverage(values):
    observed = [value for value in values if value is not None]
    return {"recorded_cases": len(observed), "missing_cases": len(values) - len(observed),
            "observed_total": format_value(sum(observed)) if observed else None}


def summary(rows):
    accepted = [row for row in rows if row["outcome"] == "accepted"]
    human_totals = [sum(row[field] for field in HUMAN) if all(row[field] is not None for field in HUMAN) else None
                    for row in rows]
    currency_totals = {}
    for row in rows:
        if row["metered_cost"] is not None:
            currency_totals[row["currency"]] = currency_totals.get(row["currency"], Decimal(0)) + row["metered_cost"]
    missing_cost = sum(row["metered_cost"] is None for row in rows)
    cost_per_acceptance = None
    if accepted and not missing_cost and len(currency_totals) == 1:
        currency, total = next(iter(currency_totals.items()))
        cost_per_acceptance = {"currency": currency, "value": format_value(total / len(accepted))}
    return {
        "cases": len(rows),
        "outcomes": dict(Counter(row["outcome"] for row in rows)),
        "review_kinds": dict(Counter(row["review_kind"] for row in rows)),
        "accepted_changes": len(accepted),
        "measurements": {field: coverage([row[field] for row in rows]) for field in (*COUNTS, *MEASURES[:-1])},
        "human_active_minutes": coverage(human_totals),
        "accepted_elapsed_minutes": coverage([row["elapsed_minutes"] for row in accepted]),
        "metered_cost": {"recorded_cases": len(rows) - missing_cost, "missing_cases": missing_cost,
                         "observed_totals": {currency: format_value(total) for currency, total in currency_totals.items()},
                         "per_accepted_change": cost_per_acceptance},
    }


def summarize(path):
    paths = {}
    for lane, rows in read_cases(path).items():
        classes = defaultdict(list)
        for row in rows:
            classes[row["task_class"]].append(row)
        paths[lane] = summary(rows) | {"task_classes": {key: summary(cases) for key, cases in classes.items()}}
    return {"schema_version": 1, "paths": paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scorecard", type=Path)
    parser.add_argument("--json", action="store_true", help="Print the full report with task-class breakdowns")
    args = parser.parse_args()
    try:
        report = summarize(args.scorecard)
        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True))
        else:
            for lane, row in report["paths"].items():
                human = row["human_active_minutes"]
                cost = row["metered_cost"]["per_accepted_change"]
                cost_text = f"{cost['value']} {cost['currency']}" if cost else "unavailable"
                print(f"{lane} | cases {row['cases']} | accepted {row['accepted_changes']} | "
                      f"human time recorded for {human['recorded_cases']} | metered cost per acceptance {cost_text}")
    except (OSError, UnicodeError, csv.Error, ValueError) as error:
        parser.exit(1, f"Factory scorecard: {error}\n")


if __name__ == "__main__":
    main()
