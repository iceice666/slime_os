include(${CMAKE_CURRENT_LIST_DIR}/../../deps/sel4/configs/AARCH64_bcm2712_verified.cmake)
# Raspberry Pi 5 (BCM2712) seL4 platform, P4's physical target.
#
# The `include` above must stay on line 1: `check-sel4-pins.py::check_profile`
# requires it there, so the overlay cannot be read without the pinned verified
# profile it narrows.
#
# The settings below mirror `qemu-arm-virt.cmake` so the two platforms differ
# only in the board they describe: one node, no MCS, kernel printing on, and
# PL0 access to the physical counter/timer that `slime-root/src/platform_timer.rs`
# programs directly from EL0.
#
# Unlike qemu-arm-virt, this platform's device tree is in-tree
# (`deps/sel4/tools/dts/rpi5b.dts` plus the overlays
# `src/plat/bcm2712/overlay-rpi5.dts` and `overlay-rpi5-2gb.dts`), so the build
# passes no `-DQEMU_DTB`: there is no emulator to extract a description from,
# and the board's memory map, GIC, timer, and console come from seL4's own
# platform description rather than any Slime-side board table.
set(KernelIsMCS OFF CACHE BOOL "")
set(KernelMaxNumNodes 1 CACHE STRING "")
# Leaving the verified configuration, deliberately and on the record.
#
# `AARCH64_bcm2712_verified.cmake` sets `KernelVerificationBuild ON`, which
# forces `PRINTING` and `DEBUG_BUILD` off. That kernel emits nothing on the
# UART and does not compile `sel4::debug_println`, so with it this board can
# produce no serial transcript — and a recorded serial transcript is exactly
# what P4's exit condition, RP3, and `contracts/rpi5-ros2-demo/v2`'s
# `serialPath`/`serialBaud` require as the board's evidence path. A verified
# kernel that cannot be observed cannot qualify the board.
#
# So this is a real cost, not a formality: the Pi 5 kernel is outside the
# verified set for the same three options `qemu-arm-virt.cmake` already turns
# off there. That file notes bcm2712 is "the config where the claim is load-
# bearing" — this is that claim being spent, for observability, with the
# consequence stated rather than discovered later. Do not read any P4/RP3
# evidence as evidence about a verified kernel.
#
# Reversing it means supplying evidence some other way: `slime-root` would need
# its own driver for the UART10 device the generation grants it, rather than
# relying on `seL4_DebugPutChar`. That is real work and does not exist today.
#
# `FORCE` is required on all three and is not decoration. The include above
# already cached `KernelVerificationBuild ON`, and seL4's `config.cmake`
# declares `KernelDebugBuild` and `KernelPrinting` with
# `DEPENDS "NOT KernelVerificationBuild" ... DEFAULT_DISABLED OFF`, so the
# include cached both as OFF too. A plain `set(... CACHE)` never overwrites an
# existing cache entry, so without `FORCE` each line silently does nothing and
# the kernel stays mute.
#
# All three must also move together. `DEBUG_BUILD` on with `PRINTING` off does
# not build: `include/machine/io.h` stubs `printf` to `((void)(0))` when
# printing is off, which strips the only caller of
# `src/arch/arm/64/c_traps.c`'s `vect_offset_to_name` — itself guarded by
# `CONFIG_DEBUG_BUILD` — and the kernel compiles with
# `-Werror=unused-function`. Observed, not predicted.
set(KernelVerificationBuild OFF CACHE BOOL "" FORCE)
set(KernelDebugBuild ON CACHE BOOL "" FORCE)
set(KernelPrinting ON CACHE BOOL "" FORCE)
set(KernelArmExportPCNTUser ON CACHE BOOL "")
set(KernelArmExportPTMRUser ON CACHE BOOL "")

# Memory: upstream's bcm2712 platform unconditionally appends
# `overlay-rpi5-2gb.dts`, which claims one range — physical 0 up to the
# VideoCore base at 0x3fc00000. seL4 then subtracts the firmware's ATF
# reservation (`reserved-memory/atf@0`, 0..0x80000) and reports a single usable
# window of 0x80000..0x3fc00000, i.e. 1019 MiB.
#
# That range exists on every Pi 5 model, so it is correct — not merely
# tolerated — on the 4 GiB board P4 qualifies; it is only conservative. It is
# deliberately not widened here. BCM2711 ships per-size overlays selected by
# `RPI4_MEMORY`, but BCM2712 ships none, and the high ranges cannot be borrowed
# from BCM2711: the Pi 5 moves its peripherals behind the RP1 southbridge and
# uses a different address map, so RPi4's `0x40000000 0xbc000000` and
# `0x1_00000000` ranges do not describe this SoC. The firmware-final device
# tree is the only authority for the ranges above VideoCore, and the tree on
# boot media carries a placeholder (`memory@0 = 0..0x28000000`) that firmware
# rewrites at hand-off, so the real high map must be read from a booted board.
#
# Consequence to respect when sizing generations: a Pi 5 generation has ~1019
# MiB of kernel-visible RAM regardless of the model's badge. Widening this is a
# platform change gated on an observed board memory map, via
# `KernelCustomDTSOverlay`, not a harness knob.
