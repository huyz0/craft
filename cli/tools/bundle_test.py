"""Exercise package rejection before filesystem writes and isolated installation."""

import io
import json
from pathlib import Path
import stat
import sys
import tempfile
import unittest
import warnings
import zipfile

import bundle


@unittest.skipUnless(sys.platform == "win32", "Windows development bundle acceptance")
class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.files = {name: b"fixture" for name in bundle.PAYLOAD}
        self.files["schemas/local-contracts.json"] = bundle.encoded({"operations": {}, "contract_version": "1.0"})
        self.files["skills/craft-cloud/compatibility.json"] = bundle.encoded({
            "contract_snapshot_sha256": bundle.digest(self.files["schemas/local-contracts.json"]),
            "binary_version": "0.1.0", "contract_version": "1.0",
            "format_version": 1, "skill_version": "0.1.0",
            "hosts": {"codex": "unqualified", "claude-code": "unqualified"},
            "live_provider_qualification": "not_established_by_skill_validation",
            "status": "development", "providers": ["confluence", "jira"]})

    def archive(self, change=None, extra=None):
        manifest = {"format": 1, "product": bundle.PRODUCT, "version": "0.1.0",
                    "status": "development", "platform": bundle.host(),
                    "files": {name: {"size": len(data), "sha256": bundle.digest(data)}
                              for name, data in self.files.items()}}
        if change:
            change(manifest)
        data = io.BytesIO()
        with zipfile.ZipFile(data, "w") as target:
            for name, content in {**self.files, "manifest.json": bundle.encoded(manifest)}.items():
                target.writestr(name, content)
            if extra:
                with warnings.catch_warnings():
                    warnings.simplefilter("ignore", UserWarning)
                    target.writestr(*extra)
        path = self.root / "bundle.zip"
        path.write_bytes(data.getvalue())
        return path, bundle.digest(data.getvalue())

    def test_isolated_install_retains_exact_payload_and_refuses_existing_destination(self):
        archive, checksum = self.archive()
        destination = self.root / "isolated"
        receipt = bundle.install(archive, checksum, destination)
        self.assertEqual(receipt["archive_sha256"], checksum)
        for name, data in self.files.items():
            self.assertEqual((destination / name).read_bytes(), data)
        (destination / "bin/craft.exe").write_bytes(b"unrelated Craft")
        with self.assertRaises(ValueError):
            bundle.install(archive, checksum, destination)
        self.assertEqual((destination / "bin/craft.exe").read_bytes(), b"unrelated Craft")

    def test_changed_archive_rejected_before_creating_destination(self):
        archive, checksum = self.archive()
        archive.write_bytes(archive.read_bytes() + b"changed")
        destination = self.root / "isolated"
        with self.assertRaises(ValueError):
            bundle.install(archive, checksum, destination)
        self.assertFalse(destination.exists())

    def test_legacy_archive_can_still_be_installed(self):
        self.files = {name: self.files[name] for name in bundle.LEGACY_PAYLOAD}
        archive, checksum = self.archive()
        destination = self.root / "legacy"
        bundle.install(archive, checksum, destination)
        self.assertEqual((destination / "skills/craft-cloud/SKILL.md").read_bytes(),
                         self.files["skills/craft-cloud/SKILL.md"])

    def test_unsafe_duplicate_link_and_oversized_members_are_rejected(self):
        for name in ["../escaped", "bin/craft.exe", "C:/escaped", "bin\\craft.exe"]:
            with self.subTest(name=name):
                archive, checksum = self.archive(extra=(name, b"bad"))
                with self.assertRaises(ValueError):
                    bundle.verified(archive, checksum)
        archive, _ = self.archive()
        raw = io.BytesIO()
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(raw, "w") as target:
            for entry in source.infolist():
                if entry.filename == "bin/craft.exe":
                    entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                target.writestr(entry, source.read(entry.filename))
        archive.write_bytes(raw.getvalue())
        with self.assertRaises(ValueError):
            bundle.verified(archive, bundle.digest(raw.getvalue()))
        archive, checksum = self.archive()
        from unittest.mock import patch
        with patch.object(bundle, "MAX_FILE", 1), self.assertRaises(ValueError):
            bundle.verified(archive, checksum)

    def test_invalid_manifest_and_payload_hash_are_rejected(self):
        changes = [lambda m: m.update(status="release"),
                   lambda m: m.update(platform={"os": "other", "machine": "other"}),
                   lambda m: m.update(unexpected=True),
                   lambda m: m["files"]["bin/craft.exe"].update(sha256="0" * 64),
                   lambda m: m["files"]["bin/craft.exe"].update(size=True)]
        for change in changes:
            with self.subTest(change=change):
                archive, checksum = self.archive(change=change)
                with self.assertRaises(ValueError):
                    bundle.verified(archive, checksum)

    def test_self_consistent_payload_still_requires_compatible_skill(self):
        value = json.loads(self.files["skills/craft-cloud/compatibility.json"])
        value["contract_snapshot_sha256"] = "0" * 64
        self.files["skills/craft-cloud/compatibility.json"] = bundle.encoded(value)
        archive, checksum = self.archive()
        with self.assertRaises(ValueError):
            bundle.verified(archive, checksum)

    def test_duplicate_json_is_rejected(self):
        with self.assertRaises(ValueError):
            bundle.document(b'{"format":1,"format":1}')

    def test_unsupported_qualification_claims_are_rejected(self):
        original = self.files["skills/craft-cloud/compatibility.json"]
        for key, value in [("hosts", {"codex": "qualified", "claude-code": "unqualified"}),
                           ("contract_version", "99.0"), ("format_version", True),
                           ("live_provider_qualification", "qualified")]:
            with self.subTest(key=key):
                compatibility = json.loads(original)
                compatibility[key] = value
                self.files["skills/craft-cloud/compatibility.json"] = bundle.encoded(compatibility)
                archive, checksum = self.archive()
                with self.assertRaises(ValueError):
                    bundle.verified(archive, checksum)

    def test_identity_probe_executes_pinned_copy_when_workspace_binary_changes(self):
        from types import SimpleNamespace
        from unittest.mock import patch
        binary = self.root / "target/debug/craft.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"pinned binary")
        for name, data in self.files.items():
            if name.startswith(("schemas/", "skills/")):
                path = self.root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion="0.1.0"\n')
        (self.root / "Cargo.lock").write_bytes(b"lock")
        inventory = self.root / "docs/evidence/dependency-inventory.json"
        inventory.parent.mkdir(parents=True)
        inventory.write_bytes(bundle.encoded({"lock_sha256": bundle.digest(b"lock")}))

        def probe(command, **options):
            binary.write_bytes(b"concurrent replacement")
            self.assertNotEqual(Path(command[0]), binary)
            self.assertEqual(Path(command[0]).read_bytes(), b"pinned binary")
            return SimpleNamespace(stdout=bundle.encoded({"ok": True, "data": {
                "product": bundle.PRODUCT, "version": "0.1.0"}}))

        output = self.root / "prepared.zip"
        with patch.object(bundle, "ROOT", self.root), patch.object(bundle.subprocess, "run", probe):
            bundle.prepare(binary, bundle.digest(b"pinned binary"), output)
        with zipfile.ZipFile(output) as source:
            self.assertEqual(source.read("bin/craft.exe"), b"pinned binary")


if __name__ == "__main__":
    unittest.main()
