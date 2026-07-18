#!/usr/bin/env bash
# Build a bootable Slime OS UEFI image using Limine.
#
# Produces a FAT32 EFI System Partition image that QEMU (with OVMF) boots
# directly. No BIOS/hybrid ISO scheme — Slime OS Tier 0 is UEFI-only, so we
# avoid the extra `limine-bios-cd.exe` / `limine-uefi-cd.bin` artifacts that
# not every distro package ships.
#
# Layout inside the image:
#   EFI/BOOT/BOOTX64.EFI     Limine UEFI loader (firmware jumps here)
#   boot/limine.conf         Limine boot config
#   boot/slime_os-kernel     the kernel ELF
#   boot/generation.bin      verified manifest + immutable component objects
set -euo pipefail

print_usage() {
    cat <<EOF
Usage: $0 <kernel-binary> <output.img> [size-miB]
  size-miB defaults to 64.
EOF
    exit 1
}

[[ $# -ge 2 ]] || print_usage
KERNEL="$1"
OUTPUT="$2"
SIZE_MIB="${3:-64}"

[[ -f "$KERNEL" ]] || { echo "kernel not found: $KERNEL" >&2; exit 1; }
GEN_DIR="$(mktemp -d -t slime-os-generation.XXXXXX)"
trap 'rm -rf "$GEN_DIR"' EXIT
"$(dirname "$0")/../../scripts/build-generation.py" "$KERNEL" "$GEN_DIR" >/dev/null

# Locate Limine's datadir (BOOTX64.EFI lives there).
LIMINE_DATADIR="$(limine --print-datadir 2>/dev/null || true)"
if [[ -z "$LIMINE_DATADIR" || ! -f "$LIMINE_DATADIR/BOOTX64.EFI" ]]; then
    # Fallback: derive from the binary location.
    LB="$(command -v limine)"
    LIMINE_DATADIR="$(dirname "$(dirname "$LB")")/share/limine"
fi
[[ -f "$LIMINE_DATADIR/BOOTX64.EFI" ]] || {
    echo "cannot find BOOTX64.EFI (looked in $LIMINE_DATADIR)" >&2
    exit 1
}

# Fresh FAT32 image.
rm -f "$OUTPUT"
truncate -s "${SIZE_MIB}MiB" "$OUTPUT"
mkfs.fat -F 32 -n SLIMEOS "$OUTPUT"

# Populate using mtools (no mount needed — works without root).
MTOOLS_SKIP_CHECK=1 mmd -i "$OUTPUT" ::/EFI ::/EFI/BOOT ::/boot
MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$LIMINE_DATADIR/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$(dirname "$0")/../limine.conf" ::/boot/limine.conf
MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$KERNEL" ::/boot/slime_os-kernel
MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$GEN_DIR/generation.bin" ::/boot/generation.bin

echo "Built $OUTPUT ($(stat -c%s "$OUTPUT") bytes)"
