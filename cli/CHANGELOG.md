# Changelog

Versions follow [semantic versioning](https://semver.org). A release tag is `v<version>` and must match `[workspace.package] version` in `Cargo.toml` (`python tools/release.py check --tag v<version>`).

## 0.1.0

Preview. First packaged build.

- 114 public commands over Jira and Confluence Cloud, with the machine-readable contract in `schemas/local-contracts.json`.
- Local whiteboard scene tools (`layout`, `check`, `preview`, `encode`, `decode`) and Confluence folder and whiteboard commands.
- Three skills: `craft-cloud` (operating the CLI), `confluence-pages` (page bodies) and `atlassian-design` (colors and layout). `python tools/skills_qualify.py` checks them against the contract and a built executable.
- Known limits: executable is unsigned; no redistribution license is selected; Codex and Claude Code skill loading is unqualified; macOS is built in CI only and unqualified; Confluence whiteboard create is refused for service-account credentials.
