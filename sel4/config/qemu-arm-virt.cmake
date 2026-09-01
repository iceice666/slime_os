set(KernelPlatform "qemu-arm-virt" CACHE STRING "")
set(KernelSel4Arch "aarch64" CACHE STRING "")
set(KernelArmHypervisorSupport ON CACHE BOOL "")
# B48's MCS half, deferred with the reason recorded rather than left blank.
#
# MCS replaces seL4's priority-only scheduler with scheduling contexts,
# budgets, periods, and timeout faults — which is exactly what a generation
# would need to bound CPU the way it already bounds memory and capabilities.
# It is off because this repository's whole claim is upstream seL4 with its
# assurance intact. State the terms precisely, because they are narrower than
# "MCS is unverified": `deps/sel4/CAVEATS.md` lists AArch64 MCS functional
# correctness as *in progress* (RISC-V MCS is verified and shipped), and it
# lists MCS as foundation-supported and stable with small API changes expected.
# Note also what this very file already concedes: `KernelVerificationBuild OFF`
# plus `KernelDebugBuild`/`KernelPrinting ON` below, and `qemu-arm-virt` in no
# verified-platform list, put *this* build outside the verified set already.
# `sel4/config/bcm2712-rpi5.cmake` is the config where the claim is load-
# bearing, since it includes upstream's own `AARCH64_bcm2712_verified.cmake`.
# So MCS costs little here and a real proof-coverage gap there; the decision is
# per-target, and flipping it in this file alone would not be the same decision.
#
# What that costs is stated rather than hidden: without MCS the kernel has no
# notion of budget or period, so `ScheduleRecord`'s `budget_us` and `period_us`
# are written zero, and B77 made that an *admitted* rule rather than a builder
# habit -- both validators now refuse a nonzero value, so a generation from any
# producer genuinely cannot declare them. Flipping this option therefore means
# editing those two predicates in the same change (`check-generation.py`'s
# `UndeclarableCpuBudget` and `Generation::validate`'s `NonZeroReserved`), or
# every generation will be refused. That coupling is deliberate: it fails loudly
# at a named line instead of silently admitting a budget nothing charges.
# Priority *is* enforced and is declared data -- see `Instance.priority` and
# `SLIME_GRAPH schedule` -- so the "one maximal child priority" fallback B48
# names is gone either way.
#
# Revisiting this means an assurance decision, not a config edit: either the
# proofs extend to MCS on this platform, or the project accepts an unverified
# kernel for a stated reason.
set(KernelIsMCS OFF CACHE BOOL "")
# Single core, with the same deferral discipline as MCS above: the reason is
# recorded rather than left blank, because raising this is the second-largest
# assurance decision in this file.
#
# `deps/sel4/CAVEATS.md` states the terms. Plain SMP is not formally verified;
# SMP with hypervisor extensions -- which this file enables -- is supported
# and "generally stable" but likewise unverified; SMP with MCS and hypervisor
# extensions exists on AArch64 only, with `gcc` only, on `odroidc4`/`tx1`/`tx2`,
# and is described as less tested with lower code coverage. As with MCS, this
# build is already outside the verified set (`KernelVerificationBuild OFF`
# plus printing, and `qemu-arm-virt` on no verified-platform list), so the cost
# lands on `sel4/config/bcm2712-rpi5.cmake`, which includes upstream's own
# `AARCH64_bcm2712_verified.cmake`. The decision is per-target and flipping it
# here alone would not be the same decision.
#
# Two couplings a future change must carry, not discover:
#
#   * Thread placement moves with MCS. `rust-sel4` gates `tcb_set_affinity` on
#     `all(not(KERNEL_MCS), not(MAX_NUM_NODES = "1"))`
#     (`deps/rust-sel4/crates/sel4/src/invocations.rs:285`), so taking SMP and
#     MCS together changes *which* API places a thread. They are not
#     independent options.
#   * `slime-root`'s bounded tables are mutated by a single-threaded root, and
#     several of its invariants hold today because only one child runs at a
#     time. SMP does not make the root multi-threaded, but it does make
#     children concurrent, so every such invariant needs re-reading before the
#     first multi-core boot. That audit is the cost, not this line.
#
# Registered as `docs/directions/34-capacity-ceilings.md`, which also carries
# the private-memory and per-component thread ceilings this bound interacts
# with.
set(KernelMaxNumNodes 1 CACHE STRING "")
set(KernelVerificationBuild OFF CACHE BOOL "")
set(KernelDebugBuild ON CACHE BOOL "")
set(KernelPrinting ON CACHE BOOL "")
# `slime-root`'s timer phase (`slime-root/src/platform_timer.rs`) programs the
# EL1 physical timer (`CNTP_*`, PPI 30) directly from EL0: the only
# architected-timer PPI seL4 does not already claim for itself when built
# with `KernelArmHypervisorSupport ON` (see that file's module docs for the
# full analysis of `CNTHP_*`/`CNTV_*` being unavailable). These two options
# grant PL0 access to exactly the physical counter/frequency and physical
# timer control/compare registers that scheme reads and writes; the virtual
# counter/timer equivalents stay off because nothing here uses them.
set(KernelArmExportPCNTUser ON CACHE BOOL "")
set(KernelArmExportPTMRUser ON CACHE BOOL "")
