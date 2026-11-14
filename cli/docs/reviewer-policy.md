# Configure trusted reviewers locally

`review policy get/set` manages operator-declared reviewer eligibility for one Confluence space. It reads no keyring, contacts no provider and creates no account, remote permission or human approval. Public live review verification remains unavailable.

Use an explicit existing Confluence Cloud service profile for every administration call. Active profiles and repository hints cannot select this authority. Account IDs are exact non-secret identifiers; obtain them independently rather than copying display names or inventing them.

```text
craft --profile cf-test review policy get --space-id 100
craft --profile cf-test review policy set --input reviewer-policy.json
```

`reviewer-policy.json` contains non-secret configuration:

```json
{
  "space_id": "100",
  "expected_actor": "service-account-id",
  "enabled": true,
  "reviewers": ["human-account-id"],
  "excluded_actors": ["another-worker-account-id"],
  "quorum": 1,
  "expected_revision": null,
  "apply": true
}
```

The profile supplies the cloud and issued grant. Optional `cloud_id` must match that profile. `expected_actor` is the intended service identity, not a supplied identity proof. Reviewer IDs declare human eligibility under operator policy; Craft does not infer humanity from account metadata. The service actor, duplicate IDs and overlaps between reviewers/exclusions are refused. Lists are stored in canonical order; quorum counts distinct eligible reviewers. Bounds are 64 IDs per list, quorum 1–64, and 60 KiB of serialized eligibility data.

An absent expected revision creates only. Read the current record before changing it and pass its revision; a stale revision fails without overwriting competing intent. Enabled state and apply are explicit. Disable before transferring the expected service actor. Changing eligibility or quorum changes the revision and invalidates captured policy. Disablement retains the record for inspection; it does not revoke the native credential grant.

```text
craft --profile cf-test review policy set --space-id 100 --expected-actor service-account-id --enabled false --reviewers human-account-id --excluded-actors another-worker-account-id --quorum 1 --expected-revision 1 --apply
```

Responses always retain the complete policy receipt; output projection is refused. A confirmed set returns the observed stored policy and `configuration_write_state: confirmed`. An uncertain commit returns `policy: null`, the proposed policy separately and `sent_unknown`. Read the same profile/space under the same state root before retrying. Do not confuse the proposal with observed state. Increase `--max-bytes` before applying a policy too large for the default output budget.

`live_verified` and `review_verified` remain false. Configuration does not verify a real account, page, approval or Jira mapping. The bridge into the internal verifier rechecks the complete enabled native record, including installation/profile/grant/space and revision; stale, disabled or forged snapshots cannot become trusted configuration.

The local state-root owner is the administration boundary. Processes allowed to write that root can administer policy; this is not isolation between workers sharing an OS principal. Worker tool permissions should exclude administration where a separate operator boundary is required. Shared team/broker authority remains separate work.

Private schema twelve adds an empty reviewer table on upgrade. Existing profiles, grants, provider policies, cursors, journals, settings format and credentials remain unchanged. [Evidence](evidence/reviewer-policy.md) records qualification and limits; [review contract](review-contract.md) describes the future live verification path.
