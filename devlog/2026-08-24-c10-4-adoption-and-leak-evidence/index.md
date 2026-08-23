# C10.4: the first product component on the private region, and two demands a fixed array had been absorbing

| Field | Value |
|---|---|
| Date | 2026-08-24 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/fabric-service/src/{main.rs}` and `Cargo.toml`; `components/bins/private-heap-probe/src/main.rs`; `slime-root/src/{main.rs,private_memory.rs}`; `contracts/component-spec/v1/schema.zt` and all 42 records; `contracts/generation/v1/fixtures/*.zti` (11); `scripts/lib/{component_spec,system_spec}.py`; `scripts/build/build-generation.py`; `scripts/check/{check-component-spec,check-system-spec,check-sel4-dango-plane,check-sel4-private-memory-plane,check-sel4-gate-controls}.py`; `Justfile` |
| Roadmap | C10.4, C10, C10.1, C10.2, C10.3, C7.3, CP1, CP3, B9, B23, B63, B70 |
| Gates | `just private_memory_check`, `just dango_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_traffic_check`, `just system_spec_check`, `just component_spec_check`, `just sel4_gate_control_check`, `just generation_check`, `just test_sel4_root`, `just lint_all` |
| Trigger | C10.4 was the next roadmap milestone with met dependencies: the backlog is empty, C10.3 closed the previous day, and every other open item is an undecomposed parent (C9) or gated on physical hardware (M5.7, RP3/P4) |
| Baseline | C10.1–C10.3 built a task-private region, a generation-declared ceiling on it, and a `GlobalAlloc` over it — but the only thing using any of it was a probe. Every shipped component still reserved its worst case at build time, no gate re-sampled the frame allocator across a spawn/exit cycle, and nothing stopped a shared buffer from being mapped into a private window |

## Summary

`fabric-service` — the graph's own broker, carried by ten shipped fixtures — now
sizes its role and frame tables from the participant rows the generation
declared rather than from the contract's ceilings. Static footprint falls by
29960 bytes (`.bss` plus `.data`, 145912 → 115952 on `sel4-boot`), and the
largest declared graph uses 4 of its 16 declared pages.

That it is a *product* component is the point. C10.3 proved the allocator with a
probe, and a probe proves a mechanism works; only something that ships proves it
was worth building. B70 is why this component was the right first one: sizing
three brokers' fixed arrays from contract ceilings overflowed the 64 KiB stack
and presented as a corrupted `static` rather than as a fault, which is why those
tables live in `.bss` at all. `.bss` fixed the corruption but not the cause —
the reservation was still the contract's worst case in every generation, and not
one of the ten declares it.

Three other things landed with it: a free-frame census across a repeated
spawn/exit cycle, a root refusal that keeps a shared buffer out of a private
window, and a holder the generation names in both budgets at once.

The most instructive part was not the conversion but its cost. Removing a fixed
array removes a bound the compiler was enforcing, and it turned out the old
`[Frame; 32]` had been absorbing two demands nobody had written down.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| `components/bins/fabric-service/src/main.rs` | `static mut PUBLISHERS/SUBSCRIBERS/FRAMES` → `Box<[_]>` on `StreamTables`, sized by a new `declared_capacity()`; ~26 signatures from `&mut [T; N]` to `&mut [T]` | A generation pays for the graph it declares, not for the graph the contract permits |
| `components/bins/fabric-service/Cargo.toml` | `slime-rt` with `private-heap` | First product consumer of the C10 mechanism |
| `slime-root/src/private_memory.rs` | `Region::overlaps()` + 3 host tests | The window is a fixed property of the VSpace, so the answer cannot depend on how far an allocator has grown |
| `slime-root/src/main.rs` | `admit_mapping_destination()` gating `BufferLifecycleRequest::Map` and `LoanLifecycleRequest::Map`; `SLIME_ROOT reclaim census` | The two memory planes are disjoint in address space, not merely in accounting |
| `contracts/component-spec/v1/` | `ResourceRequirement.privatePageQuota`, set on all 42 records | The private ceiling is a spec fact the generation must agree with, like every other resource ceiling |
| `scripts/lib/system_spec.py` | Derives `privateMemoryBudget` + its resource object | CP1's derivation covers the new section rather than leaving one field hand-authored |
| `scripts/check/check-system-spec.py` | `POST_BASELINE_SECTIONS` + `check_post_baseline()` | A section the frozen baseline predates is excused from *that* comparison and asserted against the composing specs instead |
| `scripts/build/build-generation.py` | `PRIVATE_HEAP_REQUIRED`; `ring_capacity` sums both frame terms | A component that cannot run without a quota is refused at build, not at boot; and the builder admits exactly the set the component admits |
| `scripts/check/check-sel4-dango-plane.py` | Repeat of the first command + `check_spawn_exit_returned_every_frame` | Frame conservation over one executable, from the allocator's own watermarks |
| `scripts/check/check-sel4-private-memory-plane.py` | `private-heap-both` chain + `check_the_two_planes_are_independent` | Exhausting either plane leaves the other's ceiling intact |

### The two demands the fixed array was hiding

A subscriber's queue is not the only thing that pins a fabric frame. A
`retained` publisher holds its last `retainedDepth` samples for late joiners —
`retain_for_late_joiners` takes a reference per entry and only
`release_retained` drops them — and those references are live *concurrently*
with every subscriber's queue rather than instead of it. Every shipped fixture
declares retained publishers, so a table sized from subscriber depth alone was
short by 4 to 6 frames on all of them.

Separately, `provision_edge` floors each subscriber's history at
`MIN_RING_SLOTS`, so a subscriber declaring `historyDepth = 1` — which both the
builder and the decoder admit — gets a two-entry history against a table that
budgeted one.

Both are the same class of mistake: a constant large enough for the contract's
worst case had been quietly covering a demand nobody had enumerated, and sizing
from the declared graph made the omission load-bearing. Neither would have
presented as a refusal. `pump_publisher` breaks when no frame is free and the
stalled subscriber holding its ring never releases one, which is the deadlock
the frame bound exists to make unreachable.

The fix separates two figures that had been one. Admission is on the *unfloored
declared* sum, because that is what the builder certifies and a component
refusing a toolchain-approved generation is a component second-guessing its own
composition. Storage is on the *floored* sum, because that is what
`provision_edge` will actually allocate. `MAX_FRAMES` bounds the first; the
declared private-memory quota bounds the second.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A generation pays for a graph it does not declare | `just sel4_stream_check` | `[fabric] tables sized from the declared graph publishers=3 subscribers=3 frames=18 ceilings=4/32` absent or wrong |
| The frame table under-counts a demand again | `just sel4_qos_check`, `just sel4_traffic_check` | `no free frame` / provisioning failure under real retained-publisher traffic |
| The builder admits a wider graph than the component | `just generation_check` | `fabric graph: declared frame demand exceeds the frame table` — observed by injection at 34 frames |
| A component that needs a quota ships without one | `just generation_check` | `instance fabric-service … but privateMemoryBudget declares no quota for it` — observed by injection |
| A spawn/exit cycle stops returning frames | `just dango_check` | `a repeated spawn/exit cycle did not return every frame: …` — observed by five injections |
| The repeat stops reusing the released arena, making conservation vacuous | `just dango_check` | `provisioned a new arena rather than reusing the released one` |
| A shared buffer becomes mappable into a private window | `just private_memory_check` | missing `SLIME_MEM mapping refused …`; disabling the root check fails the plane |
| The two planes' accounts merge | `just private_memory_check` | `FAIL private exhaustion consumed the buffer quota` / `FAIL buffer exhaustion consumed the private quota` |
| A releasing buffer credits the private account | the probe | `FAIL buffer release moved the private account` |
| A spec's private ceiling disagrees with the generation | `just component_spec_check` | `spec privatePageQuota N != manifest privateMemoryBudget M` |
| The new ceiling stops being a refused malformation | `just component_spec_check` | 43 named mutations, one of them `a private-memory quota above the root's reservation` |
| CP1's derivation drifts on the new section | `just system_spec_check` | `derived privateMemoryBudget … does not match the declared privatePageQuota` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_check` | Passed. 22 markers / 7 chains. Both-planes holder: `declared=24 installed=24 base=0x400000`, refused at its own window base (`window=0x400000..0x600000`), then `both pages=22 growths=4 buffers=1 window_map_refused=1 outside_map=1 released=1 reused=1` | Direct |
| `just dango_check` | Passed. 16 markers. Census after the repeat identical to the prior cycle — `slots=2799 bytes=527489392 live_objects=302` — with `arena_reuses=1` | Direct |
| `just sel4_stream_check` | Passed, 57 markers, real traffic on heap-allocated tables: `frames=18` (14 before the retained-publisher fix) | Direct |
| `just sel4_qos_check` / `traffic` / `visibility` / `matrix` / `call` / `operation` | All passed — the converted component under real traffic on every plane that carries it, including the 22-frame graph under three concurrent planes | Direct |
| Static footprint, `sel4-boot` | `.bss` 101224 → 98208, `.data` 44688 → 17744; **29960 bytes** freed. Measured by parsing PT_LOAD/section headers of the built ELF on both sides of a clean rebuild | Direct |
| Injection: builder frame guard, retained depth raised to 8 on both publishers (34 > 32) | Refused: `fabric graph: declared frame demand exceeds the frame table`. Round-2 review found the first version of this guard compared a string to an encoded int and was dead; this is the re-verified fix | Direct |
| Injection: `sel4-boot.zti`'s fabric-service quota removed | Refused by name at build | Direct |
| Injection ×8: dango census (13 frames leaked, one CSlot leaked, an object surviving, fresh arena, gained slots, two exits, an exit with no census, shell teardown mistaken for a command) | Each refused by name; the healthy transcript with teardown following passes | Direct |
| Injection ×8: two-plane independence (non-overlapping refusal, refusal off the window base, no refusal, over-quota, zero pages, no buffer allowance, no ceiling record, no report) | Each refused by name | Direct |
| Injection: `admit_mapping_destination`'s condition disabled | The plane fails, so the root check is load-bearing rather than decorative | Direct |
| All 28 emittable probe failure lines vs `FAILURE_MARKERS` | Every one matches. Round 1 found the per-role relabelling had orphaned all three markers | Direct |
| `just generation_check` | Byte-identical double build | Direct |
| `just system_spec_check`, `component_spec_check` | 20 and 43 named mutations refused | Direct |
| `just sel4_gate_control_check` | 34 gates reject 1338 mutated transcripts and layouts | Direct |
| `just test_sel4_root`, `test_host`, `contracts_check`, `sel4_root_boot_check`, `sel4_boot_layout_check`, `sel4_component_graph_check`, `component_crate_split_check`, `lint_all`, `fmt_check_all`, `ruff`, `typos`, `deny`, `machete`, `miri`, `devlog_check` | All passed; 152/152 host tests | Direct |
| "The converted component behaves identically on the same inputs" | Asserted behaviourally, not by comparison: seven plane gates replay their frozen marker contracts unchanged against the converted component. No gate compares old and new output directly | Direct, but indirect on the identity claim |

## Decisions

- Decision: convert `fabric-service` rather than a smaller candidate.
- Rationale: three candidates existed. `fabric-subscriber-b`'s mailboxes are ~1 KiB and its depth of 8 is a locally chosen retry buffer, not a contract ceiling; `sel4-filesystem-service`'s `root_entries` is a stack local in a crate that already has the bump allocator. `fabric-service` is the one where the reservation is both large (30 KiB documented) and traceable entirely to published ceilings, which is what makes "the generation pays for a graph it does not have" a measurable claim rather than a stylistic one. It also ships in ten fixtures, so the conversion is exercised by seven existing plane gates rather than by a new one.

- Decision: separate admission from storage in `declared_capacity`.
- Rationale: forced by the `MIN_RING_SLOTS` finding, and right independently. The builder certifies a graph on its declared sum; a component refusing on a larger floored figure would reject generations the toolchain approved. But the table must hold what `provision_edge` actually allocates. One number cannot be both, and picking either alone is a defect: the declared figure under-allocates, the floored figure over-refuses.

- Decision: the builder sums the same two frame terms as the component.
- Rationale: a builder admitting a wider set than the component is a generation the toolchain approves and the graph's own holder then kills at boot — the worst failure shape available, because the build is silent. This is the same lesson as C10.1's compile-time pin between the root's ceilings and the contract's, on a different axis.

- Decision: the free-frame census reads the allocator's watermarks, not the root's tallies.
- Rationale: B9 is the standing evidence. Terminated tasks were marked terminated and their buffers reclaimed while thirteen frames per spawn were never returned, and every counter the root printed agreed with every other one throughout. A leak that the bookkeeping cannot see cannot be found by reading the bookkeeping.

- Decision: the repeated command is the *same executable* as the first.
- Rationale: `begin_task_arena` retains a released arena's parent untyped for reuse by the next task of the same size, so a cycle launching a new executable legitimately consumes a new parent and the free count legitimately falls. Only a repeat makes "the count returns" a property of reclamation rather than of which binaries the script happened to name. The assertion additionally requires `arena_reuses` to have advanced, which is what distinguishes a reused arena from a fresh one that happened to cost the same.

- Decision: a command's census is the *last* record before its exit.
- Rationale: counter-intuitive and worth recording. The root reclaims a dead task in its own service-loop sweep, and only then does the spawn service's reply reach the shell that prints `result:exit`. My first version sliced forward from each exit and failed against a correct boot — for the third command it collected the shell's own teardown. Both reclamation call sites are in that one loop and the console dispatcher never reclaims, so the ordering is mechanism-enforced rather than observed.

- Decision: refuse an overlapping mapping at the dispatcher, not inside `SharedBufferTable`.
- Rationale: the table holds no task records — the window lives on the child VSpace, and the dispatcher is the one place that has already resolved both. And the rule is about the *caller*, not the region: a buffer legitimately maps into another component's address space at the same numeric address, so it is not a property of the buffer that could be checked once at creation.

- Decision: `overlaps` tests the whole reservation, not the backed prefix.
- Rationale: bounding it by the live page count would make the answer depend on how far the component's allocator happened to have grown, so the same generation would admit or refuse the same mapping depending on timing. The reservation is a fixed property of the VSpace.

- Decision: the refusal is `InvalidOperation` (`ERR_INVALID_ARG`), indistinguishable on the wire from a bad range.
- Rationale: a component that could tell the two apart could map its own window's bounds by watching which code a probe returns, and the window's placement is not something a component is told. The root's marker names the cause for the transcript, which is where an attributable refusal belongs.

- Decision: `PRIVATE_HEAP_REQUIRED` is a strict subset of `PRIVATE_HEAP_COMPONENTS`.
- Rationale: found while fixing the review's finding, and the reason matters. Linking the allocator and *requiring* a quota are different facts. `private-heap-probe` links it precisely so it can be run both ways, and the omitted instance proving it is denied is the plane's whole point. A rule keyed on the allocator would make deny-by-default unexpressible.

- Decision: excuse `privateMemoryBudget` from the frozen CP1 baseline comparison, and assert it separately.
- Rationale: the baseline is the pre-CP1 hand-authored `valid.zti`, never regenerated and never edited — that is what makes it evidence rather than the generator's own output. It cannot contain a section added after it was frozen. But an excusal with nothing behind it would let a wrong budget hide under a skipped name, so `check_post_baseline` compares the derived budget against the specs the system composes, which is a stricter test than the baseline's would have been.

- Decision: `privatePageQuota` on the component spec rather than only in fixtures.
- Rationale: `valid.zti` is generated from the spec corpus, so a hand-edited budget there would be reverted by the next regeneration. Putting it on the spec makes the generation's ceiling derived from the component's own declaration, and `check-component-spec.py` then enforces agreement in both directions, exactly as it already does for the four shared-buffer ceilings.

## Open risks and follow-ups

- [ ] The two root threads write the same serial stream one byte at a time through `seL4_DebugPutChar` with no lock, at equal priority on a single CPU. Emission *order* is mechanism-enforced, but textual integrity is not, and the census assertion is the first gate to bind a root marker positionally against a console-thread line — so a spliced line would present as a rare flake reporting "no reclaim census". Serializing that stream is a B41-scoped change affecting all 34 marker gates, not this milestone's to make.
- [ ] "The converted component behaves identically on the same inputs" rests on seven plane gates replaying frozen marker contracts, not on a comparison of old and new output. That is strong evidence and not the claim's literal form; a differential harness replaying one plane against both builds would close the gap.
- [ ] `declared_capacity` and the builder's `ring_capacity` now enumerate the same two frame consumers in two languages. They agree today and each is commented to point at the other, but nothing mechanically pins them together the way `private_memory.rs`'s `const _: () = assert!` pins the root's ceilings to the contract's. A third consumer added to one side and not the other is exactly the drift this milestone found.
- [ ] `fabric-service`'s 16-page quota is uniform across ten fixtures while their declared graphs need 4 pages at most. It is the same over-declaration this milestone removed from `.bss`, one level up: the quota is now sized for the worst graph rather than each generation's own. Deriving it from the declared graph is possible — the builder already computes the frame demand — and would make the ceiling proportional too.
- [ ] Only `fabric-service` was converted. `fabric-subscriber-b`'s mailboxes and the store plane's bump-allocator components remain worst-case sized; the mechanism is now proven on a product component, so the remaining conversions are ordinary work rather than milestone work.
- [ ] The private window is defended against the two mapping operations a component can name. Nothing prevents a *future* root operation from taking a caller-supplied destination address without consulting `overlaps`, and the check is by convention at each call site rather than structural.

## Artifacts and provenance

- Focused report: none; every finding and decision is recorded above with its cross-reference.
- Raw transcript: not retained. Every result in *Verification* is reproducible from the named `just` target, and each injection is described precisely enough to repeat (the mutation and the observed refusal string). Load-bearing marker lines are quoted verbatim.
- Review: two rounds. Round 1 ran a five-lens panel over the uncommitted diff; two lenses (canonical, concurrency) died on tree churn while I was fixing findings from the other three, and the three that reported returned twelve findings — one P0 (`system_spec_check` red against the frozen baseline), one P1 (the per-role relabelling orphaning three failure markers), seven P2, three P3. All twelve applied. Round 2 re-reviewed the settled tree with a canonical lens and a fresh concurrency lens. Canonical verified eleven of twelve fixes and found the twelfth — the builder's retained-publisher term compared a string against an encoded int, so it evaluated to zero and the guard did nothing while every gate stayed green. Fixed and injection-verified. Concurrency returned "correct" with one non-blocking P3, recorded above as an open risk.
- I also found two defects in my own work before review: the frame table ignoring retained publishers (measured across all ten fixtures) and the first census window being sliced in the wrong direction.
- Related roadmap items: [C10.4](../../roadmap/02-core-runtime.md), [C10](../../roadmap/02-core-runtime.md)
- Predecessors: [`devlog/2026-08-23-c10-3-userspace-allocator/`](../2026-08-23-c10-3-userspace-allocator/index.md) (the allocator this adopts), [`devlog/2026-08-23-c10-2-private-memory-budget/`](../2026-08-23-c10-2-private-memory-budget/index.md) (the budget), [`devlog/2026-08-23-c10-1-private-memory-mechanism/`](../2026-08-23-c10-1-private-memory-mechanism/index.md) (the region), [`devlog/2026-07-28-b9-task-frame-reclamation/`](../2026-07-28-b9-task-frame-reclamation/index.md) (the leak the census exists to catch)
