# Novatek NT98690 (NS02201) H1V1 seL4 platform, P6's physical target.
#
# The platform is `src/plat/ns02201` in the pinned seL4 fork, whose values
# were read off the named board by the P6.A probe. The settings below mirror
# `qemu-arm-virt.cmake` so the two AArch64 platforms differ only in the board
# they describe: one node, no MCS, kernel printing on, and PL0 access to the
# physical counter/timer that `slime-root/src/platform_timer.rs` programs from
# EL0. Printing is the root's only output path on this board -- the kernel's
# 16550 driver on UART0 -- so `KernelPrinting` is load-bearing evidence, not a
# convenience.
#
# Hypervisor support is not optional here: firmware hands over at EL2 (observed
# `CurrentEL` 2), and Slime's kernel loader asserts EL2 before entering the
# kernel. The device tree is in-tree (`deps/sel4/tools/dts/ns02201-h1v1.dts`
# plus `src/plat/ns02201/overlay-ns02201-h1v1.dts`), so no `-DQEMU_DTB` is
# passed; the overlay's memory node at 0x10000000 is what places the kernel.
set(KernelPlatform "ns02201-h1v1" CACHE STRING "")
set(KernelSel4Arch "aarch64" CACHE STRING "")
set(KernelArmHypervisorSupport ON CACHE BOOL "")
set(KernelIsMCS OFF CACHE BOOL "")
set(KernelMaxNumNodes 1 CACHE STRING "")
set(KernelVerificationBuild OFF CACHE BOOL "")
set(KernelDebugBuild ON CACHE BOOL "")
set(KernelPrinting ON CACHE BOOL "")
set(KernelArmExportPCNTUser ON CACHE BOOL "")
set(KernelArmExportPTMRUser ON CACHE BOOL "")
