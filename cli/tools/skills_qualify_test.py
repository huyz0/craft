"""Prove the skill qualifier rejects broken links, anchors, commands and oversized entries."""

import json
from pathlib import Path
import shutil
import tempfile
import unittest

import skills_qualify as q


class QualifierTests(unittest.TestCase):
    def setUp(self):
        self.contract = json.loads(q.CONTRACT.read_bytes())
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.skills = Path(self.temporary.name) / "skills"
        shutil.copytree(q.SKILLS, self.skills)
        self.original = q.SKILLS
        q.SKILLS = self.skills
        self.addCleanup(setattr, q, "SKILLS", self.original)
        self.entry = self.skills / "confluence-pages/SKILL.md"

    def append(self, text):
        self.entry.write_text(self.entry.read_text(encoding="utf-8") + "\n" + text + "\n", encoding="utf-8")

    def rejects(self, text):
        self.append(text)
        with self.assertRaises(ValueError):
            q.check_skill(self.skills / "confluence-pages", self.contract)

    def test_current_skills_pass(self):
        for directory in q.skill_dirs():
            q.check_skill(directory, self.contract)

    def test_rejections(self):
        self.rejects("[x](missing.md)")

    def test_missing_anchor(self):
        self.rejects("[x](references/tables.md#no-such-heading)")

    def test_escaping_link(self):
        self.rejects("[x](../../../README.md)")

    def test_unknown_inline_command(self):
        self.rejects("Run `craft confluence page vaporize --id 1`.")

    def test_unknown_example_flag(self):
        self.rejects("```sh\ncraft confluence page get --nonsense 1\n```")

    def test_oversized_entry(self):
        self.rejects("x" * q.ENTRY_BUDGET)

    def test_content_hash_ignores_line_endings(self):
        before = q.content_hash(self.skills / "atlassian-design")
        path = self.skills / "atlassian-design/SKILL.md"
        path.write_bytes(path.read_bytes().replace(b"\n", b"\r\n"))
        self.assertEqual(before, q.content_hash(self.skills / "atlassian-design"))


if __name__ == "__main__":
    unittest.main()
