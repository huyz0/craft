# Use the local development bundle

The current Windows bundle contains Craft 0.1.0, its 78-command contract, the progressive `craft-cloud` skill and dependency license metadata. It is an internal development artifact. Current validation is partial: Windows Application Control blocked the Confluence composed-host executable before execution. Workspace, eight unaffected Jira hosts and focused recovery checks passed; full qualification remains pending. Native vaults, live providers, host skill loading, other platforms, upgrade/uninstall and release distribution remain unqualified. No project redistribution license has been selected; the dependency inventory is not complete redistribution notices.

The prepared archive is `target/craft-0.1.0-windows-local-artifacts.zip`. Its SHA-256 is:

```text
bb7674ad7b409a5bc403dedcb0a4fc4e09c3da432eb5120f42d401f42d2b9625
```

Use Python 3.11 or later from the `cli` directory. Verify using the independently retained digest, then install into a new absolute directory whose parent already exists:

```powershell
python tools/bundle.py verify --archive target/craft-0.1.0-windows-local-artifacts.zip --sha256 bb7674ad7b409a5bc403dedcb0a4fc4e09c3da432eb5120f42d401f42d2b9625
python tools/bundle.py install --archive target/craft-0.1.0-windows-local-artifacts.zip --sha256 bb7674ad7b409a5bc403dedcb0a4fc4e09c3da432eb5120f42d401f42d2b9625 --destination D:/work/aifactory/cli/target/my-craft
.\target\my-craft\bin\craft.exe version
.\target\my-craft\bin\craft.exe --help
```

The actual checked installation is already at `target/craft-local-artifacts`. Run `target/craft-local-artifacts/bin/craft.exe` directly. Installation refuses any existing destination, leaves PATH and existing Craft executables alone, and does not copy skills into a Codex or Claude Code directory. An incomplete installation remains visible and must not be reused as a new destination.

`install-receipt.json` records the archive digest and destination. `manifest.json` records exact payload sizes and hashes, development status, version and platform. Verification checks the same bounded archive bytes that installation consumes, plus strict member names, metadata and skill-contract compatibility. The supplied digest must come from a trusted build record; a hash supplied alongside an untrusted archive is not a signature or proof of origin.

The normal executable's SHA-256 is `b0933297b3ee2396a0f766f21c8b46e9919634949b709770ab2ba38b92d29487`. To reproduce the bundle from this exact local build, choose a new output filename:

```powershell
python tools/bundle.py prepare --binary target/debug/craft.exe --binary-sha256 b0933297b3ee2396a0f766f21c8b46e9919634949b709770ab2ba38b92d29487 --output target/my-craft-bundle.zip
```

Preparation never overwrites an archive and requires the normal workspace development executable. It probes a private copy of the pinned bytes, checks local version/contract/skill compatibility and includes the locked dependency inventory. It does not establish that an arbitrary executable is qualified. [Current bundle evidence](evidence/local-artifact-recovery.md) identifies the tested source and limitations; [cloud quickstart](cloud-quickstart.md) covers profiles and commands.
