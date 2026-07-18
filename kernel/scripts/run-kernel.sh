#!/usr/bin/env bash
# Cargo `runner` for the Slime OS kernel and its test binaries.
#
# Cargo invokes this as:  run-kernel.sh <binary-path> [extra-qemu-args...]
# It builds a FAT ESP image containing <binary-path> as the Limine-loaded
# kernel, then boots it under QEMU with OVMF.
#
# The kernel talks back through two channels:
#   - serial port  -> stdout (so `cargo test` output is visible)
#   - isa-debug-exit at port 0xf4 -> QEMU exit code
# `exit_qemu(QemuExitCode)` writes a byte to 0xf4; QEMU then exits with
# `(byte << 1) | 1`. We translate that back to a shell exit code so
# `cargo test` sees pass/fail.
#
# This script is meant to run inside `nix develop` (needs limine, mtools,
# dosfstools, qemu-system-x86_64, and $OVMF_CODE / $OVMF_VARS).
set -euo pipefail

[[ $# -ge 1 ]] || { echo "run-kernel.sh: missing binary path" >&2; exit 2; }
BIN="$1"; shift
EXTRA_ARGS=("$@")

[[ -f "$BIN" ]] || { echo "run-kernel.sh: binary not found: $BIN" >&2; exit 2; }

# Resolve OVMF firmware. Allow override via env; default to NixOS paths.
OVMF_CODE="${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE.fd}"
OVMF_VARS_SRC="${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS.fd}"
[[ -f "$OVMF_CODE" ]]      || { echo "OVMF_CODE not found: $OVMF_CODE"      >&2; exit 2; }
[[ -f "$OVMF_VARS_SRC" ]]  || { echo "OVMF_VARS not found: $OVMF_VARS_SRC" >&2; exit 2; }
# Per-run writable copy of OVMF vars so NVRAM changes don't leak between runs.
WORK="$(mktemp -d -t slime-os-run.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
VARS="$WORK/OVMF_VARS.fd"
# Source from the nix store is read-only (mode 0444); cp copies those
# permission bits, but OVMF needs to write NVRAM here, so make it writable.
cp "$OVMF_VARS_SRC" "$VARS"
chmod +w "$VARS"

IMG="$WORK/slime_os.img"
"$(dirname "$0")/build-iso.sh" "$BIN" "$IMG" 64 >/dev/null

# Boot and wait. `isa-debug-exit` makes QEMU exit with (code<<1)|1 when the
# guest writes to port 0xf4.
set +e
qemu-system-x86_64 \
    -machine q35,accel=tcg \
    -cpu qemu64 \
    -m 256M \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$VARS" \
    -drive format=raw,file="$IMG" \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -serial stdio \
    "${EXTRA_ARGS[@]}"
RC=$?
set -e

# Translate QEMU exit code -> shell exit code.
case "$RC" in
    0)   exit 0 ;;                                       # clean QEMU shutdown
    33)  exit 0 ;;                                       # 0x10 Success
    35)  echo "kernel exit: Failed (0x11)"    >&2; exit 1 ;;
    37)  echo "kernel exit: TestFailed (0x12)" >&2; exit 1 ;;
    *)   echo "qemu exited with code $RC"     >&2; exit 1 ;;
esac
