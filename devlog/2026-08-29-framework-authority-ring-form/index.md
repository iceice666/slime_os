# The storage-write allowlist still read the pre-IO2 grant shape

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Defect |
| Status | Fixed |
| Scope | `scripts/check/check-framework-authority.py` |
| Roadmap | IO2 |
| Gates | `just framework_safety_check` |
| Trigger | CI job `docs_gates` failed in 16 s on PR #11, the IO0–IO6 branch |
| Baseline | `framework authority check: 7 product fixtures grant blockWrite only to approved service owners` on `origin/main` |

## Summary

`just framework_safety_check` guards one invariant: only approved holders may
write storage. IO2 moved that authority out of `block` capability grants and
into `contracts/block-authority/v1` ring-authority tables, where a writer is a
row binding one `holder` to one device and ring rather than a capability with a
`target`. The checker was never updated, so it kept searching for the old
shape. It found `blockWrite` in the new table, looked for the `target` that
form does not have, and exited on its own internal error path — `has blockWrite
without a target`. Fixed by reading the `blockRingAuthority` table by
`holder`, plus a new assertion that refuses a silent return to the grant form.
The allowlist that results is strictly narrower than the one it replaces.

## Observable symptom

- Command: `just framework_safety_check` (CI job `docs_gates`)
- Expected: `framework authority check: N product fixtures grant blockWrite only to approved service owners`
- Observed: `framework authority check: contracts/generation-manifest/v1/compositions/sel4-filesystem.zti has blockWrite without a target`, exit 1
- Exit/fault/serial evidence: PR #11, run 33235225048, job 99054927953, failed 16 s in

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The other three gates in `docs_gates` — `x86_portability_check`, `devlog_check`, `sel4_gate_control_check` — all pass locally | The failure is `framework_safety_check` alone, not the job's environment |
| 2 | `just framework_safety_check` reproduces locally on the branch | Not a CI artifact; a real regression |
| 3 | A worktree at `origin/main` passes with `7 product fixtures` | The branch introduced it; the gate was green before |
| 4 | On main, `sel4-filesystem.zti` carries `blockWrite` inside a `capabilityKind = "block"` grant with `source`/`target`; on the branch it appears once, inside `blockRingAuthority`, with `holder`/`device`/`ring` | The representation moved; `blockRingAuthority` does not exist on main at all |
| 5 | Scanned every composition for `blockWrite` co-occurring with `capabilityKind` | Zero matches: authority now lives *only* in the ring tables, so the checker was reading a form that no longer exists |
| 6 | Extracted the actual holders per fixture; eight fixtures, one holder each, and no `-idle` instance among them | The `-idle` holders in `EXPECTED_WRITERS` did not move — they lost the authority; they now hold only an endpoint token |

## Root cause

`writers()` matched any brace-delimited entry containing `blockWrite`, then
required a `target` field. IO2's ring-authority rows identify the holder with
`holder`, not `target`, because the row binds a client to a *ring* rather than
minting a capability. The `fail` on a missing `target` was written as an
internal-consistency assertion about the grant form — a shape that can no
longer occur — so the gate reported a malformed fixture when the fixture was
correct and the checker was stale.

Two secondary facts matter for the fix and were confirmed rather than assumed.
The `-idle` instances listed in `EXPECTED_WRITERS` are still declared, so their
absence from the new table is a genuine narrowing of authority, not fixtures
being renamed. And `sel4-io-block.zti` is a new IO2 fixture whose `io-block-probe`
legitimately writes, so the allowlist grows from seven entries to eight while
each entry shrinks from two holders to one.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `check-framework-authority.py` — `writers()` | Reads the `blockRingAuthority` table by `holder`, scoped to that table rather than the whole file | The gate measures the representation the fixtures actually use |
| `check-framework-authority.py` — `EXPECTED_WRITERS` | Eight fixtures, one holder each; `-idle` entries dropped because those instances lost the authority; `sel4-io-block.zti` added | The allowlist names exactly today's writers, and is strictly narrower than before |
| `check-framework-authority.py` — `assert_no_grant_form_block_write()` | New. Fails if any composition grants `blockWrite` as a capability | A writer moved back out of the table cannot become invisible to the gate |

The table-scoped match is deliberate: IO1 budgets, notification bindings, and
`blockRead`-only rows all carry a `holder`, so matching `holder` file-wide would
report authority nobody has.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Authority widened to an unapproved holder | `EXPECTED_WRITERS` comparison | `storage-write authority drift; missing={…}, added={…}` |
| A writer smuggled back into a capability grant, invisible to the table scan | `assert_no_grant_form_block_write()` | `… grants blockWrite as a capability; storage-write authority belongs in blockRingAuthority` |
| A ring row with rights but no holder | Retained `fail` in `writers()`, reworded for the new field | `… has blockWrite without a holder` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just framework_safety_check` | exit 0 — `8 product fixtures grant blockWrite only to approved service owners` | Direct |
| `origin/main` worktree, same gate | exit 0 — `7 product fixtures …`; confirms the branch caused it | Direct |
| Control: `sel4-filesystem.zti` holder changed to `directory-probe` | exit 1 — `storage-write authority drift; missing={'sel4-filesystem.zti': {'sel4-filesystem-service'}}, added={'sel4-filesystem.zti': {'directory-probe'}}` | Direct |
| Control: a `capabilityKind = "block"` grant with `blockWrite` injected | exit 1 — `… grants blockWrite as a capability; storage-write authority belongs in blockRingAuthority` | Direct |
| Fixtures restored after both controls | `git diff --stat contracts/` empty; gate re-passes | Direct |
| `just x86_portability_check`, `just devlog_check`, `just sel4_gate_control_check` | All pass — the rest of `docs_gates` is unaffected | Direct |
| `just ruff` | `All checks passed!` | Direct |

Only a Python checker changed; no contract, component, or Rust source was
touched, so no QEMU plane gate was rerun.

## Decisions

- **Decision:** Update the checker to the ring-authority form rather than
  restore a `target` field to the fixtures.
  **Rationale:** The fixtures are right. IO2 deliberately made
  `(device, ring)` ordered strictly ascending *without* the holder, which is what
  lets the driver decide whose rights a submission carries; reintroducing a
  capability-shaped writer would undo that.
  **Rejected alternative:** Relaxing the `target` assertion to a `continue`,
  which would have made the gate pass by measuring nothing.

- **Decision:** Add `assert_no_grant_form_block_write()` rather than only
  retarget the parser.
  **Rationale:** Retargeting alone leaves a hole in the opposite direction — a
  writer moved back into a grant would be silently unmeasured, exactly the class
  of false pass that made this gate necessary. Cheap to assert, and the control
  above shows it fires.

## Open risks and follow-ups

- [ ] `EXPECTED_WRITERS` is still a Python literal rather than a blessable
  fixture, the same shape B63 already records for marker expectations. Adding a
  ninth block-holding composition means editing this file by hand.
- [ ] The checker is regex-based over `.zti` text, not a Zutai decode. That
  predates this fix and is why both the stale-field bug and its brace-matching
  fragility were possible; a decoder-backed check would be structurally immune.

## Artifacts and provenance

- Checker: `scripts/check/check-framework-authority.py`
- Gate: `just framework_safety_check`, in CI job `docs_gates`
- Failing run: PR #11, run 33235225048, job 99054927953
- Authority contract: `contracts/block-authority/v1/schema.zt`
- Related roadmap item: [IO2](../../roadmap/11-io-substrate.md)
- Introducing entry: [`devlog/2026-08-28-io2-userspace-virtio-blk/`](../2026-08-28-io2-userspace-virtio-blk/index.md)
