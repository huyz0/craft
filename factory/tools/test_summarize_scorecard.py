import csv
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

import summarize_scorecard as scorecard


EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "pilot-scorecard.csv"


class ScorecardTest(unittest.TestCase):
    def test_rehearsal_is_not_acceptance_and_missing_values_remain_unknown(self):
        paths = scorecard.summarize(EXAMPLE)["paths"]
        factory = paths["Factory"]
        self.assertEqual(factory["review_kinds"], {"simulated": 1})
        self.assertEqual(factory["accepted_changes"], 0)
        self.assertEqual(factory["measurements"]["started_attempts"]["observed_total"], 2)
        self.assertIsNone(factory["human_active_minutes"]["observed_total"])
        self.assertIsNone(factory["metered_cost"]["per_accepted_change"])
        self.assertEqual(paths["Ordinary"]["cases"], 0)

    def report(self, *rows):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "scorecard.csv"
            with path.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.DictWriter(stream, sorted(scorecard.COLUMNS))
                writer.writeheader()
                for index, row in enumerate(rows):
                    writer.writerow({"comparison_path": "Factory", "work_item": f"WORK-{index}",
                                     "task_class": "Repair", "review_kind": "human", **row})
            return scorecard.summarize(path)["paths"]["Factory"]

    def test_all_attempt_cost_counts_abandoned_work_and_human_measurements(self):
        accepted = {"outcome": "accepted", "acceptance_evidence": "fixture:approved",
                    "metered_cost": "0.30", "currency": "USD", "elapsed_minutes": "9",
                    "started_attempts": "2", "finished_attempts": "2",
                    **dict.fromkeys(scorecard.HUMAN, "0.5")}
        abandoned = {"outcome": "abandoned", "metered_cost": "0.20", "currency": "USD",
                     "started_attempts": "1", "abandoned_attempts": "1"}
        report = self.report(accepted, abandoned)
        self.assertEqual(report["outcomes"], {"accepted": 1, "abandoned": 1})
        self.assertEqual(report["task_classes"]["Repair"]["measurements"]["started_attempts"]["observed_total"], 3)
        self.assertEqual(report["human_active_minutes"],
                         {"recorded_cases": 1, "missing_cases": 1, "observed_total": "2.0"})
        self.assertEqual(report["metered_cost"]["per_accepted_change"], {"currency": "USD", "value": "0.50"})
        for mixed in (abandoned | {"currency": "EUR"}, abandoned | {"currency": "", "metered_cost": ""}):
            self.assertIsNone(self.report(accepted, mixed)["metered_cost"]["per_accepted_change"])

    def test_invalid_or_duplicate_records_are_rejected(self):
        for row, error in (
            ({"outcome": "accepted", "review_kind": "simulated", "acceptance_evidence": "fixture"},
             "requires human review"),
            ({"outcome": "abandoned", "started_attempts": "1", "abandoned_attempts": "2"},
             "cannot exceed started attempts"),
            ({"outcome": "abandoned", "metered_cost": "0.00"}, "currency together"),
        ):
            with self.subTest(row=row), self.assertRaisesRegex(ValueError, f"CSV line 2: .*{error}"):
                self.report(row)
        duplicate = {"outcome": "in progress", "work_item": "WORK-42"}
        with self.assertRaisesRegex(ValueError, "CSV line 3: duplicate work item"):
            self.report(duplicate, duplicate)
