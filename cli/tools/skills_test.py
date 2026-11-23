import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

import skills


class SkillTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.project = Path(directory.name).resolve()
        self.bundle, self.compatibility = skills.compatible_bundle(skills.ROOT)

    def entries(self):
        targets = skills.targets(skills.HOSTS, "project", self.project)
        return list(skills.installations(self.bundle, targets))

    def test_workflow_search(self):
        cases = {
            "find a Jira issue and its related pull requests": "issue-to-pr.md",
            "review a plan before publishing Jira tickets": "plan-review.md",
            "recover an interrupted write": "recovery.md",
        }
        for query, reference in cases.items():
            with self.subTest(query=query):
                matches = skills.find(self.bundle, query, None)["matches"]
                self.assertEqual(matches[0]["reference"], f"references/{reference}")
                self.assertLessEqual(len(matches), 3)
        self.assertEqual(skills.find(self.bundle, "publish banana bread", None)["matches"], [])
        self.assertEqual(skills.find(self.bundle, "create and read a shopping list", None)["matches"], [])
        self.assertEqual(skills.find(self.bundle, "plan a birthday party", None)["matches"], [])
        self.assertEqual(skills.find(self.bundle, "renew a passport after a timeout", None)["matches"], [])
        self.assertEqual(skills.find(self.bundle, "show me whiteboards", None)["matches"][0]["reference"],
                         "references/whiteboards.md")
        self.assertEqual(skills.find(self.bundle, "Verify a published Confluence page", None)
                         ["matches"][0]["reference"], "references/verification.md")
        self.assertEqual(skills.find(self.bundle, "add a table to a Confluence page", None)
                         ["matches"][0]["reference"], "references/tables.md")
        self.assertEqual(skills.find(self.bundle, "recovery", "confluence-pages")["matches"], [])

    def test_synthetic_session_requests(self):
        fixtures = json.loads((skills.ROOT / "tools/skill_session_fixtures.json").read_text(encoding="utf-8"))
        for case, reference in {"issue-to-pr": "issue-to-pr", "plan-to-tickets": "plan-review",
                                "plan-to-tickets-approved": "plan-review",
                                "interrupted-write": "recovery"}.items():
            matches = skills.find(self.bundle, fixtures[case]["request"], None)["matches"]
            self.assertEqual(matches[0]["reference"], f"references/{reference}.md")

    def test_idempotent_project_install_preserves_other_skill(self):
        other_skill = self.project / ".agents/skills/my-skill/SKILL.md"
        other_skill.parent.mkdir(parents=True)
        other_skill.write_text("user content", encoding="utf-8")
        self.assertEqual({detail["status"] for detail, _, _ in self.entries()}, {"missing"})
        skills.install(self.entries())
        installed = self.entries()
        self.assertEqual(len(installed), 6)
        self.assertEqual({detail["status"] for detail, _, _ in installed}, {"installed"})
        stamps = [(directory / "SKILL.md").stat().st_mtime_ns for _, directory, _ in installed]
        skills.install(installed)
        self.assertEqual(stamps, [(directory / "SKILL.md").stat().st_mtime_ns for _, directory, _ in installed])
        self.assertEqual(other_skill.read_text(encoding="utf-8"), "user content")

    def test_conflict_refuses_before_installing_other_host(self):
        claude_skill = self.project / ".claude/skills/craft-cloud/SKILL.md"
        claude_skill.parent.mkdir(parents=True)
        claude_skill.write_text("user edit", encoding="utf-8")
        entries = self.entries()
        conflict = next(detail for detail, _, _ in entries if detail["status"] == "conflict")
        self.assertIn("SKILL.md", conflict["differences"])
        with self.assertRaisesRegex(ValueError, "conflicts"):
            skills.install(entries)
        self.assertFalse((self.project / ".agents").exists())
        self.assertEqual(claude_skill.read_text(encoding="utf-8"), "user edit")

    def redirect_directory(self, redirected, other):
        try:
            if sys.platform == "win32":
                import _winapi
                _winapi.CreateJunction(str(other), str(redirected))
            else:
                redirected.symlink_to(other, target_is_directory=True)
        except (NotImplementedError, OSError) as error:
            self.skipTest(f"Creating symlinks is unavailable: {error}")

    def test_redirected_discovery_directory_refused(self):
        other = self.project / "other"
        other.mkdir()
        self.redirect_directory(self.project / ".agents", other)
        entries = self.entries()
        with self.assertRaisesRegex(ValueError, "Unsafe codex discovery directory"):
            skills.install(entries)
        self.assertEqual({detail["status"] for detail, _, _ in entries}, {"unsafe", "missing"})
        self.assertEqual(list(other.iterdir()), [])

    def test_discovery_directory_redirected_after_preflight_is_refused(self):
        entries = self.entries()
        other = self.project / "other"
        other.mkdir()
        self.redirect_directory(self.project / ".agents", other)
        with self.assertRaises((OSError, ValueError)):
            skills.install(entries)
        self.assertEqual(list(other.iterdir()), [])

    def test_open_directory_guard_keeps_the_original_directory(self):
        original = self.project / "original"
        moved = self.project / "moved"
        original.mkdir()
        with skills.anchored_directory(original) as anchor:
            if sys.platform == "win32":
                with self.assertRaises(PermissionError):
                    original.rename(moved)
            else:
                original.rename(moved)
                original.symlink_to(self.project, target_is_directory=True)
                fd = os.open("check", os.O_WRONLY | os.O_CREAT | os.O_EXCL, dir_fd=anchor)
                os.close(fd)
                self.assertTrue((moved / "check").is_file())
                self.assertFalse((self.project / "check").exists())

    def test_doctor_keeps_other_results_when_one_reference_directory_is_redirected(self):
        skills.install(self.entries())
        candidate = self.project / ".claude/skills/craft-cloud/redirect"
        other = self.project / "other"
        other.mkdir()
        self.redirect_directory(candidate, other)
        results = [detail for detail, _, _ in self.entries()]
        self.assertEqual(len(results), 6)
        unsafe = next(detail for detail in results if detail["status"] == "unsafe")
        self.assertEqual(unsafe["host"], "claude-code")
        self.assertIn(str(candidate), unsafe["error"])
        self.assertEqual(sum(result["status"] == "installed" for result in results), 5)

    def test_user_targets_respect_claude_config(self):
        with patch.object(Path, "home", return_value=self.project), patch.dict(
            "os.environ", {"CLAUDE_CONFIG_DIR": str(self.project / "alt-config")}
        ):
            targets = {host: path for host, _, path in skills.targets(skills.HOSTS, "user", self.project)}
        self.assertEqual(targets, {
            "codex": self.project / ".agents/skills", "claude-code": self.project / "alt-config/skills",
        })

    def test_user_install_with_overlapping_host_directories(self):
        with patch.object(Path, "home", return_value=self.project), patch.dict(
            "os.environ", {"CLAUDE_CONFIG_DIR": str(self.project / ".agents")}
        ):
            entries = list(skills.installations(self.bundle, skills.targets(skills.HOSTS, "user", self.project)))
            skills.install(entries)
        self.assertEqual(len(entries), 6)
        self.assertEqual({detail["status"] for detail, _, _ in entries}, {"installed"})
        self.assertEqual(len({path for _, path, _ in entries}), 3)

    def test_doctor_validates_binary_identity(self):
        identity = {"product": "Craft AI Factory CLI", "version": self.compatibility["binary_version"],
                    "contract": self.compatibility["contract_version"]}
        binary = self.project / "craft"
        for data, expected in [(identity, "compatible"), ({**identity, "contract": "invalid"}, "mismatch")]:
            with patch.object(skills.subprocess, "run", return_value=SimpleNamespace(
                stdout=json.dumps({"ok": True, "data": data})
            )) as run:
                self.assertEqual(skills.binary_status(self.project, binary, self.compatibility)["status"], expected)
                self.assertEqual(run.call_args.args[0], [str(binary.resolve()), "version"])
        self.assertEqual(skills.binary_status(self.project, binary, self.compatibility)["status"], "probe_failed")


if __name__ == "__main__":
    unittest.main()
