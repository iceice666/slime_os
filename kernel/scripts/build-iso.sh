#!/usr/bin/env bash
# Build a bootable Slime OS UEFI image. Production uses immutable stage-0;
# the Cargo test runner opts into Limine for existing test entry points.
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
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
[[ -f "$KERNEL" ]] || { echo "kernel not found: $KERNEL" >&2; exit 1; }

if [[ -n "${SLIME_GENERATION_DIR:-}" ]]; then
    GEN_DIR="$SLIME_GENERATION_DIR"
    CLEAN_GEN_DIR=0
else
    GEN_DIR="$(mktemp -d -t slime-os-generation.XXXXXX)"
    CLEAN_GEN_DIR=1
fi

trap 'if [[ "$CLEAN_GEN_DIR" == 1 ]]; then rm -rf "$GEN_DIR"; fi' EXIT
if [[ "$CLEAN_GEN_DIR" == 1 ]]; then
    SLIME_GENERATION_NUMBER="${SLIME_GENERATION_NUMBER:-}" \
    SLIME_PENDING_GENERATION="${SLIME_PENDING_GENERATION:-}" \
    SLIME_PENDING_ATTEMPTS="${SLIME_PENDING_ATTEMPTS:-}" \
        "$ROOT/scripts/build-generation.py" "$KERNEL" "$GEN_DIR" >/dev/null
fi

if [[ "${SLIME_BOOT_LOADER:-stage0}" == "limine" ]]; then
    LIMINE_DATADIR="$(limine --print-datadir 2>/dev/null || true)"
    if [[ -z "$LIMINE_DATADIR" || ! -f "$LIMINE_DATADIR/BOOTX64.EFI" ]]; then
        LB="$(command -v limine)"
        LIMINE_DATADIR="$(dirname "$(dirname "$LB")")/share/limine"
    fi
    [[ -f "$LIMINE_DATADIR/BOOTX64.EFI" ]] || { echo "cannot find BOOTX64.EFI" >&2; exit 1; }
else
    cargo build --manifest-path "$ROOT/stage0/Cargo.toml" --target x86_64-unknown-uefi --release >/dev/null
    STAGE0="$ROOT/stage0/target/x86_64-unknown-uefi/release/slime-stage0.efi"
    [[ -f "$STAGE0" ]] || { echo "stage-0 EFI binary not found: $STAGE0" >&2; exit 1; }
fi

rm -f "$OUTPUT"
truncate -s "${SIZE_MIB}MiB" "$OUTPUT"
mkfs.fat -F 32 -n SLIMEOS "$OUTPUT"
MTOOLS_SKIP_CHECK=1 mmd -i "$OUTPUT" ::/EFI ::/EFI/BOOT ::/boot
if [[ "${SLIME_BOOT_LOADER:-stage0}" == "limine" ]]; then
    MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$LIMINE_DATADIR/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
    MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$ROOT/kernel/limine.conf" ::/boot/limine.conf
    MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$KERNEL" ::/boot/slime_os-kernel
    MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$GEN_DIR/generation.bin" ::/boot/generation.bin
else
    MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "$STAGE0" ::/EFI/BOOT/BOOTX64.EFI
    MTOOLS_SKIP_CHECK=1 mcopy -i "$OUTPUT" "${SLIME_BOOTSTORE_OVERRIDE:-$GEN_DIR/boot-store.bin}" ::/boot/boot-store.bin
    if [[ "${SLIME_RECOVERY_IMAGE:-}" == "1" ]]; then
        MTOOLS_SKIP_CHECK=1 mcopy -o -i "$OUTPUT" "$GEN_DIR/recovery-boot-store.bin" ::/boot/boot-store.bin
    fi
fi

echo "Built $OUTPUT ($(stat -c%s "$OUTPUT") bytes)"
