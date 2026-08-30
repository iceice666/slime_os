# B88 — the network-destination decoder read a Zutai layout through byte literals

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/network-destination/v1/{schema.zt,gen_rust.zt}`, `boot-contracts/src/{network_destination.rs,generated/network_destination.rs}` |
| Roadmap | B88, IO4 |
| Gates | `just contracts_check`, `just test_host`, `just io_network_check` |
| Trigger | The IO device-boundary survey flagged this file as the network path's one hand-written wire parser; the repository's Zutai rule requires generated bindings for any format crossing a persistence or boot boundary |
| Baseline | IO4 complete and `just io_network_check` green since 2026-08-28; the decoder correct but its layout knowledge duplicated in source literals |

## Summary

`contracts/network-destination/v1/schema.zt` has always declared complete
`headerLayout` and `entryLayout` records, and the *Python* renderer has always
emitted their offsets. The Rust renderer emitted only scalar constants. The
consequence was not a bug — the literals were all correct — but a rule
violation with a real failure mode: `boot-contracts/src/network_destination.rs`
reached all twenty-two fields of IO4's authority record through hard-coded
offsets (`entry[32]`, `u16_at(entry, 34)`, `entry[56..120]`, eight budget
literals), and its own test encoder restated the same numbers independently. Two
copies that agree with each other and are checked against the schema by nothing:
a schema field-width edit would have regenerated the Python side, left both Rust
copies stale, and still passed every test, because the encoder under test shared
the decoder's mistake. The Rust renderer now emits `OFF_HEADER_*`/`OFF_ENTRY_*`
constants from the same layout the Python side uses, every literal is gone from
both the decoder and its encoder, and the module's test count went from 3
positive-path cases to 9 covering the refusals that previously had none.

## Observable symptom

No failing command. This is debt, not a regression, and it is recorded because
the *absence* of a failure mode was the problem.

- Command: `python3 scripts/generate/generate-boot-bindings.py --check`
- Expected: a drift check that fails when the schema and its Rust consumer disagree about field placement
- Observed: reports "current" regardless, because the generated file contained no offsets to disagree about. The decoder's literals were outside the generator's knowledge entirely.
- Exit/fault/serial evidence: `boot-contracts/src/generated/network_destination.rs` was 27 lines of scalars; the Python sibling emitted a full offset table from the same schema.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `schema.zt` declares `headerLayout`/`entryLayout` with every field's width and `byteArray` flag | The layout is already schema data; nothing needed inventing. |
| 2 | `gen_rust.zt`'s `pythonBindings` calls `w.renderRecords`, which emits offsets; `rustBindings` emits only `r.*Const` scalars | The asymmetry is the whole defect. One target got the layout, the other did not. |
| 3 | Sibling generated files (`bootstate.rs`, `release.rs`, `transfer.rs`, `store_disk.rs`, `component_image.rs`, `kernel_image.rs`) all carry `*_OFFSET`/`OFF_*` constants | Emitting them is the established convention, not a new mechanism. `network_destination` was the outlier. |
| 4 | `wire.rust`'s `WireField` has no `byteArray` field, so `r.offsetConsts` cannot describe the 32-byte holder identity, 16-byte address, or 64-byte name | The shared helper is unusable here; `contracts/network-service/v1/gen_rust.zt` already solves exactly this with a local `offsetConsts`. Followed that precedent. |
| 5 | The generated offsets, once emitted, matched every hand-written literal exactly | Confirms the refactor is byte-neutral: this was latent drift risk, not live drift. |
| 6 | `decode_entry` also carried a bare `4` for the IPv4 prefix, load-bearing for the "every byte past the prefix is zero" canonical-encoding rule | Added `ipv4Bytes` to the schema rather than leaving one literal behind. |
| 7 | The module's `#[cfg(test)]` encoder restated the same offsets | The decoder's only adversary shared its layout assumptions, so a wrong offset agreed with itself. Converted it too. |
| 8 | Only 3 tests existed, all asserting successful decode plus authority denials | Every `DecodeError` arm — `BadMagic`, `UnsupportedVersion`, `UnknownRequiredFlags`, `BadBounds`, `BadOrder`, `InvalidEntry`, `Impossible` — was unreachable code as far as any check could prove. |

## Root cause

One renderer, two targets, one of them incomplete. `records f` builds
`WireRecord` values carrying `headerLayout`/`entryLayout` and hands them to
`w.renderRecords` for Python; the Rust branch never consumed the layout at all.
Because the generator emitted no offsets, the Rust consumer had no generated
names to use, so it necessarily wrote literals — and once written, those
literals were the only statement of the layout on the Rust side. The rule
violation and the missing offsets are the same fact viewed from either end.

The secondary cause is the test shape. A decoder whose test encoder is written
from the same literals cannot detect an offset error: both sides move together.
That is why the mutation controls below matter more than the test count.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/network-destination/v1/schema.zt` | New `ipv4Bytes = 4`, threaded through `format` | The IPv4 prefix length is schema data, like every other width in this format |
| `contracts/network-destination/v1/gen_rust.zt` | Local `offsetConsts` emitting `OFF_<PREFIX>_<FIELD>` and `_END` for both records, `byteArray`-aware; `IPV4_BYTES` scalar | The Rust target receives the same layout the Python target already did |
| `contracts/network-destination/v1/gen_rust.zt` | `render` additionally requires `wireBytes headerLayout == headerBytes`, the same for the entry, and `0 < ipv4Bytes <= 16` | Offsets are only sound if the declared layout sums to the declared record size; a bad schema edit now renders `INVALID_NETWORK_DESTINATION_SCHEMA` instead of wrong constants |
| `boot-contracts/src/generated/network_destination.rs` | Regenerated: +47 lines of offsets | Generated, not written |
| `boot-contracts/src/network_destination.rs` | Every literal in `decode` and `decode_entry` replaced by generated constants; `unwrap()` on layout slices became `expect("generated network-destination layout")` | The decoder's field placement has one source |
| `boot-contracts/src/network_destination.rs` (tests) | Test encoder addresses fields by generated offsets | The decoder's adversary no longer shares its assumptions |
| `boot-contracts/src/network_destination.rs` (tests) | Six new tests: malformed header fields, non-canonical addresses, DNS bounds/syntax, duplicate/descending order, impossible authority and budgets, per-holder ceiling | Every `DecodeError` arm is now reachable by a check |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Schema layout and Rust consumer drift | `just contracts_check` (runs the generator `--check`) | `boot contract bindings are stale` |
| A schema edit breaks the layout/size invariant | `gen_rust.zt`'s `render` predicate | Generated file contains `INVALID_NETWORK_DESTINATION_SCHEMA`, which does not compile |
| A wrong offset reaches the decoder | `just test_host` | Decode tests abort; verified by corrupting `OFF_ENTRY_PORT` |
| Canonical-padding rules stop being enforced | `just test_host` | `a_non_canonical_address_encoding_is_refused` |
| `name_len` stops being bounded before it slices | `just test_host` | `dns_name_bounds_and_syntax_are_enforced_at_the_boundary` |
| Duplicate destinations become representable | `just test_host` | `duplicate_and_descending_entries_are_refused` |
| The decoder stops working end to end | `just io_network_check` | Missing destination/denial marker |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just boot_gen` then `generate-boot-bindings.py --check` | PASS — `Boot contract bindings are current`; generated file grew by 47 offset lines | Direct |
| Generated offsets vs. the replaced literals | Identical at every one of the 22 fields — the refactor is byte-neutral | Direct |
| `cargo test -p boot-contracts --all-features` | PASS — 335 passed, 0 failed (was 329); the destination module went 3 → 9 | Direct |
| Mutation: DNS tail-zero check removed | Test failure | Direct |
| Mutation: IPv4 tail-zero check removed | Test failure | Direct |
| Mutation: strict order (`<=` → `<`) | Test failure | Direct |
| Mutation: `name_len > MAX_NAME_BYTES` bound removed | Test failure — the slice panics, which is the out-of-bounds the survey predicted | Direct |
| Control: `OFF_ENTRY_PORT` corrupted 36 → 34 in the generated file | Tests abort (`SIGABRT`), proving the generated offsets are load-bearing rather than decorative | Direct |
| `just io_network_check` | PASS — `exact destinations, denials, reset, restart, and backend independence proved` | Direct |
| `just contracts_check` | PASS | Direct |
| `just fmt_check_all`, `just lint_all` | PASS after `cargo fmt -p boot-contracts` | Direct |
| `just generation_check` | PASS — two isolated builds byte-identical, generation `9fe7c34d2f379cf30d61deb50768a95ae7bcd17490fde5e57e2d39a0bb2f2d64` | Direct |

### On the generation hash

`just generation_check` now reports `9fe7c34d…` where entries earlier the same
day recorded `197c86bc…`. This change is **not** the cause: stashing every file
touched here and re-running reported `9fe7c34d…` from the untouched baseline
too. The shift predates this work, no fixture pins either value, and the two
older references to `197c86bc…` are historical records in
`devlog/2026-08-29-b85-stale-proto-dependencies/` and its backlog entry, which
are frozen and correct as of their own runs. Determinism itself is intact — the
gate's whole assertion is that two isolated builds agree, and they do.
**[INFERENCE]** the likely origin is an earlier same-day change to a component
compiled into the default manifest; not investigated further, because nothing in
this entry's scope affects it.

## Decisions

- **Decision:** Extend the existing renderer rather than adopt `wire.rust`'s
  shared `offsetConsts`.
  **Rationale:** That helper's `WireField` has no `byteArray` flag and cannot
  describe this format's 32-, 16-, and 64-byte array fields.
  `contracts/network-service/v1/gen_rust.zt` already carries a local
  `offsetConsts` for exactly this reason, so following it keeps one convention
  instead of introducing a second.
  **Rejected alternative:** widening `wire.rust`'s `WireField` with `byteArray`.
  It is shared by the block/store/component family, and changing a type every
  boot contract imports to serve one contract's needs trades a local addition
  for a global risk. Worth doing as its own change, with those contracts' gates
  run; not smuggled into a defect fix.
- **Decision:** Emit both `OFF_*` and `OFF_*_END`.
  **Rationale:** Byte-array fields are read as ranges. Without the end
  constant, every call site would write `OFF_ENTRY_NAME + 64`, putting a width
  literal back into the consumer — the exact thing being removed.
  **Rejected alternative:** offsets only, matching `wire.rust`. It would have
  left the widths hand-written.
- **Decision:** Put `ipv4Bytes` in the schema rather than a `const` in the
  decoder.
  **Rationale:** The prefix length is part of the canonical encoding: the format
  requires every byte past it to be zero, and that rule is enforced during
  decode. A number the wire format depends on belongs in the contract.
  **Rejected alternative:** a local `const IPV4_BYTES: usize = 4;`. Honest and
  readable, but it re-creates the thing this entry removes — one more layout
  fact living in Rust.
- **Decision:** Convert the test encoder to generated offsets in the same change.
  **Rationale:** Leaving it on literals would have preserved the actual failure
  mode. A decoder tested by an encoder that shares its layout errors is
  self-consistent and wrong together; the corrupted-offset control only fails
  because both sides now read the same generated constants.

## Open risks and follow-ups

- [ ] The decoder is still hand-written *logic* over generated *offsets*. That
  is the same shape as every sibling boot contract and is the intended design —
  the Zutai rule owns layout, not validation policy — but no `WireDestinationEntry`
  struct with generated `decode`/`encode` exists here as it does for
  `network-service`. Emitting one would remove the remaining `u16_at`/`u32_at`
  calls; not done, because the semantic validation is interleaved with field
  reads and restructuring it is a larger change than this defect warrants.
- [ ] `wire.rust`'s `WireField` still lacks `byteArray`, so every contract with
  array fields carries its own `offsetConsts` copy: `network-service`,
  `fabric-operation`, and now `network-destination`. Consolidating them is real
  work with a real payoff and its own risk surface.
- [ ] These are fixed-input tests. The decoder's arithmetic — `HEADER_BYTES +
  index * ENTRY_BYTES`, `total != HEADER_BYTES + count * ENTRY_BYTES`, and the
  `u16_at`/`u32_at` `offset + width` additions — is a plausible Kani target in
  the manner of IO6/IO7, and is unproved.
- [ ] The generation-hash shift noted above is explained as out of scope but not
  root-caused.

## Artifacts and provenance

- Focused report: none; the rationale is in the schema comment on `ipv4Bytes`
  and the emitter comment in `gen_rust.zt`.
- Raw transcript: none retained; `just boot_gen` regenerates the bindings and
  `cargo test -p boot-contracts --all-features` reproduces the suite.
- Serial/debugger/model output: `just io_network_check`'s terminal marker, as
  emitted by `scripts/check/check-sel4-io-network-plane.py`.
- Related roadmap item: [IO4 — Network service and exact destination authority](../../roadmap/11-io-substrate.md#io4--network-service-and-exact-destination-authority), with the resolved entry at [B88](../../roadmap/00-backlog.md#resolved).

## Corrections

**2026-08-29 — the follow-up's count was wrong, and the item is now closed.**
The *Open risks and follow-ups* list above says the duplicated `offsetConsts`
affects "`network-service`, `fabric-operation`, and now `network-destination`" —
three contracts. Measuring it found **eighteen** renderers with a local
`offsetConsts`, of which **fourteen** carried a byte-identical 82-line copy of
the whole codec block once the label was parameterised. The estimate was made
from the two files read during that investigation and generalised without
checking; the observed count is in
[`devlog/2026-08-29-b89-shared-codec-emitters/`](../2026-08-29-b89-shared-codec-emitters/index.md).

That entry closes the item: `contracts/_shared/codec.zt` owns the emitters as
`wire.codec`, all fifteen renderers delegate (1382 lines deleted, 189 added),
and this contract's own local copy — added by the fix above — was migrated with
it. Every generated artifact was verified byte-identical across the migration.

The investigation also found what the clone had hidden: `fabric-qos` and
`fabric-time` emitted `expect("generated fabric-stream layout")`, naming the
contract they were copied from. Corrected there, not here.

Nothing in the frozen body above is retracted. `fabric-operation` is correctly
named as a duplicate but was *not* migrated — its `constName` takes one
argument and hard-codes a fixed prefix, so it is a different signature rather
than a copy of the shared block, and changing it would move generated constant
names.
