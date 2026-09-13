# Investigation: `memory::vmm::leaf_flags_in` page-fault hang in stage-0 boot checks

**Date:** 2026-07-24
**Symptom as reported:** page-fault hang attributed to `memory::vmm::leaf_flags_in` in
`bootstate_trace_check`, `recovery_check`, `transfer_check`, `directory_check`.
**Verdict:** `leaf_flags_in` is an innocent bystander. The root cause is a
**kernel stack overflow into the stage-0-built PML4**, which sits exactly one
page below the 64 KB boot stack. Overwritten page-table entries then detonate
wherever the next page walk or instruction-fetch TLB miss happens — sometimes
inside `leaf_flags_in`, sometimes as a silent triple-fault reboot loop.

## Reproduction

`just bootstate_trace_check` reproduces 100% on this machine. The check never
returns because `scripts/check-bootstate-trace.py` runs QEMU through
`run-kernel.sh` with **no subprocess timeout**, and the guest is stuck in an
infinite reboot loop:

```
[stage0] generation and kernel verified     ← stage-0 hands off
[serial] Hello World! … [serial] heap online
(silence — no [acpi], no [kernel fault], no panic)
… ~4–5 min later …
[stage0] immutable selector                  ← the machine RESET and rebooted
```

Serial shows repeated stage-0 banners: not a hang, a **triple-fault reboot
loop**. Each cycle takes minutes because the scripts boot the **debug** kernel
under TCG (see "Contributing factors").

## Evidence chain

All captured live from `qemu -s` + lldb and `qemu -d int,cpu_reset`, booting
the `bootstate_trace_check` image with the debug kernel at commit `8a73ff1`.

### 1. The fatal exception (QEMU `-d int` log, identical in every cycle)

```
114: v=0e e=0010 cpl=0 IP=0008:ffffffff9002bd50 SP=0010:000000000e018bb0 CR2=ffffffff9002bd50
check_exception old: 0xe new 0xe
115: v=08 …                                   ← #PF while delivering #PF
check_exception old: 0x8 new 0xe              ← #PF while delivering #DF → triple fault
RBP=000000000e0204c8  RSP=000000000e018bb0  CR3=000000000e01b000
```

- `CR2 == RIP`, error code `0x10`: an **instruction-fetch** page fault on the
  kernel's own code (`0xffffffff9002bd50` ≈ `spin::Once::call_once` closure in
  `acpi::init`, first fetch after a TLB miss).
- Both the #PF and #DF handlers could not be delivered either (their code/IDT
  live under PML4[511], which is destroyed) → triple fault.
- **The smoking geometry:** `CR3 = 0x0e01b000` (the PML4 frame) lies *between*
  `RSP = 0x0e018bb0` and `RBP = 0x0e0204c8` — the active call stack spans the
  page-table root.

### 2. Layout: stage-0 places the PML4 one page below the kernel stack

`stage0/src/main.rs`:

```rust
let stack = allocate_zeroed(KERNEL_STACK_BYTES /* 64 KiB */, LOADER_DATA)?;   // line ~165
let stack_top = stack.add(KERNEL_STACK_BYTES);
let mut tables = PageTables::new()?;   // PML4 allocated immediately after
```

OVMF's `allocate_pages(AnyPages)` allocates top-down, so the very next
allocation lands directly below the previous one:

```
0xe02c000 ┬───────────────────────── stack_top  (kernel RSP starts here)
          │  64 KiB kernel boot stack
0xe01c000 ┼───────────────────────── stack base (no guard!)
0xe01b000 │  PML4  ← CR3             (PageTables::new())
0xe018000 │  handoff memory-map array …
```

The identity map and direct map cover all of this RAM as writable 2 MiB huge
pages, so stack growth below the base **writes silently into the PML4**.

### 3. The overwrite, caught live with a watchpoint

Attached lldb to the stub, armed a write watchpoint on the identity address of
`PML4[511]` (`0x0e01bff8`):

```
Watchpoint 1 hit (write to 0x0e01bff8):
  frame #0: slime_os-kernel`memmove at crt.rs:56   ← kernel's own memmove
  rsp = 0x000000000e018a68                          ← ~13.5 KB below stack base
```

The kernel's `memmove` is filling a large **stack-allocated buffer that
straddles the PML4 frame** (deep `boot-contracts` sha256/generation frames in
the handoff path: `init_from_handoff → find_recovery_index →
Generation::decode → generation_identity → sha256`, each carrying big locals
and, in debug, per-byte UB-checked copies). Total stack use exceeds
64 KiB by ~14 KiB, i.e. the entire PML4 page plus part of the region below is
overwritten with stack data.

### 4. Why it looks like a `leaf_flags_in` page fault

The PML4's *high* entries are destroyed first (entry 511 sits at frame offset
`0xff8`, right below the stack base — the kernel half of the address space):

| overflow depth | PML4 entries overwritten | consequence |
|---|---|---|
| 8 B    | 511 (kernel image, IDT, handlers) | next code-fetch TLB miss → unservable #PF → triple fault (this repro) |
| ≥0x200 | 448 (kernel heap) …511 | heap accesses fault |
| ≥0x400 | **384 (PCI ECAM scratch VA)** …511 | see below |
| ≥0x800 | 256 (HHDM/direct map) …511 | everything faults |

With a *shallow or transient* overflow (release builds have far smaller
frames; an interrupt burst near the stack base writes a few dozen qwords),
entry 384 receives stack garbage such as a return address
(`0xffffffff90xxxxxx`). If bit 0 (PRESENT) happens to be set,
`pci::map_config_page → vmm::leaf_flags_in` treats it as a page-table pointer:

```rust
// vmm.rs — no validation of entry contents:
table = unsafe { PageTable::at(PhysAddr(entry & ADDR_MASK)) };
```

`0xffffffff90xxxxxx & ADDR_MASK = 0x000f_ffff_90xx_x000` ≈ a 4 PB "physical"
address; `to_virt()` lands far outside the mapped direct map and the **read
inside `leaf_flags_in` page-faults**. Because the corruption is partial, the
still-TLB-warm handler prints `[kernel fault] vec=14 rip=…` (rip resolving
into `leaf_flags_in`) and parks in `hlt_loop` — the "page-fault hang" as
observed. Same root cause, milder trample.

So the fault RIP identifies the *first reader* of the corrupted PML4, not the
corruptor. `page_flags_in` / `translate_in` / `next_table` are equally exposed.

### 5. Ruling out the other suspects

- **PMM recycling live frames:** dumped the 97-entry handoff memory map from
  guest RAM — the stack, PML4, PDPT/PD/PT chain, kernel image, and handoff are
  all inside `RESERVED` ranges; only CONVENTIONAL RAM is on the free list. Not
  the cause.
- **Handoff ABI skew from `8a73ff1`:** layout unchanged (assertions only);
  handoff header verified sane in guest memory (`SLIMEHND`, v1, size 0x130).
- **Huge-page mis-walk corrupting tables:** the VAs the kernel maps
  (heap 448, ECAM 384, virtio/nvme MMIO 386/388) all start from empty PML4
  slots — the walkers never descend into stage-0's huge-page subtrees during
  a healthy boot. (The walkers' lack of PS-bit/validity checks is still a
  latent hazard — see hardening.)
- **Limine path:** unaffected (Limine supplies its own stack and table
  placement). `directory_check`'s QEMU boot (Limine + release) ran to
  `[directory-probe] done` here, and `check-transfer.py` **passed** end-to-end.
  The reliably-broken checks are the stage-0 image boots with the debug
  kernel; release stage-0 boots sit within margin on this machine but have
  zero guard — the user-observed `leaf_flags_in` faults in
  recovery/transfer are the shallow-trample variant of the same bug.

## Why now

The 64 KiB stack has always been unguarded and adjacent to the PML4; recent
`boot-contracts` work (`660f703`, `8a73ff1`: schema-generated
sha256/generation/recovery-index paths, now also run during
`init_from_handoff → find_recovery_index`) pushed debug-build stack depth past
the cliff, converting a latent hazard into a deterministic boot loop.

## Contributing factors (each worth fixing)

1. **No stack guard.** Overflow writes silently into whatever OVMF happened to
   allocate below — here, the live PML4.
2. **Silent placement coupling.** Correctness depends on undocumented
   UEFI-allocator adjacency (stack directly above PML4).
3. **Unvalidated page-walks.** `leaf_flags_in` / `page_flags_in` /
   `translate_in` / `next_table` dereference `entry & ADDR_MASK` with no
   PS-bit check and no bounds check against the direct-map extent, so
   corruption faults *inside the walker* and misdirects diagnosis.
4. **Scripts boot the debug kernel.** `check-bootstate-trace.py`,
   `check-rollback.py`, `check-release-trust.py` boot
   `target/x86_64-unknown-none/debug/slime_os-kernel` even though every
   Justfile target first runs `cargo build --release` — the binary being
   tested is stale relative to the build the Justfile just made, and debug
   stack frames are enormous.
5. **No QEMU timeouts.** The check scripts' `subprocess.run` has no `timeout`,
   so a wedged guest hangs the whole check forever ("faults are reported
   deterministically rather than silently hanging" is the project's own exit
   criterion — the harness should honor it too).

## Recommended fixes

Ordered by leverage:

1. **stage-0: give the kernel stack a guard.** Simplest robust form: allocate
   `KERNEL_STACK_BYTES + PAGE_SIZE`, and *do not map* the lowest page in the
   kernel page tables (skip it when building identity/direct maps, or map the
   stack at a dedicated virtual range with an unmapped guard page and pass
   that VA as `stack_top`). Overflow then faults deterministically at the
   guard instead of shredding state.
2. **Raise `KERNEL_STACK_BYTES`** (64 KiB → 256 KiB). Debug builds of the
   current handoff path need >78 KiB.
3. **Harden the VMM walkers.** In `leaf_flags_in`/`page_flags_in`/
   `translate_in`/`next_table`: treat a set PS bit (bit 7) at PDPT/PD level as
   "not a 4 KiB mapping" (return `None`/error rather than descending), and
   optionally bounds-check `entry & ADDR_MASK` against the direct-map limit.
   Turns silent corruption into a typed, attributable error.
4. **Scripts: boot what was built.** Point the stage-0 check scripts at the
   release kernel (or `cargo build` the debug profile explicitly), and add
   `timeout=` to every QEMU `subprocess.run` so a wedged boot fails loudly.
5. **Optional canary.** Stage-0 writes a sentinel qword at the stack base;
   kernel asserts it during bring-up milestones to catch depth regressions
   before they trample anything.

## Artifacts

- Exception log: `qemu -d int,cpu_reset` capture (3 identical fault cycles).
- Watchpoint capture: lldb write-watch on `0x0e01bff8` firing from
  `memmove (crt.rs:56)` with `rsp = 0xe018a68`.
- Guest-memory dumps: handoff header, 97-entry memory map, healthy page-table
  chain for `0xffffffff9002bd50`
  (PML4 `0xe01b000` → PDPT `0xb8dd000` → PD `0xb4d6000` → PT `0xb4d5000`).
