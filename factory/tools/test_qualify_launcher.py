from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from subprocess import CompletedProcess
import unittest
from unittest.mock import patch

from qualify_launcher import SCENARIOS, qualify_launcher


class LauncherQualificationTest(unittest.TestCase):
    @patch("qualify_launcher.subprocess.run")
    def test_success_requires_every_exact_scenario(self, run):
        launcher = Path(__file__).resolve()
        successful_lines = [f"test {scenario} ... ok" for scenario in SCENARIOS]
        with redirect_stdout(StringIO()):
            run.return_value = CompletedProcess([], 0, stdout="\n".join(successful_lines[:-1]))
            with self.assertRaisesRegex(SystemExit, SCENARIOS[-1]):
                qualify_launcher(launcher, env={})

            run.return_value = CompletedProcess([], 0, stdout="\n".join(successful_lines))
            qualify_launcher(launcher, env={})

        self.assertEqual(run.call_args.kwargs["env"]["CRAFT_FACTORY_TEST_LAUNCHER"], str(launcher))
        self.assertEqual(run.call_args.args[0][-len(SCENARIOS) - 1:], ["--exact", *SCENARIOS])
