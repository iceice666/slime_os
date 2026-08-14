# B50 — deleting `endpointCreate`, and what it did not unblock

| Field | Value |
|---|---|
| Date | 2026-08-13 |
| Kind | Change |
| Status | Verified |
| Scope | eleven `contracts/generation/v1/fixtures/*.zti`, 24 `contracts/boot-layout/v1/fixtures/*.layout`, `scripts/check/check-boot-layout-resource.py` |
| Roadmap | B50, B46 |
| Gates | `just contracts_check`, `just sel4_boot_layout_check`, `just test_sel4_root`, `just lint_all` |
| Trigger | `just contracts_check` red at `e02a232`; `SLIME_GRAPH FAIL binding init-endpoint-factory names no installable resource` on three plane gates |
| Baseline | `endpointCreate` declared in eleven fixtures, refused by admission on every plane that used it |

## Summary

The native-IPC cutover removed the `EndpointCreate` operation, so
`declared_resource` has no arm for that right and admission refuses any binding
carrying it. The grant nevertheless survived in eleven fixtures, where it was
the first thing that killed several planes. Deleting it — nineteen grants, their
bindings, and a projection assertion — is a clean B50 deletion: `contracts_check`
goes green and the boot-layout bless is a net 29-line *deletion*. It did not
make the three affected plane gates pass, and the reason is the useful part of
this entry.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Eleven generation fixtures | Nineteen `endpointCreate` grants and their bindings deleted | A declared right names a resource the root can install |
| `check-boot-layout-resource.py` | `ENDPOINT_FACTORY_SLOT` dropped from the bootstrap projection | The projection asserts only roles that exist |
| 24 boot-layout fixtures | Re-blessed; net 29 lines removed | The frozen transcription matches the manifests |

The `endpoint-factory` **layout role** stays. It is a numbered entry in a
generated contract (`ROLE_ENDPOINT_FACTORY = 1`), so removing it renumbers every
role after it across `boot_layout.py`, `boot-contracts`, and the generated
bindings — a separate deletion with a far larger blast radius and no gate
depending on it.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A fixture reintroduces a right the root cannot install | `just contracts_check` | build failure naming the grant |
| The blessed layouts drift from the manifests | `just sel4_boot_layout_check` | byte mismatch against the fixture |
| The deletion disturbs a working plane | `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_visibility_check` | plane failure marker |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | PASS (was FAIL at `e02a232`) | Direct |
| `just sel4_boot_layout_check` | PASS on the re-blessed baseline | Direct |
| `just test_sel4_root`, `just test_host` | PASS | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | PASS | Direct |
| `sel4_stream/qos/visibility/channel/crossing/root_boot_check` | PASS | Direct |
| `sel4_input_check`, `sel4_spawn_check`, `generation_check` | FAIL — also FAIL at HEAD before this change | Direct |

## Decisions

- Decision: delete the right, keep the layout role.
- Rationale: the right is dead — no call site reads `ENDPOINT_FACTORY_SLOT` —
  while the role is a numbered slot in a generated contract whose removal
  renumbers everything after it.
- Rejected alternative: deleting both at once, which mixes a behaviour fix with
  a contract renumbering in one bless.

- Decision: do **not** convert `sel4-spawn.zti`'s minted bindings to declared
  grants.
- Rationale: its seven minted bindings are orphans with no grant behind them, so
  declaring them admits and boots the graph — and then `console` and `sysinfo`
  block forever awaiting a launch context, because
  `check-sel4-spawn-plane.py` asserts `grants=1` and six *at the spawn marker*.
  That plane's claim is that a parent hands its child capabilities at spawn;
  moving them into the generation deletes the property under test while making
  the gate's build succeed. The fix is init supplying them.
- Rejected alternative: the mechanical conversion that worked for
  `sel4-call.zti`. It is right only where the gate does not assert the handover.

## Open risks and follow-ups

- [ ] `sel4_input_check`, `sel4_spawn_check`, `sel4_supervision_check`, and
      `generation_check` now fail one layer deeper, on `spawn preflight …
      reason=declared-count`. Each needs init to supply the declared set, judged
      per plane against what its gate asserts.
- [ ] The probe planes' `*-run-token` minted bindings were an attempted
      conversion and reverted: the token is a native endpoint, so neither
      `cap_drop` (which addresses the root's logical table) nor a non-blocking
      `recv` (which faults on an empty slot) can test for its presence. A probe
      distinguishing the driven instance from the idle one needs a primitive
      that does not exist yet.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: reproduce with `just contracts_check` and `just sel4_input_check`
- Serial/debugger/model output: quoted inline above
- Related roadmap item: `roadmap/00-backlog.md` B50
