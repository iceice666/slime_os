# Init's capability layout resolves from generation data

| Field | Value |
|---|---|
| Date | 2026-08-01 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/src/runtime/bootstrap.rs`, `kernel/src/runtime/generation.rs`, `contracts/boot-layout/v1/`, `boot-contracts/src/boot_layout.rs`, `scripts/build/boot_layout.py`, `scripts/check/check-boot-layout-resource.py` |
| Roadmap | B10 |
| Gates | `just boot_layout_check`, `just contracts_check` |
| Trigger | B10: `launch_init` built init's capability vector by writing fixed indices, so a profile's participant set was kernel source rather than generation data, and each gate's `SLIME_*_CHECK` flag built a different kernel binary. |
| Baseline | The eighteen layouts frozen in `contracts/boot-layout/v1/fixtures/`, captured in `devlog/2026-07-31-boot-layout-baseline/`. |

## Summary

`launch_init` no longer writes capability slots by index. Each capability is
offered to a placer under the name the generation's boot layout knows it by,
and the layout decides where it lands. A capability the layout does not name,
or a declared slot nothing fills, stops the boot rather than silently shifting
the table.

Every `option_env!` and `generation.number` branch is gone from `launch_init`,
and with them the property B10 was really about: **one kernel binary now passes
every gate**. Built with no flags and built with `SLIME_FABRIC_BOOT_CHECK`,
`SLIME_DANGO_CHECK`, `SLIME_FABRIC_CALL_CHECK`, `SLIME_POWERBOX_CHECK`, and
`SLIME_GENERATION_CMD_CHECK` all set, the kernel hashes identically. Before
this, each of those flags produced a different artifact, so no single kernel
existed that the gate suite had collectively exercised.

All eighteen frozen layouts resolve byte-identically, which is the equivalence
B10's exit condition asks for.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `bootstrap.rs` | `LayoutPlacer` fills `[Option<Capability>; MAX_CAPS]` by name and checks both directions before collapsing to a vector | A slot's occupant is generation data; a mismatch between kernel and layout is fatal, not silent |
| `bootstrap.rs` | `storage_capability` and `storage_component` `match generation.number` deleted; the storage slot resolves through `entry_for_role` and `one_of` | Two of the nine `generation.number` branches B10 counted are gone by construction |
| `bootstrap.rs` | ~36 now-redundant `component_bytes` bindings removed; the placer resolves each executable itself | One lookup per component, at the point of use |
| `generation.rs` | `boot_layout()` accessor, fatal on absent/malformed/number-mismatched | The equivalence check cannot pass by falling back to the code it is checking |
| `bootstrap.rs` | Profile branches ask `declares_component` / `declares_channel` instead of comparing `generation.number` to a literal | A branch states why it is taken; the four `generation.number ==` branches are gone |
| `bootstrap.rs` | The C8.10 fork keys on the layout declaring `fabric-call-worker`, not on `SLIME_FABRIC_BOOT_CHECK` + generation 17 | The fork is selected by generation data, like the recovery fork beside it |
| `bootstrap.rs` | The script-install and idle-exit `SLIME_*_CHECK` flags dropped; each was `flag && number == N`, and every gate's number is unique | One kernel binary for the whole gate suite |
| `bootstrap.rs` | `assert!(caps.len() <= MAX_CAPS)` after the transfer append | The one path that can outgrow the table is bounded, as `launch_fabric_boot_init` already was |
| `build-generation.py` | Its own directory added to `sys.path` | The builder imports its sibling modules when invoked from another directory |

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
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | pass | Direct |
| `just fabric_authority_check`, `just fabric_qos_check` | ok | Direct |
| `just transfer_check` | ok — install, pending boot, promotion, and rollback retention. The only gate exercising the post-`finish()` append | Direct |
| Kernel built with no flags vs. with `SLIME_FABRIC_BOOT_CHECK`, `SLIME_DANGO_CHECK`, `SLIME_FABRIC_CALL_CHECK`, `SLIME_POWERBOX_CHECK`, `SLIME_GENERATION_CMD_CHECK` all set | byte-identical (`eef6fd1b5f611c20`). Before the change the same comparison gave three distinct hashes across two flag sets | Direct |

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

- [ ] `SLIME_INTERACTIVE` remains in `on_idle`. It is a user-facing mode
      selected by `just run`, not a gate, and it keeps the session alive for a
      human keystroke rather than choosing a boot path. It is the only
      `option_env!` left in `bootstrap.rs`, and it does not divide the kernel
      binary across the gate suite — the identity check above was run with it
      unset in both builds, as every gate leaves it.
- [ ] 52 `option_env!` sites remain in `components/`. They make build-time
      decisions independent of the kernel layout, and B10's text says so; the
      component images are per-generation artifacts by design, content-hashed
      into the generation. Out of scope here, and not a blocker for P1, which
      asks that architecture-neutral *kernel* code type-check for AArch64.
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
