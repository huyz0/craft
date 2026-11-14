# Local verification and coding guards

Observed on 9 November 2026 on Windows x86_64 MSVC, Rust 1.99.1. `tools/verify.py` is development tooling only. It runs the layer guard, planning check, locked dependency inventory comparison, formatting check, workspace tests and all-target Clippy with warnings denied. It stops on failure and explicitly reports native/live qualification as unrun.

`python tools/verify.py --selftest` passed six isolated controls: independent core accepted, core-to-runtime dependency rejected, dev-only test support accepted, build-time test support rejected, semantic comment accepted and an artificial tracking label rejected. The temporary label was created outside production files under the excluded build directory and removed afterward. The first control run failed because the sandbox-selected OS temp directory was inaccessible; using the workspace's `target/tmp` resolved this without escalation or relaxed checks.

`python tools/plan.py selftest` passed all 15 planning controls. `python tools/verify.py` passed with 33 Rust behavior/subprocess tests and no Clippy violations. Compiler incremental-cache hardlink warnings persisted; Cargo copied files instead. No warning/lint setting was weakened.

The [review checklist](../review.md) explicitly covers semantic naming and comment accuracy against behavior/design. Reviewed the `InputField` API comment against the implementation, round-trip/schema tests and design: the containing object must supply a default and omit the missing variant, while explicit null survives. Also inspected native-route and prepared-operation comments against their actual checked-context and private-field behavior. This is the implementing agent's source review, not an independent human review.

No Git repository exists here, so no commit subjects were available to scan or rewrite. Semantic commit review is required when commits are created. Automated label scanning does not establish prose accuracy or detect every conceivable tracking identifier. No live auth, deletion, reviewer authority or unknown-write recovery is enabled by these local checks.

Development binary SHA-256 at initial guard qualification: `84719c76010785dbbb8d159f9f06faf02af71cc6c439fe89da7e21758d74de79`. Subsequent [fixture evidence](test-fixtures.md) records added tests, the loopback sandbox restriction, targeted native passes and the updated build digest. These identify observed local builds, not signed distributions.
