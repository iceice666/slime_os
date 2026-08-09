# P5.4.2c — M5.4's object store, moved to userspace

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/main.rs`, `components/runtime/src/{lib,heap}.rs`, `components/runtime/Cargo.toml`, `components/bins/src/bin/{sel4-store-probe,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `components/bins/src/default_boot_layout.rs`, `contracts/generation/v1/fixtures/sel4-store.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{store-plane,device-plane,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.2, P5.4, M5.4 |
| Gates | `just sel4_store_check`, `just sel4_device_check`, `just sel4_storage_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | `BlockTransact` gave userspace sectors; nothing above them existed on seL4 |
| Baseline | GPT validation and the object store were reachable only from the frozen oracle's kernel |

## Summary

A userspace component now validates a GPT, opens a content-addressed object
store, retrieves an object by hash with its payload re-verified, appends a
durable commit that preserves the previous root, deduplicates identical content,
scrubs every payload, and falls back to the older superblock when the newest is
damaged. `just sel4_store_check` observes all of it across two fixtures.

The oracle keeps every one of those decisions in the kernel: `store_service`
owns a global store and `sys_store_transact` is syscall 7. **That placement is
what this port does not reproduce.** `StoreTransact` stays
`Mediation::Unavailable` deliberately — not as a gap, but because the operation
it names is policy, and the root has no business owning it. The root mediates
sectors; everything above them is a component.

The implementation is not new. `boot_contracts::{gpt, object_store}` is the same
code the oracle links, driven here over `BlockTransact` through a granted
capability. Which is the finding worth keeping: **M5.4's properties never needed
kernel residence.** They needed a block device and an allocator.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/runtime/src/heap.rs` | A bump `#[global_allocator]` behind a `heap` feature | A component that never allocates carries no allocator |
| `components/bins` | A `store` feature enabling `boot-contracts/gpt` + `slime-rt/heap` | The GPT and store code links only into the build that runs it |
| `sel4-store-probe.rs` | The plane's subject: nine arms over `BlockIo` | M5.4's properties hold with the policy above the root |
| `init.rs` | `drive_probe_plane`, shared by the storage and store planes | One composition, two subjects — the planes differ in what they prove |
| `main.rs` | Bring-up no longer writes to the device | **The root does not modify a disk it knows nothing about** |
| `check-sel4-device-plane.py` | The write marker replaced by a byte-identical image assertion | Coverage moved rather than dropped |
| generation 24, `SEL4_STORE_LAYOUT`, build wiring | The plane's artifact | The gate boots what it asserts about |

### The defect this slice found

`bring_up_block` wrote a signature to **sector 1**, flushed, and read it back —
a boot-time proof of the device-reads-a-buffer DMA direction.

Sector 1 is the GPT primary header. On any partitioned disk the root destroyed
the partition table before userspace ran.

It surfaced as `recovery=primary-damaged` with `gpt error=bad-magic` on a
*freshly built* fixture. The store still opened, because GPT redundancy did
exactly its job and fell back to the backup copy — which is why nothing had
caught it: the damage was invisible to every assertion that only cared whether
the store worked.

Deleted rather than moved to a scratch sector. Boot code that has no idea what
the disk holds should not write to it, and the round trip was already covered
better elsewhere: `sel4_storage_check` proves both directions and a flush from
userspace, on a sector its own fixture designates, through a capability. The
device gate now asserts the stronger property — the image is byte-identical
after the boot — which is not something a serial marker can express.

### Why the allocator is a runtime feature and not a macro

The first attempt gave the probe its own `declare_heap!`. That fails to link:
`extern crate alloc` anywhere in the dependency graph makes *every* binary in
the build require the `#[global_allocator]` symbol, and the store plane builds
`init` alongside the probe. So the allocator lives in `slime-rt` behind a
feature the store-plane build turns on, sized once for its largest consumer.

Bump, and `dealloc` is a no-op. That is the allocation shape rather than a
shortcut: a store component opens a partition, indexes it, answers a bounded
number of requests, and exits. The gate asserts the observed footprint leaves
headroom (72,332 of 262,144 bytes), so the bound is justified by measurement.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The GPT is not really validated | the partition span is pinned, and `recovery=none` on the happy fixture | marker mismatch |
| Superblock CRCs are ignored | the `superblock-newest-damaged` scenario must open at the older sequence | "did not fall back to the older root" |
| Fallback keeps the newer index | the older root must not expose the newer root's object | "exposed an object only the newer root committed" |
| A commit overwrites the active root | exactly one superblock slot may change, and it must be the older | "wrote superblock slots […]" |
| The store scribbles outside its partition | the GPT and protective MBR are compared byte for byte | "modified the GPT or protective MBR" |
| An append is in-memory only | the store is re-opened from disk and the object retrieved again | `reopened` marker missing |
| A payload is returned unverified | `get` re-hashes; the probe compares bytes, not lengths | `seeded object verified` missing |
| The probe outgrows its heap | the footprint must stay under the bound | "leaving no headroom" |
| Both instances run and race | exactly one `store plane complete` | "N instances ran the scenario" |
| The root writes to a disk again | the device gate's byte-identical assertion | "the root modified the disk during bring-up" |
| Two valid but disagreeing GPT copies are silently resolved | the `gpt-conflict` fixture must report `conflicting-copies` | "expected refusal not observed" |
| A store with no valid root invents one | the `superblock-both-damaged` fixture must report `no-valid-superblock` | "expected refusal not observed" |
| An uncommitted record is indexed | the `interrupted-append` fixture must run the happy scenario unchanged | "index counted an uncommitted record" |
| A panic stands in for a refusal | each refusal is checked for cleanliness | "the refusal was not clean" |
| A rejected disk is written to | both refusal fixtures are hashed before and after | "wrote to a disk it had rejected" |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 15 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_store_check` | Pass; 15 markers over five fixtures, image checks | Direct |
| `just sel4_device_check` | Pass; the disk is byte-identical after bring-up | Direct |
| `just sel4_storage_check` | Pass; the block path underneath still holds | Direct |
| `just sel4_gate_control_check` | Pass; 17 gates reject 773 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 14 plane layouts match their fixtures | Direct |
| The other fourteen seL4 plane gates | Pass | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| `just test_host` | Fails on this host: `x86_64-unknown-linux-gnu` target not installed. Reproduced identically on a clean tree | Direct, pre-existing |
| M5.6 rollback, M5.9 recovery | Not ported | — |

## Decisions

- **Decision:** Leave `StoreTransact` unmediated, permanently.
  **Rationale:** the operation names policy. Mediating it would put partition
  selection, root choice, allocation, and commit ordering back in the root,
  which is the placement this port exists to remove. The unmediated surface
  stays at eight operations because eight of them are userspace's.
  **Consequence:** the seL4 port has no store *syscall*. A component that wants
  objects links the store and drives sectors, which is strictly more authority-
  honest — the capability it holds says `blockRead`, not "the store".

- **Decision:** Reuse `boot_contracts::{gpt, object_store}` rather than write a
  store for the port.
  **Rationale:** it is the oracle's own implementation, already host-tested and
  Miri-clean, and it reads through a three-method `BlockIo` trait rather than a
  device handle. Satisfying that trait from userspace over a mediated capability
  is the entire port.

- **Decision:** Delete the bring-up write instead of relocating it.
  **Rationale:** any fixed sector is wrong for some disk. The property it proved
  is better proved from userspace where the fixture defines the scratch sector,
  and the gate that lost the marker gained a stronger check.

- **Decision:** One shared heap in the runtime, not per-binary.
  **Rationale:** forced by `extern crate alloc` making the allocator symbol
  crate-wide. Documented in `heap.rs` so the next author does not re-derive it.

- **Decision:** Two fixtures in one gate.
  **Rationale:** the happy path cannot distinguish a store that validates
  superblock CRCs from one that ignores them. M5.4's redundancy requirement is
  only observable when a copy is damaged.

## Open risks and follow-ups

- [ ] M5.6 (rollback) and M5.9 (recovery) are not ported. Both need BootState
      persistence above this store, which is the rest of P5.4.2.
- [ ] The append path writes one sector per `BlockTransact`, so a 32 KiB object
      is 64 round trips. Correct, and slow; a multi-sector request would need
      the transfer window to carry more than one sector.
- [ ] The probe holds both block rights, so "a store operation the rights do not
      cover is refused" is not asserted here. A read-only store client would
      close it.

## Artifacts and provenance

- Gate output, both scenarios, and the image comparisons:
  [`store-check.txt`](store-check.txt).
- The block path this sits on:
  [`devlog/2026-08-08-p5-4-2c-storage-plane/`](../2026-08-08-p5-4-2c-storage-plane/index.md).
- The driver under that:
  [`devlog/2026-08-08-p5-4-2b-virtio-blk/`](../2026-08-08-p5-4-2b-virtio-blk/index.md).
- The store's host tests, written when it moved to `boot-contracts`:
  [`devlog/2026-08-07-p5-4-2-object-store/`](../2026-08-07-p5-4-2-object-store/index.md).
- GPT validation's host tests:
  [`devlog/2026-08-07-p5-4-2-gpt-validation/`](../2026-08-07-p5-4-2-gpt-validation/index.md).
- Related roadmap item: P5.4.2 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
