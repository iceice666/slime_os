# P6.1's x86-64 seL4 reference profile, derived from the pinned upstream x64
# release configuration rather than restated. `X64_verified.cmake` is the only
# canned x64 profile this seL4 pin ships; including it means a kernel bump
# moves this profile's inherited settings instead of leaving a hand-copied
# table silently stale.
#
# `FORCE` is required on every override below: the include above populates the
# CMake cache, and a plain `set(... CACHE ...)` would not replace an existing
# entry.
include(${CMAKE_CURRENT_LIST_DIR}/../../deps/sel4/configs/X64_verified.cmake)

# The verified profile is a proof configuration, not a product one. Slime needs
# the debug build and kernel printing that every other platform profile here
# selects, because the ordered serial marker chain is how a boot gate observes
# anything at all. Verification-build mode additionally removes the debug
# invocations `slime-root` uses.
set(KernelVerificationBuild OFF CACHE BOOL "" FORCE)
set(KernelDebugBuild ON CACHE BOOL "" FORCE)
set(KernelPrinting ON CACHE BOOL "" FORCE)

# Stated rather than left inherited, matching the other platform profiles: the
# platform identity, architecture, node count, and MCS mode are what a reader
# and `check-sel4-pins.py` compare across the five profiles, and a value that
# only exists inside the include is invisible to both.
set(KernelPlatform "pc99" CACHE STRING "" FORCE)
set(KernelSel4Arch "x86_64" CACHE STRING "" FORCE)
set(KernelMaxNumNodes 1 CACHE STRING "" FORCE)
set(KernelIsMCS OFF CACHE BOOL "" FORCE)

# `KernelFSGSBase "inst"` matches what the verified profile selects and is kept
# deliberately. It makes the kernel set `CR4.FSGSBASE` at boot
# (`fsgsbase_enable` in `deps/sel4/src/arch/x86/64/head.S`), which is what
# permits the userspace `rdfsbase` the component runtime reads its thread index
# with. The MSR route would leave that bit clear and the instruction would
# fault. `sel4/pins.toml` therefore pins a QEMU CPU model that implements
# `FSGSBASE`; the kernel prints a diagnostic and halts if it does not, so a
# wrong model fails loudly rather than silently.
set(KernelFSGSBase "inst" CACHE STRING "" FORCE)

# The retained x86 rollback artifacts and the Framework target both run without
# a hypervisor; VT-x support would add kernel objects no admitted generation
# grants.
set(KernelVTX OFF CACHE BOOL "" FORCE)

# QEMU's `virt`-equivalent for x86 is firmware-described through ACPI rather
# than a device tree, so the kernel derives its own timer calibration. Left at
# the upstream default (0, meaning calibrate from the MSR or the PIT) instead of
# pinning a QEMU-specific TSC frequency that a physical Framework would not
# share.
set(KernelPC99TSCFrequency 0 CACHE STRING "" FORCE)
