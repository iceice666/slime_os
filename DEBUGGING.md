# Debugging SlimeOS Kernel

## The Issue
The kernel is loaded at a high virtual address (0x8000000000) by the bootloader, but LLDB sets breakpoints using file offsets. This causes breakpoints to not be hit.

## Solution 1: Using the debug.sh script
```bash
./debug.sh
```

This script automatically:
- Builds the kernel
- Starts QEMU with debugging enabled
- Loads symbols with the correct address offset
- Sets breakpoints properly

## Solution 2: Manual LLDB commands
1. Start QEMU in one terminal:
```bash
cargo run --bin qemu
```

2. In another terminal, find your kernel binary:
```bash
find target -name "slime_os_kernel-*" -type f | grep -E "bin/slime_os_kernel-[a-f0-9]+$"
```

3. Start LLDB with the kernel binary:
```bash
lldb target/x86_64-unknown-none/debug/deps/artifact/slime_os-kernel-*/bin/slime_os_kernel-*
```

4. In LLDB, run these commands:
```lldb
gdb-remote localhost:1234
target modules load --file <your-kernel-binary> --slide 0x8000000000
b kernel_main
c
```

## Solution 3: Break at entry point
Since the entry point address is known from QEMU output (0x8000032ef0), you can set a breakpoint there directly:

```lldb
gdb-remote localhost:1234
b *0x8000032ef0
c
```

Then step through to reach kernel_main.

## Tips
- The kernel virtual address base is 0x8000000000
- The entry point shown in QEMU logs includes this offset
- File offsets in the binary need to be adjusted by adding 0x8000000000
- The `nop` instruction added to kernel_main makes it easier to find in disassembly
