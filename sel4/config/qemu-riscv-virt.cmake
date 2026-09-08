set(KernelPlatform "qemu-riscv-virt" CACHE STRING "")
set(KernelSel4Arch "riscv64" CACHE STRING "")
# MEM-ARENAS: four adversarial 256 MiB private holders plus the product graph
# need 19 address bits in the root CSpace. This CNode costs 16 MiB of kernel
# memory at 32 bytes per slot; the 3 GiB QEMU profile's capacity report includes
# that cost and actual free slots. Physical targets keep their existing values.
set(KernelRootCNodeSizeBits 19 CACHE STRING "" FORCE)
set(KernelIsMCS OFF CACHE BOOL "")
set(KernelMaxNumNodes 1 CACHE STRING "")
set(KernelVerificationBuild OFF CACHE BOOL "")
set(KernelDebugBuild ON CACHE BOOL "")
set(KernelPrinting ON CACHE BOOL "")
