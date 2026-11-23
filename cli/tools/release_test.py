"""Exercise release metadata checks and archive rejection without a built executable."""

import io
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def archive(self, mutate=None):
        files = {name: path.read_bytes() for name, path in (
            (n, release.ROOT / n) for n in ("schemas/local-contracts.json", "tools/skills.py"))}
        for path in (release.ROOT / "skills").rglob("*"):
            if path.is_file():
                files[path.relative_to(release.ROOT).as_posix()] = path.read_bytes()
        files["bin/craft.exe"] = b"binary"
        manifest = {"format": 1, "product": release.PRODUCT, "version": release.version(),
                    "channel": release.CHANNEL, "platform": {"os": "windows", "machine": "x86_64"},
                    "files": {n: {"size": len(d), "sha256": release.digest(d)} for n, d in files.items()}}
        members = {**files, "manifest.json": release.encoded(manifest)}
        if mutate:
            mutate(members, manifest)
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w") as zipped:
            for name, data in members.items():
                zipped.writestr(name, data)
        directory = Path(self.temporary.name)
        path = directory / "craft-test.zip"
        path.write_bytes(out.getvalue())
        return path, release.digest(out.getvalue())

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)

    def test_metadata_and_tag(self):
        release.check_metadata("v" + release.version())
        with self.assertRaises(ValueError):
            release.check_metadata("v99.0.0")

    def test_good_archive(self):
        path, sha = self.archive()
        files, manifest = release.read_archive(path, sha)
        self.assertEqual(manifest["version"], release.version())

    def test_archive_without_skill_companion_remains_verifiable(self):
        def legacy(members, manifest):
            for name in ("tools/skills.py", "skills/craft-cloud/reference-index.json"):
                del members[name]
                del manifest["files"][name]
            members["manifest.json"] = release.encoded(manifest)
        path, sha = self.archive(legacy)
        files, _ = release.read_archive(path, sha)
        self.assertNotIn("tools/skills.py", files)

    def test_wrong_digest(self):
        path, _ = self.archive()
        with self.assertRaises(ValueError):
            release.read_archive(path, "0" * 64)

    def test_tampered_member(self):
        def mutate(members, manifest):
            members["bin/craft.exe"] = b"other"
        path, sha = self.archive(mutate)
        with self.assertRaises(ValueError):
            release.read_archive(path, sha)

    def test_unsafe_member_name(self):
        def mutate(members, manifest):
            members["../evil"] = b"x"
            manifest["files"]["../evil"] = {"size": 1, "sha256": release.digest(b"x")}
            members["manifest.json"] = release.encoded(manifest)
        path, sha = self.archive(mutate)
        with self.assertRaises(ValueError):
            release.read_archive(path, sha)

    def test_install_refuses_existing_destination(self):
        path, sha = self.archive()
        with self.assertRaises(ValueError):
            release.install(path, sha, Path(self.temporary.name), None)

    def test_skill_files_are_not_overwritten(self):
        path, sha = self.archive()
        skills = Path(self.temporary.name) / "skills"
        release.install(path, sha, Path(self.temporary.name) / "one", skills)
        with self.assertRaises(ValueError):
            release.install(path, sha, Path(self.temporary.name) / "two", skills)


if __name__ == "__main__":
    unittest.main()
