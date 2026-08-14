set(KernelPlatform "qemu-arm-virt" CACHE STRING "")
set(KernelSel4Arch "aarch64" CACHE STRING "")
set(KernelArmHypervisorSupport ON CACHE BOOL "")
# B48's MCS half, deferred with the reason recorded rather than left blank.
#
# MCS replaces seL4's priority-only scheduler with scheduling contexts,
# budgets, periods, and timeout faults — which is exactly what a generation
# would need to bound CPU the way it already bounds memory and capabilities.
# It is off because this repository's whole claim is upstream seL4 with its
# assurance intact, and the functional-correctness proofs do not cover the MCS
# configuration on AArch64. Turning it on would trade a verified kernel for a
# scheduling feature, silently, in a config file.
#
# What that costs is stated rather than hidden: without MCS the kernel has no
# notion of budget or period, so `ScheduleRecord`'s `budget_us` and
# `period_us` are written zero and a generation cannot declare them. Priority
# *is* enforced and is declared data — see `Instance.priority` and
# `SLIME_GRAPH schedule` — so the "one maximal child priority" fallback B48
# names is gone either way.
#
# Revisiting this means an assurance decision, not a config edit: either the
# proofs extend to MCS on this platform, or the project accepts an unverified
# kernel for a stated reason.
set(KernelIsMCS OFF CACHE BOOL "")
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
