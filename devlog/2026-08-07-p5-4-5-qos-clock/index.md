# P5.4.5 (part) — a monotonic-time channel makes three C8.5 arms fire on seL4

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Root-caused |
| Scope | `contracts/generation/v1/fixtures/sel4-qos.zti`, `scripts/build/{build-generation,build-sel4}.py`, `components/bins/src/bin/init.rs` |
| Roadmap | P5.4.5, C8.5, B25 |
| Gates | none |
| Trigger | P5.4.6 blocked on B25; P5.4.5's remaining arms checked for the same dependency |
| Baseline | Three C8.5 arms asserted by `just sel4_stream_check`; the rest recorded as needing a clock |

## Summary

P5.4.5's remaining C8.5 arms were recorded as needing "a plane that stalls
deliberately and advances a clock". An eleventh image, `sel4-qos`, supplies the
clock: the stream graph at generation 19 with one runtime-minted time channel,
granted to `fabric-service` at its `TIME_SLOT` 9 and to `fabric-publisher-b` at
its `TIME_SLOT` 3. Three time-driven arms that had been unreachable now fire —
**bounded RELIABLE retry accounting, deadline miss, and liveliness loss**. The
plane does not complete: the QoS scenario reaches
`[fabric] fail: no inline retained publisher`, because it expects a retained
publisher whose head sample is *inline* and the stream graph's retained
publisher lends a large one. That is a graph-shape gap, not a mechanism gap, and
no gate is registered because the plane cannot pass yet.

The load-bearing finding for the wider track is that **B25 does not block this**.
Every capability the QoS plane needs is a spawn grant — including the clock —
so the post-spawn introduction P5.4.6 is stuck on never arises here.

## Observable symptom

- Command: `qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53
  -smp 1 -m 2048 -nographic -serial mon:stdio -kernel build/slime-sel4-qos.elf`
- Expected: the oracle's `just fabric_call_check`-adjacent QoS transcript —
  credit and acknowledgement, retry exhaustion, deadline, lifespan, liveliness,
  lease, and tie ordering.
- Observed: `[fabric] reliable retry accounted` twice,
  `[fabric] QoS deadline missed` once, `[fabric] QoS liveliness lost` three
  times, then `[fabric] fail: no inline retained publisher`.
- Exit/fault/serial evidence: [`boot.log`](boot.log).

## Changes

| Area | Change | Effect |
|---|---|---|
| `sel4-qos.zti` | New fixture: the stream graph at generation 19, byte-identical otherwise | Generation 19 resolves the same 31-row layout, so no new boot-layout table is needed — the clock is minted at runtime rather than declared |
| `init.rs` | `qos_plane()`; the time pair minted only for that plane; grant 9 to the fabric and grant 3 to `fabric-publisher-b` | The clock reaches both components at the slots they compile against |
| `init.rs` | The x86 QoS branch gained its second guard half | Without it that branch claimed this plane and walked the x86 boot layout |
| `build-generation.py` | `sel4-qos` in the manifest registry; the manifest→flag table now maps a manifest to *several* flags | This plane sets `SLIME_SEL4_STREAM_CHECK` and the oracle's `SLIME_FABRIC_QOS_CHECK` together |
| `build-sel4.py` | `--qos-plane`, `QOS_VARIANT`, image and manifest paths | `build/slime-sel4-qos.elf` |

The QoS flag is the **oracle's own** rather than a new seL4-only one, and that
is the point: `fabric-service`, `fabric-publisher-b`, and `fabric-subscriber-b`
all select their QoS behaviour from it, so a separate flag would have meant init
built a clock the components ignored. `qos_plane()` requires both flags, so the
x86 QoS generation — built with the same one — cannot walk this composition.

## Regression guards

None registered: the plane does not pass, so a gate would be red. The existing
nine seL4 gates guard against this work disturbing them, and one of them caught
a real regression while it was being written (below).

| Risk | Guard | Failure signal |
|---|---|---|
| The shared driver breaks the stream plane | `just sel4_stream_check` | Boot exceeds its window without the final marker |
| The new fixture stops encoding | none — reached only through `build-sel4.py --qos-plane` | The image build fails |
| The QoS plane silently stops advancing its clock | none — this is what the missing gate would catch | — |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/build/build-sel4.py --qos-plane --skip-pin-check` | Builds — `wrote build/slime-sel4-qos.elf` | Direct |
| Boot under the pinned QEMU line | Clock reaches both components; `reliable retry accounted` ×2, `QoS deadline missed` ×1, `QoS liveliness lost` ×3; then the retained-publisher failure | Direct — [`boot.log`](boot.log) |
| `just sel4_stream_check` | **Failed first**, then passed after the flag-ordering fix below | Direct |
| The nine seL4 plane gates, all images rebuilt | All pass | Direct |
| `just sel4_boot_layout_check` | 9 plane layouts match | Direct |
| `just test_sel4_root` | 109/109 across 13 modules | Direct |
| `just contracts_check`, `just devlog_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| `QoS retry exhausted`, `QoS lifespan expired`, lease and tie-ordering arms | **Not observed.** The plane stops before them | Unobserved |
| `just fabric_qos_check`, the oracle's own C8.5 gate | **Cannot run on this host** — needs x86 OVMF firmware absent from this store | Inherited |

## Decisions

- Decision: reuse the oracle's `SLIME_FABRIC_QOS_CHECK` rather than adding a
  seL4-only flag.
- Rationale: three components read it to select their QoS behaviour. A separate
  flag would leave them in stream mode while init built a clock, and would let
  the two planes' QoS logic diverge with nothing noticing — the same reason
  P5.4.6 selects its broker on either flag.

- Decision: generation 19 with no new boot-layout table.
- Rationale: the clock is minted at runtime, so the layout is the stream plane's
  unchanged. Verified by resolving both: 31 rows, identical. A new table would
  have been a second thing to keep in agreement for no benefit.

- Decision: no gate.
- Rationale: the plane does not pass. Registering it would register a red gate,
  which the P5.4.6 slice already established as the wrong trade.

## Open risks and follow-ups

- [ ] **The retained-publisher shape is the remaining blocker.**
      `create_late_subscriber` requires a `DURABILITY_RETAINED` publisher whose
      retained head is `inline`; the stream graph's retained publisher
      (`fabric-publisher-b`) lends a `>MAX_INLINE_BYTES` sample, so the head is
      loan-backed. Closing this needs the fixture to declare a retained
      publisher that publishes small, which is a graph change rather than a
      wiring one.
- [ ] **`QoS retry exhausted`, `QoS lifespan expired`, lease, and tie ordering
      remain unobserved.** They sit after the failure above in the scenario.
- [ ] **P5.4.5 is not closed.** Its exit condition asks for every item in
      C8.5's required-checks list to have an observed seL4 gate; three arms now
      *run* but none of the new ones is gated.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`boot.log`](boot.log).
- Serial/debugger/model output: [`boot.log`](boot.log).
- Related roadmap item:
  [P5.4.5](../../roadmap/07-architecture-portability.md),
  [C8.5](../../roadmap/02-core-runtime.md),
  [B25](../../roadmap/00-backlog.md).
- Extends the three arms asserted in
  [`devlog/2026-08-07-p5-4-5-qos-arms/`](../2026-08-07-p5-4-5-qos-arms/index.md).

## Corrections

- **2026-08-07.** The first Open-risk item above names the retained-publisher
  shape as the remaining blocker. That is right, but the transcript also shows a
  *second* ordering fact worth recording, because the obvious fix for it makes
  things worse.

  `fabric-publisher` — the graph's inline retained publisher — reaches
  `publish role received` at boot.log:333, *after* the fabric fails at :304. So
  when `create_late_subscriber` runs, that publisher is still parked awaiting its
  role and no retained inline head exists yet. It reads like a race that one
  `yield_now` before the clock driver's spawn would settle.

  It does not. Adding exactly that yield removed **all** QoS activity: the
  re-run showed zero `reliable retry accounted`, zero `QoS deadline missed`, and
  zero `QoS liveliness lost`, where the committed order shows two, one, and
  three. Perturbing the ready-queue order costs the arms that currently work
  rather than buying the one that does not, so the yield was reverted and the
  committed plane keeps its order.

  Recorded because it narrows the follow-up: the gap is not a scheduling nudge
  away. `create_late_subscriber` runs when the fabric's scheduled boundaries run,
  and the graph must have an inline retained head by then — which the fixture has
  to guarantee structurally, by declaring a retained publisher that publishes
  small, rather than by any ordering of the existing one.

- **2026-08-07 (second correction).** The correction above says one `yield_now`
  before the clock driver's spawn removes the working arms, and concludes the
  fixture must guarantee an inline retained head structurally. The first half is
  right; the second is better founded now, because a *bounded run* of yields was
  tried too and it fails differently.

  With 64 yields before `fabric-publisher-b`'s spawn the race genuinely closes:
  `[fabric-publisher] inline samples published` appears, `no inline retained
  publisher` is gone, `fabric-subscriber` reaches `done` with exit 0, and the six
  remaining failures are exactly the unconfigured root-launched instances the
  stream gate budgets for. So the ordering diagnosis was correct — the probes
  (`route receive denied`, `re-delegation denied`, `widening denied`) are three
  root round-trips on this mechanism where they are plain syscalls on x86, and
  they are what let the clock run ahead.

  But the plane then hangs instead: `fabric-publisher-b` stops at
  `diagnostics sample published` and never returns from `publish_large`, so
  simulated time is never advanced at all and every timed arm reads zero.
  Retrying with 8 yields hangs the same way. The delay that lets the inline
  publisher win the race also perturbs the loan handshake `publish_large`
  depends on.

  So the gap is **not** reachable by scheduling at all — neither by removing a
  yield nor by adding a bounded number of them. Both ends of that range are
  worse than the committed order, which keeps three arms observable. The
  follow-up stands and is now better justified: the graph has to give the fabric
  an inline retained head without depending on when any publisher runs, which
  means declaring a retained publisher that publishes small and early rather
  than reordering the ones it has.

  Recorded because the two experiments bracket the option: the committed plane is
  a local maximum for scheduling, and the remaining work is a fixture change.
