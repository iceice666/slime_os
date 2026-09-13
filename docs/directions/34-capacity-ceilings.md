# 34. Root capacity ceilings: memory, threads, and cores

| | |
| --- | --- |
| Status | Memory portion routed to the [capacity epic](../../.tasks/items/01a07a2c-9c4f-7ca0-8417-aba2b48ce16b.md); thread/core portions remain parked |
| Route | capacity |
| Depends on | [Core C10](../../roadmap/02-core-runtime.md) private-memory mechanism (complete) and C7 shared buffers (complete) for the ceilings themselves; **B47**'s process/thread separation (resolved) for the thread half; nothing for the large-page half. The SMP half depends on an assurance decision, not on a milestone. |
| Enables | workloads whose working set or concurrency exceeds a single 2 MiB window — a foreign-workload guest, an inference component, a multi-queue driver — without which those directions cannot be sized at all |
| Now | Paper, plus one implementation half that is legal today: `slime-root` allocates only 4 KiB granules while both architectures expose 2 MiB and 1 GiB frame objects. Large-page support is pure mechanism and changes no contract. |

The memory portion now has [architectural sequencing](../../roadmap/02-core-runtime.md#memory-capacity)
and canonical work items, recorded in the [planning decision](../../devlog/2026-09-07-memory-capacity-milestones/index.md).
The sketches below are design background, not implementation status or a
second tracker. Their memory open questions are resolved for this plan by
exact 4 KiB accounting, target-qualified ceilings, scalable task backing, and
unchanged shared-buffer authority; the thread/core questions remain separate.

## Motivation

Three ceilings decide what Slime can host, and all three are lower than
the hardware they run on:

| Ceiling | Value | Where |
| --- | --- | --- |
| Per-task private window | 512 pages (2 MiB) | `slime-root/src/private_memory.rs:65` |
| All live private regions | 2048 pages (8 MiB) | `slime-root/src/private_memory.rs:79` |
| All live shared buffers | 256 pages (1 MiB) | `slime-root/src/shared_buffer.rs:47` |
| Threads per component | 2 | `contracts/component-runtime-abi/v1/schema.zt:24` |
| Cores | 1 | `KernelMaxNumNodes` in all four `sel4/config/*.cmake` |

The private ceiling is a small fraction of the target memory envelopes below.
These are not measurements of free allocatable RAM after boot reservations:

| Target | Memory envelope | Live private ceiling | Share of envelope |
| --- | --- | --- | --- |
| `qemu-arm-virt` | 1024 MiB kernel DTB / 2048 MiB launcher | 8 MiB | 0.78% of kernel DTB |
| `qemu-riscv-virt` | 3072 MiB | 8 MiB | 0.26% |
| `bcm2712-rpi5` | 1019 MiB | 8 MiB | 0.8% |
| `cv1800b-duo` | 63 MiB | 8 MiB | 13% |

The ARM row distinguishes the kernel-build description from the launcher:
`scripts/build/build-sel4.py::QEMU_DTB_PARAMETERS` builds the ARM kernel
DTB with 1024 MiB while the product launches with 2048 MiB. The platform
milestone qualifies the added ordinary-untyped range; neither number alone
states usable memory after kernel/root reservations.

Duo is the only target where 8 MiB is a defensible fraction, and
`private_memory.rs:58` says exactly that — the value is chosen to be
"defensible on a smaller target". That reasoning is sound and should not
be discarded; what it argues for is a *per-target* ceiling, which the
current single constant cannot express.

The entry exists because several registered directions are unsizable
until these move. A foreign-workload guest, a local inference component,
and a multi-queue accelerator driver each need one to two orders of
magnitude more than 8 MiB, and each would otherwise rediscover the same
coupling below from its own dead end.

## What exists today

### The ceilings are downstream of a root CSlot budget, not of caution

```
MAX_TASK_SLOTS = MAX_CHILD_IMAGE_PAGES + MAX_REGION_PAGES + 16
               = 512 + 512 + 16 = 1040          object_allocator.rs:38
```

Every page of a task's reservation costs one root CSlot, because the root
holds one frame capability per page. The root CNode holds
`2^CONFIG_ROOT_CNODE_SIZE_BITS` slots; `deps/sel4/config.cmake:103`
defaults that to 12, or 4096, and none of the four Slime configs
overrides it. So a naive raise of `MAX_REGION_PAGES` to 65536 (256 MiB)
would demand 66064 slots per task against a 4096-slot CNode — sixteen
times over, for one task.

This is not a projection. The repository has hit the wall twice and kept
both measurements:

- `object_allocator.rs:82` — `PlanExceedsRootSlots { required: 2313, available: 2185 }`
- `boot_selector.rs:44` — `PlanExceedsRootSlots { required: 1368, available: 1188 }`,
  a generation every non-selector image admits.

### `.bss` in the root is capacity, not memory

The seL4 loader creates one root CSlot per page of the root image's
`.bss` before the root runs. A large static in `slime-root` therefore
spends boot capacity:

- `object_allocator.rs:79-81`: a `[usize; MAX_ROOT_CSLOTS]` array was
  2 MB of `.bss` and **512 root CSlots**; replacing it with an
  open-addressed table cost 16 KiB and 4 slots.
- `boot_selector.rs:34-48`: an 8 MiB generation buffer spent ~2048 slots.
  Measured directly — 1188 free slots on the selector image against 3017
  on the demo image from the same sources. Halving the buffer to 4 MiB
  restored ~1024.

This makes the reverse coupling load-bearing for the shared-buffer half:
`MAX_FRAME_ANCHORS = MAX_TOTAL_PAGES` (`shared_buffer.rs:58`) feeds
`MAX_PHYSICAL_PROVENANCE` (`object_allocator.rs:94`), which is rounded up
to a power of two and lives in `.bss`. Raising the shared ceiling from
256 to 16384 pages grows that table from 16 KiB to ~512 KiB and takes
back ~128 root CSlots.

### Both architectures already expose large frames; the root uses none

| | AArch64 | RISC-V |
| --- | --- | --- |
| 4 KiB | `SmallPage` / `_4kPage` | `_4kPage` |
| 2 MiB | `LargePage` | `MegaPage` |
| 1 GiB | `HugePage` | `GigaPage` (PT_LEVELS 3/4) |

Sources: `deps/rust-sel4/crates/sel4/src/arch/arm/{object,vspace}.rs`,
`.../arch/riscv/{object,vspace}.rs`.

`slime-root` allocates `sel4::FrameObjectType::GRANULE` exclusively;
a search for `LargePage` or `MegaPage` under `slime-root/src` returns
nothing. The private window is already aligned to its own 2 MiB span
(`child_vspace.rs:751`, asserted by
`the_private_window_is_span_aligned…` at `child_vspace.rs:927`), so the
alignment precondition for a 2 MiB block mapping is met today.

### The thread ceiling is a macro's shape, not a design bound

`maxThreads = 2` because `entry!`'s worker form emits exactly one set of
symbols (`components/runtime/src/lib.rs:102-126`):

```
__slime_rt_worker_stack
__slime_rt_worker_entrypoint
__slime_rt_worker_anchor
```

and the root resolves them by two fixed strings
(`child_vspace.rs:227-230`). `child_vspace.rs:241-244` states the
coupling plainly: "the runtime declares one worker stack and one worker
entry point … Raising it means raising both."

Everything around it is already general:

- `extraThreads?` / `workerClass?` in the manifest carry no ceiling
  (`contracts/generation-manifest/v1/schema.zt:120-135`).
- The ABI validator requires only `abi.maxThreads > 0`
  (`contracts/component-runtime-abi/v1/gen_bindings.zt:149`) — no upper bound.
- The transfer descriptor's thread index occupies the high 32 bits
  (`descriptorThreadShift = 32`).
- `contracts/generation/v5/schema.zt:46` already admits `maxThreads = 48`
  TCBs across a generation.
- `PrivateHeap` is already atomic-locked and its comment names the
  multi-thread case as reachable (`private_heap.rs:365-367`).
- `WINDOW_BASE`/`WINDOW_LEN` are `[_; MAX_THREADS]` and scale with the
  constant (`syscall/sel4_transport.rs:431-432`).

Per-thread cost is two granules (IPC buffer + transfer window) plus a TCB.

### Single core is a config value with an assurance price

All four configs and `AARCH64_verified_include.cmake` set
`KernelMaxNumNodes 1`. `deps/sel4/CAVEATS.md`:

- plain SMP: **not formally verified**;
- SMP + hypervisor extensions: supported and "generally stable",
  **not verified**;
- SMP + MCS + hypervisor: AArch64 only, `gcc` only, `odroidc4`/`tx1`/`tx2`,
  "less tested with lower code coverage".

`rust-sel4` gates `tcb_set_affinity` on
`all(not(KERNEL_MCS), not(MAX_NUM_NODES = "1"))`
(`crates/sel4/src/invocations.rs:285`) — so placement is a TCB invocation
in the non-MCS build and moves to the scheduling context under MCS. The
two assurance decisions are therefore not independent: choosing SMP and
choosing MCS together changes *which* API places a thread.

`sel4/config/qemu-arm-virt.cmake:35-37` already records the shape of this
class of decision for MCS — "either the proofs extend … or the project
accepts an unverified kernel for a stated reason" — and
`bcm2712-rpi5.cmake:21-26` records one such departure taken deliberately
and on the record. SMP would be a second, and it is load-bearing on RPi5
in a way it is not on the QEMU planes, because that config includes
upstream's own verified file.

## Design sketch

Five separable sketches. The memory work items refine (a)-(c), including
the task-arena and small-growth costs that large-frame arithmetic alone misses.

### (a) Large-page allocation in the root — no contract changes

Teach `object_allocator` to retype `LargePage`/`MegaPage` and
`private_memory` to back a window with large blocks plus a granule tail.
The slot arithmetic is the whole point:

$$\frac{256\ \text{MiB}}{2\ \text{MiB}} = 128 \text{ slots} \quad\text{versus}\quad \frac{256\ \text{MiB}}{4\ \text{KiB}} = 65536 \text{ slots}$$

a 512-fold reduction, which is what turns a raised ceiling from
impossible into ordinary. [INFERENCE: a 2 MiB block also fills an
AArch64 L2 entry directly and should remove the leaf table for that
span, but this repository has never mapped one, so treat the table
saving as unverified until measured.]

Costs to state rather than hide: allocation granularity coarsens, so a
component declaring 3 MiB rounds to 4 MiB unless the tail stays 4 KiB;
the transactional grow path must handle mixed-granularity regions and return
objects and alignment capacity on failed growth. There is no live shrink API:
successful growth is reclaimed with the task, not returned piecemeal.

This piece changes no `.zt` file, no generated binding, and no builder
check. It is a mechanism slice with existing host tests to extend.

### (b) Raising the ceilings — a contract change, deliberately

`MAX_REGION_PAGES` and `MAX_TOTAL_PAGES` are pinned to
`contracts/private-memory-budget/v1` by two compile-time asserts
(`private_memory.rs:87-94`). That pin is correct and should stay: the
contract's own comment explains that drift would make the builder reject
budgets the root honours, or admit ones it does not, surfacing as a
runtime refusal against a quota the generation promised.

So a raise moves, in one change:

```
contracts/private-memory-budget/v1/schema.zt  regionPages, totalPages
  → scripts/generate/…                        regenerate boot-contracts/src/generated/
  → slime-root/src/private_memory.rs          the two constants and their asserts
  → scripts/build/build-generation.py         over-declaration refusal
  → docs/capability-matrix.md                 the Bounds table (roadmap invariant 4)
```

`MAX_TASK_SLOTS` follows automatically. The open question is whether the
ceiling should stay one constant: Duo at 63 MiB and `qemu-riscv-virt` at
3072 MiB do not want the same number, and `private_memory.rs:59-62`
argues for one constant precisely because it is coupled to the arena and
slot bounds. A per-target-profile ceiling is expressible —
`contracts/target-profile/v1` already distinguishes the four profiles —
but it makes the arena plan target-dependent, which is a real change to
how a generation is sized.

### (c) `KernelRootCNodeSizeBits` as the independent safety margin

`deps/sel4/config.cmake:96-104` permits 7–26 on 64-bit and defaults to
12; the verified configs use 19. None of the Slime configs sets it.
Raising it is one line per config plus a re-recorded
`kernel_config_sha256` in `sel4/pins.toml`, guarded by `sel4_pin_check`.
It buys headroom for (b) without touching any contract, and it is
orthogonal to (a) — but it is not a substitute for (a), because slots
are not free: they are backed by the rootserver allocation
(`deps/sel4/src/kernel/boot.c:178`), which on a 63 MiB Duo is not
negligible.

### (d) N worker threads

Emit N sets of worker symbols from `entry!` (or one indexed table the
root resolves once), raise `maxThreads` in
`contracts/component-runtime-abi/v1/schema.zt`, and let the existing
arrays scale. Independent of (a)–(c).

What it buys on one core is overlap of blocking IPC, not throughput.
`components/testkit/scheduling-class-probe/src/main.rs:19-23` already
records why: under strict priority on one vCPU, a spinning or yielding
thread runs to completion before a lower band is scheduled once, and
`yield_now` only re-schedules within a band.

### (e) SMP, when a workload justifies the assurance cost

The register cannot decide this; it can fix what the decision must
contain. Whoever raises `KernelMaxNumNodes` must state, in the config
file beside the option in the manner `qemu-arm-virt.cmake` already uses
for MCS: which target, what the verified-set departure is on that
target, whether MCS is being taken in the same change (because it moves
affinity from `tcb_set_affinity` to the scheduling context), and what
the root's fixed tables assume about single-core exclusion.

That last item is the one this entry cannot answer from the outside.
`slime-root`'s tables are `[Option<T>; N]` arrays mutated by a
single-threaded root; SMP does not by itself make the root
multi-threaded, but it does make *children* concurrent, so every
root-mediated invariant that currently holds because only one child runs
at a time needs re-reading. That audit is the real cost of (e), not the
config line.

## Open questions

- Resolved for the memory plan: use target-qualified ceilings, keeping Duo's
  conservative budget separate from the QEMU profiles. The arena plan and
  all spec/admission validators must consume the same target-bound budget.
- Resolved for the memory plan: retain 4 KiB small heads/tails and exact quota
  accounting; never round a caller's committed pages up to an uncharged block.
- Shared-buffer ceilings stay unchanged in the memory plan. Large private
  working sets remain non-transferable; moving them through a loan would be
  a separate authority change, not an allocator shortcut.
- Is `MAX_TASKS = 48` (`task.rs:52`) reached before the memory ceilings in
  any realistic composition, or does it stay slack?
- For (e): which root invariants currently rely on children being
  mutually exclusive in time rather than on a lock or a capability?

## Exit-condition sketch

Not one exit condition — the pieces are separately observable:

- **(a)** A component declaring a multi-megabyte quota is backed by large
  frames, the plane boots, and the root's reported slot consumption for
  that task is the large-page figure rather than the granule figure.
- **(b)** A generation declaring a quota above today's ceiling is admitted,
  the component grows into it, and the frame allocator's watermarks return
  exactly on exit — the property C10.4 already observes, at the new size.
- **(d)** A component runs N > 2 threads, each with its own IPC buffer and
  transfer window, and a fault in one is attributed to that thread.
- **(e)** Deliberately unspecified: an SMP claim needs a named target, a
  recorded assurance departure, and a re-read of the root's concurrency
  assumptions, none of which this register can pre-decide.

## Probe guidance

Start with the large-frame work item's existing-ceiling probe and failure
unwind evidence. A 2 MiB runtime case can prove block backing without first
raising a contract; larger mixed extents can be planner cases until the
budget milestone qualifies them through the live path. Architecture-specific
mapping evidence and reusable capacity matter, not compilation alone.

Do not raise a ceiling first. Without (a), every page added to a
reservation costs a root CSlot, and the failure mode is
`PlanExceedsRootSlots` at boot — a message that names the plan, not the
constant that caused it.

The user-requested memory plan supplies a named capacity-qualification
workload; it does not claim an inference engine, foreign guest, or other
product workload. Work-item state and exit conditions now live only in the
linked store. No memory planning result promotes the thread or SMP sketches.
