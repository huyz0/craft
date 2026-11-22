---
name: verify-candidate
description: Check a software factory candidate against a pinned work order and produce evidence for the reviewer.
---

Identify the approved intent, acceptance criteria, candidate commit, and affected paths. Check the candidate against the agreed scope. Flag changes to test expectations, CI policy, or protected files for the responsible owner.

When a Factory verification packet is supplied, verify that the clean checkout is at the packet's full draft head. Use the acceptance criteria and scope in the packet. If the launcher reports that the packet is stale, request a fresh packet before attesting.

Use the repository's documented test entry points and trusted check instructions. Start with the focused checks that cover changed behavior. Record the command or CI run, tested commit, observed result, and any missing evidence. Report a blocker when acceptance cannot be established. Keep this independent of the implementer's claims.

Give the reviewer the proposed behavior, the test intent, any gaps, and links or paths to the consequential code and evidence.
