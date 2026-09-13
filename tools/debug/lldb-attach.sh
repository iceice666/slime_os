#!/usr/bin/env bash
# LLDB attach helper for Slime OS kernel debugging.
#
# Assumes QEMU is already running with `-s -S` (paused, gdb stub on 1234).
# Start it in another terminal with:  just debug_server
#
# The kernel is a higher-half ELF loaded by Limine at
# 0xffffffff80000000. Limine maps the ELF's virtual addresses as-is, so
# the LLDB slide is 0 — the ELF's program headers already describe the
# correct virtual layout. We just load the symbol file directly.

set -euo pipefail

# Default to the root-workspace dev kernel; override with KERNEL_PATH=... if needed.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KERNEL_PATH="${KERNEL_PATH:-$ROOT/target/x86_64-unknown-none/debug/slime_os-kernel}"
LLDB_CMD="${LLDB_CMD:-rust-lldb}"
GDB_PORT="${GDB_PORT:-1234}"

if [[ ! -f "$KERNEL_PATH" ]]; then
    echo "Error: Kernel binary not found at $KERNEL_PATH" >&2
    echo "Build it first with: cargo build -p slime_os-kernel" >&2
    exit 1
fi

echo "Starting LLDB debugging session for Slime OS kernel..."
echo "Kernel: $KERNEL_PATH"
echo "GDB remote port: $GDB_PORT"
echo ""

exec "$LLDB_CMD" \
    -o "target create \"$KERNEL_PATH\"" \
    -o "gdb-remote localhost:$GDB_PORT" \
    -o "b _start" \
    -o "c"
