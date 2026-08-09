# P5.4.9 — C8.9 and C8.10 on seL4: the full C8 graph in one generation

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-boot.{zti,md}`, `contracts/boot-layout/v1/fixtures/sel4-boot.layout`, `slime-root/src/{channel,task}.rs`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{boot-plane,root-boot,boot-layout,gate-controls}.py`, `components/bins/build.rs`, `components/bins/src/bin/init.rs`, `Justfile` |
| Roadmap | P5.4.9, P5.4, C8.9, C8.10 |
| Gates | `just sel4_boot_check`, `just sel4_crossing_check`, `just sel4_root_boot_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.1 recorded C8.9 and C8.10 as uncovered on seL4; P5.4.8 closed the last slice before them |
| Baseline | Thirteen seL4 plane gates, each booting exactly one plane; no generation running all three at once |

## Summary

A fourteenth image, `sel4-boot`, carries generation 22: every C8 role in one
generation — the stream, call, and operation planes, an unauthorized probe, a
declared interposition proxy, and a filtered-introspection client — launched
concurrently in disjoint slots. The fabric splits itself into three bounded
route workers, all nineteen composition tasks reach a checked role or a declared
role-less idle, and the graph comes to rest without any of them exiting.
`just sel4_boot_check` asserts 44 markers across sixteen causal chains.

C8.9 needed no seL4 work: its resolution and satisfiability path is the shared
host builder, exercised by construction whenever a graph-bearing seL4 fixture is
built. What this slice adds for C8.9 is the widest such fixture.

Two `slime-root` bounds were raised — `MAX_CHANNELS` 32→48 and `MAX_TASKS`
32→48 — both sized against single-plane graphs, both B28's class.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/channel.rs` | `MAX_CHANNELS` 32 → 48 | A full graph's 37 live channels fit; see [`bounds-raised.txt`](bounds-raised.txt) |
| `slime-root/src/task.rs` | `MAX_TASKS` 32 → 48 | 20 root-launched instances plus init's 17 children fit |
| `contracts/generation/v1/fixtures/sel4-boot.zti` | Generation 22: 20 components, 39 grants, five routes, four schemas, the `unified` profile carrying the telemetry→`fabric-proxy` chain | One generation declares every C8 role |
| `scripts/build/boot_layout.py` | `SEL4_BOOT_LAYOUT`, 21 rows, registered as the generation-22 replacement | The three planes' executables occupy disjoint slots with no profile rewrite |
| `components/bins/src/bin/init.rs` | `drive_boot_plane`; the oracle branch stays pinned to generation 17 | Generation 22 cannot walk generation 17's layout |
| `scripts/check/check-sel4-boot-plane.py`, `Justfile` | The gate and `just sel4_boot_check` | C8.10 has a standing seL4 assertion |
| `components/bins/src/bin/init.rs` | `CHANNEL_LOOP_PAIRS` 33 → 49 | The crossing gate still exceeds the bound it tests |
| `scripts/check/check-sel4-root-boot.py` | Reclaimed CSlot ranges repinned 832..882/882..932 → 839..889/889..939 | The width-and-adjacency property survives a table resize |
| build/variant/flag wiring, both check registries | `--boot-plane`, variant `boot`, `SLIME_SEL4_BOOT_CHECK` | Each gate boots the artifact it asserts about, and the new gate is itself guarded |

### C8.9 is covered by construction

C8.9's substance is host-side: one canonical resolved graph feeding both the
authenticated bytes and the userspace tables, with every declared limit checked
against the fabric holder's quota, the channel bound, and the capability layout.
`build_sel4_generation` calls the same `resolve_fabric_profile` and
`render_fabric_profile_rust` every x86 profile calls, so every graph-bearing seL4
fixture — stream, QoS, call, operation, visibility, and now boot — exercises it.
`just data_fabric_profile_check` boots nothing.

What generation 22 adds is scale: five routes, four schemas, fifteen
participants, and every operation and call ceiling non-zero at once. A limit that
is individually legal but mutually unsatisfiable in that combination fails the
build rather than the boot, which is C8.9's third required check applied to the
widest graph the repo declares.

### The composition, and where it differs

`init` mints sixteen control pairs, spawns the three subscribers, then the fabric
with the two worker executables it spawns itself, then the remaining
participants, then yields once so every role request is enqueued before any
supervision descriptor follows it on the same channel.

One thing differs from `launch_fabric_boot`, and it is the recurring seL4 fact:
the oracle's boot layout numbers **both halves of all sixteen control channels**,
because its kernel materializes a declared channel into the bootstrap component's
layout slots. This root numbers a launched component's declared ends from its own
cursor, so a declared control reaches the fabric at a slot no
`FABRIC_FIRST_CONTROL_SLOT + index` describes — observed directly on the first
boot, where the fabric received its ends at cursor positions and every worker
spawn then failed. Every seL4 plane since P5.4.6 mints its controls for this
reason; this one mints sixteen.

So `SEL4_BOOT_LAYOUT` is 21 rows against the oracle's 53. The C8.10 property a
boot layout can carry — all three planes' executables in disjoint slots, no
profile-dependent rewrite — is intact; the 32 control rows are simply not the
generation's to place here.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A role stops launching, or launches twice | the spawn-parent and child-set checks in `just sel4_boot_check` | "init spawned …, expected …" or "spawned twice" |
| The graph finishes instead of coming to rest | the inverted lifecycle check | "composition tasks exited before the graph came to rest" |
| A plane's slots start aliasing another's | one layout report, all slots distinct, strictly under the ceiling | "the layout claims a slot twice; the planes are not disjoint" |
| The fabric stops splitting into workers | the bounded-route-worker chain | missing `route worker provisioned` marker |
| The probe stops being refused | its own chain, from both sides | missing marker |
| A role-less participant silently takes a role | the four declared idles are asserted by name | "did not report its declared role-less idle" |
| A raised bound leaves a gate vacuous | `just sel4_crossing_check` reads both constants from source | "it must exceed the bound or the gate proves nothing" |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 44 markers | a mutated transcript is accepted, or the count drifts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_check` | Pass; 44 markers across 16 chains; 21 layout slots; 19 tasks, 11 roles, 4 idles, none exited | Direct |
| `just sel4_gate_control_check` | Pass; 15 gates reject 716 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 12 plane layouts match their fixtures | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| `just sel4_root_boot_check` | Pass after repinning the shifted CSlot ranges | Direct |
| `just sel4_crossing_check` | Pass after raising `CHANNEL_LOOP_PAIRS` to 49 | Direct |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_channel_check` | Pass | Direct |
| `just sel4_loan_check` | Pass | Direct |
| `just sel4_spawn_check` | Pass | Direct |
| `just sel4_sample_check` | Pass | Direct |
| `just sel4_supervision_check` | Pass | Direct |
| `just sel4_stream_check` | Pass | Direct |
| `just sel4_qos_check` | Pass | Direct |
| `just sel4_call_check` | Pass | Direct |
| `just sel4_operation_check` | Pass | Direct |
| `just sel4_visibility_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| C8.9's host determinism and C8.10's semantics | Inherited from `just data_fabric_profile_check` and `just data_fabric_boot_check` on the frozen oracle | Inherited |

Every seL4 gate was re-run, and here it was necessary rather than cautious: both
raised bounds change the root every plane runs on. Two gates went red and are
recorded above.

## Decisions

- **Decision:** Raise both bounds to 48 rather than to 37.
  **Rationale:** B28's rule. Neither bound fails cleanly — channel exhaustion
  cascades into every downstream participant, and task exhaustion surfaces as a
  spawn error at the parent — so a bound raised to the first passing number is a
  bound that moves again with a worse symptom next time.
  **Rejected alternative:** exactly 37, which would make the next added role a
  boot failure rather than a headroom question.

- **Decision:** Mint the sixteen control channels rather than declaring both
  halves in the layout.
  **Rationale:** the root numbers a launched component's declared ends from its
  own cursor. Observed, not assumed: the first boot declared them and the fabric
  received `[0, 3, 4, …]` where the broker addresses `base + index`.
  **Rejected alternative:** teaching the root to place a declared channel end at
  a bootstrap layout slot, which is the oracle's mechanism and a much larger
  change to a root that deliberately does not map grant names onto layout labels.

- **Decision:** Reuse the oracle's `unified` profile name in the fixture.
  **Rationale:** `resolve_fabric_profile` selects `FABRIC_BOOT_STREAM_CONTROL_GRANTS`
  — the seven-control table with the observer, probe, and proxy — on that name
  alone. Naming the profile `sel4` produced a five-control table and the worker
  spawns failed on an ungranted executable slot.
  **Rejected alternative:** a `sel4` profile plus a branch in the resolver, which
  would fork the control-table derivation the two planes must share.

- **Decision:** Repin the reclaimed CSlot ranges rather than relaxing them to
  wildcards.
  **Rationale:** the base is allocator state and moved because the static tables
  grew; the width and adjacency are the property. Relaxing to `\d+` would restore
  exactly the hole P5.4.10 closed when it replaced the wildcards.
  **Rejected alternative:** matching only the width, which cannot express "the
  second range adjoins the first".

## Open risks and follow-ups

- [ ] This plane provisions and rests; it carries no traffic. That is C8.10's own
      exit condition — "healthy blocked idle with no traffic" — but it means the
      full graph is not exercised end to end on seL4. Per-plane traffic is
      covered by the stream, call, operation, and visibility gates against the
      same unmodified brokers.
- [ ] C8.7's fourth required check wanted peer death to leave unrelated
      **stream** and **call** routes live, which P5.4.7 could only prove for an
      unrelated operation route. This graph has all three planes but no fault
      injection; adding one would close that P5.4.7 follow-up here.
- [ ] `MAX_GRAPH_ITERATIONS` is 2048 and no gate reports how close a boot came.
      This is the widest graph yet and still fits, which raises rather than
      lowers the value of reporting the margin.

## Artifacts and provenance

- Direct gate evidence: [`boot-check.txt`](boot-check.txt), captured from
  `just sel4_boot_check` on 2026-08-08 with the pinned qemu-arm-virt profile.
- Both bound raises, their arithmetic, and the two gates that caught the
  consequences: [`bounds-raised.txt`](bounds-raised.txt).
- The slice before it: [`devlog/2026-08-08-p5-4-8-visibility-plane/`](../2026-08-08-p5-4-8-visibility-plane/index.md).
- The iteration-budget precedent this reasoning follows:
  [`devlog/2026-08-07-b28-iteration-budget/`](../2026-08-07-b28-iteration-budget/index.md).
- The inventory that recorded both as uncovered:
  [`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../2026-08-07-p5-4-1-oracle-inventory/index.md).
- Related roadmap item: P5.4.9 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
