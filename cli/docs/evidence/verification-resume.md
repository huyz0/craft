# Resuming interrupted verification

Date: 10 November 2026. Scope: local verification tooling only; no public CLI command or tenant change.

The earlier change-aware selector avoided unchanged full matrices but discarded successful command progress when a later command failed. `tools/verify.py` now checkpoints each successful command atomically in `target/verification-progress.json`. Normal invocations reuse only an exact command, tracked-source snapshot and execution-environment digest. Failed/unstarted commands remain pending. Missing/malformed checkpoints, changed inputs/environment and explicit `--full` prevent reuse. Executed and reused counts are separate; cached passes do not establish a fresh full qualification. Previews remain read-only.

Qualified `tools/verify.py` SHA-256: `99054ab01f7a276a509dd0205d9c99465ba8be1bd04c23c4a15f1835472aa36e`.

The workflow scenario and `docs/validation.md` describe the boundary. The checkpoint is a local development optimization, not live, native-platform or independent approval evidence. Changes outside tracked inputs/environment require fresh qualification.

Observed validation:

- `python tools/verify.py` selected and passed planning and verifier selftests, with no Cargo command. Final implementation log: `target/verification-resume-final.log`.
- Real subprocess controls inside the verifier selftest cover successful-prefix preservation after a later exit-seven failure, reuse without incrementing the subprocess counter, source/environment invalidation, malformed checkpoint recovery and explicit fresh execution.
- Concurrent-edit controls finish the selected commands without caching unstable passes. A persistent documentation edit leaves only that edit pending in the baseline. Observed drift followed by reversion, or a persistent implementation edit, cannot qualify the original inputs.
- The earlier implementation and corrected runs are retained in `target/verification-resume.log` and `target/verification-resume-corrected.log`. They passed their then-current controls but were subsequently changed after review findings; the final log supersedes them.
- An unchanged smoke run recorded zero executed checks in `target/verification-resume-unchanged.log`. Final documentation checks are recorded in `target/verification-resume-documentation.log`.

Independent read-only review by the already-authorized `/root/activation_review` identified an early-abort regression for concurrent documentation edits and a subsequent drift/reversion false-green. Both were corrected with regression controls. No Rust, native-platform or live-provider matrix was repeated for this tooling-only fix. The full CLI scope remains incomplete; this work closes a verification inefficiency, not a product acceptance task.

The final review confirmed resolution, including temporary code drift that reverts while a documentation edit persists. The verifier keeps the union of observed changed paths, so final documentation-only differences cannot conceal that observed code change. The reviewer inspected source without editing files or executing tests.
