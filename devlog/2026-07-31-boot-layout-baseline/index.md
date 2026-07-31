# Boot-layout equivalence baseline

| Field | Value |
|---|---|
| Date | 2026-07-31 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/src/runtime/bootstrap.rs`, `scripts/check/check-boot-layout.py`, `contracts/boot-layout/v1/fixtures/`, `Justfile` |
| Roadmap | B10 |
| Gates | `just boot_layout_check` |
| Trigger | Starting B10: init's capability layout is a positional convention with no observable form, so a refactor of it cannot be shown to preserve behavior. |
| Baseline | The layout resolved by each gate profile at the commit that opened B10, before any layout change. |

## Summary

B10 replaces init's positional capability layout with generation-declared
resolution, under the hard constraint that every profile in use must resolve to
the slot numbers it occupies today. That constraint was unfalsifiable: the
layout existed only as source order in `launch_init`, so "the slots did not
move" could be argued but not observed. This change makes the layout an
artifact. The kernel now emits init's resolved table to the serial log, a check
script boots each of the sixteen distinct gate profiles and freezes the result
as a fixture, and `just boot_layout_check` fails — naming the slot — when a
layout moves. No layout changed here; this records what the layouts are so the
refactor that follows can be measured against them.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `kernel/src/runtime/bootstrap.rs` | `dump_boot_layout` emits `[layout] <slot> <kind> <name> <rights>` per capability; called from all three of `launch_init`, `launch_fabric_boot_init`, `launch_recovery_init` | Init's capability layout is observable rather than implied by source order |
| `scripts/check/check-boot-layout.py` | Boots sixteen profiles, extracts each layout block, compares against a fixture; `--bless` rewrites, `--profile` narrows | A layout change produces a reviewable diff instead of a silent behavior change |
| `contracts/boot-layout/v1/fixtures/*.layout` | Sixteen frozen baselines, captured before any B10 edit | The pre-refactor layout is recoverable and diffable |
| `Justfile` | `boot_layout_check`, `boot_layout_bless` | The equivalence claim has a gate that exists |
| `contracts/boot-layout/v1/{schema,gen_rust}.zt`, `boot-contracts/src/boot_layout.rs` | The layout format and its decoder | The layout is expressible as generation data |
| `scripts/build/boot_layout.py`, `build-generation.py` | Emit the resource per generation number, inside `build_generation` | Two generations built from one manifest cannot share one layout |
| `scripts/check/check-boot-layout-resource.py` | Host-side agreement between the emitted resource and every fixture | The emitter's agreement with the kernel is checked in under a second rather than by an 18-boot QEMU cycle |

The dump names object *kind*, and identity for the two kinds that would
otherwise be ambiguous: component name for executables, channel label for
endpoints. Endpoint addresses, executable bytes, and block-device addresses are
deliberately excluded: they vary per boot or per host, and including them would
make the fixture unstable and therefore worthless as an equivalence check.

The endpoint label was not in the first draft of this dump, and its absence
would have made the check unsound. Roughly half of init's slots are endpoints,
and most carry identical rights — thirty of the sixty-one default slots, and
`fabric-call` slots 51 through 59 rendered as nine identical `endpoint - 0x7`
lines. A refactor that swapped two control endpoints, or placed a service half
where a client half belonged, would have compared *equal*, and the resulting
failure would have surfaced as a fabric correlation error far from its cause —
precisely the failure this check exists to catch. `ipc::Endpoint` therefore
carries an optional `&'static str` label, set by the boot path and used only by
the dump. It carries no authority and gates nothing; endpoints minted at
runtime through `SYS_ENDPOINT_CREATE` have none, because no layout describes
them.

After labelling, every slot in fifteen of the sixteen fixtures is uniquely
identified. The exception is the store profile, where slots 9 and 18 are both
`object-store - 0x3004`; those two capabilities are genuinely identical, so a
swap between them is not an observable change.

`check-boot-layout.py` clears every gate-selecting `SLIME_*` flag from the
inherited environment before applying a profile's own settings. Without that, a
profile invoked from a shell that already exported another gate's flag would
capture a layout its gate never boots.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A refactor silently moves a slot | `just boot_layout_check` | `<name>: layout differs`, with the `was:`/`now:` lines for each moved slot |
| A refactor swaps two same-rights endpoints, or a client half for its service half | `just boot_layout_check` | The endpoint label differs on both slots. Guarded only because endpoints carry a channel label; without it these compare equal |
| A profile stops booting far enough to build init's table | `just boot_layout_check` | `<name>: boot emitted no complete layout block` |
| A layout change lands without review | fixture diff in version control | `.layout` files appear in the changeset |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sample_plane_live_check` (before any edit) | ok — baseline was green | Direct |
| `just contracts_check` (before any edit) | passed, 50/50 host tests | Direct |
| `check-boot-layout.py --bless --profile sample-plane` | 61 slots recorded | Direct |
| `check-boot-layout.py --profile sample-plane` (rerun against its own fixture) | 61 slots match — dump is deterministic across boots | Direct |
| `check-boot-layout.py --bless` (all sixteen) | all captured; 61 slots typical, 63 for fabric-qos and fabric-call, 53 for fabric-boot | Direct |
| Duplicate-line scan of all sixteen fixtures after labelling | 15 fixtures fully discriminated; storage-store has one genuine duplicate pair (slots 9/18, identical capabilities) | Direct |
| `check-boot-layout.py` on fabric-call, sample-plane, fabric-boot after labelling | all match — labels are deterministic across boots | Direct |
| `just sample_plane_live_check`, `just fabric_call_check`, `just data_fabric_boot_check` after labelling | ok; fabric-boot still reports 53 of 64 slots and 20 roles | Direct |
| Generation 14 booted with `SLIME_FABRIC_CALL_CHECK` unset | layout byte-identical to the `fabric-call` fixture — the flag does not select the layout | Direct |
| Generation 17 booted with `SLIME_FABRIC_BOOT_CHECK` unset | 61-slot `init` layout, not the 53-slot `fabric-boot` one — the only flag that does select a layout | Direct |
| Storage profiles booted with a virtio-blk device attached | slot 9 becomes `block-device` (`0x404` read, `0xc04` write/fault) where it was the `object-store` fallback — object kind is decided by PCI enumeration | Direct |
| `just boot_layout_check`, all eighteen profiles | all match | Direct |
| `check-boot-layout-resource.py` | 17 generations encode and decode; 18 fixtures agree with the emitted resource | Direct |
| Built generation with `SLIME_GENERATION_NUMBER=14` | generation 1's embedded resource carries number 1 with 61 entries, generation 2's carries number 14 with 63 — one build, two layouts | Direct |
| `just rollback_check`, `just bootstate_trace_check` | pass — generation 1 still boots as the known-good | Direct |
| `just generation_check` | generation bytes still reproducible across two builds | Direct |
| Fault injection into the emitter: duplicate slot, named role without a label, unnamed role with one | all three rejected. The duplicate initially passed — `layout_for` merges into a dict, so a slot claimed twice within one source table was silently dropped before validation saw it. Now checked per table | Direct |
| `just storage_read_check` | ok | Direct |
| `just storage_write_check` | not observed — the gate does not complete on this host, before or after these changes (see open risks) | Direct |
| `just fmt_check_all` | passed | Direct |
| `just lint_all` | passed | Direct |
| `just ruff`, `just typos` | passed | Direct |

The captured fixtures confirm B10's premise directly rather than by reading
source. Sixteen profiles produce eight distinct layouts (nine profiles share
one). Slot 46 holds `fabric-publisher` under the default layout and
`fabric-call-client` under the call profile; the call profile also carries
`RIGHT_TRANSFER` on slots 45-47 and 49 (`0x1000c` against `0x10008`) where the
default does not. Slot 9 is an `object-store` with rights `0x1000` by default
and `0x3004` under the store profile. This is the aliasing B10 exists to
remove, now recorded rather than inferred.

## Decisions

Two properties of the layout were assumed rather than known. The fixtures
settle both, and each settles a schema decision that would be expensive to
revisit later.

- Decision: the layout resource is keyed by generation number alone.
- Rationale: booting generation 14 with `SLIME_FABRIC_CALL_CHECK` unset
  produces a layout byte-identical to the blessed `fabric-call` fixture, so the
  flag does not participate. Detail below.
- Rejected alternative: keying the resource by flag *and* number, which would
  have carried the compile-time coupling into the data format B10 exists to
  replace it with.

- Decision: a layout entry declares a *role* and rights, not a concrete object
  kind; the kernel resolves the role.
- Rationale: slot 9's kind is decided by PCI enumeration at boot and is not
  knowable to the host builder. Detail below.
- Rejected alternative: an entry carrying a primary kind and a fallback kind.
  It models today's code faithfully but widens every entry to accommodate one
  case, and it moves hardware probing into a declarative format.

**The layout is a function of `generation.number` alone, not of the gate flag.**
Booting generation 14 with `SLIME_FABRIC_CALL_CHECK` unset produces a layout
byte-identical to the blessed `fabric-call` fixture. The `SLIME_*_CHECK` flags
paired with a generation number in `launch_init` are therefore redundant for
layout purposes; the three at lines 68/71/76 are in `start()`'s script-install
path and the sites past line 1585 are in the healthy/idle exit condition,
neither of which touches `caps`. The single exception is generation 17: with
`SLIME_FABRIC_BOOT_CHECK` unset it resolves the ordinary 61-slot layout rather
than `launch_fabric_boot_init`'s 53-slot one, because that flag gates the fork
itself. Since only the fabric-boot gate ever builds generation 17, that flag is
redundant too — which is what lets the fork be selected by generation data.

**A slot's object kind is not always host-knowable, but its role is.** Slot 9
is `object-store - 0x1000` with no drive attached and `block-device - 0x404`
with one, because `optional_block_function()` decides the kind by PCI
enumeration at boot. The host builder cannot emit that. Across all four storage
profiles the slot is always the *storage capability*, while its kind varies with
both generation number and attached hardware — `block-device 0x404` for read,
`block-device 0xc04` for write and fault, `object-store 0x3004` for store.

The resource must therefore declare **which slot holds which role with which
rights**, and leave the kernel to resolve a role to a concrete object. That is
exactly the positional coupling B10 removes, and nothing more: hardware
probing stays kernel-side where it belongs.

This also corrected the storage fixtures. As first captured they booted without
a drive and recorded the no-disk fallback — not the layout `just storage_check`
exercises. The storage profiles now attach a virtio-blk device, and a
`storage-read` profile was added so the read-only block capability has a
baseline of its own.

- Decision: the layout resource declares slot assignment keyed by a per-half
  channel label; the kernel keeps minting its 27 channels in source.
- Rationale: B10's stated fix is to "resolve init's grants by name from the
  generation instead of by index in kernel source." Channel *creation* was
  never the defect. Keeping it kernel-side also means the blessed fixtures are
  the lookup keys verbatim, so the layout needed no third re-blessing.
- Rejected alternative: declaring channels in the resource and having the
  kernel mint from that declaration. It restructures IPC setup for no gain
  against the defect B10 names.

- Decision: the storage slot declares the authority a *present* block device
  carries; the no-disk fallback stays kernel-side.
- Rationale: the host cannot know whether a device will be enumerated. The
  check accepts exactly one substitution — a read-only object store — so a
  kernel that resolved anything else in that slot still fails.
- Rejected alternative: declaring both a primary and a fallback kind per entry.
  It widens every entry for one case and moves hardware knowledge into a
  declarative format.

- Decision: capture the baseline before writing any part of the fix.
- Rationale: the exit condition requires demonstrating that every profile
  resolves to the slots it holds today. Recording the layouts after the
  refactor would prove only that the new code agrees with itself.
- Rejected alternative: derive the expected layout by reading `launch_init` and
  asserting against a hand-written table. That re-encodes the positional
  convention in a second place and would need editing in lockstep with the
  refactor, which is precisely the coupling B10 removes.

- Decision: the dump is unconditional, not behind an `option_env!` gate.
- Rationale: B10's stated harm is that gate flags make each check build a
  different kernel binary. A flag-gated dump would add one more such flag and
  could not observe the layout of a boot that did not set it.
- Rejected alternative: a `SLIME_LAYOUT_DUMP` flag. Cheaper on serial output,
  but it would mean the layout check does not run against the same binary the
  other gates run against — which is the property under repair.

- Decision: `just boot_layout_check` is the gate B10 names at closure.
- Rationale: B10's text observes that P0/P1 name `just
  architecture_contract_check` and `just x86_portability_check`, neither of
  which exists, and requires the item to name a gate that does exist. This one
  guards exactly the claim B10 makes.

## Open risks and follow-ups

- [ ] The eighteen profiles are enumerated by hand in `PROFILES`, mirroring what
      the check scripts set. A new gate that boots a new layout will not be
      covered until it is added there. Not automated because the check scripts
      set their environment inline rather than declaring it.
- [ ] Two layouts remain uncovered. `launch_recovery_init`'s four-slot table
      needs a recovery bootstore rather than a plain `cargo run`, and the
      transfer layout needs two block devices at specific PCI addresses plus a
      staged receiver generation. **Decision:** `launch_recovery_init` is not
      collapsed in B10. Its trigger — `component_named("recovery").is_some()` —
      is already generation-data-driven rather than a compile-time flag, so it
      is not the defect B10 names, and collapsing it would rest on
      `check-recovery.py`'s behavioral markers rather than a layout fixture.
      The transfer tail (`bootstrap.rs:866`, appending two slots when PCI
      devices 5 and 6 are both present) is in scope and must be preserved, but
      is verified by `just transfer_check` rather than by equivalence.
- [ ] `fabric-boot` captures 53 slots from `launch_fabric_boot_init`, a
      different function than the other fifteen. When the forks collapse (B10,
      later step) this fixture must still match, which is the point — it is the
      strongest single check that the collapse preserved behavior.
- [ ] The dump adds ~62 serial lines per boot. Harmless for QEMU gates; revisit
      if serial throughput becomes a boot-time cost on hardware.
- [ ] `just storage_write_check` did not complete on this host. It is not
      caused by these changes: it reproduces identically with every change
      stashed, on the unmodified commit. `check-storage.py` runs `cargo run`
      without `--release`, so the guest is a debug build under TCG, and it
      reached only line 37 of the ~141 a release boot emits in the same wall
      time. The same image boots clean in a few seconds with `--release`.
      `run_guest` passes `timeout=None`, so a slow boot hangs indefinitely
      rather than failing — worth a bound, and worth asking whether the gate
      should build release like its siblings. Filed here rather than fixed:
      unrelated to B10, and changing a gate's build profile mid-item would
      confuse this item's evidence.
- [ ] Storage identity selection at `bootstrap.rs:571`/`:595` is decided **in
      scope** for B10: leaving it means `launch_init` still contains
      `generation.number ==` branches, which B10's exit condition forbids. The
      `storage-write`/`storage-fault`/`storage-store` fixtures are the evidence
      that removing it preserves behavior.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: `contracts/boot-layout/v1/fixtures/*.layout` (the observed
  layouts themselves; not a log, and regenerated only through `--bless`)
- Serial/debugger/model output: `[layout]` lines in every gate's serial capture
- Related roadmap item: `roadmap/00-backlog.md` B10; diagnosis in
  `devlog/2026-07-31-boot-layout-positional-coupling/`
