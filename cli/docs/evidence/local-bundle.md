# Local Windows development bundle

11 November 2026. `tools/bundle.py` now prepares, verifies and installs an internal development ZIP. The payload is the normal pinned Windows executable, contract snapshot, progressive runtime skill, dependency license inventory and explicit limitations. It does not install globally, change PATH, provision credentials, contact a tenant, publish an archive or establish a redistribution license.

`tools/bundle_test.py` passed eight cases: isolated exact-payload installation and existing-destination collision, changed archive refusal before directory creation, unsafe/duplicate/link/oversized members, invalid manifest and payload integrity, skill mismatch, duplicate JSON, contradictory qualification metadata, and workspace binary replacement during the identity probe. The latter proves both probe and ZIP retain the pinned bytes. The format-one manifest uses a closed payload allowlist and bounded archive/member/expanded sizes. Verification and installation require the archive digest independently supplied by the caller.

`python tools/verify.py > target/bundle-qualified.log 2>&1` passed three selected commands: planning validation, verifier selftests and the eight packaging tests. No Rust source changed and no Rust/provider matrix was repeated. The verifier now includes packaging tests in full runs, and packaging-only changes select them without unrelated runtime builds. `target/bundle-qualified-source-hashes.json` preserves the qualified tooling checkpoint. Windows-only packaging tests explicitly skip on other platforms, where bundle support is unavailable.

Actual preparation and installation:

- Prepared `target/craft-0.1.0-windows-development.zip` from normal binary SHA-256 `61dc13b494c6ca762a21a44a0d34ee00c243edb2858af457db5777e79d8a5501` using the documented `bundle.py prepare` command. Build output is `target/bundle-prepare.json`.
- Verified the archive and installed into the new `target/craft-local-development` directory. Ten payload files plus manifest and installation receipt are present. Installed binary hash matches the source, and offline `version` and `command find --query saved --limit 2` returned successful envelopes.
- Repeated preparation with the final tool into `target/craft-bundle-reproduced.zip`; both archives have SHA-256 `6d11f1a67e1ce94f57b89ecbd7e0509a59a04f362f73f6660e330890238cc676`. This proves reproducible packaging of these inputs, not reproducible compilation.

Authorized reviewer `/root/skill_review` performed source-only review. Two findings were fixed: validate qualification metadata and contract-version agreement, and execute an isolated pinned copy for the identity probe. Focused re-review found no remaining actionable blocker. The reviewer ran no tests or executables and edited no files.

The packaged binary retains [saved-source qualification](saved-request-source.md); packaging does not broaden it. Unrun or unfinished: native vault/live tenant qualification, supported host skill loading, non-Windows artifacts, complete redistribution license texts/notices, signatures, continuous integration, upgrade/uninstall and release qualification. Distribution remains unchecked in the ledger. [Local usage](../local-install.md) provides concrete commands and artifact identity.
