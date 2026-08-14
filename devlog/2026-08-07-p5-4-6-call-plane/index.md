# P5.4.6 (part) — the C8.6 call plane builds and boots; the broker's slot model does not fit

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Root-caused |
| Scope | `contracts/generation/v1/fixtures/sel4-call.zti`, `scripts/build/{build-generation,build-sel4}.py` |
| Roadmap | P5.4.6, P5.4, P5.4.1, C8.6 |
| Gates | none |
| Trigger | P5.4.6 opened after P5.4.2's device half proved blocked |
| Baseline | Nine seL4 plane images; C8.6 with no seL4 coverage |

## Summary

A tenth seL4 image, `sel4-call`, embedding a C8.6 call generation: one
`ParameterCall` route, two clients, a server, and a time source. It builds,
admits its fabric graph, launches all five components, and mints every declared
control channel. It does **not** pass: the call broker resolves its endpoint
factory at a fixed slot 0, and on seL4 a non-bootstrap component's factories
land at `executables + 1`. The plane is committed in this state deliberately —
the scaffolding is correct and reusable, and the blocking finding is worth more
recorded than re-derived.

## Observable symptom

- Command: `qemu-system-aarch64 … -kernel build/slime-sel4-call.elf`
- Expected: the C8.6 transcript — role provisioning, correlated replies,
  duplicate/stale rejection, distinct terminal events.
- Observed: graph admitted with the right shape
  (`schemas=1 routes=1 participants=3 interpositions=0`), all five components
  staged, five control channels minted, then
  `[fabric] fail: call role request` and three `[fabric-call] fail: call send`.
- Evidence: [`boot.log`](boot.log).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | First build failed on `assert!(fabric_worker_wait_sources("stream") <= MAX_WAIT_SOURCES)` | A latent defect: a call-only graph is the first with no stream route, and the assert admitted no `WORKER_ABSENT` where both sibling brokers do. Fixed separately in `f9d2434` |
| 2 | Plane booted but ran the *stream* path | The broker is selected by `SLIME_FABRIC_CALL_CHECK` at build time; the manifest→flag table needed a `sel4-call` row |
| 3 | `fabric-call-time` failed `time phase receive` | It sends on slot 0 and waits on slot 1 — two endpoints. The x86 boot layout hands it both; seL4 numbers from grants, so a second grant is required |
| 4 | With the second grant, all five tasks parked | The fabric never reached its first marker: it was reading controls from `FABRIC_FIRST_CONTROL_SLOT` = 2 while its own first control sat at slot 0 |
| 5 | Granting `fabric-service` its own factories moved them to slots 1 and 2 | `next_runtime_slot = executables + 1` (`slime-root/src/main.rs:1105`), and the fabric has zero executables, so slot 0 is taken by its first *channel*, not a factory |
| 6 | The working stream plane grants factories only to `init` | The stream broker never mints route endpoints; the call broker does, so it genuinely needs a factory the stream plane does not — the two planes cannot share one grant shape |

## Changes

| Area | Change | Effect |
|---|---|---|
| `sel4-call.zti` | New fixture: 5 components, 14 grants, a one-route `ParameterCall` graph | The generation builds and admits |
| `build-generation.py` | `sel4-call` in the manifest registry and in the manifest→plane-flag table, mapped to the oracle's own `SLIME_FABRIC_CALL_CHECK` | The image compiles the call broker rather than the stream one |
| `build-sel4.py` | `--call-plane`, `CALL_VARIANT`, image and manifest paths | `build/slime-sel4-call.elf` |

The flag is the oracle's rather than a new seL4-only one on purpose: the call
broker is the same code, and a separate flag would let the two planes diverge
with nothing noticing.

## Regression guards

None yet, and that is the finding rather than an omission: the plane does not
pass, so registering a gate would register a red one. What the scaffolding
*does* guard is narrower and real — `just contracts_check` validates
`sel4-call.zti` as a generation, so the fixture cannot drift into an
unencodable shape while the broker fix is written, and the nine existing seL4
gates prove the new build wiring did not disturb them.

| Risk | Guard | Failure signal |
|---|---|---|
| The new fixture stops encoding | `just contracts_check` | The manifest fails validation |
| The build wiring breaks an existing plane | the nine seL4 plane gates | Any of them fails |
| The call plane silently starts running the stream broker again | none — this is what the missing gate would catch | — |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/build/build-sel4.py --call-plane --skip-pin-check` | Builds — `wrote build/slime-sel4-call.elf` | Direct |
| Boot under the pinned QEMU line | Graph admitted, 5 components staged, 5 channels minted, then the broker fails | Direct — [`boot.log`](boot.log) |
| `just sel4_stream_check` and the other nine seL4 gates | Pass — unaffected | Direct |
| `just contracts_check`, `ruff`, `typos`, `fmt_check_all`, `lint_all` | Pass | Direct |
| The C8.6 transcript | **Not observed.** The plane does not reach it | Unobserved |

`just fabric_call_check`, the oracle's own C8.6 gate, **cannot run on this
host**: it needs OVMF firmware for x86 QEMU
(`OVMF_CODE not found: /nix/store/…-OVMF-202605-fd/FV/OVMF_CODE.fd`). Confirmed
pre-existing by stashing every change and re-running. So the oracle transcript
this plane must reproduce is not readable here either.

## Decisions

- Decision: commit the plane in a failing state rather than revert or force it.
- Rationale: the fixture, the build wiring, and the flag mapping are correct and
  independently verifiable — the image builds, the graph admits with the right
  shape, and every channel mints. What remains is one specific, named
  incompatibility. Reverting would discard six steps of diagnosis to re-derive;
  forcing it — hand-numbering slots until the transcript appeared — would make
  the plane assert about a layout rather than about C8.6.

- Decision: no gate is registered.
- Rationale: a gate that cannot pass is not a gate. `just sel4_call_check` lands
  with the fix, not before it.

## Open risks and follow-ups

- [ ] **The blocking finding, precisely.** `fabric-service` resolves
      `FACTORY_SLOT = 0` and `BUFFER_FACTORY_SLOT = 1` as literals
      (`fabric-service.rs:99,103`), which is the x86 boot layout. On seL4 a
      non-bootstrap component's runtime slots start at `executables + 1`, so a
      fabric with no executables gets its factories at 1 and 2 while slot 0 goes
      to its first channel. The fix is to resolve both from the generated
      profile as `FABRIC_FIRST_CONTROL_SLOT` already is, rather than from
      literals — a component change, and one that touches the working stream
      plane, so it needs its own slice and its own fault injection.
- [ ] **`fabric-call-time` needs two control grants**, which the x86 layout
      gives it implicitly. `sel4-call.zti` declares
      `fabric-call-time-phase` explicitly for this reason; if the slot model
      above changes, re-check whether that grant is still the right shape.
- [ ] **P5.4.6 is not closed.** C8.6's required checks — correlated replies,
      duplicate/stale rejection, one execution of a non-idempotent operation,
      seven distinct terminal events, reclamation on client or server death —
      are all unobserved on seL4.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`boot.log`](boot.log).
- Related roadmap item:
  [P5.4.6](../../roadmap/07-architecture-portability.md),
  [C8.6](../../roadmap/02-core-runtime.md),
  [P5.4.1](../../roadmap/07-architecture-portability.md).

## Corrections

- **2026-08-07.** The blocking finding above is not the whole story, and the
  fix it proposes does not work. I tried it: deriving `firstControlSlot` from
  the fabric's factory-grant count instead of the constant `2`. The derived
  value was correct — `3`, matching the observed factories at slots 1 and 2 —
  and the plane still failed identically.

  The reason is one layer down, in `slime-root/src/channel.rs`.
  `SlotCursors::take` hands every task its **slot 0 first**, unconditionally
  (`used_slot_zero`), and only then continues from `executables + 1`. A fabric
  holding two factory grants therefore receives its control endpoints at slots
  `[0, 3, 4, 5, 6]` — slot 0 from the `used_slot_zero` rule, then 3 upward past
  the factories. The call broker maps a control's slot to the caller's identity
  by `FIRST_CONTROL_SLOT + index`, which assumes contiguity. **No single base
  value can describe a set with a hole in it**, so the constant was never the
  problem.

  Why the stream plane is unaffected: its fabric holds no factory grant at all,
  so `used_slot_zero` gives it slot 0 and the cursor continues from 1 —
  contiguous by accident of having nothing installed between them.

  The real fix is one of: make `used_slot_zero` yield to installed factories so
  channel slots stay contiguous above them; or have the broker resolve each
  control by identity rather than by `base + index`. Both are load-bearing
  changes to code the nine passing planes depend on, which is why neither is
  attempted here. The derivation experiment was reverted; the tree is back to
  the constant.

- **2026-08-07 (second correction).** The correction above says the fabric's
  controls land at `[0, 3, 4, 5, 6]` and calls the cause a slot *collision*
  between channels and factory grants. The slot set is right; the word
  "collision" is wrong, and the backlog entry B25 was filed on that error.

  `cursors.declare` receives `next_runtime_slot - 1`, and staging has already
  advanced that counter past every factory it installed. The cursor therefore
  resumes *clear* of the factories, never on top of one. Verified two ways: by
  the arithmetic (two executables plus two factories gives `declare(4)`, so the
  cursor's first non-zero slot is 5), and by a temporary unit test against the
  real `SlotCursors`, which returned `[0, 5, 6]`. `CapabilityTable::install` is
  never called with an occupied slot, so nothing fails closed and nothing is
  overwritten.

  What is true, and what actually breaks the call plane, is narrower: the set is
  **discontiguous**. Slot 0 comes from `used_slot_zero` and sits *below* the
  factories, so the controls run `0, 3, 4, 5, 6` with a hole at 1–2. The broker
  addresses controls as `FIRST_CONTROL_SLOT + index`, and no base value
  describes a set with a gap. B25 has been rewritten to say this.

  Recorded because the two diagnoses imply different fixes — a collision would
  be a correctness bug in slot allocation, while a gap is a mismatch between how
  slots are assigned and how one broker addresses them.

- **2026-08-07 (third correction — superseded).** Both corrections above, and
  the body they correct, name `SlotCursors::take` as the blocker. That is
  wrong, and the entry is superseded by
  [`devlog/2026-08-07-p5-4-6-call-spawn-semantics/`](../2026-08-07-p5-4-6-call-spawn-semantics/index.md).

  The slot gap is real but is a *consequence of this fixture's shape*, not a
  defect in slot allocation: declaring the control channels as generation
  grants is what put them on the root's channel cursor. Having `init` mint the
  pairs and hand them out at spawn — which is what `drive_stream_plane` already
  does — removes the gap entirely, and the plane still fails.

  The real blocker is that `slime-root`'s `distribute_channel_ends` treats a
  spawn-granted endpoint as a **move** while the oracle's
  `preflight_spawn_grant` derives a **copy**. The x86 call plane depends on the
  copy: `launch_fabric_calls` keeps every service half and transfers each
  participant's supervision handle afterwards. B25 has been rewritten again to
  say this.

  Recorded because the two diagnoses send a reader to different files: the
  retired one to `slime-root/src/channel.rs`, where nothing is wrong, and the
  current one to a semantic divergence from the frozen oracle.
