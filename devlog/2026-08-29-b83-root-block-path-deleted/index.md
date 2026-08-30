# B83 — the root's virtio-blk product path is deleted

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/virtio_blk.rs` (deleted) → `slime-root/src/boot_selector_block.rs`, `slime-root/src/{main,lib,console,device,directory,generation,ipc,object_allocator}.rs`, `slime-root/src/graph_runtime{.rs,/console_runtime.rs,/platform.rs,/services.rs}`, `components/runtime/src/{lib,syscall}.rs` and its seL4 transport, `contracts/component-runtime-abi/v1/schema.zt` with both generated outputs, three deleted `components/testkit/storage-*` crates and their `contracts/component-spec/v1/components/` records, `scripts/check/check-sel4-{device-plane,root-boot,gate-controls,component-graph}.py`, `scripts/check/check-component-spec.py`, `scripts/lib/component_spec.py` |
| Roadmap | B83, B84, IO2 |
| Gates | `just sel4_device_check`, `just sel4_root_boot_check`, `just io_block_check`, `just sel4_qemu_image_check`, `just sel4_boot_check`, `just sel4_storage_check`, `just sel4_store_check`, `just sel4_rollback_check`, `just replay_check`, `just sel4_generation_check`, `just sel4_filesystem_check`, `just sel4_recovery_plane_check`, `just sel4_transfer_check`, `just sel4_boot_selection_check`, `just sel4_component_graph_check`, `just sel4_gate_control_check`, `just component_spec_check`, `just contracts_check`, `just generation_check` |
| Trigger | B84 (resolved 2026-08-28) migrated the last two block-holding planes, leaving the root's virtio-blk command/descriptor implementation linked but unreachable from every seL4 composition. |
| Baseline | Before this entry `slime-root/src/virtio_blk.rs`, `console.rs::serve_block_transact`, the `ConsoleKind::BlockTransact` label, and the `block_transact*` runtime wrappers were still compiled into every product image, and the post-admission `probe_devices` call could still construct a root-owned block driver. |

## Summary

B83's residue is gone. The root's virtio-blk opcode and descriptor parser no
longer exists in any product image: it survives only as
`slime-root/src/boot_selector_block.rs`, compiled under `#[cfg(slime_boot_selector)]`
into the immutable selector, which is the acknowledged bounded ordering
exception — decoding a generation requires first reading it from the boot device,
so decoded policy cannot select its own prerequisite. Everything above it went
with it: `console.rs::serve_block_transact`, the `ConsoleKind::BlockTransact`
label and its console-ABI number, the three `block_transact*` runtime wrappers,
and the three frozen CP1 testkit crates that were their last callers. The
product root now has no code path that can reach a block device at all, which is
stronger than the runtime guard it replaces.

The cutover also removed the `SLIME_IO FAIL device ownership conflict` fatal.
That is not a dropped safety check: `BlockDevices` and every call site that
could populate it are now `#[cfg(slime_boot_selector)]`, while the `userspace_io`
condition was only ever true under `not(slime_boot_selector)`, so the two
ownership modes are mutually exclusive at compile time rather than at boot.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/virtio_blk.rs` | Deleted; recreated as `boot_selector_block.rs` under `#[cfg(slime_boot_selector)]`, with the non-selector bring-up read and IRQ-binding arms removed | No virtio opcode or descriptor parsing is linked into a product image |
| `slime-root/src/console.rs` | `serve_block_transact`, `ConsoleContext::devices`, and the `RIGHT_BLOCK_*` imports removed | The console thread serves console, input, and directory traffic only |
| `slime-root/src/ipc.rs` | `ConsoleKind::BlockTransact` and its label constant removed | No root endpoint label names a block operation |
| `contracts/component-runtime-abi/v1/schema.zt` | Console operations renumbered to WRITE 0, INPUT_READ 1, DIRECTORY_INSPECT 2, DIRECTORY_COMMIT 3; both generated outputs regenerated | The ABI's label space has no hole where a retired operation was |
| `slime-root/src/device.rs` | `MAX_BLOCK_DEVICES` split: `MAX_IO_DEVICES` bounds raw userspace device authority; the selector-only alias and `BlockDevices` are cfg-gated | The device bound the product path uses names transports, not root drivers |
| `slime-root/src/main.rs` | Post-admission `probe_devices` call and the ownership-conflict fatal removed; `BLOCK_*_PAGES` split into `AUTHORITY_MMIO_PAGES` and selector-only `BOOT_*_PAGES` | A non-selector image cannot construct a root block driver |
| `slime-root/src/object_allocator.rs` | `MAX_PHYSICAL_PROVENANCE` gains a cfg-selected `SELECTOR_PHYSICAL_PROVENANCE`: 448 entries in product images, 452 in the selector | The provenance bound is the sum of live declared ceilings, with no dead term |
| `components/runtime` | `block_transact`, `block_transact_sector`, `block_transact_write`, `transact_on`, and `exact_sector_reply_len` removed | The syscall surface exposes no retired transport |
| `components/testkit/storage-{probe,writer,fault-probe}` | Crates deleted; their component-spec records flipped to `provider = "undeclared"` | The corpus records the gap rather than naming a binary that does not exist |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The root reclaims a block path | `just sel4_device_check` | `SLIME_ROOT block ready `/`block read `/`virtio irq bound ` is a failure marker; the attached disk must stay byte-identical |
| A retired symbol returns to the tree | `just sel4_component_graph_check` | The forbidden-symbol scan names `block_transact*`, `serve_block_transact`, `BlockTransaction`, and asserts `virtio_blk.rs` is absent and `boot_selector_block.rs` present |
| A gate silently loses markers | `just sel4_gate_control_check` | Pins `sel4_device_plane` at 2 and `sel4_root_boot` at 56; 45 gates reject 1768 mutated transcripts |
| The selector's bootstrap reader breaks | `just sel4_boot_selection_check` | Fresh QEMU processes fail to consume attempts, roll back, or promote from the boot disk |
| A `sel4-`-prefixed crate hides behind a frozen record | `just component_spec_check` | The undeclared-provider guard checks both `name` and `sel4-<name>` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo check -p slime-root` under all three cfgs (none, `SLIME_ROOT_FIXTURE=1`, `SLIME_BOOT_SELECTOR=1`) | Clean, no warnings | Direct |
| `just sel4_device_check` | Pass — product root left the disk byte-identical, emitted no retired marker | Direct |
| `just sel4_root_boot_check` | Pass — ordered generation, timer, task, IPC, fault, ready markers | Direct |
| `just io_block_check` | Pass — oracle parity, async identity, faults, reclamation, stale epoch | Direct |
| `just sel4_boot_selection_check` | Pass — attempts persisted across fresh QEMU processes, exhaustion rolled back, health promoted, only BootState sectors changed | Direct |
| `just sel4_storage_check`, `sel4_store_check`, `sel4_rollback_check`, `replay_check`, `sel4_generation_check`, `sel4_filesystem_check`, `sel4_recovery_plane_check`, `sel4_transfer_check` | All eight migrated planes pass | Direct |
| `just sel4_gate_control_check` | Pass — 45 gates reject 1768 mutated transcripts and layouts | Direct |
| `just sel4_component_graph_check`, `just sel4_boot_check` | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass — byte-identical generation across two isolated builds | Direct |
| `just component_spec_check` | Pass — 57 records, 43 mutations refused | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |

## Decisions

- **Decision:** Keep `check-sel4-device-plane.py` as a 2-marker gate rather than
  retiring it.
  **Rationale:** its disk byte-identity assertion is a runtime property no
  static scan can produce. The forbidden-symbol scan proves the *source* holds
  no block path; byte-identity proves the *running product image* leaves an
  attached disk untouched. Those are different claims, and the second is the one
  a reintroduced driver would violate first.
  **Rejected alternative:** delete the script and fold byte-identity into
  `check-sel4-component-graph.py`. That gate boots without a disk attached, so
  it cannot make the assertion at all without gaining the same QEMU cost.

- **Decision:** `check-sel4-root-boot.py`'s catalogue expectation moves 5 → 6.
  **Rationale:** this corrects pre-existing drift, not this cutover. The default
  fixture variant resolves its manifest through `VARIANT_MANIFESTS.get(variant,
  "sel4")` to `contracts/generation-manifest/v1/compositions/sel4.zti`, which grew
  from five to six executables in commit `e4caf6c` (2026-08-27), "feat(sel4/product):
  keep Dango resident". That commit updated
  `check-sel4-component-graph.py` and left this gate stale.
  **Rejected alternative:** leaving it at 5 and reporting the failure as
  unrelated — the gate is in this cutover's own required set, so it must be green.

- **Decision:** the undeclared-provider guard keeps checking both `name` and
  `sel4-<name>`, with a single named exemption for `storage-probe`.
  **Rationale:** this corpus's convention is that a `sel4-`-prefixed binary *is*
  the implementation of the unprefixed identity — `filesystem-service` names
  `sel4-filesystem-service` and `generation-manager` names
  `sel4-generation-manager`, both on the same frozen `x86_64-qemu-virtio`
  profile. Dropping the `sel4-` arm repo-wide would let a shipping component sit
  outside the spec corpus. `storage-probe` is exempt on authority rather than
  naming: its record's `requires` is projected from `valid.zti`'s grants as
  `["block"]`, while `sel4-storage-probe` requires `["endpoint"]` and reaches its
  device over an IO0 ring, so the two identities need different capabilities.
  **Rejected alternative:** adding a `sel4-storage-probe.zti` record. It is not
  declared by the frozen reference generation, so the corpus would not project
  it onto any graph.

- **Decision:** `contracts/block/v1` and `components/proto/src/block.rs` are
  retained though this cutover removes their last consumer.
  **Rationale:** versioned contract directories coexisting is this repo's own
  pattern (`component/v1` beside `/v2`), `contracts/block/v1/schema.zt` is cited
  by `docs/concepts/contracts.md` as the canonical readable example, and
  `check-contracts.py` still proves its bindings are current. Deleting a
  versioned wire contract is a separate decision from deleting the code that
  spoke it.
  **Rejected alternative:** delete the contract, its generated module, and the
  `check-contracts.py` arm in this change — it would conflate a dead-code
  cutover with retiring a published format version.

## Open risks and follow-ups

- [ ] `contracts/generation-manifest/v1/fixtures/valid.zti` still declares
      `storage-probe`, `storage-writer`, and `storage-fault-probe` as executables
      with `sha256:` objects. They are inert — `build-generation.py` refuses that
      fixture's `x86_64-qemu-virtio` target profile outright, so no build ever
      resolves them — but the frozen fixture still names three identities with no
      implementation. Their removal belongs to the CP1 fixture's own migration.
- [ ] `scripts/build/boot_layout.py`'s `OVERRIDE_2`/`OVERRIDE_3` and the frozen
      `contracts/boot-layout/v1/fixtures/*.layout` files still carry
      `storage-writer`/`storage-fault-probe`/`storage-probe` layout rows, for the
      same frozen-fixture reason.
- [ ] `contracts/block/v1` now has no consumer, retained by the decision above.
      A future contract-retirement pass owns it.
- [ ] The selector's pre-admission bootstrap-device read path remains the one
      place the root parses virtio descriptors. It is bounded, single-request,
      and reachable only in `slime_boot_selector` images, and removing it would
      require the generation to be readable without reading the boot device.

## Artifacts and provenance

- Focused report: this entry; the diff is the commit named below.
- Raw transcript: gate output was read from the supervised runs listed under
  Verification; no transcript is frozen here because every claim is a `just`
  target the reader can re-run.
- Serial/debugger/model output: `just sel4_device_check` prints the byte-identity
  conclusion; `just sel4_gate_control_check` prints the 45-gate/1768-mutation count.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) B83
  (resolved), [`roadmap/11-io-substrate.md`](../../roadmap/11-io-substrate.md) IO2.
  Predecessors: [`devlog/2026-08-28-b83-userspace-block-cutover/`](../2026-08-28-b83-userspace-block-cutover/index.md)
  and [`devlog/2026-08-28-b84-two-device-driver-instances/`](../2026-08-28-b84-two-device-driver-instances/index.md).
