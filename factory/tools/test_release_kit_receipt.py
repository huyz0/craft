from io import BytesIO
from hashlib import sha256
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
import zipfile

import preview_bundle
import release_kit_receipt as receipts


SOURCE = "a" * 40
REPOSITORY = "team/craft"


class ReleaseKitReceiptTest(unittest.TestCase):
    def setUp(self):
        self.directory = self.enterContext(TemporaryDirectory())
        self.enterContext(patch.object(receipts.subprocess, "check_output", return_value=SOURCE + "\n"))

    def archive(self, platform, directory=None):
        filename = next(name for name, info in receipts.KITS.items() if info[0] == platform)
        archive = (directory or Path(self.directory)) / filename
        if platform == "Linux":
            with tarfile.open(archive, "w:gz") as package:
                for name, data in (("craft-factory", b"runner"), ("shared/craft-factory-queue", b"queue")):
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    package.addfile(info, BytesIO(data))
            self.native_comparison(archive)
        else:
            archive.write_bytes(b"fixture archive")
        return archive

    def native_comparison(self, archive):
        report = archive.with_name(receipts.NATIVE_COMPARISON)
        report.write_text(json.dumps({"schema_version": 1, "model_responses": "scripted_loopback",
                                      "base_commit": SOURCE, "verified_tree": "b" * 40,
                                      "launcher_sha256": sha256(b"runner").hexdigest(),
                                      "queue_sha256": sha256(b"queue").hexdigest(),
                                      "comparison": {"results": [
                                          {"adapter": adapter, "independent_result": "passed", "head": SOURCE,
                                           "verification_sequence": sequence}
                                          for sequence, adapter in enumerate(("codex", "claude"), start=1)
                                      ]}}), encoding="utf-8")
        return report

    def environment(self, archive):
        platform, job, checks = receipts.KITS[archive.name]
        return {
            "GITHUB_EVENT_NAME": "workflow_dispatch", "KIT_PROFILE": "release", "GITHUB_JOB": job,
            "RUNNER_OS": platform, "RUNNER_ARCH": "X64", "GITHUB_WORKSPACE": self.directory,
            "GITHUB_SHA": SOURCE, "GITHUB_WORKFLOW_SHA": "b" * 40, "GITHUB_REPOSITORY": REPOSITORY,
            "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
            **{"KIT_CHECK_" + name.upper(): "success" for name in checks},
        }

    def write_receipt(self, archive):
        platform, _, _ = receipts.KITS[archive.name]
        if platform == "Linux":
            receipts.write(archive)
            return
        with TemporaryDirectory() as scratch:
            report = Path(scratch) / "windows-smoke.json"
            report.write_text(json.dumps({"archive_sha256": receipts.archive_record(archive)["sha256"],
                                          "result": receipts.WINDOWS_SMOKE_RESULT}), encoding="utf-8")
            receipts.write(archive, windows_smoke_report=report)

    def test_each_platform_binds_archive_source_and_attempt(self):
        for platform in ("Windows", "Linux"):
            with self.subTest(platform=platform):
                archive = self.archive(platform)
                with patch.dict(os.environ, self.environment(archive), clear=True):
                    self.write_receipt(archive)
                receipt = receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)
                self.assertEqual(receipt["archive"]["bytes"], archive.stat().st_size)
                self.assertEqual(receipt["platform"], platform)
                self.assertEqual(receipt["schema_version"], receipts.SCHEMA_VERSION)
                self.assertIn("ci_repair", receipt["checks"])
                self.assertEqual(receipt["checks"]["queue_https"], "success")
                self.assertEqual("native_workers" in receipt["checks"], platform == "Linux")
                self.assertIn("queue_smoke", receipt["checks"])
                self.assertEqual("windows_smoke" in receipt, platform == "Windows")
                with self.assertRaisesRegex(ValueError, "different workflow run or attempt"):
                    receipts.verify(archive, SOURCE, REPOSITORY, 123, 1)
                with self.assertRaisesRegex(ValueError, "different source revision"):
                    receipts.verify(archive, "c" * 40, REPOSITORY, 123, 2)
                archive.write_bytes(b"changed archive")
                with self.assertRaisesRegex(ValueError, "archive does not match"):
                    receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)

    def test_failed_or_skipped_checks_and_wrong_source_cannot_write_receipt(self):
        archive = self.archive("Linux")
        env = self.environment(archive)
        for update, reason in [({"KIT_CHECK_QUEUE_HTTPS": "skipped"}, "queue_https qualification"),
                               ({"KIT_CHECK_QUEUE_SMOKE": "skipped"}, "queue_smoke qualification"),
                               ({"KIT_CHECK_NATIVE_WORKERS": "skipped"}, "native_workers qualification"),
                               ({"KIT_CHECK_NATIVE_WORKERS": "failure"}, "native_workers qualification"),
                               ({"KIT_CHECK_CI_REPAIR": "failure"}, "ci_repair qualification"),
                               ({"GITHUB_SHA": "c" * 40}, "checked-out source"),
                               ({"KIT_PROFILE": "dev"}, "matching manual release job")]:
            with self.subTest(update=update), patch.dict(os.environ, env | update, clear=True):
                with self.assertRaisesRegex(ValueError, reason):
                    self.write_receipt(archive)
                self.assertFalse(archive.with_name(receipts.RECEIPT).exists())

    def test_verifier_rejects_missing_checks_and_mixed_checksum(self):
        archive = self.archive("Linux")
        with patch.dict(os.environ, self.environment(archive), clear=True):
            self.write_receipt(archive)
        receipt_file = archive.with_name(receipts.RECEIPT)
        original = receipt_file.read_text(encoding="utf-8")
        changed = json.loads(original)
        del changed["checks"]["queue_sql"]
        receipt_file.write_text(json.dumps(changed), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "missing required successful checks"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)
        receipt_file.write_text(original, encoding="utf-8")
        archive.with_name("SHA256SUMS").write_text("0" * 64 + "  " + archive.name + "\n", encoding="ascii")
        with self.assertRaisesRegex(ValueError, "SHA256SUMS does not match"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)

    def test_receipt_versions_distinguish_packaged_smoke(self):
        for platform in ("Windows", "Linux"):
            archive = self.archive(platform)
            with patch.dict(os.environ, self.environment(archive), clear=True):
                self.write_receipt(archive)
            receipt_file = archive.with_name(receipts.RECEIPT)
            receipt = json.loads(receipt_file.read_text(encoding="utf-8"))
            receipt["checks"].pop("native_workers", None)
            if platform == "Windows":
                del receipt["checks"]["queue_https"]
            receipt.pop("native_comparison", None)
            receipt.pop("windows_smoke", None)
            del receipt["checks"]["queue_smoke"]
            for version in (1, 2, 3):
                with self.subTest(platform=platform, version=version):
                    receipt["schema_version"] = version
                    receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
                    if version == 1 or (version == 2 and platform == "Windows"):
                        self.assertEqual(receipts.verify(archive, SOURCE, REPOSITORY, 123, 2), receipt)
                    else:
                        with self.assertRaisesRegex(ValueError, "missing required successful checks"):
                            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)

    def test_linux_receipt_version_four_requires_native_workers(self):
        archive = self.archive("Linux")
        with patch.dict(os.environ, self.environment(archive), clear=True):
            self.write_receipt(archive)
        receipt_file = archive.with_name(receipts.RECEIPT)
        receipt = json.loads(receipt_file.read_text(encoding="utf-8"))
        receipt["schema_version"] = 4
        del receipt["native_comparison"]
        receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
        self.assertEqual(receipts.verify(archive, SOURCE, REPOSITORY, 123, 2), receipt)
        del receipt["checks"]["native_workers"]
        receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "missing required successful checks"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)
        receipt["schema_version"] = 3
        receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
        self.assertEqual(receipts.verify(archive, SOURCE, REPOSITORY, 123, 2), receipt)

    def test_linux_receipt_rejects_a_swapped_or_missing_native_report(self):
        archive = self.archive("Linux")
        report = archive.with_name(receipts.NATIVE_COMPARISON)
        original = report.read_text(encoding="utf-8")
        changed = json.loads(original)
        changed["queue_sha256"] = "0" * 64
        report.write_text(json.dumps(changed), encoding="utf-8")
        with patch.dict(os.environ, self.environment(archive), clear=True):
            with self.assertRaisesRegex(ValueError, "different Linux executables"):
                self.write_receipt(archive)
            self.assertFalse(archive.with_name(receipts.RECEIPT).exists())
            report.write_text(original, encoding="utf-8")
            self.write_receipt(archive)
        report.write_text(original + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "does not match the qualification receipt"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)
        report.unlink()
        with self.assertRaisesRegex(ValueError, "native worker comparison is missing"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)

    def test_windows_receipt_requires_the_confluence_report_for_the_tested_archive(self):
        archive = self.archive("Windows")
        report = archive.with_name("windows-smoke.json")
        digest = receipts.archive_record(archive)["sha256"]
        ordinary_smoke = receipts.WINDOWS_SMOKE_RESULT.copy()
        del ordinary_smoke["confluence_drift_report"]
        with patch.dict(os.environ, self.environment(archive), clear=True):
            for data, reason in (
                (None, "report is missing"),
                ({"archive_sha256": "0" * 64, "result": receipts.WINDOWS_SMOKE_RESULT}, "different archive"),
                ({"archive_sha256": digest, "result": ordinary_smoke}, "successful Confluence scenario"),
            ):
                with self.subTest(reason=reason):
                    if data is not None:
                        report.write_text(json.dumps(data), encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, reason):
                        receipts.write(archive, windows_smoke_report=report)
                    self.assertFalse(archive.with_name(receipts.RECEIPT).exists())
            self.write_receipt(archive)
        receipt_file = archive.with_name(receipts.RECEIPT)
        receipt = json.loads(receipt_file.read_bytes())
        receipt["windows_smoke"]["result"] = ordinary_smoke
        receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "successful Confluence scenario"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)

    def test_windows_receipt_version_seven_requires_https_qualification(self):
        archive = self.archive("Windows")
        env = self.environment(archive)
        with patch.dict(os.environ, env | {"KIT_CHECK_QUEUE_HTTPS": "skipped"}, clear=True):
            with self.assertRaisesRegex(ValueError, "queue_https qualification did not succeed"):
                self.write_receipt(archive)
        receipt_file = archive.with_name(receipts.RECEIPT)
        self.assertFalse(receipt_file.exists())
        with patch.dict(os.environ, env, clear=True):
            self.write_receipt(archive)
        receipt = json.loads(receipt_file.read_bytes())
        del receipt["checks"]["queue_https"]
        receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "missing required successful checks"):
            receipts.verify(archive, SOURCE, REPOSITORY, 123, 2)
        receipt["schema_version"] = 6
        receipt_file.write_text(json.dumps(receipt), encoding="utf-8")
        self.assertEqual(receipts.verify(archive, SOURCE, REPOSITORY, 123, 2), receipt)

    def test_new_bundles_require_current_receipts_and_historical_bundles_still_verify(self):
        root = Path(self.directory)
        self.stage_bundle(root)
        binding = (root, SOURCE, REPOSITORY, 123, 2, "b" * 40)
        path = root / "windows" / receipts.RECEIPT
        receipt = json.loads(path.read_bytes())
        del receipt["checks"]["queue_https"]
        receipt["schema_version"] = 6
        path.write_text(json.dumps(receipt), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "receipt version 7 or later"):
            receipts.bundle(*binding)
        historical = receipts.bundle_manifest(*binding)
        root.joinpath(receipts.RELEASE_MANIFEST).write_text(json.dumps(historical), encoding="utf-8")
        self.assertEqual(receipts.verify_bundle(*binding), historical)

    def stage_bundle(self, root, *, linux_env=None):
        for platform in ("Windows", "Linux"):
            directory = root / platform.lower()
            directory.mkdir()
            archive = self.archive(platform, directory)
            overrides = (linux_env or {}) if platform == "Linux" else {}
            with patch.dict(os.environ, self.environment(archive) | overrides, clear=True):
                self.write_receipt(archive)

    def test_bundle_checks_both_archives_and_exact_receipts(self):
        root = Path(self.directory)
        self.stage_bundle(root)
        binding = (SOURCE, REPOSITORY, 123, 2, "b" * 40)
        receipts.bundle(root, *binding)
        receipts.verify_bundle(root, *binding)
        manifest = json.loads(root.joinpath(receipts.RELEASE_MANIFEST).read_text(encoding="utf-8"))
        self.assertEqual(set(manifest["packages"]), {"windows", "linux"})
        self.assertEqual(manifest["packages"]["windows"]["path"], "windows/craft-factory-shared-windows-x64.zip")
        self.assertEqual(manifest["packages"]["linux"]["native_comparison"],
                         receipts.archive_record(root / "linux" / receipts.NATIVE_COMPARISON))
        receipt = root / "linux" / receipts.RECEIPT
        with receipt.open("a", encoding="utf-8") as stream:
            stream.write("\n")
        with self.assertRaisesRegex(ValueError, "Release manifest does not match"):
            receipts.verify_bundle(root, *binding)

    def test_bundle_rejects_mixed_attempts_workflows_and_missing_platform(self):
        binding = (SOURCE, REPOSITORY, 123, 2, "b" * 40)
        for override, error in [
            ({"GITHUB_RUN_ATTEMPT": "1"}, "different workflow run or attempt"),
            ({"GITHUB_WORKFLOW_SHA": "c" * 40}, "requires the bundle workflow"),
            ({}, None),
        ]:
            with self.subTest(override=override), TemporaryDirectory() as directory:
                root = Path(directory)
                self.stage_bundle(root, linux_env=override)
                if error:
                    with self.assertRaisesRegex(ValueError, error):
                        receipts.bundle(root, *binding)
                else:
                    root.joinpath("linux", receipts.RECEIPT).unlink()
                    with self.assertRaisesRegex(ValueError, "requires exactly its archive, receipt, and checksum"):
                        receipts.bundle(root, *binding)
                self.assertFalse(root.joinpath(receipts.RELEASE_MANIFEST).exists())

    def install_fixture(self, root, extras=(), *, tar_link=False):
        root.mkdir()
        for filename, (platform, _, _) in receipts.KITS.items():
            directory = root / platform.lower()
            directory.mkdir()
            archive = directory / filename
            suffix = ".exe" if platform == "Windows" else ""
            entries = [("QUICKSTART.md", b"Run the smoke test."), (f"craft-factory{suffix}", b"runner"),
                       (f"shared/craft-factory-queue{suffix}", b"queue"), ("shared/migrate_queue.py", b"pass")]
            if platform == "Windows":
                with zipfile.ZipFile(archive, "w") as package:
                    for name, data in entries + list(extras):
                        package.writestr(name, data)
            else:
                with tarfile.open(archive, "w:gz") as package:
                    for name, data in entries:
                        info = tarfile.TarInfo(name)
                        info.size = len(data)
                        info.mode = 0o755 if name.endswith("craft-factory") else 0o644
                        package.addfile(info, BytesIO(data))
                    if tar_link:
                        link = tarfile.TarInfo("shared/fixture-link")
                        link.type = tarfile.SYMTYPE
                        link.linkname = "migrate_queue.py"
                        package.addfile(link)
                self.native_comparison(archive)
            with patch.dict(os.environ, self.environment(archive), clear=True):
                self.write_receipt(archive)
        binding = (root, SOURCE, REPOSITORY, 123, 2, "b" * 40)
        receipts.bundle(*binding)
        return binding

    def test_local_preview_binds_and_installs_both_platforms(self):
        root = Path(self.directory)
        self.install_fixture(root / "fixtures")
        archives = [root / "fixtures" / platform / name for platform, name in preview_bundle.PLATFORM_ARCHIVES.items()]
        output = root / "preview.zip"
        with self.assertRaisesRegex(ValueError, "windows archive does not match"):
            preview_bundle.assemble(output, archives[0], "0" * 64, archives[1], "0" * 64)
        self.assertFalse(output.exists())

        checksums = [receipts.archive_record(path)["sha256"] for path in archives]
        digest = preview_bundle.assemble(output, archives[0], checksums[0], archives[1], checksums[1])
        for platform, suffix in (("windows", ".exe"), ("linux", "")):
            destination = root / platform
            command = [sys.executable, "-I", "-B", str(output), str(destination),
                       "--sha256", digest, "--platform", platform]
            installed = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=15)
            self.assertEqual(installed.returncode, 0, installed.stderr)
            self.assertEqual((destination / f"craft-factory{suffix}").read_bytes(), b"runner")

        replacement = root / "replacement.zip"
        with zipfile.ZipFile(replacement, "w") as bundle:
            bundle.write(archives[0], arcname=preview_bundle.PLATFORM_ARCHIVES["windows"])
            bundle.writestr(preview_bundle.PLATFORM_ARCHIVES["linux"], b"changed platform")
        destination = root / "rejected"
        with self.assertRaisesRegex(ValueError, "bundle does not match"):
            preview_bundle.install(replacement, digest, "windows", destination)
        self.assertFalse(destination.exists())

    def test_install_selects_one_verified_kit_and_refuses_an_existing_directory(self):
        root = Path(self.directory)
        binding = self.install_fixture(root / "bundle")
        for platform, suffix in (("windows", ".exe"), ("linux", "")):
            destination = root / platform
            receipts.install_bundle(*binding, platform=platform, destination=destination)
            launcher = destination / f"craft-factory{suffix}"
            self.assertEqual(launcher.read_bytes(), b"runner")
            self.assertFalse((destination / ("craft-factory" if suffix else "craft-factory.exe")).exists())
            if platform == "linux" and os.name != "nt":
                self.assertTrue(launcher.stat().st_mode & 0o100)
            with self.assertRaisesRegex(ValueError, "destination already exists"):
                receipts.install_bundle(*binding, platform=platform, destination=destination)
            self.assertEqual(launcher.read_bytes(), b"runner")

    def test_install_cli_takes_bundle_before_destination(self):
        root = Path(self.directory)
        bundle = root / "paired release"
        self.install_fixture(bundle)
        destination = root / "new install"
        result = subprocess.run(
            [sys.executable, "-B", receipts.__file__, "install-bundle", bundle, destination, "--platform", "windows",
             "--source-commit", SOURCE, "--workflow-commit", "b" * 40, "--repository", REPOSITORY,
             "--run-id", "123", "--run-attempt", "2"], capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((destination / "craft-factory.exe").is_file())

    def test_install_rejects_unsafe_members_without_creating_a_destination(self):
        for index, name in enumerate(("../outside.txt", "shared/NUL.txt", "Shared/case.txt")):
            with self.subTest(member=name):
                root = Path(self.directory)
                binding = self.install_fixture(root / f"bundle-{index}", [(name, b"rejected fixture")])
                destination = root / f"install-{index}"
                with self.assertRaisesRegex(ValueError, "Unsafe kit archive member|Conflicting kit archive path"):
                    receipts.install_bundle(*binding, platform="windows", destination=destination)
                self.assertFalse(destination.exists())
                self.assertFalse(list(root.glob(".craft-install-*")))
                self.assertFalse((root / "outside.txt").exists())

    def test_install_rejects_tar_links_and_rolls_back_a_failed_publication(self):
        root = Path(self.directory)
        binding = self.install_fixture(root / "linked-bundle", tar_link=True)
        destination = root / "not-installed"
        with self.assertRaisesRegex(ValueError, "Unsupported tar member type"):
            receipts.install_bundle(*binding, platform="linux", destination=destination)
        self.assertFalse(destination.exists())

        binding = self.install_fixture(root / "ordinary-bundle")
        rename = Path.rename

        def fail_one_move(source, target):
            if target == destination / "craft-factory.exe":
                raise OSError("fixture publication failure")
            return rename(source, target)

        with patch.object(Path, "rename", fail_one_move):
            with self.assertRaisesRegex(OSError, "fixture publication failure"):
                receipts.install_bundle(*binding, platform="windows", destination=destination)
        self.assertFalse(destination.exists())
        self.assertFalse(list(root.glob(".craft-install-*")))
