# P5.4.2 (part) — M5.4's superblock validator, made portable

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/store_disk.rs`, `contracts/store/disk/v1/{schema.zt,gen_rust.zt}`, `boot-contracts/src/generated/store_disk.rs` |
| Roadmap | P5.4.2, P5.4, P5.4.1, M5.4 |
| Gates | `just test_host`, `just miri`, `just contracts_check` |
| Trigger | P5.4.2 opened; `object_store.rs`'s thirty-two ungated assertions are its largest single hole |
| Baseline | `boot-contracts` at 108 host tests; `store_disk.rs` five lines of `include!` and no logic |

## Summary

P5.4.1 recorded `kernel/tests/object_store.rs` as thirty-two assertions no
named gate runs — the largest ungated block in the oracle, and the reason
P5.4.2 carries it. This slice takes the part that is portable *by nature*: the
superblock validator. It is a fixed 64-byte header in a 512-byte sector, and
whether one is well formed is a question about bytes — no disk, no allocator,
no architecture. It lived in `kernel/src/storage/object_store.rs`, so the rules
were reachable only from a `no_std` x86 test binary nothing ran. They are now
in `boot-contracts`, host-tested and Miri-clean, callable by any root on any
architecture.

## Changes

| Area | Change | Effect |
|---|---|---|
| `contracts/store/disk/v1/schema.zt` | `sectorBytes :: Int = 512` | The sector size is schema-owned; the validator's bounds are not a hardcoded 512 |
| `contracts/store/disk/v1/gen_rust.zt` | Renders `SECTOR_BYTES` (Rust) and `STORE_SECTOR_BYTES` (Python) | Both binding sets stay reflected from the one schema |
| `store_disk.rs` | `Superblock`, `SuperblockError`, `encode_superblock`, `decode_superblock` | The rules exist off the oracle |
| `store_disk.rs` | Eight tests | The rules are checked, which they were not before |

The encoder is ported alongside the validator deliberately: a refusal corpus
that hand-assembled sectors would be testing the corpus. With an exact inverse,
each negative case is one mutation away from a sector known to be good.

The append/commit machinery — which sector to write next, which slot to
overwrite, how to recover the previously committed root — is **not** ported. It
is a device concern and belongs with whichever kernel owns the block device.
Validation is not.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A bounds check is dropped or inverted | `just test_host` | The matching case in `each_out_of_range_field_is_refused_on_its_own` fails |
| The CRC is recomputed rather than compared | same | `a_corrupted_checksum_field_is_refused` fails |
| A superblock from another partition is trusted | same | `a_superblock_from_a_different_partition_is_refused` fails |
| Validation becomes uniformly strict | same | `the_extreme_legal_values_are_admitted` fails |
| The generated layout drifts | same | `the_generated_layout_is_self_consistent` fails |
| UB in the byte handling | `just miri` | 116 tests, clean |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo test --all-features --lib store_disk` | 8 passed — [`store-tests.log`](store-tests.log) | Direct |
| `just test_host` (boot-contracts arm) | 116 passed, up from 108 | Direct |
| `just miri` | 116 passed, clean | Direct |
| Fault injection: each of five guards neutered in turn | All five caught — [`fault-injection.log`](fault-injection.log) | Direct |
| `just contracts_check` | Pass — the schema change round-trips | Direct |
| `just fmt_check_all`, `lint_all`, `ruff`, `typos` | Pass | Direct |
| `just test_sel4_root`, `sel4_root_boot_check`, `sel4_stream_check`, `generation_check` | Pass — unaffected | Direct |

`just test_host`'s **`slime-proto` arm fails on this `aarch64-apple-darwin`
host** and did so before this change: that arm pins
`x86_64-unknown-linux-gnu`. Pre-existing and unrelated; the boot-contracts arm
is the one this slice touches.

## Decisions

- Decision: port the **superblock**, not the GPT validator.
- Rationale: GPT's `validate_store_partition` needs a `SectorReader` callback
  and `alloc::Vec` for the entry array, and `boot-contracts` is `no_std` with
  no allocator. Its header parse and PMBR check *are* allocation-free and could
  follow, but GPT is an external UEFI format rather than a Slime-authored one,
  so it is not a Zutai-schema obligation the way the superblock is. Splitting
  at the format boundary keeps this slice's claim exactly as wide as its
  evidence.

- Decision: add `sectorBytes` to the schema rather than importing the kernel's
  `SECTOR_SIZE`.
- Rationale: `kernel/src/protocol/block_proto.rs` is on the deletion path, and
  the repository rule is that a format crossing a persistence boundary is
  defined by its schema. The superblock's own bounds depend on the sector size,
  so it belongs to the same contract.

- Decision: attribute each refusal to a distinct error.
- Rationale: the oracle's `superblock_rejects_corruption` asserted only
  `is_err()` across three mutations, which cannot tell a damaged sector from a
  superblock that does not belong to this partition — operationally different
  events. The ported corpus names the cause for each.

## Open risks and follow-ups

- [ ] **The oracle keeps its own copy.** `kernel/src/storage/object_store.rs`
      still defines `decode_superblock`, and the two could drift until
      P5.4.final deletes it. Same posture as the `component_image.rs` segment
      corpus: the frozen oracle is not edited, so a duplicate is the correct
      cost until deletion.
- [ ] **This is a part of P5.4.2, not the slice.** M5's remaining surface is
      the store's append/commit behaviour, GPT partition validation, the
      recovery paths, and the five `Mediation::Unavailable` planes — all of
      which need a block device `slime-root` does not have. That is genuinely
      multi-session work and the slice stays open.
- [ ] **Eight of the oracle's thirty-two `object_store.rs` assertions are
      superblock-shaped**; the rest exercise the store through a `MockDisk`.
      Porting those needs the append/commit machinery, which needs the device
      decision above.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`store-tests.log`](store-tests.log).
- Serial/debugger/model output:
  [`fault-injection.log`](fault-injection.log).
- Related roadmap item:
  [P5.4.2](../../roadmap/07-architecture-portability.md),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (the inventory that
  recorded the gap),
  [`devlog/2026-08-07-p5-4-10-segment-corpus/`](../2026-08-07-p5-4-10-segment-corpus/index.md)
  (the same move, for `component_image.rs`).
