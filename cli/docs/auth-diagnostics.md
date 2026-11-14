# Inspect local authentication state

`auth status` and `auth doctor` are local read commands. They do not log in, refresh a token, verify product access, repair storage or write native credential items. The Windows native backend is qualified for these probes; other backend implementations remain unfinished.

Create/select a non-secret [profile](profiles.md), then inspect it:

```text
craft auth status --profile factory
craft auth status --profile factory --fields credential_state,shared_profiles
craft auth doctor --profile factory
craft auth doctor --profile factory --fields health
```

`--name factory` is an alternative explicit selector. Otherwise selection uses repository hints and then the active profile. Conflicting `--name` and `--profile` values fail. `--state-root` selects trusted private local state. These commands support strict structured `--input`, output budgets, selected fields and invocation timeouts like other local commands.

## Interpret status

| Field/value | Meaning |
| --- | --- |
| `credential_state: absent` | The locked grant index has no active credential generation |
| `stored_unverified` | An active generation is indexed; chunks, token expiry, scopes and live access have not been checked |
| `recovery_required` | A replacement intent is pending; status does not repair or activate it |
| `cleanup_pending` | Retired generation cleanup remains recorded |
| `shared_profiles` | Validated aliases in this installation referring to the same grant, available through explicit field selection |
| `token_state: unknown` | Status has not loaded token data |
| `live_verified: false` | No provider request was made; this field remains present under selection |

Status reads the bounded generation index under the native grant lock. It does not load secret chunks or serialize generation versions. An invalid index or inaccessible vault returns a safe error rather than pretending credentials are absent. The lock can create an internal metadata lock file; no native credential write occurs.

## Interpret doctor

Doctor reports the explicitly configured native backend's reserved read-only health probe. The default projection includes backend, read access and write access. Select `health` for principal, persistence and native item limit as well.

`read_access: available` establishes accessibility of one namespaced probe. It does not prove all credentials can be read or that a product accepts them. `write_access` remains `unknown`; doctor never tests it by creating an entry. It never unlocks a store or opens an authentication browser. An unsupported configured backend fails explicitly.

## Provisioning boundary

Secure service input has an internal pipe reader that accepts strict `client_id`/`client_secret` JSON through an actual native pipe. It refuses a terminal or redirected regular file before reading, enforces the provisioning byte limit and invocation budget, and uses zeroizing owned buffers. It is not yet connected to a public provision command. Do not pass secrets through normal `--input`, command arguments or chat.

An internal [interactive input guard](interactive-input.md) now has Windows native acceptance. Public provisioning, consequential auth commands, eligibility policy, live service access and independent review remain separate work. See [internal service auth](service-auth.md) and [diagnostic/input evidence](evidence/auth-diagnostics.md).
