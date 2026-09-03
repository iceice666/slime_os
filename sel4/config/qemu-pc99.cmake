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

# Turned off so this profile can boot at all, and the only inherited setting
# P6.2 had to weaken. With `KernelSupportPCID` on, the kernel's boot path calls
# `pcid_check` and `invpcid_check` (`deps/sel4/src/arch/x86/64/head.S`) before
# installing `boot_pml4` and halts with a diagnostic unless the CPU reports
# both `CPUID.01h:ECX[17]` and `CPUID.07h:EBX[10]`. QEMU's TCG accelerator
# implements neither feature on any model — including `-cpu max` — so no CPU
# choice makes a PCID kernel reach the root task under the pinned emulator.
#
# PCIDs are hardware address-space identifiers, a TLB-flush optimization. The
# kernel's own option help calls them optional ("Not all processor models
# support this feature"), and `deps/sel4/src/arch/x86/config.cmake` defaults
# them off outside x86-64. Disabling them changes no capability, mapping,
# rights, or observable syscall semantic; it costs address-space-switch TLB
# flushes. The Framework's Ryzen AI 300 does implement PCID/INVPCID, so P6.6's
# physical profile may re-enable it — that is a separate profile with its own
# pins and its own observation, not a silent inheritance from this one.
set(KernelSupportPCID OFF CACHE BOOL "" FORCE)

# Restored to seL4's own default (`deps/sel4/config.cmake`), which the
# inherited proof configuration lowers to 50 to bound its verification effort.
# 50 is not enough to describe this machine: `create_untypeds` emits every
# *device* region before any kernel-window memory, and a q35 machine's ACPI,
# LAPIC/IOAPIC, PCI ECAM, and firmware-reserved ranges exhaust the list before
# the first free-memory untyped is reached. The kernel then prints "Too many
# untyped regions for boot info" and hands the root task a bootinfo with zero
# kernel untypeds, which `ObjectAllocator::new` correctly refuses with
# `NoKernelUntyped`. Every other Slime platform profile already runs at this
# default; matching it is what makes the pc99 root able to allocate at all.
set(KernelMaxNumBootinfoUntypedCaps 230 CACHE STRING "" FORCE)

# Restored to seL4's own default for the same reason as the line above: the
# inherited proof configuration raises it to 19, giving the root task a
# 2^19-slot initial CNode whose empty-slot span (523448 slots here) exceeds
# `MAX_ROOT_CSLOTS`, the bound `slime-root`'s reusable slot bitmap is sized
# against. 12 is what the AArch64 and RISC-V product profiles run, so the root
# sees a CSpace of the shape its allocator was built and tested for on every
# platform rather than one shape per platform.
set(KernelRootCNodeSizeBits 12 CACHE STRING "" FORCE)

# QEMU's `virt`-equivalent for x86 is firmware-described through ACPI rather
# than a device tree, so the kernel derives its own timer calibration. Left at
# the upstream default (0, meaning calibrate from the MSR or the PIT) instead of
# pinning a QEMU-specific TSC frequency that a physical Framework would not
# share.
set(KernelPC99TSCFrequency 0 CACHE STRING "" FORCE)
