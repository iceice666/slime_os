[private]
default:
    @just --list --unsorted

# Run QEMU with optional feature (bios by default)
qemu feature="bios":
    #!/usr/bin/env bash
    if [ "{{feature}}" = "bios" ]; then
        cargo run --bin qemu
    elif [ "{{feature}}" = "uefi" ]; then
        cargo run --bin qemu --features uefi --no-default-features
    else
        echo "Invalid feature: {{feature}}. Use 'bios' or 'uefi'"
        exit 1
    fi

# Separate recipes for convenience
qemu-bios:
    just qemu bios

qemu-uefi:
    just qemu uefi

debug feature="bios":
    ./debug.sh