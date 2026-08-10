# B39 — generation v5 header cutover: authenticated boot action, host checkers, and fabric provenance

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Root-caused |
| Scope | `boot-contracts/src/generation.rs`, `stage0/src/lib.rs`, `slime-root/src/generation.rs`, `scripts/check/check-generation.py`, `scripts/lib/release_trust.py`, `just contracts_check`, `just generation_check`, `just sel4_boot_check` |
| Roadmap | B39 |
| Gates | `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host` |
| Trigger | The in-flight generation v4→v5 cutover for B39 left the host-side checkers, stage-0 consumer, and fabric provenance check on the retired v4 header layout and instance model. |
| Baseline | Before the v5 header grew its process/thread/kernel-object/mapping/binding/schedule/quota plan sections, `just generation_check` passed and every consumer read a 31-field header whose string table began at byte 208. |

## Summary

The v5 wire format landed in `boot-contracts` with ten new plan record types and a
22-field offset table, but four consumers were never migrated with it: the Python
structural checker still destructured the v4 31-field header, `release_trust.py`
still read the string-table offset from the v4 byte position, `stage0` still called
the deleted `component()` API, and `slime-root`'s fabric provenance check resolved
participant names against declared *instances* rather than the *executable*
catalogue. The first three made `just generation_check` fail outright; the fourth
made every fabric-bearing generation unbootable
(`SLIME_ROOT FATAL generation admission rejected: UndeclaredFabricParticipant`).
All four are fixed and their gates pass. B39 remains open: `just sel4_boot_check`
now fails later, at `SLIME_GRAPH spawn refused task=0 slot=6 ungranted`, because
`preflight_spawn_grants` requires each dynamically spawned child to match a
declared owned instance while the seL4 fixtures still declare only `init`.

## Observable symptom

- Command: `just generation_check`
- Expected: byte-identical double build, then generation and boot-store admission.
- Observed: `ValueError: too many values to unpack (expected 31, got 51)` in
  `check_generation`, then after that fix `CheckError: WrongReleaseTarget`.
- Exit/fault/serial evidence: `just sel4_boot_check` reached
  `SLIME_ROOT FATAL generation admission rejected: UndeclaredFabricParticipant`
  immediately after `SLIME_ROOT virtio probed`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `GENERATION_HEADER` is now `8s I I Q 32s Q 32s` + 22×`I` + 22×`Q` + 152 pad = 51 fields; `check_generation` destructured 31. | The Python twin of the decoder had not been migrated at all. |
| 2 | Header byte 100 was a reserved `u32` in v4 and is the boot-action string offset in v5. | The authenticated boot-composition selector needed a decoder-side type, not a raw offset. |
| 3 | `release_trust.generation_release_fields` read `string_offset` from byte 208 (`208 if version >= 4 else 184`); in v5 it lives at byte 328. | Release target text decoded from `dependency_offset`, so every release mismatched its generation. |
| 4 | `stage0/src/lib.rs` called `generation.component(index)` and `generation.kernel_object`, both removed by the v5 rewrite. | `just lint_all` failed on both UEFI targets. |
| 5 | `fabric_graph_participants_are_declared` built its name set from `generation.instance(slot)`, but every seL4 fixture declares exactly one instance (`init`) and carries participants as executables spawned dynamically. | Fabric admission rejected the graph before any component launched. |
| 6 | With admission fixed, the boot plane advanced to `[init] fabric boot control channels minted` and then `spawn refused … ungranted`; `preflight_spawn_grants` requires an instance owned by the caller whose executable matches, plus matching bindings on both parent and child. | The remaining B39 work is the fixture instance-model migration, not a decoder defect. |

## Root cause

The v5 header inserted ten count fields and ten offset fields between
`health_offset` and `string_offset`, shifting every offset past byte 184 and
growing the header tuple from 31 to 51 fields. Consumers that addressed the
header positionally (`fields[17]`, `fields[8]`) or by hardcoded byte offset
(`208`) silently read neighbouring fields instead of failing. Separately, the
v5 rewrite introduced `fabric_graph_participants_are_declared` against the
instance catalogue, but a fabric participant only exists as an *executable*
until init spawns it, so the check could never be satisfied by any existing
fixture.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/generation.rs` | Added `BootAction` (25 variants, `#[repr(u32)]`, parsed from the header's boot-action string at byte 100) and decoded it into `Generation::boot_action`; an unknown spelling is `DecodeError::UnknownEnum`. | The boot composition is authenticated generation data with a stable numeric ABI, independent of the source spelling. |
| `scripts/check/check-generation.py` | Rewrote `check_generation` for the 51-field v5 header: all twenty section-bound assertions, trailing header padding, boot-action admission, and structural validation of process, thread, kernel-object, mapping, cap-binding, service-binding, schedule, fault-policy, spawn-template, and resource-quota records, including "every grant materializes exactly once, or is explicitly policy-only". | The host checker is again an independent twin of the Rust decoder. |
| `scripts/check/check-generation.py` | `check_release` no longer reads `object_offset`/kernel index positionally, and the unreachable v2/v3 kernel-bundle branch is deleted. | No consumer addresses the header by tuple position. |
| `scripts/lib/release_trust.py` | `generation_release_fields` reads `GENERATION_HEADER_STRING_OFFSET_OFFSET` instead of the hardcoded v4 byte 208. | Release target, identity, and authority manifest are derived from the actual v5 layout. |
| `stage0/src/lib.rs` | `admit_generation_closure` locates the kernel by scanning the object closure for `KIND_KERNEL` and walks `executable_count()`/`executable()` instead of the removed `kernel_object` field and `component()` API. | Stage-0 admits a v5 closure on both UEFI targets. |
| `slime-root/src/generation.rs` | `fabric_graph_participants_are_declared` resolves participant identities against the executable catalogue (`MAX_ADMITTED_EXECUTABLES`, `TooManyExecutables`). | A graph naming a component the generation dropped is still refused, while a participant the generation carries as a spawnable executable is admitted. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A header field is added and a positional consumer silently misreads its neighbour | `just generation_check` | Determinism check raises `CheckError` or a bound assertion fires |
| A v5 grant is declared with no materializing capability | `just generation_check` (`UnmaterializedGrant` in `check_generation`) | Admission of an unbacked grant |
| Fabric provenance regresses to matching the wrong catalogue | `just test_sel4_root` (`a_graph_may_not_name_a_component_the_generation_lacks`) | An undeclared participant admits, or a declared one is refused |
| Stage-0 drifts from the generation API again | `just lint_all` | `E0599` on the UEFI targets |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | Pass — 178 contract tests, all bindings current, boot-layout resource current | Direct |
| `just generation_check` | Pass — two isolated builds byte-identical; generation and boot-store admission passed | Direct |
| `just test_host` | Pass — 203 boot-contracts tests plus the slime-proto suites | Direct |
| `just test_sel4_root` | Pass — 130/130 | Direct |
| `just lint_all` | Pass — stage0 (both UEFI targets), boot-contracts, slime-root, components | Direct |
| `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |
| `just sel4_boot_check` | **Fail** — advances past fabric admission to `SLIME_GRAPH spawn refused task=0 slot=6 ungranted` | Direct |

## Decisions

- Decision: `BootAction` is a decoder-side enum with an explicit numeric ABI rather than a string compared at each use site.
- Rationale: the boot composition must reach the bootstrap thread as a word, and component images must stay byte-identical across manifests that differ only in composition.
- Rejected alternative: keeping the raw string offset in the header and letting each consumer compare text, which reintroduces the build-flag coupling B39 exists to remove.

- Decision: fabric participants are resolved against the executable catalogue.
- Rationale: a participant is spawned dynamically by init and has no instance record until it exists; the provenance property under test is "the graph names something this generation carries", and the executable catalogue is what carries it.
- Rejected alternative: requiring an instance per participant, which is the same fixture migration `preflight_spawn_grants` already demands and would have conflated two separate invariants in one check.

## Open risks and follow-ups

- [ ] B39 is not closed. `just sel4_boot_check` fails at `preflight_spawn_grants`: it requires every dynamically spawned child to have a declared instance owned by the caller, with the transferred grant bound on both parent and child, while `sel4-boot.zti` and the other seL4 fixtures declare only `init`. Closing B39 requires migrating each fixture's instance model (`contracts/generation/v1/fixtures/sel4-*.zti`), which is a content migration rather than a decoder change.
- [ ] `init.rs` still selects its composition through `option_env!("SLIME_GENERATION_NUMBER")` and the `SLIME_*_CHECK` flags. `Generation::boot_action` is decoded but not yet passed to the bootstrap thread or consumed by `init`, so B39's "two builds of one component image cannot select different boot graphs" clause is unproven.
- [ ] `scripts/build/build-transfer.py` references `CHECK.GENERATION_COMPONENT`, which no longer exists. It is unreachable from every Justfile target and was already stale before this work; it belongs to B50's residue deletion.
- [ ] The Python checker deliberately does not reimplement `grant_applies_to_instance`'s ownership semantics; that arm is covered only by the QEMU planes. **[INFERENCE]** that this is adequate, on the grounds that the property is admission policy rather than wire-format integrity.

## Artifacts and provenance

- Focused report: none; the investigation log above is the record.
- Raw transcript: none retained.
- Serial/debugger/model output: quoted inline under *Observable symptom* from `just sel4_boot_check`.
- Related roadmap item: [`roadmap/00-backlog.md` B39](../../roadmap/00-backlog.md)
