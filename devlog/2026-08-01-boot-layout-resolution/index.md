# Init's capability layout resolves from generation data

| Field | Value |
|---|---|
| Date | 2026-08-01 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/src/runtime/bootstrap.rs`, `kernel/src/runtime/generation.rs`, `contracts/boot-layout/v1/`, `boot-contracts/src/boot_layout.rs`, `scripts/build/boot_layout.py`, `scripts/check/check-boot-layout-resource.py` |
| Roadmap | B10 |
| Gates | `just boot_layout_check`, `just contracts_check` |
| Trigger | B10: `launch_init` built init's capability vector by writing fixed indices, so a profile's participant set was kernel source rather than generation data. |
| Baseline | The eighteen layouts frozen in `contracts/boot-layout/v1/fixtures/`, captured in `devlog/2026-07-31-boot-layout-baseline/`. |

## Summary

`launch_init` no longer writes capability slots by index. Each capability is
offered to a placer under the name the generation's boot layout knows it by,
and the layout decides where it lands. A capability the layout does not name,
or a declared slot nothing fills, stops the boot rather than silently shifting
the table. The storage-identity `generation.number` matches are gone —
resolved from the layout instead — and the remaining branches choose which
capabilities to *mint*, not where they go. All eighteen frozen layouts resolve
byte-identically, which is the equivalence B10's exit condition asks for.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `bootstrap.rs` | `LayoutPlacer` fills `[Option<Capability>; MAX_CAPS]` by name and checks both directions before collapsing to a vector | A slot's occupant is generation data; a mismatch between kernel and layout is fatal, not silent |
| `bootstrap.rs` | `storage_capability` and `storage_component` `match generation.number` deleted; the storage slot resolves through `entry_for_role` and `one_of` | Two of the nine `generation.number` branches B10 counted are gone by construction |
| `bootstrap.rs` | ~36 now-redundant `component_bytes` bindings removed; the placer resolves each executable itself | One lookup per component, at the point of use |
| `generation.rs` | `boot_layout()` accessor, fatal on absent/malformed/number-mismatched | The equivalence check cannot pass by falling back to the code it is checking |

The placer checks agreement in both directions. A declared slot left empty
means the kernel did not mint something the layout expects; a filled slot the
layout does not declare means the kernel minted something with nowhere to go.
Without the second check, a kernel that minted a capability the layout forgot
would drop it and boot on regardless.

Rights are checked too: `place` asserts the minted rights equal the declared
ones. That caught nothing during this change, which is the point — it is what
makes the layout's rights column load-bearing rather than decorative.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A capability moves to a different slot | `just boot_layout_check` | `<profile>: layout differs`, naming the slot |
| The kernel mints something the layout does not place | placer assertion at boot | `kernel minted a capability for slot N, which the layout does not declare` |
| The layout declares a slot the kernel never fills | placer assertion at boot | `boot layout declares slot N, but the kernel minted nothing for it` |
| Minted rights drift from declared rights | placer assertion at boot | `slot N (<what>) declares rights 0x…, kernel minted 0x…` |
| A generation boots another generation's layout | `generation::boot_layout` assertion | `boot layout belongs to another generation` |
| The emitter and the kernel disagree | `just contracts_check` | Host-side, in under a second, without booting |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just boot_layout_check` | all eighteen profiles match their pre-change fixtures | Direct |
| `just sample_plane_live_check` | ok | Direct |
| `just fabric_call_check` | ok | Direct |
| `just fabric_operation_check` | ok | Direct |
| `just fabric_stream_check` | ok | Direct |
| `just fabric_visibility_check` | ok | Direct |
| `just data_fabric_boot_check` | ok — 53 of 64 slots, 20 roles, three route workers | Direct |
| `just dango_check` | ok | Direct |
| `just generation_cmd_check` | ok | Direct |
| `just powerbox_check` | ok | Direct |
| `just directory_check` | ok | Direct |
| `just rollback_check` | ok — failing pending generation returns to known-good | Direct |
| `just bootstate_trace_check` | ok — 3 durable transitions conform | Direct |
| `just test` | passes | Direct |
| `just contracts_check` | passes, including the host-side resource check | Direct |
| `just fmt_check_all`, `just lint_all` | pass | Direct |

Three defects surfaced during the change, each caught by a fixture rather than
by reading code, and each a case the old positional writes had encoded
implicitly:

1. **Generation 4 declares two identical `object-store` entries** (slots 9 and
   18). Resolving a role by "first match" filled slot 9 twice and left 18
   empty. `role()` now takes the first *unfilled* match, so repeated calls walk
   the declared entries in order.
2. **Generation 14 leaves `fabric-subscriber-b` in slot 50.** The call profile
   rewrote slots 46-49 and stopped; the stream participant stayed, inert but
   still handed to init.
3. **Generation 15 stops at a different point.** The operation profile takes
   slot 50 as well, but leaves subscriber-b's control channel at 55 and 60.

The second and third are the clearest argument for this change. Which slots a
profile overwrote was previously implied by the index range a rewrite block
happened to cover — not stated anywhere, not checked, and different between two
profiles that read as parallel. They are now declared, and a fixture fails if
either drifts.

## Decisions

- Decision: `generation::boot_layout` panics rather than returning `Option`.
- Rationale: `fabric_graph` and `shared_buffer_quota` degrade to a permissive
  default because their negative case is legitimate — a generation may declare
  no fabric, a component may hold no quota. Init always has a capability table,
  so `None` could only mean "fall back", and the only fallback available is the
  hardcoded layout this resource replaces. `boot_layout_check` would then
  compare the old path against itself and report a match.
- Rejected alternative: returning `Option` and keeping the positional path as
  fallback during migration. Cheaper to land, but it makes the gate that proves
  the migration worked incapable of failing.

- Decision: the kernel keeps minting channels; the layout only places them.
- Rationale: what a channel is, and which half a client holds, is knowable only
  in the kernel. B10's defect was resolution by index, not channel creation.
- Rejected alternative: declaring channels in the resource. It restructures IPC
  setup for no gain against the defect B10 names.

- Decision: the storage slot's no-disk fallback rights stay in kernel source.
- Rationale: the layout declares the authority a present block device carries.
  When none is enumerated a read-only object store stands in, and applying the
  declared block rights to it would grant an authority the object cannot
  answer for.

## Open risks and follow-ups

- [ ] Four `generation.number` branches remain in `launch_init`, down from
      nine. They now select which capabilities to mint — the call, operation,
      and QoS profiles build different channel sets — rather than where any of
      them goes. The `SLIME_FABRIC_BOOT_CHECK` + `generation.number == 17` fork
      at the top of the function is untouched and is the next step's work.
- [ ] `init.rs` still hardcodes its slot constants. Until it consumes the same
      table, the layout is authoritative on one side only, and the two agree by
      inspection rather than by construction.
- [ ] `launch_recovery_init` still builds its four-slot table positionally.
      Decided out of scope in the baseline entry: its trigger is already
      generation-data-driven, and no layout fixture covers it.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: `contracts/boot-layout/v1/fixtures/*.layout`
- Serial/debugger/model output: `[layout]` lines in every gate's serial capture
- Related roadmap item: `roadmap/00-backlog.md` B10; baseline in
  `devlog/2026-07-31-boot-layout-baseline/`; diagnosis in
  `devlog/2026-07-31-boot-layout-positional-coupling/`
