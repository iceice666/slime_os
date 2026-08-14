# P5.4.3 — M6.6's powerbox, and a placement order that is an ABI

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/main.rs`, `components/bins/src/bin/init.rs`, `components/bins/build.rs`, `contracts/generation/v1/fixtures/sel4-powerbox.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{powerbox-plane,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.3, P5.4, M6.6 |
| Gates | `just sel4_powerbox_check`, all 24 seL4 plane gates, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check`, `just contracts_check` |
| Trigger | M6.3's directory mechanism and `InputRead` both landed; M6.6 needed only them |
| Baseline | No powerbox plane; the directory transfer kind had just been added |

## Summary

A chooser holds directory authority the requester lacks and hands over exactly
one narrowed view on a selection gesture. The requester holds one RPC endpoint
and verifies, before asking for anything, that it holds no directory at all — so
the capability it later inspects can only have come from the mint.

Three of the four claims are refusals: a request for rights the chooser itself
does not hold is denied, the transferred view cannot be derived past its scope,
and a cancellation mints nothing.

**Both components are the oracle's, unmodified** — `powerbox-chooser.rs` and
`powerbox-probe.rs`, shared with `just powerbox_check`. M6.6 needed no new
mechanism: the directory capability, its transfer kind, and `InputRead` all
landed in the two slices before it.

M6.6 is closed. **M6.3, M6.5, and M6.6 are complete on seL4**; M6.4 and M6.7
remain, both blocked and both recorded.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| generation 32, `SEL4_POWERBOX_LAYOUT`, build wiring | The plane's artifact | The gate boots what it asserts about |
| `main.rs` | The plane's key script: select, then escape | A deterministic gesture sequence |
| `init.rs` | `drive_powerbox_plane` | Init composes a channel and grants no directory |
| `main.rs` | Both placement lists reordered: directory, input, factories, block | A component finds its capabilities where it was compiled to look |

### The placement order is an ABI, and this is the second time

`powerbox-chooser.rs` reads a directory at slot 1 and input at slot 2. The
previous slice had ordered declared authority input-first — chosen to satisfy
`dango.rs` — so the chooser read its input capability as a directory, prompted
once, and never saw the selection keystroke.

The order is now directory, input, endpoint factory, buffer factory, block, in
*both* placement paths, with a comment on each saying it is an ABI rather than a
preference. That is the third defect in this area across two slices:

1. the two paths disagreed with each other (caught by the filesystem gate);
2. the shared order disagreed with `dango.rs`;
3. the shared order disagreed with `powerbox-chooser.rs`.

The real problem is that the order is implicit. A component's expected slot
layout is written in its own constants and nowhere else, so the root and the
component agree only by inspection. The boot layout already solves exactly this
for the bootstrap component — it is declared data, checked against a fixture —
and non-bootstrap components have no equivalent.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The requester had a directory all along | it verifies it holds none before asking | "was not confirmed to hold no directory" |
| The chooser launders authority upward | a widening request must be denied | `derive closure denied` missing |
| The granted view is not narrowed | the probe checks the scope is `note` and `directoryWrite` is absent | `selected single object received` missing |
| The chooser grants rights it lacks | the provenance record is checked for `directoryWrite` | "granted directoryWrite it does not hold" |
| A cancellation mints something | exactly one capability may cross all three requests | "N capabilities crossed" |
| A mint leaves no record | the provenance marker is required with its gesture, path, and rights | marker missing |
| The placement order drifts again | `sel4_powerbox_check`, `sel4_filesystem_check`, `sel4_input_check` | a component reads the wrong capability |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 11 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_powerbox_check` | Pass; 11 markers, one capability crossed with rights `0x80004` | Direct |
| `just sel4_gate_control_check` | Pass; 24 gates reject 962 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 21 plane layouts match their fixtures | Direct |
| The other twenty-three seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just contracts_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| M6.6 against the oracle's `just powerbox_check` markers | Not compared line by line | — |

## Decisions

- **Decision:** Reuse both oracle components unmodified.
  **Rationale:** the same argument as the filesystem plane. A chooser written
  for this plane could encode its quirks; the oracle's predates it, so its
  passing is evidence about the mechanism rather than about the test.

- **Decision:** Assert the "no directory of its own" arm by presence, not
  position.
  **Rationale:** the root launches an unconfigured copy that reaches it before
  init spawns anything. The claim is true of both instances — which is the
  arm's point — so ordering it would pin an accident of scheduling.

- **Decision:** Count capabilities crossing the channel rather than trusting the
  probe's refusal assertions.
  **Rationale:** the probe can tell that a reply carried no capability. It
  cannot tell that the chooser did not mint one and drop it, and "mints nothing
  on cancellation" is a claim about the mint.

- **Decision:** Fix the placement order rather than change `powerbox-chooser.rs`.
  **Rationale:** the component is the oracle's and its being unmodified is the
  evidence. The root is the thing that should agree with its components.

## Open risks and follow-ups

- [ ] **A non-bootstrap component's slot layout is implicit.** Three defects in
      two slices came from the root and a component disagreeing about it, each
      found by booting rather than by a check. The boot layout already solves
      this for the bootstrap component as declared, fixture-checked data; every
      other component agrees with `construct_child` only by inspection. Extending
      the layout contract to cover them would turn a class of boot failures into
      a build failure.
- [ ] The gate asserts this plane's markers, not the oracle's
      `just powerbox_check` set. The components are identical, so the arms are;
      nobody has diffed the two gates' expectations.
- [ ] M6.4 remains blocked on B30, M6.7 on B29.

## Artifacts and provenance

- Gate output, the provenance record, and the transfer count:
  [`powerbox-check.txt`](powerbox-check.txt).
- The directory mechanism it hands out:
  [`devlog/2026-08-08-p5-4-3-directory-plane/`](../2026-08-08-p5-4-3-directory-plane/index.md).
- The input mechanism its gesture uses, and the first two placement defects:
  [`devlog/2026-08-08-p5-4-3-input-mediation/`](../2026-08-08-p5-4-3-input-mediation/index.md).
- B29 and B30 in [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
