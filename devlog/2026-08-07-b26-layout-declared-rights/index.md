# B26 — the boot-layout dump reported the grant's rights, hiding a too-permissive layout row

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/main.rs`, `scripts/check/check-sel4-boot-layout.py`, `contracts/boot-layout/v1/fixtures/sel4-{loan,sample,stream}.layout` |
| Roadmap | B26, B10, P5.4.6 |
| Gates | `just sel4_boot_layout_check` |
| Trigger | Fault-injecting the P5.4.6 call plane's newly frozen layout |
| Baseline | `just sel4_boot_layout_check` freezing nine plane layouts, all matching |

## Summary

`just sel4_boot_layout_check` could not see a boot-layout row that declares
*more* authority than its generation grant confers. The `[layout]` dump printed
each row's rights from the **installed capability**, which comes from the grant,
rather than from the layout entry the row exists to freeze — and the two are
related by containment rather than equality, so a layout could be arbitrarily
permissive without moving a single fixture byte. Found by fault injection while
closing P5.4.6's layout guard: perturbing a row's rights rebuilt the generation
to different bytes and the gate still passed, while perturbing a slot *number*
in the same table was caught instantly. Fixed by appending `declared=0x…` to a
row whose layout rights differ from its installed ones. The fix immediately
surfaced three pre-existing disagreements in already-frozen fixtures.

## Observable symptom

- Command: perturb `SEL4_CALL_LAYOUT`'s `fabric-call-server` row from `0x10008`
  to `0x1000c` (adding `RIGHT_TRANSFER`), rebuild with
  `python3 scripts/build/build-sel4.py --call-plane --skip-pin-check`, then run
  `python3 scripts/check/check-sel4-boot-layout.py --no-build`.
- Expected: the gate fails — the declared table and the frozen fixture disagree.
- Observed: `seL4 boot layout check: 9 plane layouts match their fixtures`,
  exit 0. The boot printed `[layout] 5 executable fabric-call-server 0x10008` —
  the *grant's* value — with the layout's `0x1000c` nowhere in the transcript.
- Exit/fault/serial evidence: [`fault-injection.log`](fault-injection.log).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The perturbed build's `generation.bin` md5 differs from the clean one (`484178e4…` vs `3f82c1f1…`) | The injection does reach the encoded resource; this is not a stale-artifact or caching artifact |
| 2 | Swapping the `fabric-call-server` and `fabric-call-time` rows *is* caught, printing both moved rows | The gate works; the blindness is specific to the rights field rather than general |
| 3 | Booting the perturbed image directly prints `[layout] 5 executable fabric-call-server 0x10008` | The dump never carries the layout's value at all, so no fixture could record it |
| 4 | `slime-root/src/main.rs` printed `capability.rights`, filled by `launch_component_graph` from the generation grant | The dump reports what was *installed*, not what the layout *declared* |
| 5 | `bootstrap_executable_slot`/`bootstrap_slot` test `rights & !entry.rights != 0` | Containment, not equality — and deliberately so, so the two values are *allowed* to differ and one of them cannot stand in for the other |
| 6 | The predecessor: `sel4-call.layout`'s first blessing carried `0x20004`/`0x1000004` on its two factory rows, copied from generation 17, and the gate accepted them | The gap had already produced a wrong fixture once, caught by a reviewer reading rather than by anything running |

## Root cause

`slime-root/src/main.rs`'s B10 dump emitted four fields per row — slot, kind,
label, rights — matching `kernel/src/runtime/bootstrap.rs::dump_boot_layout`'s
line shape so the two are comparable. The `rights` field was
`capability.rights`: the rights of the capability actually installed in init's
table, which `launch_component_graph` takes from the generation's grant.

The boot layout's own `entry.rights` never appeared. Because
`channel::bootstrap_slot` and `bootstrap_executable_slot` gate on containment
(`rights & !entry.rights != 0`) rather than equality, a layout row may
legitimately declare more than the grant confers — that is the documented
behaviour, and requiring equality once rejected a well-formed graph, since a
layout marks a channel half `RIGHT_TRANSFER` because init hands it on while the
grant is not about delegation at all.

So the two values are independent by design, and the transcript carried only
one. A layout row could grant anything at all above the grant's mask and every
fixture would stay byte-identical. B10 exists to keep the table that *declares*
a slot and the table that *fills* it in agreement; this was the one direction of
disagreement nothing could observe.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/main.rs` | `declared_layout_rights` resolves the layout entry behind a bootstrap row — by identity for an executable, by role for the two singular factories — and the dump appends `declared=0x…` when it differs from the installed value | The transcript states both what the layout declared and what the root placed |
| `scripts/check/check-sel4-boot-layout.py` | `ENTRY` admits the optional `declared=0x…` tail | A row carrying the new field is well formed rather than malformed |
| `sel4-{loan,sample,stream}.layout` | Re-blessed: each records `declared=0x1000004` on its shared-buffer-factory row | Three real, previously invisible disagreements are now frozen |

Appended rather than substituted, and only on disagreement. Every row where the
two agree — which is every row of every other fixture — keeps the retired
kernel's exact four fields, so the two dumps stay comparable slot for slot and
twenty-five of the twenty-eight fixtures are untouched.

A channel end is deliberately excluded. It is named by its *grant*, and one
capability can be reached by more than one grant name, so reporting a declared
value would mean picking one arbitrarily. `resource_label` already reports `-`
for it. Executables and the two singular factories are the rows where a layout
entry is unambiguous, and they are the rows a layout edit actually touches.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A layout row declares more authority than its grant confers | `just sel4_boot_layout_check` | `now: [layout] … declared=0x…` against a frozen row without it |
| A row's declared and installed rights silently converge or diverge | `just sel4_boot_layout_check` | The `declared=` tail appears or disappears versus the fixture |
| The new field breaks the oracle-comparable line shape | `just sel4_boot_layout_check` | `malformed entry` for any row the `ENTRY` pattern rejects |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_layout_check` before the fix, with the rights injection applied | **Passes — the defect.** `9 plane layouts match their fixtures`, exit 0 | Direct — [`fault-injection.log`](fault-injection.log) |
| The same injection after the fix | **Fails as intended** — `now: [layout] 5 executable fabric-call-server 0x10008 declared=0x1000c` | Direct — [`fault-injection.log`](fault-injection.log) |
| Restored, all eight other planes rebuilt, gate re-run | `9 plane layouts match their fixtures` | Direct |
| Re-blessing after the fix | Exactly three fixtures moved, one line each, all `shared-buffer-factory … declared=0x1000004` | Direct |
| The nine seL4 plane gates | All pass | Direct |
| `just test_sel4_root` | 109/109 across 13 modules | Direct |
| `just contracts_check`, `just devlog_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| The nineteen x86 fixtures | **Unmoved, and unmovable by this change.** Their dump is `kernel/src/runtime/bootstrap.rs::dump_boot_layout`, in the frozen oracle, which this does not touch | Direct |
| `just boot_layout_check`, the x86 gate | **Cannot run on this host** — needs OVMF firmware absent from this store; pre-existing | Inherited |

## Decisions

- Decision: append a field rather than replace the existing one.
- Rationale: the four-field shape is what makes this dump comparable to the
  oracle's, and P5.4.final's equivalence argument rests on that comparability.
  Replacing `rights` with the declared value would have made every row disagree
  with the oracle for rows where nothing is wrong.

- Decision: emit `declared=` only when the two differ.
- Rationale: a fixture should read as the exception it records. Emitting it on
  every row would move all twenty-eight fixtures to say "these agree", which is
  the default and not worth freezing, and would bury the three rows that
  actually disagree.

- Decision: re-bless the three surfaced fixtures rather than change the layouts
  to match.
- Rationale: `0x1000004` on a shared-buffer-factory row is a *correct* layout
  entry — the extra bit is `RIGHT_TRANSFER`, and containment permits it. The
  disagreement is information, not a defect, and the point of the fix is to
  record it rather than to erase it.

- Rejected alternative: tightening `bootstrap_slot` to equality. That is the
  change the containment rule was written to prevent, and it would reject
  well-formed graphs — the failure mode `channel.rs`'s own comment records.

## Open risks and follow-ups

- [ ] **Channel-end rows still carry only the installed value.** A layout entry
      for a channel half is named by a grant, and a capability can be reached by
      more than one, so the declared value is ambiguous there. If a layout edit
      ever widens a channel row's rights, this gate will still not see it.
- [ ] **The x86 dump is unchanged**, so the nineteen oracle fixtures retain the
      original blindness. `kernel/` is frozen until P5.4.final, and its gate
      cannot run on this host, so this is recorded rather than fixed.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`fault-injection.log`](fault-injection.log).
- Serial/debugger/model output: [`fault-injection.log`](fault-injection.log).
- Related roadmap item:
  [B26](../../roadmap/00-backlog.md),
  [B10](../../roadmap/00-backlog.md),
  [P5.4.6](../../roadmap/07-architecture-portability.md).
- Found while closing the layout guard in
  [`devlog/2026-08-07-p5-4-6-call-spawn-semantics/`](../2026-08-07-p5-4-6-call-spawn-semantics/index.md).
