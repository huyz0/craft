# Install a Craft release

Craft ships as one archive per platform: `craft-<version>-<os>-<arch>.zip` on Windows, `.tar.gz` on Linux and macOS (`x86_64` or `aarch64`). Each contains `bin/craft`, the contract snapshot, the three skills, a dependency inventory, the changelog and a manifest. Releases are **previews**: unsigned, with no redistribution license selected. SHA-256 catches corruption and substitution only when you get the digest from a source other than the archive.

## Verify and install

Use Python 3.11 or later and the repository's `tools/release.py` from the same release tag. `SHA256SUMS` is published beside the archives.

```sh
python tools/release.py verify --archive craft-0.1.0-linux-x86_64.tar.gz --sha256 <digest from SHA256SUMS>
python tools/release.py install --archive craft-0.1.0-linux-x86_64.tar.gz --sha256 <digest> --destination /opt/craft
/opt/craft/bin/craft version
```

Installation needs a new absolute destination and never edits `PATH`, credentials, profiles or cloud state. Add `--skills-dir <absolute dir>` to copy the skills into a host's skill directory; it refuses to overwrite existing files. Without Python, unpack the archive yourself and compare the SHA-256 of each file with `manifest.json`.

## Upgrade and remove

Install the new version to a new directory and switch your `PATH` or alias to it. Local state (profiles, journals, operations) lives in the state root, not the install directory, so removing an install directory leaves state untouched. Delete the state root only after reading [local state](local-artifacts.md). A newer binary may migrate state in place and older binaries then refuse it, so do not run two versions against one state root at once.

## What is qualified

| Area | Status |
| --- | --- |
| Windows | Full workspace gate and live Atlassian runs on the test site |
| Linux | Workspace gate and a real Secret Service vault ([platform qualification](evidence/platform-qualification.md)) |
| macOS | Built and smoke-tested in release CI only; Keychain vault not live-tested |
| Skills | Checked against the contract and executable by `tools/skills_qualify.py` ([evidence](evidence/skills-qualification.md)) |
| Codex, Claude Code skill loading | Unqualified |

## Cut a release

1. Bump `version` in `Cargo.toml` and `binary_version` in `skills/craft-cloud/compatibility.json`, add a `## <version>` section to `CHANGELOG.md`, regenerate the contract snapshot if commands changed.
2. `python tools/release.py check` and `python tools/skills_qualify.py check --binary <built craft>`. Re-run the A/B or live checks for any skill reported under `changed_since_evidence`, record them in `docs/evidence`, then `python tools/skills_qualify.py baseline`.
3. Tag `v<version>` and push. `.github/workflows/release.yml` builds each platform with `--locked`, runs the qualifier against the built binary, packages, and attaches archives and `SHA256SUMS` to a draft pre-release.
