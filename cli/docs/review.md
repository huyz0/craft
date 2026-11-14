# Focused implementation review

Read the changed behavior's capability scenario and design section, then inspect the implementation and observed evidence. A reviewer can be a human or an explicitly authorized independent reviewer; this checklist does not authorize parallel agents.

- Do source/test/configuration names describe behavior, with planning traceability kept in evidence documents?
- Do changed comments explain the implementation's actual contract, invariant, failure mode or non-obvious design choice? Remove progress notes and obvious restatements. A regex cannot verify comment accuracy.
- Do source dependencies follow the design's core/host/provider/workflow direction? Test-support belongs only in dev dependencies.
- Do local malformed requests fail before auth/network effects? Do partial and unknown results retain confirmed effects without claiming completion?
- Does evidence distinguish deterministic, native-platform and live-provider results? Is consequential auth, destructive/review authority or unknown-write behavior independently reviewed before being enabled?

For example, the `InputField` documentation requires a containing field to supply a default and omit the missing variant during serialization. The behavior test validates this contract against both serde and the generated schema, including explicit null. Its comment describes the required API behavior rather than its delivery status. No source comment needs a planning identifier to make that relationship reviewable.

Run `python tools/verify.py` from `cli/` for the local gate. `python tools/verify.py --selftest` checks isolated label and dependency-layer negative controls; `python tools/plan.py selftest` checks the planning validator. Commit subjects need semantic review because this workspace currently has no Git history or commit hook to inspect.
