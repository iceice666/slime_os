set(KernelPlatform "qemu-riscv-virt" CACHE STRING "")
set(KernelSel4Arch "riscv64" CACHE STRING "")
# The root CSpace needs 19 address bits to hold the allocator's descriptor
# tables alongside the product graph. This CNode costs 16 MiB of kernel memory
# at 32 bytes per slot. Physical targets keep their existing 12-bit default, so
# `slime-root` selects its descriptor-table sizes from the linked kernel's own
# `ROOT_CNODE_SIZE_BITS` rather than from a per-platform allowlist.
set(KernelRootCNodeSizeBits 19 CACHE STRING "" FORCE)
set(KernelIsMCS OFF CACHE BOOL "")
set(KernelMaxNumNodes 1 CACHE STRING "")
set(KernelVerificationBuild OFF CACHE BOOL "")
set(KernelDebugBuild ON CACHE BOOL "")
set(KernelPrinting ON CACHE BOOL "")
