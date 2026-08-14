# P5.4.2c (part) — a userspace component reaches the disk

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,ipc,graph,transfer_window}.rs`, `components/runtime/src/{lib,syscall}.rs`, `components/runtime/src/syscall/{sel4_transport,legacy}.rs`, `components/bins/src/bin/{sel4-storage-probe,init}.rs`, `contracts/generation/v1/fixtures/sel4-storage.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{storage-plane,component-graph,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.2, P5.4, M5.2, M5.3 |
| Gates | `just sel4_storage_check`, `just sel4_device_check`, `just sel4_component_graph_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.2b moved sectors from the root; nothing in userspace could reach the device |
| Baseline | `BlockTransact` answered `Mediation::Unavailable`; no component had ever touched a disk on seL4 |

## Summary

A userspace component now reads, writes, and flushes a real disk on seL4,
through nothing but a capability its generation granted it.
`just sel4_storage_check` asserts ten ordered markers, corroborates them against
the root's own mediation records, and re-reads the host image after the boot to
confirm the flushed write is durable.

`BlockTransact` moved from `Mediation::Unavailable` to `RootService`: the root
owns the device untyped and the DMA frames, so it owns the driver. Storage
*policy* — partitioning, the object store, generations, recovery — stays in
userspace and stays unmediated, which is why the unmediated surface is eight
operations rather than nine.

This is part of P5.4.2c. M5.2 and M5.3's transport and durability arms hold;
M5.4, M5.6, and M5.9 need the object store above this, which is the rest of the
slice.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `graph.rs` | `Resource::Block` | The device is a capability, and read/write are separate rights on it |
| `main.rs` | `serve_block_transact`: capability, decode, rights, one sector | A component reaches the device only through authority the generation placed |
| `main.rs` | `Resource::Block` placed by the factory loop at `Role::StorageCapability` | Declared device authority is installed where the layout says |
| `main.rs` | `construct_child` installs the child's own declared authority above the parent's grants | **A spawned child now receives what its generation declared it** |
| `transfer_window.rs` | `write_staged_region` | A reply larger than one message is bounded by the window, not by `MAX_MESSAGE_BYTES` |
| `syscall.rs` and both transports | `block_transact_sector`, `block_transact_write` | A sector crosses in the caller's own window; there is no ambient `buffer_phys` |
| `sel4-storage-probe.rs` | The plane's subject: three positive arms, three refusals | Every claim is about bytes off the device or authority refused |
| `sel4-storage.zti`, `SEL4_STORAGE_LAYOUT`, build wiring | Generation 23 and its plane | The gate boots the artifact it asserts about |

### The defect this slice found

`construct_child` installed the parent's grant list and stopped. A spawned child
never received the authority its own generation declared for it.

That was invisible for eleven planes because every declared non-channel
authority went to a component the root *launches* — a factory grant names its
holder, and the launched instance got it. The storage plane is the first
composition where the spawned instance is the subject: both copies of
`sel4-storage-probe` are declared the same block capability, and only the
launched one had it.

Fixed where it belongs, in `construct_child`, numbered from a cursor above the
parent's grants — the same rule `launch_component_graph` follows for a
non-bootstrap component's table.

### Two instances, one device capability

A generation grant names a *component*, not a task, so the root-launched copy
and the spawned copy both hold the block capability. The device cannot tell them
apart and neither can authority.

What distinguishes them is a run token `init` grants only to the instance it
spawns; the probe checks for it with the same `ERR_BAD_CAP` authority probe
`fabric-call-time` uses. The gate asserts the unconfigured copy parks, so a
regression that let both run — and race on the scratch sector — turns the gate
red rather than passing intermittently.

### Where the sector travels

The oracle's `sys_block_transact` reads a caller-supplied `buffer_phys` pointer
and dereferences it. There is no such ambient addressing here, so the sector
rides the caller's transfer window: behind the 64-byte record on a write, behind
the reply record on a read. `buffer_phys` is set to zero by the seL4 probe
deliberately — a root that honoured it would be reintroducing exactly what the
capability model removes.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A component reaches the device without a capability | the `ungranted slot refused` arm | marker missing |
| A malformed request is interpreted | the `malformed refused` arm | marker missing |
| A request past capacity is served | the `out-of-range refused` arm | marker missing |
| The read fabricates data | the fixture's signature is required back | `sector 0 verified` missing |
| A flush is acknowledged but not honoured | the host-side image check after the boot | "sector 1 holds … expected" |
| Both instances run and race | exactly one `storage plane complete` | "N instances ran the scenario" |
| The component's claims are only self-reported | the root's `block served` records are required to show read, write, and flush | "the root served no successful …" |
| A spawned child stops receiving declared authority | `declared placed … kind=block` | marker missing |
| `BlockTransact` silently reverts to unmediated | `sel4_component_graph_check` pins the eight-operation surface | the surface check fails |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 10 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_storage_check` | Pass; 10 markers, three refusals, durable write | Direct |
| `just sel4_device_check` | Pass; the root's own driver arms still hold | Direct |
| `just sel4_component_graph_check` | Pass; unmediated surface is eight operations | Direct |
| `just sel4_gate_control_check` | Pass; 16 gates reject 744 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 13 plane layouts match their fixtures | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| The other twelve seL4 plane gates | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| M5.2/M5.3's remaining arms — descriptor recovery, reset, stale completions, interrupted flush | Not covered on seL4 | — |

## Decisions

- **Decision:** Keep the driver in the root and mediate, rather than granting a
  component the MMIO frame.
  **Rationale:** the device untyped and the DMA frames are the root's, and
  handing a component raw MMIO would give it authority over every register in
  the granule — including transports it was never granted. Mediation is what
  makes `blockRead` narrower than "the device".
  **Rejected alternative:** a driver component holding the frame, which is the
  right destination once there is a resource model for granting one and is not
  this slice.

- **Decision:** A separate `sel4-storage-probe` rather than a branch in the
  oracle's `storage-probe`.
  **Rationale:** the oracle reads through `buffer_phys`; the seL4 payload
  crosses in the transfer window. The two do not share a body. What they do
  share is the wire contract — the same `contracts/block/v1` records, the same
  generated bindings.

- **Decision:** Fix `construct_child` rather than granting the device from init.
  **Rationale:** init routing the capability would have worked and would have
  hidden a real defect: every spawned child was silently missing its declared
  authority. The plane exists to exercise the capability model, not to work
  around it.

- **Decision:** `write_staged_region` rather than raising `MAX_STAGED_BYTES`.
  **Rationale:** that constant is the *message* bound, and channels depend on
  it. A sector is not a message — the same distinction P5.4.8 made for a
  diagnostic line.

## Open risks and follow-ups

- [ ] M5.2 and M5.3 are not closed. Their remaining arms — descriptor and pin
      recovery, device reset, stale completions not reused, an interrupted flush
      never appearing durable — need fault injection the driver does not have.
      The `contracts/block/v1` schema already declares the flags for it
      (`FLAG_INJECT_RESET` and friends); nothing honours them yet.
- [ ] `StoreTransact` is still unmediated. M5.4, M5.6, and M5.9 need the object
      store above this block path, which is the rest of P5.4.2c.
- [ ] One sector per request. `sectors_done` can express a partial completion
      and nothing produces one; a multi-sector request is refused rather than
      truncated.
- [ ] The probe's block capability carries both rights, so "a request the rights
      do not cover is refused" is asserted only for a slot holding *no* device.
      A second component granted `blockRead` alone would close it.

## Artifacts and provenance

- Gate output, the root's mediation records, and the host-side durability check:
  [`storage-check.txt`](storage-check.txt).
- The driver this sits on:
  [`devlog/2026-08-08-p5-4-2b-virtio-blk/`](../2026-08-08-p5-4-2b-virtio-blk/index.md).
- The substrate under that:
  [`devlog/2026-08-08-p5-4-2a-device-substrate/`](../2026-08-08-p5-4-2a-device-substrate/index.md).
- Related roadmap item: P5.4.2 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
