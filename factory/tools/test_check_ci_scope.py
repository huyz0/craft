from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from check_ci_scope import docs_only_changes, git, select, validate_docs


class ScopeTests(unittest.TestCase):
    def test_only_regular_edits_in_the_shortcut(self):
        guide = b':100644 100644 123 456 M\0factory/README.md\0'
        self.assertTrue(docs_only_changes(guide))
        for diff in (
            b'', guide.rstrip(b'\0'), guide.replace(b'100644 100644', b'100644 120000'),
            guide.replace(b' M', b' D'), guide.replace(b' M', b' A'),
            guide.replace(b'factory/README.md', b'factory/skills/verify-candidate/SKILL.md'),
            guide + b':100644 100644 456 789 M\0.github/workflows/factory-checks.yml\0',
            guide.replace(b'README.md', b'README.md\n'),
        ):
            with self.subTest(diff=diff):
                self.assertFalse(docs_only_changes(diff))

    def test_checks_actual_merge_parents_and_change_set(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git(root, 'init', '-q', '-b', 'base')
            git(root, 'config', 'user.email', 'fixture@example.invalid')
            git(root, 'config', 'user.name', 'Factory fixture')
            git(root, 'config', 'commit.gpgsign', 'false')
            git(root, 'config', 'core.hooksPath', '.git/no-hooks')
            guide = root / 'factory/README.md'
            guide.parent.mkdir()
            for text in ('original', 'edited'):
                guide.write_text(text, encoding='utf-8')
                git(root, 'add', '--', 'factory/README.md')
                git(root, 'commit', '-qm', text)
                if text == 'original':
                    base = git(root, 'rev-parse', 'HEAD').decode().strip()
                    git(root, 'switch', '-qc', 'proposed')
            head = git(root, 'rev-parse', 'HEAD').decode().strip()
            git(root, 'switch', '-q', 'base')
            git(root, 'merge', '--no-ff', '-qm', 'test merge', 'proposed')
            self.assertEqual(select(root, 'pull_request', base, head)[0], 'docs')
            shallow_boundary = root / '.git/shallow'
            shallow_boundary.write_bytes(f'{base}\n{head}\n'.encode('ascii'))
            self.assertEqual(git(root, 'rev-parse', '--is-shallow-repository').strip(), b'true')
            self.assertEqual(select(root, 'pull_request', base, head)[0], 'docs')
            shallow_boundary.unlink()
            self.assertEqual(select(root, 'pull_request', head, base)[0], 'full')
            self.assertEqual(select(root, 'workflow_dispatch', base, head)[0], 'full')
            self.assertEqual(select(root, 'merge_group', base, head)[0], 'full')

            git(root, 'switch', '-qc', 'additional', head)
            (root / 'factory/src.rs').write_text('fn main() {}', encoding='utf-8')
            git(root, 'add', '--', 'factory/src.rs')
            git(root, 'commit', '-qm', 'code edit')
            updated = git(root, 'rev-parse', 'HEAD').decode().strip()
            git(root, 'switch', '-qc', 'next-base', base)
            git(root, 'merge', '--no-ff', '-qm', 'test mixed merge', updated)
            self.assertEqual(select(root, 'pull_request', base, updated)[0], 'full')

    def test_missing_comparison_uses_full_suite(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(select(Path(directory), 'pull_request', 'base', 'head')[0], 'full')

    def test_docs_validation_checks_sections_and_examples(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            guide = root / 'README.md'
            guide.write_text('## Working section\n[Link](#working-section)\n[Example](example.json)\n',
                             encoding='utf-8')
            example = root / 'example.json'
            example.write_text('{}', encoding='utf-8')
            with patch('check_ci_scope.DOCS', {'README.md'}):
                self.assertEqual(validate_docs(root), [])
                guide.write_text(guide.read_text(encoding='utf-8') + '[Broken](#absent)\n', encoding='utf-8')
                example.write_text('{', encoding='utf-8')
                errors = validate_docs(root)
                self.assertEqual(len(errors), 2)
                self.assertIn('invalid JSON example', errors[0])
                self.assertIn('missing section', errors[1])


if __name__ == '__main__':
    unittest.main()
