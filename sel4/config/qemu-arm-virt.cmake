set(KernelPlatform "qemu-arm-virt" CACHE STRING "")
set(KernelSel4Arch "aarch64" CACHE STRING "")
set(KernelArmHypervisorSupport ON CACHE BOOL "")
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
