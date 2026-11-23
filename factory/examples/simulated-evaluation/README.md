# Paired simulated Factory evaluation

From the source checkout, run both comparisons against a qualified extracted Factory kit and disposable local PostgreSQL. Use a new output directory. On Linux, run as a non-root user. The runner starts temporary PostgreSQL and loopback HTTPS with fixture credentials.

```bash
python -B factory/tools/compare_disposable_evaluation.py \
  --kit /path/to/extracted-kit --postgres-bin /path/to/postgresql/bin \
  --output factory/target/simulated-evaluation
```

[cases.json](cases.json) is the source for four matched cases covering input validation, CI repair, dependency compatibility and documentation handoff. The original [rehearsal scorecard](../pilot-scorecard.csv) records a separate observation.

Windows and Ubuntu produced identical [M3 scorecard input](m3-scorecard.csv), [M3 results](m3-results.json), and [M6 results](m6-results.json).

- **M3.** Both lanes use the same task descriptions and expected outcomes. Each scorecard input lists outcome, started, finished and abandoned attempts, repair cycles, queue and elapsed minutes, the four active-time fields in specification, steering, review and repair order, input, cached and output tokens, simulated USD, and material misses. The scorecard reports verified drafts and other test outcomes. Simulated reviews do not count as human acceptance, so cost per accepted change is unavailable.
- **M6.** The same pinned inputs and token reservation go to a mock Codex medium-effort profile and a mock Claude high-effort profile. The runner submits scripted Git candidates and usage to the disposable queue. A separate verifier identity checks the only changed path and the expected bytes. The launcher compares matched work orders through the regular shared-queue command. One fixture records a start without a candidate or usage so the reservation remains visible and the case is unscored.

All durations, usage, costs, candidate results and review decisions are simulated. This run tests accounting, candidate binding and review mechanics. The adapter and effort differ together in M6, so the results cannot isolate an adapter effect. No model or provider is contacted.
