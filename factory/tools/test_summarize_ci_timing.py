import unittest

import summarize_ci_timing as timing


def job(name, start="2026-11-22T01:00:00Z", end="2026-11-22T01:01:00Z", result="success", **fields):
    return {"name": name, "started_at": start, "completed_at": end, "status": "completed",
            "conclusion": result, "run_id": 42, **fields}


def report(*jobs, lane="full"):
    return timing.render({"total_count": len(jobs), "jobs": list(jobs)}, 42, 2, lane,
                         {timing.WINDOWS: "hit", timing.LINUX: "no exact hit"})


class CiTimingTest(unittest.TestCase):
    def test_completed_steps_and_cache_are_separate_from_dispatch_delay(self):
        linux = job(timing.LINUX, "2026-11-22T01:01:08Z", "2026-11-22T01:03:08Z", steps=[
            job("Test | <fixture>", "2026-11-22T01:01:10Z", "2026-11-22T01:02:10Z", "failure"),
            job("Build", "2026-11-22T01:01:08Z", "2026-11-22T01:01:09Z"),
            job("Skipped step", result="skipped"),
        ])
        text = report(job(timing.SCOPE), linux, job(timing.WINDOWS, "2026-11-22T03:01:04+02:00",
                                                   "2026-11-22T03:02:04+02:00"))
        self.assertIn("Run 42, attempt 2, full lane", text)
        self.assertIn(f"| {timing.LINUX} | success | 120 s | 8 s | no exact hit |", text)
        self.assertIn(f"| {timing.WINDOWS} | success | 60 s | 4 s | hit |", text)
        self.assertIn("| Test &#124; &#60;fixture&#62; | failure | 60 s |", text)
        self.assertLess(text.index("| Test "), text.index("| Build "))
        self.assertNotIn("Skipped step", text)

    def test_skipped_incomplete_and_malformed_timestamps_are_unavailable(self):
        text = report(job(timing.SCOPE), job(timing.LINUX, result="skipped"),
                      job(timing.WINDOWS, end="2026-11-22T01:00:00"), lane="docs")
        self.assertIn(f"| {timing.LINUX} | skipped | unavailable | unavailable | unavailable |", text)
        self.assertIn(f"| {timing.WINDOWS} | success | unavailable | unavailable | unavailable |", text)
        self.assertIsNone(timing.duration(job(timing.WINDOWS, "2026-11-22T01:02:00Z")))

    def test_rejects_incomplete_data_and_wrong_run(self):
        with self.assertRaisesRegex(ValueError, "complete page"):
            timing.render({"total_count": 2, "jobs": [job(timing.SCOPE)]}, 42, 2, "docs", {})
        with self.assertRaisesRegex(ValueError, "different run ID"):
            report(job(timing.SCOPE) | {"run_id": 43})
        with self.assertRaisesRegex(ValueError, "duplicate name"):
            report(job(timing.SCOPE), job(timing.SCOPE))
