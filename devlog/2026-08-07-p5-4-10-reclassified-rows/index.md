# P5.4.10 (part) — two rows that need no seL4 gate

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Audit |
| Status | Verified |
| Scope | `roadmap/07-architecture-portability.md`, `contracts/generation/v1/fixtures/sel4*.zti`, `boot-contracts/src/generation.rs` |
| Roadmap | P5.4.10, P5.4.1, C7.1, B11 |
| Gates | `just test_host`, `just sel4_root_boot_check` |
| Trigger | P5.4.10's remaining rows, worked in order |
| Baseline | Six open rows after the B10 layout fixtures landed |

## Summary

Two of P5.4.10's rows — C7.1's retained-v2 rollback arm and B11's
product-vs-test profile pair — turn out to need no seL4 gate, for structural
reasons rather than by deferral. A v2 generation names its own kernel object, so
a rollback boots the v2-era kernel and never reaches `slime-root`; and v2
predates the ELF component revision entirely, so every payload it carries is
unloadable here by construction. B11's defect is a *shared* manifest whose
product graph declares probes as peers of real services — the seL4 fixtures are
per-scenario siblings, so there is no shared graph to contaminate. Both rows are
reclassified in the roadmap with the evidence, rather than left open against
gates that would assert nothing.

## Observable symptom

- Command: none — this is an audit of two recorded gaps, not a defect.
- Expected: each open P5.4.10 row either closes with a gate or is shown not to
  need one.
- Observed: two rows are unreachable on the seL4 path for reasons the inventory
  did not check when it recorded them.
- Exit/fault/serial evidence:
  [`retained-v2-tests.log`](retained-v2-tests.log),
  [`fixture-components.log`](fixture-components.log).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `boot_contracts::generation::decode` admits `MAGIC_V2`/`FORMAT_VERSION_V2`, and `slime-root` never inspects the version | A v2 generation would decode here, so "unreachable" cannot rest on the root refusing it |
| 2 | `retained_v2_generation_passes_stage0_admission` records that each generation embeds the kernel it boots, so a v2 rollback runs its v2-era kernel | The rollback path does not traverse `slime-root` at all. That is the load-bearing half |
| 3 | v2 predates the ELF component revision — `Revision::Elf` is the newer magic — so every v2 payload is a SLIMECM image | Even if booted here, no component could launch. `sel4_root_boot_check` already asserts exactly that shape with `slimecm=[1-9]\d* elf=\d+` |
| 4 | The retained P5.1 fixture is `SLIMEG3`, version 3 | The one retained generation on this path is not v2, so no existing gate accidentally covers or contradicts the claim |
| 5 | Counted the `components = [...]` block of all eight seL4 fixtures: 2, 2, 3, 3, 3, 7, 3, 5 | Each is its own scenario. `sel4.zti`, the product graph, declares five real components and no probe |
| 6 | `fabric-intruder` — the one probe-shaped component — appears only in `sel4-stream.zti` | The contamination B11 describes requires a shared manifest; these fixtures do not share one |

## Changes

| Area | Change | Effect |
|---|---|---|
| `roadmap/07-architecture-portability.md` | C7.1's row reclassified, with the two reasons and the host tests that keep the decode path covered | The row is closed on evidence rather than left open against a gate that would assert nothing |
| `roadmap/07-architecture-portability.md` | B11's row reclassified, with the per-fixture component counts | Same |

No code changed. That is the finding: the work these rows implied does not
exist.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cd boot-contracts && cargo test --all-features --lib retained_v2` | Pass — 5 tests keep the v2 decode, ceiling, rights, stage-0 admission, and manifest-width properties — [`retained-v2-tests.log`](retained-v2-tests.log) | Direct |
| Component census of all eight seL4 fixtures | [`fixture-components.log`](fixture-components.log) — grants excluded, since they are edges rather than components | Direct |
| `slime-root/fixtures/generation.bin` header | `SLIMEG3`, version 3 — read directly | Direct |
| `just sel4_root_boot_check` | Pass — its `slimecm=[1-9]\d*` marker is the existing assertion that an unloadable payload is reported unloadable | Direct |
| `just devlog_check`, `just typos` | Pass | Direct |
| A v2 generation booted under `slime-root` | **Not attempted.** Step 2 establishes the rollback path does not reach this root; constructing one to watch it fail to launch would assert step 3's tautology | Unobserved, with reason |

## Open risks and follow-ups

- [ ] **C7.1's reasoning depends on generations embedding their own kernel.** If
      that ever changes — a generation resolving the *running* kernel rather
      than its recorded one — the rollback path would reach `slime-root` and the
      row reopens. Recorded because the reclassification is only as durable as
      that property.
- [ ] **B11's reasoning depends on the sibling-fixture design.** `sel4.md`
      records why the fixtures are siblings rather than profiles of one
      manifest; if they are ever consolidated, B11's defect shape becomes
      possible and this row reopens with it.
- [ ] **Four P5.4.10 rows remain**: C8.1 collision rejection, C8.3 graph
      provenance, C8.4's structural arm, and `task_reclamation.rs`'s three
      properties.
- [ ] Reclassifying is weaker than gating, and it should stay uncomfortable. The
      claim is "this cannot happen here", which is a statement about today's
      design; a gate would be a statement about every future boot. Both rows say
      what would have to change for the claim to lapse.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`fixture-components.log`](fixture-components.log) — the
  component census.
- Serial/debugger/model output:
  [`retained-v2-tests.log`](retained-v2-tests.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md) (two rows
  reclassified), [C7.1](../../roadmap/02-core-runtime.md),
  [B11](../../roadmap/00-backlog.md).
